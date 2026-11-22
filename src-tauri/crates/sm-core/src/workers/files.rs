use std::path::{Path, PathBuf};

use sm_domain::{AccountId, DraftId};

pub(crate) const ATTACHMENTS_DIR: &str = "attachments";
const DRAFTS_DIR: &str = "drafts";

pub(crate) fn attachments_dir(data_dir: &Path, account: AccountId) -> PathBuf {
    data_dir.join(ATTACHMENTS_DIR).join(account.0.to_string())
}

pub(crate) fn drafts_dir(data_dir: &Path) -> PathBuf {
    data_dir.join(DRAFTS_DIR)
}

pub(crate) fn draft_folder(draft: &DraftId) -> String {
    safe_name(draft.as_str())
}

pub(crate) fn draft_dir(data_dir: &Path, draft: &DraftId) -> PathBuf {
    drafts_dir(data_dir).join(draft_folder(draft))
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
