//! Trying authentication methods in OpenSSH order: agent, key files, password.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::bail;
use rooste_protocols::ssh::{AgentSource, Auth, PrivateKey, SshClient, SshError, load_private_key};

const PROMPT_ATTEMPTS: usize = 3;
const DEFAULT_KEYS: [&str; 3] = ["id_ed25519", "id_ecdsa", "id_rsa"];

#[derive(Debug, Clone)]
pub struct Methods {
    pub agent: bool,
    /// Explicit key files; empty means the default keys in `~/.ssh`.
    pub identities: Vec<PathBuf>,
}

pub async fn authenticate(
    client: &mut SshClient,
    user: &str,
    host: &str,
    methods: Methods,
) -> anyhow::Result<()> {
    if methods.agent {
        match client
            .authenticate(user, &Auth::Agent(AgentSource::Default))
            .await
        {
            Ok(true) => return Ok(()),
            // No agent running is normal; fall through to the next method.
            Ok(false) | Err(SshError::Agent(_)) => {}
            Err(e) => return Err(e.into()),
        }
    }

    let explicit = !methods.identities.is_empty();
    let keys = if explicit {
        methods.identities
    } else {
        default_keys()
    };
    for path in keys {
        if !explicit && !path.exists() {
            continue;
        }
        let Some(key) = load_key(&path).await? else {
            continue;
        };
        if client.authenticate(user, &Auth::PublicKey(key)).await? {
            return Ok(());
        }
    }

    for _ in 0..PROMPT_ATTEMPTS {
        let Some(password) = prompt_secret(format!("{user}@{host}'s password: ")).await? else {
            break;
        };
        if client.authenticate(user, &Auth::password(password)).await? {
            return Ok(());
        }
        eprintln!("Permission denied, please try again.");
    }
    bail!("permission denied for {user}@{host}")
}

fn default_keys() -> Vec<PathBuf> {
    std::env::home_dir()
        .map(|home| {
            DEFAULT_KEYS
                .iter()
                .map(|name| home.join(".ssh").join(name))
                .collect()
        })
        .unwrap_or_default()
}

/// Load a key, asking for its passphrase if it is encrypted. Returns `None`
/// (after a warning) for keys that cannot be used.
async fn load_key(path: &Path) -> anyhow::Result<Option<Arc<PrivateKey>>> {
    match load_private_key(path, None) {
        Ok(key) => return Ok(Some(Arc::new(key))),
        Err(SshError::KeyEncrypted(_)) => {}
        Err(e) => {
            eprintln!("warning: {e}");
            return Ok(None);
        }
    }
    for _ in 0..PROMPT_ATTEMPTS {
        let prompt = format!("Enter passphrase for key '{}': ", path.display());
        let Some(passphrase) = prompt_secret(prompt).await? else {
            return Ok(None);
        };
        match load_private_key(path, Some(&passphrase)) {
            Ok(key) => return Ok(Some(Arc::new(key))),
            Err(_) => eprintln!("Bad passphrase, try again."),
        }
    }
    Ok(None)
}

/// Read a secret without echo. `None` if the user entered nothing.
async fn prompt_secret(prompt: String) -> anyhow::Result<Option<String>> {
    let secret = tokio::task::spawn_blocking(move || rpassword::prompt_password(prompt)).await??;
    Ok((!secret.is_empty()).then_some(secret))
}
