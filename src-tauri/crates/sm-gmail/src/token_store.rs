use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};

use parking_lot::Mutex;

use crate::oauth::RefreshToken;

#[derive(Debug, thiserror::Error)]
pub enum TokenStoreError {
    #[error("token file {path}: could not {what}: {source}")]
    Io {
        path: PathBuf,
        what: &'static str,
        #[source]
        source: io::Error,
    },
    #[error("token file {path} is damaged at line {line} column {column}")]
    Damaged {
        path: PathBuf,
        line: usize,
        column: usize,
    },
}

#[derive(Debug)]
pub struct TokenFile {
    path: PathBuf,
    file: Mutex<()>,
}

impl TokenFile {
    #[must_use]
    pub const fn new(path: PathBuf) -> Self {
        Self {
            path,
            file: Mutex::new(()),
        }
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
        restrict(&file).map_err(|source| io_error(&tmp, "restrict", source))?;
        file.write_all(&json)
            .and_then(|()| file.sync_all())
            .map_err(|source| io_error(&tmp, "write", source))?;
        drop(file);
        fs::rename(&tmp, &self.path).map_err(|source| io_error(&self.path, "replace", source))?;
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

impl TokenFile {
    pub fn store(&self, email: String, token: RefreshToken) -> Result<(), TokenStoreError> {
        self.update(|tokens| {
            tokens.insert(email, token.into_secret());
            true
        })
    }

    pub fn load(&self, email: &str) -> Result<Option<RefreshToken>, TokenStoreError> {
        let _file = self.file.lock();
        Ok(self.read()?.remove(email).map(RefreshToken::new))
    }

    pub fn delete(&self, email: &str) -> Result<(), TokenStoreError> {
        self.update(|tokens| tokens.remove(email).is_some())
    }

    pub fn entries(&self) -> Result<Vec<(String, RefreshToken)>, TokenStoreError> {
        let _file = self.file.lock();
        Ok(self
            .read()?
            .into_iter()
            .map(|(email, token)| (email, RefreshToken::new(token)))
            .collect())
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
