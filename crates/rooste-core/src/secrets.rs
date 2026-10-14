//! Password storage, keyed by profile id. The desktop app uses the OS
//! credential store; tests use [`MemoryStore`].

use std::collections::HashMap;
use std::sync::Mutex;

/// Somewhere to keep saved passwords.
pub trait SecretStore: Send + Sync {
    fn get(&self, profile_id: &str) -> Option<String>;
    fn set(&self, profile_id: &str, password: &str) -> Result<(), String>;
    fn delete(&self, profile_id: &str);
}

/// Passwords held in memory only.
#[derive(Debug, Default)]
pub struct MemoryStore {
    secrets: Mutex<HashMap<String, String>>,
}

impl SecretStore for MemoryStore {
    fn get(&self, profile_id: &str) -> Option<String> {
        self.secrets
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(profile_id)
            .cloned()
    }

    fn set(&self, profile_id: &str, password: &str) -> Result<(), String> {
        self.secrets
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(profile_id.to_owned(), password.to_owned());
        Ok(())
    }

    fn delete(&self, profile_id: &str) {
        self.secrets
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(profile_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_store() {
        let store = MemoryStore::default();
        assert_eq!(store.get("p"), None);
        store.set("p", "secret").unwrap();
        assert_eq!(store.get("p").as_deref(), Some("secret"));
        store.delete("p");
        assert_eq!(store.get("p"), None);
    }
}
