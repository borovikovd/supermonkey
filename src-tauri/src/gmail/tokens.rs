use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use parking_lot::Mutex;
use thiserror::Error;

use super::oauth::RefreshToken;

#[derive(Debug, Error)]
pub enum TokenStoreError {
    #[error("couldn't {action} the token file: {source}")]
    Io {
        action: &'static str,
        #[source]
        source: std::io::Error,
    },
    #[error("the token file is not valid JSON")]
    Corrupt,
}

#[derive(Debug)]
pub struct TokenFile {
    path: PathBuf,
    lock: Mutex<()>,
}

impl TokenFile {
    #[must_use]
    pub const fn new(path: PathBuf) -> Self {
        Self {
            path,
            lock: Mutex::new(()),
        }
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn load(&self, email: &str) -> Result<Option<RefreshToken>, TokenStoreError> {
        let _guard = self.lock.lock();
        let map = self.read_map()?;
        Ok(map.get(&normalize(email)).map(RefreshToken::new))
    }

    pub fn store(&self, email: &str, token: &RefreshToken) -> Result<(), TokenStoreError> {
        let _guard = self.lock.lock();
        let mut map = self.read_map()?;
        map.insert(normalize(email), token.expose().to_owned());
        self.write_map(&map)
    }

    pub fn delete(&self, email: &str) -> Result<bool, TokenStoreError> {
        let _guard = self.lock.lock();
        let mut map = self.read_map()?;
        let removed = map.remove(&normalize(email)).is_some();
        if removed {
            self.write_map(&map)?;
        }
        Ok(removed)
    }

    pub fn entries(&self) -> Result<Vec<(String, RefreshToken)>, TokenStoreError> {
        let _guard = self.lock.lock();
        let map = self.read_map()?;
        Ok(map
            .into_iter()
            .map(|(k, v)| (k, RefreshToken::new(v)))
            .collect())
    }

    pub async fn load_async(
        self: &Arc<Self>,
        email: String,
    ) -> Result<Option<RefreshToken>, TokenStoreError> {
        let store = Arc::clone(self);
        tokio::task::spawn_blocking(move || store.load(&email))
            .await
            .map_err(|_join| TokenStoreError::Io {
                action: "join token read for",
                source: std::io::Error::other("blocking task panicked"),
            })?
    }

    pub async fn store_async(
        self: &Arc<Self>,
        email: String,
        token: RefreshToken,
    ) -> Result<(), TokenStoreError> {
        let store = Arc::clone(self);
        tokio::task::spawn_blocking(move || store.store(&email, &token))
            .await
            .map_err(|_join| TokenStoreError::Io {
                action: "join token write for",
                source: std::io::Error::other("blocking task panicked"),
            })?
    }

    pub async fn delete_async(self: &Arc<Self>, email: String) -> Result<bool, TokenStoreError> {
        let store = Arc::clone(self);
        tokio::task::spawn_blocking(move || store.delete(&email))
            .await
            .map_err(|_join| TokenStoreError::Io {
                action: "join token delete for",
                source: std::io::Error::other("blocking task panicked"),
            })?
    }

    fn read_map(&self) -> Result<BTreeMap<String, String>, TokenStoreError> {
        let raw = match fs::read_to_string(&self.path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
            Err(source) => {
                return Err(TokenStoreError::Io {
                    action: "read",
                    source,
                });
            }
        };
        serde_json::from_str(&raw).map_err(|_json| TokenStoreError::Corrupt)
    }

    fn write_map(&self, map: &BTreeMap<String, String>) -> Result<(), TokenStoreError> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).map_err(|source| TokenStoreError::Io {
                action: "create folder for",
                source,
            })?;
        }
        let encoded = serde_json::to_vec_pretty(map).map_err(|_json| TokenStoreError::Corrupt)?;
        let tmp = tmp_path(&self.path);
        let mut file = open_private(&tmp)?;
        file.write_all(&encoded)
            .map_err(|source| TokenStoreError::Io {
                action: "write",
                source,
            })?;
        file.sync_all().map_err(|source| TokenStoreError::Io {
            action: "sync",
            source,
        })?;
        drop(file);
        restrict(&tmp)?;
        fs::rename(&tmp, &self.path).map_err(|source| TokenStoreError::Io {
            action: "replace",
            source,
        })?;
        if let Some(parent) = self.path.parent()
            && let Ok(dir) = File::open(parent)
        {
            drop(dir.sync_all());
        }
        Ok(())
    }
}

fn normalize(email: &str) -> String {
    email.trim().to_ascii_lowercase()
}

fn tmp_path(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(".tmp");
    PathBuf::from(name)
}

#[cfg(unix)]
fn open_private(path: &Path) -> Result<File, TokenStoreError> {
    use std::os::unix::fs::OpenOptionsExt as _;
    OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
        .map_err(|source| TokenStoreError::Io {
            action: "create",
            source,
        })
}

#[cfg(not(unix))]
fn open_private(path: &Path) -> Result<File, TokenStoreError> {
    OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(path)
        .map_err(|source| TokenStoreError::Io {
            action: "create",
            source,
        })
}

#[cfg(unix)]
fn restrict(path: &Path) -> Result<(), TokenStoreError> {
    use std::os::unix::fs::PermissionsExt as _;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).map_err(|source| {
        TokenStoreError::Io {
            action: "set permissions on",
            source,
        }
    })
}

#[cfg(not(unix))]
const fn restrict(_path: &Path) -> Result<(), TokenStoreError> {
    Ok(())
}
