//! The async side: Gmail sync, the op queue, the outbox, notifications.
//! It consumes writer effects and submits sync jobs back to the writer.

pub mod attachments;
pub mod auth;
pub mod contacts;
pub mod webbridge;

use std::path::PathBuf;

use tauri::AppHandle;
use tokio::sync::mpsc::UnboundedReceiver;

use crate::db::ReadPool;
use crate::writer::{Effect, Writer};

pub fn start(
    app: AppHandle,
    writer: Writer,
    reads: ReadPool,
    data_dir: PathBuf,
    mut effects: UnboundedReceiver<Effect>,
) {
    let _ = (&writer, &data_dir);
    webbridge::start(app.clone());
    tauri::async_runtime::spawn(async move {
        while let Some(effect) = effects.recv().await {
            match effect {
                Effect::Badge => update_badge(&app, &reads),
                other => log::debug!("effect {other:?}"),
            }
        }
    });
}

/// The Dock badge: unread Inbox threads across accounts that are not muted.
pub fn update_badge(app: &AppHandle, reads: &ReadPool) {
    use tauri::Manager;
    let count: i64 = reads
        .with(|c| {
            Ok(c.query_row(
                "SELECT count(*) FROM threads t JOIN accounts a ON a.id = t.account_id
                 WHERE t.in_inbox = 1 AND t.unread = 1 AND t.snooze_until IS NULL
                   AND NOT EXISTS (SELECT 1 FROM settings s WHERE s.account_id = a.id AND s.key = 'muted' AND s.value = 'on')",
                [],
                |r| r.get(0),
            )?)
        })
        .unwrap_or(0);
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.set_badge_count(if count > 0 { Some(count) } else { None });
    }
}
