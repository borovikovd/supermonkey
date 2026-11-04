//! The commands, typed: one function per command over the core's handles,
//! with its result or the error string the UI reads. The Tauri commands call
//! these directly; [`crate::dispatch`] calls them by name.

use base64::Engine as _;
use ub_core::{App, ChannelSink, NewAttachment};
use ub_domain::{AccountId, AttachmentId, DraftId, MessageId, ThreadId};
use ub_ipc::{
    BlockResult, BlockedSenders, ContactSuggestion, DraftAttachment, LaunchInfo, ListRow,
    MessageBody, NumberedAction, SnoozeChoice, Unsubscribed,
};

/// A command's result: its value, or the message the UI shows.
pub type CmdResult<T> = Result<T, String>;

/// The message for a core error, with its causes, as the UI shows it.
pub fn message(error: &(dyn std::error::Error + 'static)) -> String {
    let mut text = error.to_string();
    let mut cause = error.source();
    while let Some(e) = cause {
        let more = e.to_string();
        // `#[error(transparent)]` and `{0}` messages already hold their cause.
        if !text.contains(&more) {
            text.push_str(": ");
            text.push_str(&more);
        }
        cause = e.source();
    }
    text
}

fn err(error: &ub_core::Error) -> String {
    message(error)
}

/// The page's first call: channel messages go to `sink` from now on.
/// Returns where the page's action numbers continue.
pub async fn connect(app: &App, sink: ChannelSink) -> CmdResult<LaunchInfo> {
    app.lifecycle().connect(sink).await.map_err(|e| err(&e))
}

/// Queues the page's numbered actions. Returns once they are queued.
pub fn submit(app: &App, actions: Vec<NumberedAction>) {
    app.actions().submit(actions);
}

/// Rows outside the pushed window, with their `rev`.
pub async fn fetch_rows(app: &App, drafts: bool, keys: Vec<String>) -> CmdResult<Vec<ListRow>> {
    app.queries()
        .fetch_rows(drafts, keys)
        .await
        .map_err(|e| err(&e))
}

/// Sanitized bodies. A message without a stored body is left out.
pub async fn get_bodies(app: &App, ids: Vec<MessageId>) -> CmdResult<Vec<MessageBody>> {
    app.queries().get_bodies(ids).await.map_err(|e| err(&e))
}

/// Recipient suggestions for what the user typed.
pub async fn autocomplete(
    app: &App,
    account: AccountId,
    query: String,
) -> CmdResult<Vec<ContactSuggestion>> {
    app.queries()
        .autocomplete(account, query)
        .await
        .map_err(|e| err(&e))
}

/// Signs a new account in with Google in the browser and adds it.
pub async fn add_account(app: &App) -> CmdResult<AccountId> {
    app.accounts().add().await.map_err(|e| err(&e))
}

/// Signs an account in again.
pub async fn sign_in_again(app: &App, account: AccountId) -> CmdResult<()> {
    app.accounts()
        .sign_in_again(account)
        .await
        .map_err(|e| err(&e))
}

/// Downloads an attachment if needed and opens it with its default app.
pub async fn open_attachment(app: &App, attachment: AttachmentId) -> CmdResult<()> {
    app.mail()
        .open_attachment(attachment)
        .await
        .map_err(|e| err(&e))
}

/// Opens a link from a message in the default browser or mail app.
pub fn open_url(app: &App, url: &str) -> CmdResult<()> {
    app.mail().open_url(url).map_err(|e| err(&e))
}

/// Unsubscribes from the list a conversation came from.
pub async fn unsubscribe(
    app: &App,
    account: AccountId,
    thread: ThreadId,
) -> CmdResult<Unsubscribed> {
    app.mail()
        .unsubscribe(account, thread)
        .await
        .map_err(|e| err(&e))
}

/// Blocks a conversation's sender, or the sender's domain.
pub async fn block(
    app: &App,
    account: AccountId,
    thread: ThreadId,
    domain: bool,
) -> CmdResult<BlockResult> {
    app.mail()
        .block(account, thread, domain)
        .await
        .map_err(|e| err(&e))
}

/// The account's blocked senders.
pub async fn blocked_senders(app: &App, account: AccountId) -> CmdResult<BlockedSenders> {
    app.mail()
        .blocked_senders(account)
        .await
        .map_err(|e| err(&e))
}

/// Unblocks a sender by deleting its Gmail filter `id`.
pub async fn unblock(app: &App, account: AccountId, id: &str) -> CmdResult<()> {
    app.mail().unblock(account, id).await.map_err(|e| err(&e))
}

/// Inbox conversations from the sender of `thread`'s latest message.
pub async fn sender_threads(
    app: &App,
    account: AccountId,
    thread: ThreadId,
) -> CmdResult<Vec<ThreadId>> {
    app.queries()
        .sender_threads(account, thread)
        .await
        .map_err(|e| err(&e))
}

/// Keeps a file the user picked with the draft's files. `data` is the
/// file's content in base64, as the page reads it.
pub async fn add_attachment(
    app: &App,
    draft: DraftId,
    filename: String,
    mime_type: String,
    data: &str,
) -> CmdResult<DraftAttachment> {
    let data = base64::engine::general_purpose::STANDARD
        .decode(data)
        .map_err(|e| format!("the file's content is not base64: {e}"))?;
    let file = NewAttachment {
        filename,
        mime_type,
        data,
    };
    app.mail()
        .add_attachment(draft, file)
        .await
        .map_err(|e| err(&e))
}

/// Debug builds: the keydown-to-paint meter reports how long an action took.
pub fn perf_report(action: &str, ms: f64, budget: f64) {
    if ms > budget {
        tracing::warn!(action, ms, budget, "over the performance budget");
    } else {
        tracing::debug!(action, ms, "within the performance budget");
    }
}

/// The snooze picker's choices for what the user typed.
pub fn snooze_options(app: &App, text: &str) -> Vec<SnoozeChoice> {
    app.queries().snooze_options(text)
}
