//! The operating system's credential store.
//!
//! Every secret this application holds lives here and nowhere else. Not in
//! SQLite, not in a config file, not in an environment variable, not in a
//! command line, and not in the activity log. There are exactly three kinds:
//!
//! * the Cloudflare API token,
//! * an optional passphrase for a user-chosen SSH private key,
//! * nothing else — a tunnel's run token is fetched when a deployment needs
//!   it and dropped again, because storing it would double the number of
//!   places a tunnel-controlling credential exists.
//!
//! `keyring` is entirely blocking (it speaks D-Bus to the Secret Service), so
//! every call in this module must be made from a background thread. See
//! [`crate::services::runtime::blocking`].

use secrecy::{ExposeSecret, SecretString};

use crate::branding::KEYRING_SERVICE;

/// The account name the Cloudflare token is filed under.
const API_TOKEN_KEY: &str = "cloudflare-api-token";

/// Prefix for a per-host SSH key passphrase.
const HOST_PASSPHRASE_PREFIX: &str = "ssh-key-passphrase";

#[derive(Debug, thiserror::Error)]
pub enum SecretError {
    /// No Secret Service on the session bus. On a normal desktop this means
    /// gnome-keyring or KWallet is not running; over SSH or in a bare
    /// container it is expected.
    #[error("no credential store is available")]
    Unavailable { detail: String },

    /// The store is there but locked, or refused access.
    #[error("the credential store refused access")]
    Locked { detail: String },

    #[error("the credential store failed: {detail}")]
    Failed { detail: String },
}

impl SecretError {
    fn from_keyring(error: keyring::Error) -> Self {
        // The message is redacted before it is kept: a platform error can
        // quote the item it was handling.
        let detail = crate::domain::activity::redact(&error.to_string());
        match error {
            keyring::Error::NoStorageAccess(_) => Self::Locked { detail },
            keyring::Error::PlatformFailure(_) => Self::Unavailable { detail },
            _ => Self::Failed { detail },
        }
    }

    /// One sentence for a banner.
    pub fn user_message(&self) -> String {
        match self {
            Self::Unavailable { .. } => {
                "No system credential store is available, so the API token cannot be \
                 saved. Tunnel Manager will still work for this session, but you will \
                 have to paste the token again next time. On GNOME this usually means \
                 gnome-keyring is not running."
                    .into()
            }
            Self::Locked { .. } => {
                "The system credential store is locked. Unlock your login keyring and \
                 try again."
                    .into()
            }
            Self::Failed { .. } => {
                "The system credential store returned an error. The API token was not \
                 saved."
                    .into()
            }
        }
    }
}

pub type Result<T> = std::result::Result<T, SecretError>;

/// Whether a credential store is present at all.
///
/// Checked once at startup so the Connection page can say "this will not be
/// remembered" *before* the user types a token, rather than after.
pub fn store_available() -> bool {
    keyring::Entry::store_status().is_ok()
}

fn entry(key: &str) -> Result<keyring::Entry> {
    keyring::Entry::new(KEYRING_SERVICE, key).map_err(SecretError::from_keyring)
}

/// Reads a secret. `Ok(None)` means "not stored", which is not an error.
fn read(key: &str) -> Result<Option<SecretString>> {
    match entry(key)?.get_password() {
        Ok(value) => Ok(Some(SecretString::from(value))),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(error) => Err(SecretError::from_keyring(error)),
    }
}

fn write(key: &str, value: &SecretString) -> Result<()> {
    entry(key)?
        .set_password(value.expose_secret())
        .map_err(SecretError::from_keyring)
}

/// Removes a secret. Removing one that is not there succeeds — the caller
/// wanted it gone, and it is.
fn remove(key: &str) -> Result<()> {
    match entry(key)?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(error) => Err(SecretError::from_keyring(error)),
    }
}

// ── The Cloudflare API token ─────────────────────────────────────────────

pub fn load_api_token() -> Result<Option<SecretString>> {
    read(API_TOKEN_KEY)
}

pub fn save_api_token(token: &SecretString) -> Result<()> {
    write(API_TOKEN_KEY, token)
}

pub fn delete_api_token() -> Result<()> {
    remove(API_TOKEN_KEY)
}

// ── SSH key passphrases ──────────────────────────────────────────────────

fn passphrase_key(host_id: i64) -> String {
    format!("{HOST_PASSPHRASE_PREFIX}-{host_id}")
}

pub fn load_host_passphrase(host_id: i64) -> Result<Option<SecretString>> {
    read(&passphrase_key(host_id))
}

pub fn save_host_passphrase(host_id: i64, passphrase: &SecretString) -> Result<()> {
    write(&passphrase_key(host_id), passphrase)
}

pub fn delete_host_passphrase(host_id: i64) -> Result<()> {
    remove(&passphrase_key(host_id))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_are_namespaced_per_host() {
        assert_eq!(passphrase_key(1), "ssh-key-passphrase-1");
        assert_ne!(passphrase_key(1), passphrase_key(2));
    }

    #[test]
    fn the_token_key_is_distinct_from_any_host_key() {
        assert_ne!(API_TOKEN_KEY, passphrase_key(0));
        assert!(!API_TOKEN_KEY.starts_with(HOST_PASSPHRASE_PREFIX));
    }

    #[test]
    fn every_error_has_an_actionable_message() {
        let errors = [
            SecretError::Unavailable {
                detail: "no bus".into(),
            },
            SecretError::Locked {
                detail: "locked".into(),
            },
            SecretError::Failed {
                detail: "boom".into(),
            },
        ];
        for error in errors {
            let message = error.user_message();
            assert!(message.len() > 30, "{message}");
            assert!(!message.contains("keyring::"), "no crate names in UI text");
        }
    }

    #[test]
    fn store_availability_is_answerable_without_panicking() {
        // The answer depends on the machine — a headless test runner has no Secret
        // Service — so this asserts only that asking is safe.
        let _ = store_available();
    }
}
