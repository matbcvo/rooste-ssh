//! A saved connection.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::Protocol;

/// A saved session. Passwords are never stored here; see
/// [`SecretStore`](crate::SecretStore).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Profile {
    /// Stable identifier, also the key for the stored password.
    pub id: String,
    pub name: String,
    /// Folder path in the session manager, `/`-separated; empty for the top level.
    pub folder: String,
    pub tags: Vec<String>,
    pub protocol: Protocol,
    pub host: String,
    /// `None` uses the protocol's default port.
    pub port: Option<u16>,
    pub user: String,
    pub key_file: Option<PathBuf>,
    pub use_agent: bool,
    /// A password for this profile is kept in the OS credential store.
    pub save_password: bool,
    /// Offer outdated algorithms for old devices.
    pub allow_legacy: bool,
    /// Jump host(s), as in OpenSSH `ProxyJump`.
    pub proxy_jump: Option<String>,
    pub notes: String,
}

impl Default for Profile {
    fn default() -> Self {
        Self {
            id: new_id(),
            name: String::new(),
            folder: String::new(),
            tags: Vec::new(),
            protocol: Protocol::Ssh,
            host: String::new(),
            port: None,
            user: String::new(),
            key_file: None,
            use_agent: true,
            save_password: false,
            allow_legacy: false,
            proxy_jump: None,
            notes: String::new(),
        }
    }
}

/// A fresh random profile id.
pub fn new_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

impl Profile {
    /// Port to connect to.
    pub fn effective_port(&self) -> u16 {
        self.port
            .or_else(|| self.protocol.default_port())
            .unwrap_or(22)
    }

    /// Name shown in lists: the profile name, or `user@host`.
    pub fn display_name(&self) -> String {
        if !self.name.trim().is_empty() {
            self.name.clone()
        } else if self.user.is_empty() {
            self.host.clone()
        } else {
            format!("{}@{}", self.user, self.host)
        }
    }

    /// Case-insensitive match against name, host, user, folder and tags.
    pub fn matches(&self, query: &str) -> bool {
        let query = query.trim().to_lowercase();
        if query.is_empty() {
            return true;
        }
        query.split_whitespace().all(|word| {
            [&self.name, &self.host, &self.user, &self.folder]
                .into_iter()
                .chain(&self.tags)
                .any(|field| field.to_lowercase().contains(word))
        })
    }

    /// A copy with a new id and " (copy)" appended to the name.
    pub fn duplicate(&self) -> Self {
        Self {
            id: new_id(),
            name: format!("{} (copy)", self.display_name()),
            save_password: false,
            ..self.clone()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Profile {
        Profile {
            name: "Web server".into(),
            folder: "Work/Production".into(),
            tags: vec!["nginx".into()],
            host: "web1.example.com".into(),
            user: "deploy".into(),
            ..Profile::default()
        }
    }

    #[test]
    fn ids_are_unique() {
        assert_ne!(Profile::default().id, Profile::default().id);
    }

    #[test]
    fn effective_port_falls_back_to_protocol_default() {
        let mut p = sample();
        assert_eq!(p.effective_port(), 22);
        p.protocol = Protocol::Telnet;
        assert_eq!(p.effective_port(), 23);
        p.port = Some(2222);
        assert_eq!(p.effective_port(), 2222);
    }

    #[test]
    fn display_name_fallbacks() {
        let mut p = sample();
        assert_eq!(p.display_name(), "Web server");
        p.name.clear();
        assert_eq!(p.display_name(), "deploy@web1.example.com");
        p.user.clear();
        assert_eq!(p.display_name(), "web1.example.com");
    }

    #[test]
    fn search_matches_every_word_in_any_field() {
        let p = sample();
        assert!(p.matches(""));
        assert!(p.matches("WEB"));
        assert!(p.matches("production nginx"));
        assert!(p.matches("deploy web1"));
        assert!(!p.matches("staging"));
        assert!(!p.matches("web staging"));
    }

    #[test]
    fn duplicate_gets_new_id_and_no_saved_password() {
        let mut p = sample();
        p.save_password = true;
        let copy = p.duplicate();
        assert_ne!(copy.id, p.id);
        assert_eq!(copy.name, "Web server (copy)");
        assert!(!copy.save_password);
        assert_eq!(copy.host, p.host);
    }
}
