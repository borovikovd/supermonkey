//! Attachments download on demand into the app cache and open with the
//! default app (SPEC Reading).

use std::fmt;
use std::path::PathBuf;

use rusqlite::{OptionalExtension, params};
use ub_gmail::{Outcome, Priority, cost};

use super::Engine;
use crate::model::AccountId;

/// Account, Gmail message id, filename, Gmail attachment id, cached path.
type AttachmentRow = (AccountId, String, String, Option<String>, Option<String>);

pub fn account_dir(e: &Engine, account: AccountId) -> PathBuf {
    e.data_dir.join("attachments").join(account.to_string())
}

fn safe_name(name: &str) -> String {
    let n: String = name
        .chars()
        .map(|c| {
            if c == '/' || c == ':' || c.is_control() {
                '_'
            } else {
                c
            }
        })
        .collect();
    if n.trim().is_empty() {
        "attachment".into()
    } else {
        n
    }
}

/// Why Gmail could not supply an attachment. Typed, so a draft write can tell
/// waiting for the network from a real failure.
#[derive(Debug)]
pub enum DownloadError {
    /// The account is not syncing: offline at launch, or signed out.
    NoAccount,
    Gmail(Outcome),
}

impl fmt::Display for DownloadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DownloadError::NoAccount => f.write_str("The account is offline or signed out"),
            DownloadError::Gmail(outcome) => write!(f, "Download failed: {outcome:?}"),
        }
    }
}

impl std::error::Error for DownloadError {}

/// Downloads an attachment row if it is not cached yet. Returns its path.
pub async fn ensure(e: &Engine, attachment: i64) -> anyhow::Result<PathBuf> {
    let row: Option<AttachmentRow> = e.reads.with(|c| {
        Ok(c.query_row(
            "SELECT m.account_id, m.gmail_id, a.filename, a.gmail_att_id, a.local_path
             FROM attachments a JOIN messages m ON m.id = a.message_id WHERE a.id = ?1",
            [attachment],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .optional()?)
    })?;
    let Some((account, gmail_msg, filename, att_id, local)) = row else {
        anyhow::bail!("attachment {attachment} is gone");
    };
    if let Some(p) = local.map(PathBuf::from).filter(|p| p.exists()) {
        return Ok(p);
    }
    let rt = e.account(account).ok_or(DownloadError::NoAccount)?;
    let att_id = att_id.ok_or_else(|| anyhow::anyhow!("attachment has no Gmail id"))?;
    let bytes = rt
        .sched
        .run(
            Priority::P2,
            cost::ATTACHMENTS_GET,
            rt.gmail.get_attachment(&gmail_msg, &att_id),
        )
        .await
        .map_err(|err| DownloadError::Gmail(err.outcome))?;
    let dir = account_dir(e, account).join(&gmail_msg);
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(safe_name(&filename));
    std::fs::write(&path, &bytes)?;
    let p = path.to_string_lossy().into_owned();
    e.writer
        .call(move |ctx| {
            ctx.tx.execute(
                "UPDATE attachments SET local_path = ?2 WHERE id = ?1",
                params![attachment, p],
            )?;
            Ok(())
        })
        .await?;
    Ok(path)
}

pub async fn open(e: &Engine, attachment: i64) -> anyhow::Result<()> {
    let path = ensure(e, attachment).await?;
    e.platform.open_path(&path)
}
