//! The commands that do more than call the core, and the error string the
//! UI reads. [`crate::dispatch`] calls the core's handles itself for the
//! rest; the app's `connect` calls [`connect`] with its channel.

use base64::Engine as _;
use sm_core::{App, MAX_ATTACHMENT_BYTES, NewAttachment, attachment_too_large};
use sm_domain::DraftId;
use sm_ipc::{ChannelMessage, DraftAttachment, LaunchInfo};

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

fn err(error: &sm_core::Error) -> String {
    message(error)
}

/// The page's first call: channel messages go to `sink` from now on.
/// Returns where the page's action numbers continue.
pub async fn connect(
    app: &App,
    sink: Box<dyn FnMut(ChannelMessage) + Send>,
) -> CmdResult<LaunchInfo> {
    app.lifecycle().connect(sink).await.map_err(|e| err(&e))
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
    let data = decode_file(data)?;
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

/// A picked file's base64 content, decoded. One too large to attach is
/// refused before it is decoded: the webview is not trusted with memory.
fn decode_file(data: &str) -> CmdResult<Vec<u8>> {
    // Base64 holds 3 bytes in every 4 characters; the padding of the last
    // group is the slack.
    if base64::decoded_len_estimate(data.len()) > MAX_ATTACHMENT_BYTES + 2 {
        return Err(err(&attachment_too_large()));
    }
    base64::engine::general_purpose::STANDARD
        .decode(data)
        .map_err(|e| format!("the file's content is not base64: {e}"))
}

/// Debug builds: the keydown-to-paint meter reports how long an action took.
pub fn perf_report(action: &str, ms: f64, budget: f64) {
    if ms > budget {
        tracing::warn!(action, ms, budget, "over the performance budget");
    } else {
        tracing::debug!(action, ms, "within the performance budget");
    }
}
