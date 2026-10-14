//! Saved passwords in the OS credential store: Windows Credential Manager,
//! or the Secret Service (GNOME Keyring, KWallet) on Linux.

use rooste_core::SecretStore;

const SERVICE: &str = "rooste-ssh";

#[derive(Debug, Default)]
pub struct KeyringStore;

impl SecretStore for KeyringStore {
    fn get(&self, profile_id: &str) -> Option<String> {
        keyring::Entry::new(SERVICE, profile_id)
            .and_then(|entry| entry.get_password())
            .ok()
    }

    fn set(&self, profile_id: &str, password: &str) -> Result<(), String> {
        keyring::Entry::new(SERVICE, profile_id)
            .and_then(|entry| entry.set_password(password))
            .map_err(|e| format!("Could not save the password in the system keyring: {e}"))
    }

    fn delete(&self, profile_id: &str) {
        if let Ok(entry) = keyring::Entry::new(SERVICE, profile_id) {
            let _ = entry.delete_credential();
        }
    }
}
