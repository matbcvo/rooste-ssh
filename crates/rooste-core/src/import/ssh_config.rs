//! OpenSSH `~/.ssh/config`: one profile per concrete `Host` alias, with
//! options resolved the way OpenSSH does (first value wins, wildcard blocks
//! such as `Host *` supply defaults, `Include` is followed).

use std::io;
use std::path::{Path, PathBuf};

use super::expand_tilde;
use crate::Profile;

/// Folder imported hosts are placed in.
pub const FOLDER: &str = "Imported/SSH config";
const MAX_INCLUDE_DEPTH: usize = 8;

#[derive(Debug, Default)]
struct Block {
    /// Host patterns; empty for `Match` blocks, which are never applied.
    patterns: Vec<String>,
    options: Vec<(String, String)>,
}

/// Import `path`, resolving relative `Include`s against its directory.
pub fn import_file(path: &Path) -> io::Result<Vec<Profile>> {
    let text = std::fs::read_to_string(path)?;
    let base = path.parent().unwrap_or(Path::new("."));
    Ok(parse(&text, base))
}

/// Import `~/.ssh/config`.
pub fn import_default() -> io::Result<Vec<Profile>> {
    let home = std::env::home_dir()
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no home directory"))?;
    import_file(&home.join(".ssh").join("config"))
}

/// Parse config text. `base` resolves relative `Include` paths.
pub fn parse(text: &str, base: &Path) -> Vec<Profile> {
    let mut blocks = vec![Block {
        patterns: vec!["*".into()],
        options: Vec::new(),
    }];
    collect(text, base, &mut blocks, 0);

    let mut aliases: Vec<String> = Vec::new();
    for block in &blocks[1..] {
        for pattern in &block.patterns {
            let concrete = !pattern.starts_with('!') && !pattern.contains(['*', '?']);
            if concrete && !aliases.contains(pattern) {
                aliases.push(pattern.clone());
            }
        }
    }

    aliases
        .into_iter()
        .map(|alias| {
            let option = |key: &str| -> Option<String> {
                blocks
                    .iter()
                    .filter(|b| block_matches(&b.patterns, &alias))
                    .flat_map(|b| &b.options)
                    .find(|(k, _)| k == key)
                    .map(|(_, v)| v.clone())
            };
            let host = option("hostname")
                .map(|h| h.replace("%h", &alias))
                .unwrap_or_else(|| alias.clone());
            Profile {
                name: alias.clone(),
                folder: FOLDER.into(),
                host,
                port: option("port").and_then(|p| p.parse().ok()),
                user: option("user").unwrap_or_default(),
                key_file: option("identityfile").map(|f| expand_tilde(&f)),
                proxy_jump: option("proxyjump").filter(|j| !j.eq_ignore_ascii_case("none")),
                notes: "Imported from ~/.ssh/config".into(),
                ..Profile::default()
            }
        })
        .collect()
}

fn collect(text: &str, base: &Path, blocks: &mut Vec<Block>, depth: usize) {
    for line in text.lines() {
        let Some((key, value)) = split_line(line) else {
            continue;
        };
        match key.as_str() {
            "host" => blocks.push(Block {
                patterns: words(&value),
                options: Vec::new(),
            }),
            "match" => blocks.push(Block::default()),
            "include" if depth < MAX_INCLUDE_DEPTH => {
                for pattern in words(&value) {
                    for path in expand_include(&pattern, base) {
                        if let Ok(included) = std::fs::read_to_string(&path) {
                            collect(&included, base, blocks, depth + 1);
                        }
                    }
                }
            }
            _ => {
                if let Some(block) = blocks.last_mut() {
                    block.options.push((key, unquote(&value)));
                }
            }
        }
    }
}

/// `Key value`, `Key=value` or `Key = value`; `None` for blanks and comments.
fn split_line(line: &str) -> Option<(String, String)> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return None;
    }
    let split = line.find(|c: char| c.is_whitespace() || c == '=')?;
    let (key, rest) = line.split_at(split);
    let value = rest
        .trim_start_matches(|c: char| c.is_whitespace())
        .trim_start_matches('=')
        .trim();
    (!value.is_empty()).then(|| (key.to_ascii_lowercase(), value.to_owned()))
}

