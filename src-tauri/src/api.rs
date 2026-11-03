//! The command surface without Tauri: what the Tauri commands and the web
//! bridge both run, so the browser-driven E2E suite exercises the same code.

use crate::engine::Engine;
use crate::model::{
    AccountId, AttachmentId, ContactSuggestion, DraftAttachment, LaunchInfo, ListRow, MessageBody,
    MessageId, NumberedAction, SnoozeChoice,
};
use std::sync::Arc;

pub type CmdResult<T> = Result<T, String>;

fn err(e: impl std::fmt::Display) -> String {
    format!("{e:#}")
}

/// Where the page's action numbers continue.
pub fn launch_info(e: &Engine) -> CmdResult<LaunchInfo> {
    let last_action = e.reads.with(crate::db::queries::last_action).map_err(err)?;
    Ok(LaunchInfo {
        last_action,
        debug: cfg!(debug_assertions),
    })
}

/// Queues UI actions in number order. Returns as soon as they are queued.
pub fn submit(e: &Engine, actions: Vec<NumberedAction>) {
    e.writer.actions(actions);
}

/// Rows outside the pushed window, read on a separate connection with their `rev`.
pub fn fetch_rows(e: &Engine, drafts: bool, keys: &[String]) -> CmdResult<Vec<ListRow>> {
    e.reads
        .with(|c| crate::db::queries::rows_for_keys(c, drafts, keys))
        .map_err(err)
}

/// Sanitized bodies. A message without a stored body is left out.
pub fn get_bodies(e: &Engine, ids: &[MessageId]) -> CmdResult<Vec<MessageBody>> {
    e.reads
        .with(|c| crate::db::queries::bodies(c, ids))
        .map_err(err)
}

pub fn autocomplete(
    e: &Engine,
    account: AccountId,
    query: &str,
) -> CmdResult<Vec<ContactSuggestion>> {
    e.reads
        .with(|c| crate::engine::contacts::suggest(c, account, query, 6))
        .map_err(err)
}

/// Runs Google sign-in in the browser and adds the account.
pub async fn add_account(e: &Arc<Engine>) -> CmdResult<AccountId> {
    crate::engine::auth::add_account(e).await.map_err(err)
}

pub async fn sign_in_again(e: &Arc<Engine>, account: AccountId) -> CmdResult<()> {
    crate::engine::auth::sign_in_again(e, account)
        .await
        .map_err(err)
}

/// Downloads an attachment if needed and opens it with the default app.
pub async fn open_attachment(e: &Engine, attachment: AttachmentId) -> CmdResult<()> {
    crate::engine::attachments::open(e, attachment)
        .await
        .map_err(err)
}

/// Opens a link from a message in the default browser or mail app: the web
/// build's stand-in for the opener plugin, which the app's webview calls itself.
pub fn open_url(e: &Engine, url: &str) -> CmdResult<()> {
    let parsed = reqwest::Url::parse(url).map_err(err)?;
    if !matches!(parsed.scheme(), "http" | "https" | "mailto") {
        return Err(format!("not a web or mail link: {}", parsed.scheme()));
    }
    e.platform.open_url(url);
    Ok(())
}

/// Copies a file the user picked into the draft's folder (SPEC Compose: attachments).
pub fn add_attachment(
    draft: &str,
    filename: &str,
    mime_type: &str,
    data: &str,
) -> CmdResult<DraftAttachment> {
    store_attachment(draft, filename, mime_type, data).map_err(err)
}

fn store_attachment(
    draft: &str,
    filename: &str,
    mime_type: &str,
    data_b64: &str,
) -> anyhow::Result<DraftAttachment> {
    use base64::Engine as _;
    let bytes = base64::engine::general_purpose::STANDARD.decode(data_b64)?;
    let dir = crate::drafts::draft_dir(draft);
    std::fs::create_dir_all(&dir)?;
    let id = uuid::Uuid::new_v4().to_string();
    let safe: String = filename
        .chars()
        .map(|c| if c == '/' || c == ':' { '_' } else { c })
        .collect();
    let path = dir.join(format!("{}-{safe}", &id[..8]));
    std::fs::write(&path, &bytes)?;
    Ok(DraftAttachment {
        id,
        filename: filename.to_string(),
        mime_type: if mime_type.is_empty() {
            "application/octet-stream".into()
        } else {
            mime_type.to_string()
        },
        size: i64::try_from(bytes.len())?,
        path: Some(path.to_string_lossy().into_owned()),
        source: None,
    })
}

/// Debug builds: the keydown-to-paint meter reports actions over budget.
pub fn perf_report(action: &str, ms: f64, budget: f64) {
    if ms > budget {
        log::warn!("over budget: {action} took {ms:.1} ms (budget {budget} ms)");
    } else {
        log::debug!("{action}: {ms:.1} ms");
    }
}

/// The snooze picker: later today, tomorrow morning, next week, and a typed date and time.
pub fn snooze_options(text: &str) -> Vec<SnoozeChoice> {
    use chrono::Local;
    use ub_rules::{SnoozeKind, parse_snooze_text, snooze_options};
    let now = crate::clock::now_local();
    let fmt = |d: &chrono::DateTime<Local>| d.format("%a %b %-d, %-I:%M %p").to_string();
    let mut out: Vec<SnoozeChoice> = Vec::new();
    let text = text.trim();
    if !text.is_empty()
        && let Some(at) = parse_snooze_text(text, &now)
    {
        out.push(SnoozeChoice {
            label: fmt(&at),
            at: at.timestamp_millis(),
        });
    }
    let needle = text.to_lowercase();
    for o in snooze_options(&now) {
        let name = match o.kind {
            SnoozeKind::LaterToday => "Later today",
            SnoozeKind::TomorrowMorning => "Tomorrow morning",
            SnoozeKind::NextWeek => "Next week",
        };
        if name.to_lowercase().contains(&needle) {
            out.push(SnoozeChoice {
                label: format!("{name} · {}", fmt(&o.at)),
                at: o.at.timestamp_millis(),
            });
        }
    }
    out
}
