//! Where refresh tokens live between launches (SPEC Accounts).
//!
//! The app keeps them in `tokens.json` in its data folder, only the user can
//! read it, and it is replaced atomically on each change. Not the Keychain: an
//! unsigned build has no stable identity, so macOS would ask for the login
//! password after every update. Tests and demo mode use [`MemoryTokenStore`].
//!
//! The calls block on file I/O: run them off the async runtime.

use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};

use parking_lot::Mutex;

use crate::oauth::RefreshToken;

/// A token store that could not be read or written.
#[derive(Debug, thiserror::Error)]
pub enum TokenStoreError {
    /// The file could not be read, written, or replaced.
    #[error("token file {path}: could not {what}: {source}")]
    Io {
        /// The file or the temporary file next to it.
        path: PathBuf,
        /// The step that failed.
        what: &'static str,
        /// The error underneath.
        #[source]
        source: io::Error,
    },
    /// The file is not the JSON object of tokens it should be. The serde
    /// message is left out, since it could quote a token.
    #[error("token file {path} is damaged at line {line} column {column}")]
    Damaged {
        /// The file.
        path: PathBuf,
        /// Where parsing failed.
        line: usize,
        /// Where parsing failed.
        column: usize,
    },
}

/// Where refresh tokens live, keyed by account address. One store per app:
/// the file, or memory for tests and demo mode.
pub trait TokenStore: Send + Sync {
    /// Saves or replaces the account's refresh token.
    fn store(&self, email: &str, token: RefreshToken) -> Result<(), TokenStoreError>;
    /// The account's refresh token, or `None` if there is none.
    fn load(&self, email: &str) -> Result<Option<RefreshToken>, TokenStoreError>;
    /// Deletes the account's refresh token. A missing one is not an error.
    fn delete(&self, email: &str) -> Result<(), TokenStoreError>;
    /// Every stored account with its token, by address.
    fn entries(&self) -> Result<Vec<(String, RefreshToken)>, TokenStoreError>;
}

/// Refresh tokens in a JSON file, `{email: token}`, mode 0600, replaced
/// atomically so a crash leaves the old file or the new one, whole.
#[derive(Debug)]
pub struct TokenFile {
    path: PathBuf,
    /// Held across a read-modify-write, so two accounts saving at once both
    /// keep their tokens. It guards the file, not memory.
    file: Mutex<()>,
}

impl TokenFile {
    /// The store at `path`. The file is made on the first save.
    #[must_use]
    pub const fn new(path: PathBuf) -> Self {
        Self {
            path,
            file: Mutex::new(()),
        }
    }

    /// The file's path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    fn read(&self) -> Result<BTreeMap<String, String>, TokenStoreError> {
        match fs::read(&self.path) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(|e| TokenStoreError::Damaged {
                path: self.path.clone(),
                line: e.line(),
                column: e.column(),
            }),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(BTreeMap::new()),
            Err(source) => Err(io_error(&self.path, "read", source)),
        }
    }

    /// Writes a temporary file only the user can read, flushes it to disk,
    /// and renames it over the old one.
    fn write(&self, tokens: &BTreeMap<String, String>) -> Result<(), TokenStoreError> {
        let json = serde_json::to_vec(tokens).map_err(|source| {
            io_error(
                &self.path,
                "encode",
                io::Error::other(format!("{:?}", source.classify())),
            )
        })?;
        if let Some(dir) = self.path.parent() {
            fs::create_dir_all(dir).map_err(|source| io_error(dir, "create the folder", source))?;
        }
        let tmp = self.path.with_extension("json.tmp");
        let mut file = owner_only()
            .open(&tmp)
            .map_err(|source| io_error(&tmp, "create", source))?;
        // A stale temporary file keeps its old mode through `open`.
        restrict(&file).map_err(|source| io_error(&tmp, "restrict", source))?;
        file.write_all(&json)
            .and_then(|()| file.sync_all())
            .map_err(|source| io_error(&tmp, "write", source))?;
        drop(file);
        fs::rename(&tmp, &self.path).map_err(|source| io_error(&self.path, "replace", source))?;
        // Make the rename itself durable.
        if let Some(dir) = self.path.parent()
            && let Err(err) = fs::File::open(dir).and_then(|d| d.sync_all())
        {
            tracing::warn!(error = %err, "token file folder not flushed");
        }
        Ok(())
    }

    fn update(
        &self,
        change: impl FnOnce(&mut BTreeMap<String, String>) -> bool,
    ) -> Result<(), TokenStoreError> {
        let _file = self.file.lock();
        let mut tokens = self.read()?;
        if change(&mut tokens) {
            self.write(&tokens)?;
        }
        Ok(())
    }
}

fn io_error(path: &Path, what: &'static str, source: io::Error) -> TokenStoreError {
    TokenStoreError::Io {
        path: path.to_path_buf(),
        what,
        source,
    }
}

#[cfg(unix)]
fn owner_only() -> fs::OpenOptions {
    use std::os::unix::fs::OpenOptionsExt as _;
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true).mode(0o600);
    options
}

#[cfg(unix)]
fn restrict(file: &fs::File) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    file.set_permissions(fs::Permissions::from_mode(0o600))
}

impl TokenStore for TokenFile {
    fn store(&self, email: &str, token: RefreshToken) -> Result<(), TokenStoreError> {
        self.update(|tokens| {
            tokens.insert(email.to_owned(), token.secret().to_owned());
            true
        })
    }

    fn load(&self, email: &str) -> Result<Option<RefreshToken>, TokenStoreError> {
        let _file = self.file.lock();
        Ok(self.read()?.remove(email).map(RefreshToken::new))
    }

    fn delete(&self, email: &str) -> Result<(), TokenStoreError> {
        self.update(|tokens| tokens.remove(email).is_some())
    }

    fn entries(&self) -> Result<Vec<(String, RefreshToken)>, TokenStoreError> {
        let _file = self.file.lock();
        Ok(self
            .read()?
            .into_iter()
            .map(|(email, token)| (email, RefreshToken::new(token)))
            .collect())
    }
}

/// Refresh tokens in memory, for tests and demo mode, which must keep tokens
/// out of the user's data folder.
#[derive(Debug, Default)]
pub struct MemoryTokenStore {
    tokens: Mutex<BTreeMap<String, RefreshToken>>,
}

impl TokenStore for MemoryTokenStore {
    fn store(&self, email: &str, token: RefreshToken) -> Result<(), TokenStoreError> {
        self.tokens.lock().insert(email.to_owned(), token);
        Ok(())
    }

    fn load(&self, email: &str) -> Result<Option<RefreshToken>, TokenStoreError> {
        Ok(self.tokens.lock().get(email).cloned())
    }

    fn delete(&self, email: &str) -> Result<(), TokenStoreError> {
        self.tokens.lock().remove(email);
        Ok(())
    }

    fn entries(&self) -> Result<Vec<(String, RefreshToken)>, TokenStoreError> {
        Ok(self
            .tokens
            .lock()
            .iter()
            .map(|(email, token)| (email.clone(), token.clone()))
            .collect())
    }
}