/// Whitespace-separated words, honoring double quotes.
fn words(value: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    for c in value.chars() {
        match c {
            '"' => quoted = !quoted,
            c if c.is_whitespace() && !quoted => {
                if !current.is_empty() {
                    out.push(std::mem::take(&mut current));
                }
            }
            c => current.push(c),
        }
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

fn unquote(value: &str) -> String {
    value
        .strip_prefix('"')
        .and_then(|v| v.strip_suffix('"'))
        .unwrap_or(value)
        .to_owned()
}

fn block_matches(patterns: &[String], host: &str) -> bool {
    let mut matched = false;
    for pattern in patterns {
        if let Some(negated) = pattern.strip_prefix('!') {
            if glob_match(negated, host) {
                return false;
            }
        } else if glob_match(pattern, host) {
            matched = true;
        }
    }
    matched
}

/// `*` and `?` wildcards, case-insensitive like OpenSSH host matching.
fn glob_match(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.to_lowercase().chars().collect();
    let t: Vec<char> = text.to_lowercase().chars().collect();
    let (mut pi, mut ti) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while ti < t.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == t[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some((pi, ti));
            pi += 1;
        } else if let Some((sp, st)) = star {
            pi = sp + 1;
            ti = st + 1;
            star = Some((sp, st + 1));
        } else {
            return false;
        }
    }
    p[pi..].iter().all(|&c| c == '*')
}

/// Paths for an `Include` argument: `~` expanded, relative to `base`, and a
/// `*` in the file name matched against the directory listing.
fn expand_include(pattern: &str, base: &Path) -> Vec<PathBuf> {
    let path = expand_tilde(pattern);
    let path = if path.is_absolute() {
        path
    } else {
        base.join(path)
    };
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
        return Vec::new();
    };
    if !name.contains(['*', '?']) {
        return vec![path];
    }
    let Some(dir) = path.parent() else {
        return Vec::new();
    };
    let mut matches: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter(|e| {
            e.file_name()
                .to_str()
                .is_some_and(|file| glob_match(name, file))
        })
        .map(|e| e.path())
        .collect();
    matches.sort();
    matches
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    const CONFIG: &str = r#"
# Global defaults come last in practice, first-match wins
Host web
    HostName web1.example.com
    User deploy
    Port 2222
    IdentityFile ~/.ssh/web_key

Host db db-replica
  HostName=%h.internal
  ProxyJump bastion

Host bastion
    HostName bastion.example.com
    ProxyJump none

Host *.example.com !secret.example.com
    User admin

Match host foo
    User should-not-apply

Host *
    User fallback
    Port 22
"#;

    fn by_name<'a>(profiles: &'a [Profile], name: &str) -> &'a Profile {
        profiles.iter().find(|p| p.name == name).unwrap()
    }

    #[test]
    fn one_profile_per_concrete_alias() {
        let profiles = parse(CONFIG, Path::new("/tmp"));
        let names: Vec<_> = profiles.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["web", "db", "db-replica", "bastion"]);
        assert!(profiles.iter().all(|p| p.folder == FOLDER));
    }

    #[test]
    fn first_value_wins_and_wildcards_fill_defaults() {
        let profiles = parse(CONFIG, Path::new("/tmp"));
        let web = by_name(&profiles, "web");
        assert_eq!(web.host, "web1.example.com");
        assert_eq!(web.user, "deploy");
        assert_eq!(web.port, Some(2222));
        assert!(web.key_file.as_ref().unwrap().ends_with(".ssh/web_key"));

        let db = by_name(&profiles, "db");
        assert_eq!(db.host, "db.internal", "%h expands to the alias");
        assert_eq!(db.user, "fallback");
        assert_eq!(db.port, Some(22));
        assert_eq!(db.proxy_jump.as_deref(), Some("bastion"));
        assert_eq!(by_name(&profiles, "db-replica").host, "db-replica.internal");

        let bastion = by_name(&profiles, "bastion");
        assert_eq!(bastion.proxy_jump, None, "ProxyJump none");
        assert_eq!(
            bastion.user, "fallback",
            "pattern matches the alias, not HostName"
        );
    }

    #[test]
    fn negated_patterns_and_match_blocks() {
        let text = "Host secret.example.com other.example.com\nHost *.example.com !secret.example.com\n  User admin\n";
        let profiles = parse(text, Path::new("/tmp"));
        assert_eq!(by_name(&profiles, "secret.example.com").user, "");
        assert_eq!(by_name(&profiles, "other.example.com").user, "admin");
    }

    #[test]
    fn includes_are_followed() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("config.d")).unwrap();
        std::fs::write(
            dir.path().join("config.d").join("work.conf"),
            "Host work\n  HostName work.example.com\n",
        )
        .unwrap();
        std::fs::write(dir.path().join("extra"), "Host extra\n  User x\n").unwrap();
        let main = "Include config.d/*.conf extra\nHost home\n  HostName home.lan\n";
        let path = dir.path().join("config");
        std::fs::write(&path, main).unwrap();

        let profiles = import_file(&path).unwrap();
        let names: Vec<_> = profiles.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["work", "extra", "home"]);
        assert_eq!(by_name(&profiles, "work").host, "work.example.com");
    }

    #[test]
    fn line_syntax() {
        assert_eq!(split_line("  Port 22 "), Some(("port".into(), "22".into())));
        assert_eq!(split_line("Port=22"), Some(("port".into(), "22".into())));
        assert_eq!(split_line("Port = 22"), Some(("port".into(), "22".into())));
        assert_eq!(split_line("# comment"), None);
        assert_eq!(split_line("Port"), None);
        assert_eq!(words(r#"a "b c" d"#), ["a", "b c", "d"]);
        assert_eq!(unquote("\"/path with space\""), "/path with space");
    }

    #[test]
    fn globbing() {
        assert!(glob_match("*", "anything"));
        assert!(glob_match("web?", "web1"));
        assert!(glob_match("*.EXAMPLE.com", "a.example.COM"));
        assert!(!glob_match("*.example.com", "example.com"));
        assert!(glob_match("a*b*c", "aXXbYYc"));
        assert!(!glob_match("a*b*c", "aXXbYY"));
    }

    proptest! {
        #[test]
        fn parser_never_panics(text in "\\PC{0,400}") {
            let _ = parse(&text, Path::new("/nonexistent"));
        }

        #[test]
        fn glob_without_wildcards_is_equality(a in "[a-z.]{0,10}", b in "[a-z.]{0,10}") {
            prop_assert_eq!(glob_match(&a, &b), a == b);
        }
    }
}
