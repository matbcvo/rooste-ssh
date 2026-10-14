//! OpenSSH-style forward specs: `-L`, `-R` and `-D`.

use anyhow::{Context, bail};
use rooste_protocols::ssh::{Forward, SshClient};

/// `[bind:]port:host:hostport`
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortSpec {
    pub bind: Option<String>,
    pub port: u16,
    pub host: String,
    pub host_port: u16,
}

/// `[bind:]port`
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SocksSpec {
    pub bind: Option<String>,
    pub port: u16,
}

/// Split on `:` while keeping `[v6]` addresses together.
fn fields(spec: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut bracket = false;
    for c in spec.chars() {
        match c {
            '[' => bracket = true,
            ']' => bracket = false,
            ':' if !bracket => out.push(std::mem::take(&mut current)),
            c => current.push(c),
        }
    }
    out.push(current);
    out
}

fn port(text: &str, spec: &str) -> anyhow::Result<u16> {
    text.parse()
        .with_context(|| format!("invalid port `{text}` in `{spec}`"))
}

impl PortSpec {
    pub fn parse(spec: &str) -> anyhow::Result<Self> {
        let f = fields(spec);
        let (bind, rest) = match f.len() {
            3 => (None, &f[..]),
            4 => (Some(f[0].clone()), &f[1..]),
            _ => bail!("`{spec}` is not [bind:]port:host:hostport"),
        };
        Ok(Self {
            bind: bind.filter(|b| !b.is_empty()),
            port: port(&rest[0], spec)?,
            host: rest[1].clone(),
            host_port: port(&rest[2], spec)?,
        })
    }
}

impl SocksSpec {
    pub fn parse(spec: &str) -> anyhow::Result<Self> {
        let f = fields(spec);
        match f.as_slice() {
            [p] => Ok(Self {
                bind: None,
                port: port(p, spec)?,
            }),
            [bind, p] => Ok(Self {
                bind: Some(bind.clone()).filter(|b| !b.is_empty()),
                port: port(p, spec)?,
            }),
            _ => bail!("`{spec}` is not [bind:]port"),
        }
    }
}

fn bind_addr(bind: &Option<String>, port: u16) -> String {
    match bind.as_deref() {
        Some(addr) if addr.contains(':') => format!("[{addr}]:{port}"),
        Some(addr) => format!("{addr}:{port}"),
        None => format!("127.0.0.1:{port}"),
    }
}

#[derive(Debug, Default)]
pub struct Specs {
    local: Vec<PortSpec>,
    remote: Vec<PortSpec>,
    dynamic: Vec<SocksSpec>,
}

impl Specs {
    pub fn parse(local: &[String], remote: &[String], dynamic: &[String]) -> anyhow::Result<Self> {
        Ok(Self {
            local: local
                .iter()
                .map(|s| PortSpec::parse(s))
                .collect::<Result<_, _>>()?,
            remote: remote
                .iter()
                .map(|s| PortSpec::parse(s))
                .collect::<Result<_, _>>()?,
            dynamic: dynamic
                .iter()
                .map(|s| SocksSpec::parse(s))
                .collect::<Result<_, _>>()?,
        })
    }

    /// Start every forward; they stop when the returned values are dropped.
    pub async fn start(&self, client: &SshClient) -> anyhow::Result<Vec<Forward>> {
        let mut forwards = Vec::new();
        for spec in &self.local {
            let bind = bind_addr(&spec.bind, spec.port);
            forwards.push(
                client
                    .forward_local(&bind, &spec.host, spec.host_port)
                    .await?,
            );
            eprintln!("Forwarding {bind} -> {}:{}", spec.host, spec.host_port);
        }
        for spec in &self.dynamic {
            let bind = bind_addr(&spec.bind, spec.port);
            forwards.push(client.forward_dynamic(&bind).await?);
            eprintln!("SOCKS proxy on {bind}");
        }
        for spec in &self.remote {
            let addr = spec.bind.clone().unwrap_or_else(|| "localhost".into());
            let forward = client
                .forward_remote(&addr, spec.port, &spec.host, spec.host_port)
                .await?;
            eprintln!(
                "Server port {} -> {}:{}",
                forward.listen.port(),
                spec.host,
                spec.host_port
            );
            forwards.push(forward);
        }
        Ok(forwards)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn port_specs() {
        let s = PortSpec::parse("8080:db.internal:5432").unwrap();
        assert_eq!(
            (s.bind, s.port, s.host.as_str(), s.host_port),
            (None, 8080, "db.internal", 5432)
        );
        let s = PortSpec::parse("0.0.0.0:80:web:8080").unwrap();
        assert_eq!(s.bind.as_deref(), Some("0.0.0.0"));
        let s = PortSpec::parse("[::1]:80:[fe80::1]:8080").unwrap();
        assert_eq!(
            (s.bind.as_deref(), s.host.as_str()),
            (Some("::1"), "fe80::1")
        );
        assert!(PortSpec::parse("80:host").is_err());
        assert!(PortSpec::parse("x:host:1").is_err());
    }

    #[test]
    fn socks_specs() {
        assert_eq!(SocksSpec::parse("1080").unwrap().port, 1080);
        let s = SocksSpec::parse("0.0.0.0:1080").unwrap();
        assert_eq!(s.bind.as_deref(), Some("0.0.0.0"));
        assert!(SocksSpec::parse("a:b:c").is_err());
    }

    #[test]
    fn bind_addresses() {
        assert_eq!(bind_addr(&None, 1), "127.0.0.1:1");
        assert_eq!(bind_addr(&Some("0.0.0.0".into()), 2), "0.0.0.0:2");
        assert_eq!(bind_addr(&Some("::1".into()), 3), "[::1]:3");
    }
}
