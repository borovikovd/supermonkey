//! Refresh-token storage (SPEC: Accounts). Production uses the macOS Keychain with service
//! `ubermensch` and the email address as the account; tests use [`MemoryTokenStore`].

use std::collections::HashMap;

use parking_lot::Mutex;

use crate::oauth::RefreshToken;

/// The Keychain service name.
pub const KEYCHAIN_SERVICE: &str = "ubermensch";

/// A failure of the credential store itself (locked keychain, access denied, ...).
#[derive(Debug, thiserror::Error)]
#[error("keychain: {0}")]
pub struct TokenStoreError(pub String);

/// Where refresh tokens live, keyed by account email.
pub trait TokenStore: Send + Sync {
    /// Saves or replaces the account's refresh token.
    fn store_refresh_token(&self, email: &str, token: &RefreshToken)
    -> Result<(), TokenStoreError>;
    /// The account's refresh token, or `None` if there is none.
    fn load_refresh_token(&self, email: &str) -> Result<Option<RefreshToken>, TokenStoreError>;
    /// Deletes the account's refresh token. Deleting a missing one is not an error.
    fn delete_refresh_token(&self, email: &str) -> Result<(), TokenStoreError>;
}

/// The macOS Keychain, through the `keyring` crate.
#[derive(Debug, Default, Clone, Copy)]
pub struct Keychain;

impl Keychain {
    fn entry(email: &str) -> Result<keyring::Entry, TokenStoreError> {
        keyring::Entry::new(KEYCHAIN_SERVICE, email).map_err(|e| store_error(&e))
    }
}

fn store_error(e: &keyring::Error) -> TokenStoreError {
    TokenStoreError(e.to_string())
}

impl TokenStore for Keychain {
    fn store_refresh_token(
        &self,
        email: &str,
        token: &RefreshToken,
    ) -> Result<(), TokenStoreError> {
        Self::entry(email)?
            .set_password(&token.0)
            .map_err(|e| store_error(&e))
    }

    fn load_refresh_token(&self, email: &str) -> Result<Option<RefreshToken>, TokenStoreError> {
        match Self::entry(email)?.get_password() {
            Ok(token) => Ok(Some(RefreshToken(token))),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(store_error(&e)),
        }
    }

    fn delete_refresh_token(&self, email: &str) -> Result<(), TokenStoreError> {
        match Self::entry(email)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(store_error(&e)),
        }
    }
}

/// An in-memory store for tests.
#[derive(Debug, Default)]
pub struct MemoryTokenStore {
    tokens: Mutex<HashMap<String, RefreshToken>>,
}

impl MemoryTokenStore {
    /// Every stored account with its token, by account.
    pub fn entries(&self) -> Vec<(String, RefreshToken)> {
        let mut entries: Vec<(String, RefreshToken)> = self
            .tokens
            .lock()
            .iter()
            .map(|(email, token)| (email.clone(), token.clone()))
            .collect();
        entries.sort_unstable_by(|(a, _), (b, _)| a.cmp(b));
        entries
    }
}

impl TokenStore for MemoryTokenStore {
    fn store_refresh_token(
        &self,
        email: &str,
        token: &RefreshToken,
    ) -> Result<(), TokenStoreError> {
        self.tokens.lock().insert(email.into(), token.clone());
        Ok(())
    }

    fn load_refresh_token(&self, email: &str) -> Result<Option<RefreshToken>, TokenStoreError> {
        Ok(self.tokens.lock().get(email).cloned())
    }

    fn delete_refresh_token(&self, email: &str) -> Result<(), TokenStoreError> {
        self.tokens.lock().remove(email);
        Ok(())
    }
}
