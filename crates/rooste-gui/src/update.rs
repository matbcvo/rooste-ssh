//! Checking GitHub for a newer release.

use std::time::Duration;

use serde::Deserialize;

/// The latest-release endpoint of the GitHub API.
pub const RELEASES_API: &str = "https://api.github.com/repos/matbcvo/rooste-ssh/releases/latest";

/// A newer release than the running one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    pub version: String,
    pub url: String,
}

#[derive(Deserialize)]
struct GithubRelease {
    tag_name: String,
    html_url: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
}

/// `1.2.3` or `v1.2.3` (pre-release suffixes ignored).
pub fn parse_version(text: &str) -> Option<(u64, u64, u64)> {
    let core = text.trim().trim_start_matches('v');
    let core = core.split(['-', '+']).next()?;
    let mut parts = core.split('.').map(|p| p.parse::<u64>().ok());
    let version = (
        parts.next()??,
        parts.next().flatten().unwrap_or(0),
        parts.next().flatten().unwrap_or(0),
    );
    Some(version)
}

/// Whether `latest` is a higher version than `current`.
pub fn is_newer(latest: &str, current: &str) -> bool {
    match (parse_version(latest), parse_version(current)) {
        (Some(latest), Some(current)) => latest > current,
        _ => false,
    }
}

/// Ask `api_url` for the latest release. `Ok(None)` means up to date, or no
/// release published yet.
pub fn fetch_latest(api_url: &str, current: &str) -> Result<Option<Release>, String> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(10)))
        .user_agent(format!("rooste-ssh/{current}"))
        .http_status_as_error(false)
        .tls_config(
            ureq::tls::TlsConfig::builder()
                .root_certs(ureq::tls::RootCerts::PlatformVerifier)
                .build(),
        )
        .build()
        .into();
    let mut response = agent
        .get(api_url)
        .header("Accept", "application/vnd.github+json")
        .call()
        .map_err(|e| e.to_string())?;
    match response.status().as_u16() {
        404 => return Ok(None),
        200 => {}
        status => return Err(format!("update check failed with HTTP {status}")),
    }
    let release: GithubRelease = response.body_mut().read_json().map_err(|e| e.to_string())?;
    if release.draft || release.prerelease || !is_newer(&release.tag_name, current) {
        return Ok(None);
    }
    Ok(Some(Release {
        version: release.tag_name.trim_start_matches('v').to_owned(),
        url: release.html_url,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    #[test]
    fn versions() {
        assert_eq!(parse_version("v1.2.3"), Some((1, 2, 3)));
        assert_eq!(parse_version("0.10"), Some((0, 10, 0)));
        assert_eq!(parse_version("2.0.0-beta.1"), Some((2, 0, 0)));
        assert_eq!(parse_version("nope"), None);
        assert!(is_newer("v0.2.0", "0.1.9"));
        assert!(is_newer("0.10.0", "0.9.9"), "numeric, not lexical");
        assert!(!is_newer("0.1.0", "0.1.0"));
        assert!(!is_newer("garbage", "0.1.0"));
    }

    /// Serve one HTTP response on a local port; returns the URL.
    fn serve_once(status: &str, body: &str) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/releases/latest", listener.local_addr().unwrap());
        let response = format!(
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        std::thread::spawn(move || {
            if let Ok((mut socket, _)) = listener.accept() {
                let mut request = [0u8; 2048];
                let _ = socket.read(&mut request);
                let _ = socket.write_all(response.as_bytes());
            }
        });
        url
    }

    #[test]
    fn reports_only_newer_published_releases() {
        let body = r#"{"tag_name":"v9.0.0","html_url":"https://example.com/r/9","draft":false,"prerelease":false}"#;
        let found = fetch_latest(&serve_once("200 OK", body), "0.1.0").unwrap();
        assert_eq!(
            found,
            Some(Release {
                version: "9.0.0".into(),
                url: "https://example.com/r/9".into()
            })
        );

        let same = r#"{"tag_name":"v0.1.0","html_url":"x"}"#;
        assert_eq!(
            fetch_latest(&serve_once("200 OK", same), "0.1.0").unwrap(),
            None
        );

        let pre = r#"{"tag_name":"v9.0.0","html_url":"x","prerelease":true}"#;
        assert_eq!(
            fetch_latest(&serve_once("200 OK", pre), "0.1.0").unwrap(),
            None
        );

        assert_eq!(
            fetch_latest(&serve_once("404 Not Found", "{}"), "0.1.0").unwrap(),
            None
        );
        assert!(fetch_latest(&serve_once("500 Internal Server Error", "{}"), "0.1.0").is_err());
    }
}

#[cfg(test)]
mod network {
    /// Real HTTPS request to GitHub with the platform certificate store.
    #[test]
    #[ignore = "needs internet access"]
    fn github_https() {
        let result = super::fetch_latest(
            "https://api.github.com/repos/rust-lang/rust/releases/latest",
            "0.0.1",
        );
        let release = result
            .expect("HTTPS to GitHub works")
            .expect("rust has releases");
        assert!(
            release
                .url
                .starts_with("https://github.com/rust-lang/rust/releases/")
        );
    }
}
