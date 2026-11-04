//! Where the app keeps files in its data folder: downloaded attachments,
//! one folder per account, and the files added to drafts, one folder per
//! draft.

use std::path::{Path, PathBuf};

use ub_domain::{AccountId, DraftId};

/// The folder of downloaded attachments.
pub(crate) const ATTACHMENTS_DIR: &str = "attachments";
/// The folder of files added to drafts.
const DRAFTS_DIR: &str = "drafts";

/// The folder of an account's downloaded attachments.
pub(crate) fn attachments_dir(data_dir: &Path, account: AccountId) -> PathBuf {
    data_dir.join(ATTACHMENTS_DIR).join(account.0.to_string())
}

/// The folder of a draft's added files, kept until it is sent or discarded.
/// The draft id is the UI's, so it is made safe as a folder name.
pub(crate) fn draft_dir(data_dir: &Path, draft: &DraftId) -> PathBuf {
    data_dir.join(DRAFTS_DIR).join(safe_name(draft.as_str()))
}

/// A file name safe to create: no path separators or control characters,
/// and never `.` or `..`.
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

/// Whether `path` is a file inside `dir`, links resolved. A draft's
/// attachment paths come from the webview, which is not trusted: only files
/// the app keeps may be read and sent.
pub(crate) fn is_inside(path: &Path, dir: &Path) -> bool {
    match (std::fs::canonicalize(path), std::fs::canonicalize(dir)) {
        (Ok(path), Ok(dir)) => path.starts_with(dir) && path.is_file(),
        _ => false,
    }
}
