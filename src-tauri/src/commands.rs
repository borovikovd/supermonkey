//! The Tauri command surface: thin wrappers over [`crate::api`]. Every
//! argument and result type is generated into `src/bindings/` by ts-rs.

#![expect(
    clippy::needless_pass_by_value,
    reason = "Tauri commands receive owned arguments by design."
)]

use std::sync::Arc;
use tauri::ipc::Channel;
use tauri::{AppHandle, Manager, State};

use crate::api::{self, CmdResult};
use crate::engine::Engine;
use crate::model::{
    AccountId, AttachmentId, ChannelMessage, ContactSuggestion, DraftAttachment, DraftId,
    LaunchInfo, ListRow, MessageBody, MessageId, NumberedAction, SnoozeChoice,
};

// Commands that read SQLite or files are `async`: a plain command runs on
// the main thread, where a slow read would stall the window.

/// The page's first call: where channel messages go, and where action numbers continue.
#[tauri::command]
pub fn connect(
    app: AppHandle,
    engine: State<'_, Arc<Engine>>,
    channel: Channel<ChannelMessage>,
) -> CmdResult<LaunchInfo> {
    engine.writer.set_channel(move |msg| {
        let _ = channel.send(msg);
    });
    let info = api::launch_info(&engine)?;
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.set_focus();
    }
    Ok(info)
}

#[tauri::command]
pub fn submit(engine: State<'_, Arc<Engine>>, actions: Vec<NumberedAction>) {
    api::submit(&engine, actions);
}

#[tauri::command(async)]
pub fn fetch_rows(
    engine: State<'_, Arc<Engine>>,
    drafts: bool,
    keys: Vec<String>,
) -> CmdResult<Vec<ListRow>> {
    api::fetch_rows(&engine, drafts, &keys)
}

#[tauri::command(async)]
pub fn get_bodies(
    engine: State<'_, Arc<Engine>>,
    ids: Vec<MessageId>,
) -> CmdResult<Vec<MessageBody>> {
    api::get_bodies(&engine, &ids)
}

#[tauri::command(async)]
pub fn autocomplete(
    engine: State<'_, Arc<Engine>>,
    account: AccountId,
    query: String,
) -> CmdResult<Vec<ContactSuggestion>> {
    api::autocomplete(&engine, account, &query)
}

#[tauri::command]
pub async fn add_account(engine: State<'_, Arc<Engine>>) -> CmdResult<AccountId> {
    api::add_account(&engine).await
}

#[tauri::command]
pub async fn sign_in_again(engine: State<'_, Arc<Engine>>, account: AccountId) -> CmdResult<()> {
    api::sign_in_again(&engine, account).await
}

#[tauri::command]
pub async fn open_attachment(
    engine: State<'_, Arc<Engine>>,
    attachment: AttachmentId,
) -> CmdResult<()> {
    api::open_attachment(&engine, attachment).await
}

#[tauri::command(async)]
pub fn add_attachment(
    draft: DraftId,
    filename: String,
    mime_type: String,
    data: String,
) -> CmdResult<DraftAttachment> {
    api::add_attachment(&draft, &filename, &mime_type, &data)
}

/// Quits after the UI has closed an open composer (SPEC Undo send: Cmd+Q).
#[tauri::command]
pub fn quit_now(app: AppHandle) {
    quit(&app);
}

/// Exits once the writer has committed the UI jobs queued so far, so the
/// last actions the page sent are not lost; queued sync jobs are dropped.
pub fn quit(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        if let Some(engine) = app.try_state::<Arc<Engine>>() {
            engine.shutdown().await;
        }
        app.exit(0);
    });
}

#[tauri::command]
pub fn perf_report(action: String, ms: f64, budget: f64) {
    api::perf_report(&action, ms, budget);
}

#[tauri::command]
pub fn snooze_options(text: String) -> Vec<SnoozeChoice> {
    api::snooze_options(&text)
}
