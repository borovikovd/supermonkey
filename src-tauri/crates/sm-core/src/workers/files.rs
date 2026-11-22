use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use parking_lot::Mutex;
use sm_domain::{AccountId, DraftId};
use sm_store::ReadPool;

use crate::Error;
use crate::activity::Tasks;
use crate::read::read;

pub(crate) const ATTACHMENTS_DIR: &str = "attachments";
const DRAFTS_DIR: &str = "drafts";

pub(crate) fn attachments_dir(data_dir: &Path, account: AccountId) -> PathBuf {
    data_dir.join(ATTACHMENTS_DIR).join(account.0.to_string())
}

pub(crate) struct DraftFiles {
    dir: PathBuf,
    gone: Mutex<HashSet<DraftId>>,
    tasks: Tasks,
}

impl DraftFiles {
    pub(crate) fn new(data_dir: &Path, tasks: Tasks) -> Self {
        Self {
            dir: data_dir.join(DRAFTS_DIR),
            gone: Mutex::default(),
            tasks,
        }
    }

    pub(crate) fn dir(&self, draft: &DraftId) -> PathBuf {
        self.dir.join(draft_folder(draft))
    }

    pub(crate) async fn keep(
        &self,
        draft: DraftId,
        filename: &str,
        data: Vec<u8>,
    ) -> Result<(String, PathBuf), Error> {
        let dir = self.dir(&draft);
        let name = safe_name(filename);
        let kept = tokio::task::spawn_blocking(move || keep_file(&dir, &name, &data)).await??;
        if self.gone.lock().contains(&draft) {
            self.delete(draft);
            return Err(Error::DraftGone);
        }
        Ok(kept)
    }

    pub(crate) fn delete(&self, draft: DraftId) {
        let dir = self.dir(&draft);
        self.gone.lock().insert(draft);
        self.tasks.spawn("delete draft files", async move {
            let deleted =
                tokio::task::spawn_blocking(move || match std::fs::remove_dir_all(&dir) {
                    Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
                    _ => Ok(()),
                })
                .await;
            match deleted {
                Ok(Ok(())) => {}
                Ok(Err(e)) => tracing::warn!(error = %e, "couldn't delete a draft's files"),
                Err(e) => tracing::warn!(error = %e, "deleting a draft's files failed"),
            }
        });
    }

    pub(crate) fn back(&self, draft: &DraftId) {
        self.gone.lock().remove(draft);
    }

    pub(crate) async fn sweep(&self, pool: &Arc<ReadPool>) {
        if let Err(e) = self.delete_left(pool).await {
            tracing::warn!(error = %e, "the launch clean-up of draft files failed; keeping them");
        }
    }

    async fn delete_left(&self, pool: &Arc<ReadPool>) -> Result<(), Error> {
        let dir = self.dir.clone();
        let scanned = dir.clone();
        let found = tokio::task::spawn_blocking(move || folders(&scanned)).await?;
        if found.is_empty() {
            return Ok(());
        }
        let kept: HashSet<String> = read(pool, |r| r.drafts().ids())
            .await?
            .iter()
            .map(draft_folder)
            .collect();
        tokio::task::spawn_blocking(move || delete_folders(&dir, found, &kept)).await?;
        Ok(())
    }
}

fn draft_folder(draft: &DraftId) -> String {
    safe_name(draft.as_str())
}

fn keep_file(dir: &Path, name: &str, data: &[u8]) -> Result<(String, PathBuf), std::io::Error> {
    std::fs::create_dir_all(dir)?;
    for n in 1u32.. {
        let slot = dir.join(n.to_string());
        #[expect(
            clippy::create_dir,
            reason = "the folder must be new: making it reserves its number, and `create_dir_all` would take one that is there"
        )]
        match std::fs::create_dir(&slot) {
            Ok(()) => {
                let path = slot.join(name);
                std::fs::write(&path, data)?;
                return Ok((format!("file-{n}"), path));
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e),
        }
    }
    Err(std::io::Error::other("no free slot for the file"))
}

pub(crate) fn folders(dir: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|t| t.is_dir()))
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect()
}

pub(crate) fn delete_folders(dir: &Path, found: Vec<String>, kept: &HashSet<String>) {
    for name in found.into_iter().filter(|name| !kept.contains(name)) {
        if let Err(e) = std::fs::remove_dir_all(dir.join(name)) {
            tracing::warn!(error = %e, "couldn't delete files left behind");
        }
    }
}

pub(crate) fn safe_name(name: &str) -> String {
    let safe: String = name
        .chars()
        .map(|c| {
            if c == '/' || c == ':' || c == '\\' || c.is_control() {
                '_'
            } else {
                c
            }
        })
        .collect();
    if safe.trim().is_empty() || safe == "." || safe == ".." {
        "attachment".to_owned()
    } else {
        safe
    }
}

pub(crate) fn is_inside(path: &Path, dir: &Path) -> bool {
    match (std::fs::canonicalize(path), std::fs::canonicalize(dir)) {
        (Ok(path), Ok(dir)) => path.starts_with(dir) && path.is_file(),
        _ => false,
    }
}
