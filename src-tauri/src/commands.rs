//! The Tauri commands: typed wrappers over [`ub_bridge::commands`], so the
//! app and the debug web bridge run the same code. Commands that read or
//! wait are `async`; a plain command runs on the main thread.

#![expect(
    clippy::needless_pass_by_value,
    reason = "Tauri hands commands owned arguments and state"
)]

use tauri::ipc::Channel;
use tauri::{AppHandle, State};
use ub_bridge::commands::{self as cmd, CmdResult};
use ub_domain::{AccountId, AttachmentId, DraftId, MessageId, ThreadId};
use ub_ipc::{
    BlockResult, BlockedSenders, ChannelMessage, ContactSuggestion, DraftAttachment, LaunchInfo,
    ListRow, MessageBody, NumberedAction, SnoozeChoice, Unsubscribed,
};

use crate::Shell;

/// The page's first call: channel messages go to `channel` from now on.
/// Shows the window, which starts hidden until the UI is ready.
#[tauri::command]
pub(crate) async fn connect(
    app: AppHandle,
    shell: State<'_, Shell>,
    channel: Channel<ChannelMessage>,
) -> CmdResult<LaunchInfo> {
    let sink = Box::new(move |message: ChannelMessage| {
        if let Err(e) = channel.send(message) {
            tracing::debug!(error = %e, "the page's channel is gone");
        }
    });
    let info = cmd::connect(&shell.core, sink).await?;
    crate::show_main(&app);
    Ok(info)
}

#[tauri::command]
pub(crate) fn submit(shell: State<'_, Shell>, actions: Vec<NumberedAction>) {
    cmd::submit(&shell.core, actions);
}

#[tauri::command]
pub(crate) async fn fetch_rows(
    shell: State<'_, Shell>,
    drafts: bool,
    keys: Vec<String>,
) -> CmdResult<Vec<ListRow>> {
    cmd::fetch_rows(&shell.core, drafts, keys).await
}

#[tauri::command]
pub(crate) async fn get_bodies(
    shell: State<'_, Shell>,
    ids: Vec<MessageId>,
) -> CmdResult<Vec<MessageBody>> {
    cmd::get_bodies(&shell.core, ids).await
}

#[tauri::command]
pub(crate) async fn autocomplete(
    shell: State<'_, Shell>,
    account: AccountId,
    query: String,
) -> CmdResult<Vec<ContactSuggestion>> {
    cmd::autocomplete(&shell.core, account, query).await
}

#[tauri::command]
pub(crate) async fn add_account(shell: State<'_, Shell>) -> CmdResult<AccountId> {
    cmd::add_account(&shell.core).await
}

#[tauri::command]
pub(crate) async fn sign_in_again(shell: State<'_, Shell>, account: AccountId) -> CmdResult<()> {
    cmd::sign_in_again(&shell.core, account).await
}

#[tauri::command]
pub(crate) async fn open_attachment(
    shell: State<'_, Shell>,
    attachment: AttachmentId,
) -> CmdResult<()> {
    cmd::open_attachment(&shell.core, attachment).await
}

#[tauri::command]
pub(crate) fn open_url(shell: State<'_, Shell>, url: String) -> CmdResult<()> {
    cmd::open_url(&shell.core, &url)
}

#[tauri::command]
pub(crate) async fn unsubscribe(
    shell: State<'_, Shell>,
    account: AccountId,
    thread: ThreadId,
) -> CmdResult<Unsubscribed> {
    cmd::unsubscribe(&shell.core, account, thread).await
}

#[tauri::command]
pub(crate) async fn block(
    shell: State<'_, Shell>,
    account: AccountId,
    thread: ThreadId,
    domain: bool,
) -> CmdResult<BlockResult> {
    cmd::block(&shell.core, account, thread, domain).await
}

#[tauri::command]
pub(crate) async fn blocked_senders(
    shell: State<'_, Shell>,
    account: AccountId,
) -> CmdResult<BlockedSenders> {
    cmd::blocked_senders(&shell.core, account).await
}

#[tauri::command]
pub(crate) async fn unblock(
    shell: State<'_, Shell>,
    account: AccountId,
    id: String,
) -> CmdResult<()> {
    cmd::unblock(&shell.core, account, &id).await
}

#[tauri::command]
pub(crate) async fn sender_threads(
    shell: State<'_, Shell>,
    account: AccountId,
    thread: ThreadId,
) -> CmdResult<Vec<ThreadId>> {
    cmd::sender_threads(&shell.core, account, thread).await
}

#[tauri::command]
pub(crate) async fn add_attachment(
    shell: State<'_, Shell>,
    draft: DraftId,
    filename: String,
    mime_type: String,
    data: String,
) -> CmdResult<DraftAttachment> {
    cmd::add_attachment(&shell.core, draft, filename, mime_type, &data).await
}

/// Quits after the UI has closed an open composer (SPEC Undo send: Cmd+Q).
#[tauri::command]
pub(crate) fn quit_now(app: AppHandle) {
    crate::quit(&app);
}

/// Quits at once, as `quit_now` does.
#[tauri::command]
pub(crate) fn quit(app: AppHandle) {
    crate::quit(&app);
}

#[tauri::command]
pub(crate) fn perf_report(action: String, ms: f64, budget: f64) {
    cmd::perf_report(&action, ms, budget);
}

#[tauri::command]
pub(crate) fn snooze_options(shell: State<'_, Shell>, text: String) -> Vec<SnoozeChoice> {
    cmd::snooze_options(&shell.core, &text)
}
