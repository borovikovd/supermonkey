//! Refresh-token storage (SPEC: Accounts). The app keeps them in a file only the
//! user can read, next to its database; demo mode and some tests use
//! [`MemoryTokenStore`].

use std::collections::{BTreeMap, HashMap};
use std::io::Write as _;
use std::path::PathBuf;

use parking_lot::Mutex;

use crate::oauth::RefreshToken;

/// A failure of the token store itself (a file that can't be read or written).
#[derive(Debug, thiserror::Error)]
#[error("token store: {0}")]
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

/// Refresh tokens in a JSON file, `{email: token}`, that only the user can read
/// (mode 0600). Not the macOS Keychain: an unsigned build has no stable
/// identity, so macOS asks for the login password whenever a new build reads
/// an item an older one wrote, which is after every update.
#[derive(Debug)]
pub struct TokenFile {
    path: PathBuf,
    /// Held across the file's read-modify-write, so two accounts saving at once
    /// both keep their tokens. It guards the file, not memory.
    lock: Mutex<()>,
}

impl TokenFile {
    /// The store at `path`. The file is created on the first save.
    pub fn new(path: PathBuf) -> TokenFile {
        TokenFile {
            path,
            lock: Mutex::new(()),
        }
    }

    /// Every stored account with its token, by account.
    pub fn entries(&self) -> Result<Vec<(String, RefreshToken)>, TokenStoreError> {
        let _file = self.lock.lock();
        Ok(self
            .read()?
            .into_iter()
            .map(|(email, token)| (email, RefreshToken(token)))
            .collect())
    }

    fn read(&self) -> Result<BTreeMap<String, String>, TokenStoreError> {
        match std::fs::read(&self.path) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(|e| self.error("parse", &e)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(BTreeMap::new()),
            Err(e) => Err(self.error("read", &e)),
        }
    }

    /// Writes a new file and renames it over the old one, so a crash leaves
    /// one whole file or the other.
    fn write(&self, tokens: &BTreeMap<String, String>) -> Result<(), TokenStoreError> {
        let json = serde_json::to_vec(tokens).map_err(|e| self.error("encode", &e))?;
        let tmp = self.path.with_extension("json.tmp");
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        let mut file = options.open(&tmp).map_err(|e| self.error("create", &e))?;
        file.write_all(&json)
            .and_then(|()| file.sync_all())
            .map_err(|e| self.error("write", &e))?;
        std::fs::rename(&tmp, &self.path).map_err(|e| self.error("replace", &e))
    }

    fn update(
        &self,
        change: impl FnOnce(&mut BTreeMap<String, String>),
    ) -> Result<(), TokenStoreError> {
        let _file = self.lock.lock();
        let mut tokens = self.read()?;
        change(&mut tokens);
        self.write(&tokens)
    }

    fn error(&self, what: &str, e: &dyn std::fmt::Display) -> TokenStoreError {
        TokenStoreError(format!("{what} {}: {e}", self.path.display()))
    }
}

impl TokenStore for TokenFile {
    fn store_refresh_token(
        &self,
        email: &str,
        token: &RefreshToken,
    ) -> Result<(), TokenStoreError> {
        self.update(|tokens| {
            tokens.insert(email.to_owned(), token.0.clone());
        })
    }

    fn load_refresh_token(&self, email: &str) -> Result<Option<RefreshToken>, TokenStoreError> {
        let _file = self.lock.lock();
        Ok(self.read()?.remove(email).map(RefreshToken))
    }

    fn delete_refresh_token(&self, email: &str) -> Result<(), TokenStoreError> {
        let _file = self.lock.lock();
        let mut tokens = self.read()?;
        if tokens.remove(email).is_some() {
            self.write(&tokens)?;
        }
        Ok(())
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
