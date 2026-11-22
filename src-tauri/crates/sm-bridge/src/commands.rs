use base64::Engine as _;
use sm_core::{App, Error, MAX_ATTACHMENT_BYTES, NewAttachment, attachment_too_large};
use sm_domain::DraftId;
use sm_ipc::{CommandError, DraftAttachment, ErrorKind};

pub type CmdResult<T> = Result<T, CommandError>;

const INTERNAL: &str = "Something went wrong inside Supermonkey";

pub fn chain(error: &(dyn std::error::Error + 'static)) -> String {
    let mut text = error.to_string();
    let mut cause = error.source();
    while let Some(e) = cause {
        let more = e.to_string();
        if !text.contains(&more) {
            text.push_str(": ");
            text.push_str(&more);
        }
        cause = e.source();
    }
    text
}

#[must_use]
pub fn failed(error: &Error) -> CommandError {
    let kind = match error {
        Error::SignInCancelled => ErrorKind::SignInCancelled,
        Error::DraftGone => ErrorKind::DraftGone,
        Error::Invalid(_) | Error::Platform(_) | Error::SignIn(_) | Error::Gmail(_) => {
            ErrorKind::Other
        }
        Error::Store(_)
        | Error::ReadTask(_)
        | Error::NoRuntime(_)
        | Error::Tokens(_)
        | Error::GmailEndpoint(_)
        | Error::Http(_)
        | Error::Io(_) => return internal(&chain(error)),
    };
    CommandError {
        kind,
        message: error.to_string(),
    }
}

pub(crate) fn internal(detail: &str) -> CommandError {
    tracing::error!(error = detail, "a command failed inside the core");
    CommandError {
        kind: ErrorKind::Other,
        message: INTERNAL.to_owned(),
    }
}

pub(crate) fn shown<T>(result: Result<T, Error>) -> CmdResult<T> {
    result.map_err(|e| failed(&e))
}

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
    shown(app.mail().add_attachment(draft, file).await)
}

fn decode_file(data: &str) -> CmdResult<Vec<u8>> {
    if base64::decoded_len_estimate(data.len()) > MAX_ATTACHMENT_BYTES + 2 {
        return Err(failed(&attachment_too_large()));
    }
    base64::engine::general_purpose::STANDARD
        .decode(data)
        .map_err(|e| internal(&format!("the file's content is not base64: {e}")))
}

pub fn perf_report(action: &str, ms: f64, budget: f64) {
    if ms > budget {
        over_budget(action, ms, budget);
    } else {
        within_budget(action, ms);
    }
}

fn over_budget(action: &str, ms: f64, budget: f64) {
    tracing::warn!(action, ms, budget, "over the performance budget");
}

fn within_budget(action: &str, ms: f64) {
    tracing::debug!(action, ms, "within the performance budget");
}
