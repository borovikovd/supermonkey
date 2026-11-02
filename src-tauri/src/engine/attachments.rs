//! Attachments download on demand into the app cache and open with the
//! default app (SPEC Reading).

use std::path::PathBuf;

use rusqlite::{OptionalExtension, params};
use tauri::{AppHandle, Manager};
use tauri_plugin_opener::OpenerExt;
use ub_gmail::{Priority, cost};

use super::EngineRef;
use crate::model::*;

/// Account, Gmail message id, filename, Gmail attachment id, cached path.
type AttachmentRow = (AccountId, String, String, Option<String>, Option<String>);

pub fn account_dir(e: &EngineRef, account: AccountId) -> PathBuf {
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

/// Downloads an attachment row if it is not cached yet. Returns its path.
pub async fn ensure(e: &EngineRef, attachment: i64) -> anyhow::Result<PathBuf> {
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
    let rt = e
        .account(account)
        .ok_or_else(|| anyhow::anyhow!("The account is offline or signed out"))?;
    let att_id = att_id.ok_or_else(|| anyhow::anyhow!("attachment has no Gmail id"))?;
    let bytes = rt
        .sched
        .run(
            Priority::P2,
            cost::ATTACHMENTS_GET,
            rt.gmail.get_attachment(&gmail_msg, &att_id),
        )
        .await
        .map_err(|err| anyhow::anyhow!("Download failed: {:?}", err.outcome))?;
    let dir = account_dir(e, account).join(&gmail_msg);
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(safe_name(&filename));
    std::fs::write(&path, &bytes)?;
    let p = path.to_string_lossy().to_string();
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

pub async fn open(app: &AppHandle, attachment: i64) -> anyhow::Result<()> {
    let e = app.state::<EngineRef>().inner().clone();
    let path = ensure(&e, attachment).await?;
    app.opener()
        .open_path(path.to_string_lossy(), None::<&str>)
        .map_err(|err| anyhow::anyhow!("{err}"))?;
    Ok(())
}
