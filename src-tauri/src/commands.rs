//! The Tauri command surface. Every argument and result type is generated
//! into `src/bindings/` by ts-rs.

#![expect(
    clippy::needless_pass_by_value,
    reason = "Tauri commands receive owned arguments by design."
)]

use tauri::ipc::Channel;
use tauri::{AppHandle, Manager, State};

use crate::Core;
use crate::model::{
    AccountId, ChannelMessage, ContactSuggestion, DraftAttachment, DraftId, LaunchInfo, ListRow,
    MessageBody, MessageId, NumberedAction, SnoozeChoice,
};

type CmdResult<T> = Result<T, String>;

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

/// The page's first call: where channel messages go, and where action numbers continue.
#[tauri::command]
pub fn connect(
    app: AppHandle,
    core: State<'_, Core>,
    channel: Channel<ChannelMessage>,
) -> CmdResult<LaunchInfo> {
    core.writer.set_channel(move |msg| {
        let _ = channel.send(msg);
    });
    let last_action = core
        .reads
        .with(crate::db::queries::last_action)
        .map_err(err)?;
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.set_focus();
    }
    Ok(LaunchInfo {
        last_action,
        debug: cfg!(debug_assertions),
    })
}

/// Queues UI actions in number order. Returns as soon as they are queued.
#[tauri::command]
pub fn submit(core: State<'_, Core>, actions: Vec<NumberedAction>) {
    core.writer.actions(actions);
}

/// Rows outside the pushed window, read on a separate connection with their `rev`.
#[tauri::command]
pub fn fetch_rows(
    core: State<'_, Core>,
    drafts: bool,
    keys: Vec<String>,
) -> CmdResult<Vec<ListRow>> {
    core.reads
        .with(|c| crate::db::queries::rows_for_keys(c, drafts, &keys))
        .map_err(err)
}

/// Sanitized bodies. A message without a stored body is left out.
#[tauri::command]
pub fn get_bodies(core: State<'_, Core>, ids: Vec<MessageId>) -> CmdResult<Vec<MessageBody>> {
    core.reads
        .with(|c| crate::db::queries::bodies(c, &ids))
        .map_err(err)
}

#[tauri::command]
pub fn autocomplete(
    core: State<'_, Core>,
    account: AccountId,
    query: String,
) -> CmdResult<Vec<ContactSuggestion>> {
    core.reads
        .with(|c| crate::engine::contacts::suggest(c, account, &query, 6))
        .map_err(err)
}

/// Runs Google sign-in in the system browser and adds the account.
#[tauri::command]
pub async fn add_account(app: AppHandle) -> CmdResult<AccountId> {
    crate::engine::auth::add_account(&app).await.map_err(err)
}

#[tauri::command]
pub async fn sign_in_again(app: AppHandle, account: AccountId) -> CmdResult<()> {
    crate::engine::auth::sign_in_again(&app, account)
        .await
        .map_err(err)
}

/// Downloads an attachment if needed and opens it with the default app.
#[tauri::command]
pub async fn open_attachment(app: AppHandle, attachment: i64) -> CmdResult<()> {
    crate::engine::attachments::open(&app, attachment)
        .await
        .map_err(err)
}

/// Copies a file the user picked into the draft's folder (SPEC Compose: attachments).
#[tauri::command]
pub fn add_attachment(
    draft: DraftId,
    filename: String,
    mime_type: String,
    data: String,
) -> CmdResult<DraftAttachment> {
    store_attachment(&draft, &filename, &mime_type, &data).map_err(err)
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

/// Quits after the UI has closed an open composer (SPEC Undo send: Cmd+Q).
#[tauri::command]
pub fn quit_now(app: AppHandle) {
    app.exit(0);
}

/// Debug builds: the keydown-to-paint meter reports actions over budget.
#[tauri::command]
pub fn perf_report(action: String, ms: f64, budget: f64) {
    if ms > budget {
        log::warn!("over budget: {action} took {ms:.1} ms (budget {budget} ms)");
    } else {
        log::debug!("{action}: {ms:.1} ms");
    }
}

/// The snooze picker: later today, tomorrow morning, next week, and a typed date and time.
#[tauri::command]
pub fn snooze_options(text: String) -> Vec<SnoozeChoice> {
    use chrono::Local;
    use ub_rules::{SnoozeKind, parse_snooze_text, snooze_options};
    let now = Local::now();
    let fmt = |d: &chrono::DateTime<Local>| d.format("%a %b %-d, %-I:%M %p").to_string();
    let mut out: Vec<SnoozeChoice> = Vec::new();
    let text = text.trim();
    if !text.is_empty()
        && let Some(at) = parse_snooze_text(text, now)
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
