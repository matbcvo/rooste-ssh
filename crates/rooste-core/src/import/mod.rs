//! Importing saved sessions from other clients.

pub mod putty;
pub mod ssh_config;

use std::path::PathBuf;

/// Expand a leading `~` to the home directory.
pub fn expand_tilde(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/").or_else(|| path.strip_prefix("~\\"))
        && let Some(home) = std::env::home_dir()
    {
        return home.join(rest);
    }
    if path == "~"
        && let Some(home) = std::env::home_dir()
    {
        return home;
    }
    PathBuf::from(path)
}
