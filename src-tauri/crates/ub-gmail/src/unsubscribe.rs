//! The one-click unsubscribe POST (RFC 8058, SPEC Unsubscribe and block).

use std::time::Duration;

use reqwest::header::CONTENT_TYPE;

use crate::http::{self, HttpClient, ReadResponse};
use crate::outcome::{GmailError, Outcome};

/// How long the list's server gets to answer.
const TIMEOUT: Duration = Duration::from_secs(10);

/// Posts `List-Unsubscribe=One-Click` to a list's `https` URL, with a 10 s
/// timeout and no Gmail credentials. A URL that is not `https` is
/// [`Outcome::Rejected`] with status 0 and nothing is sent; debug builds also
/// allow loopback `http`, where the E2E server stands in for a list.
/// Redirects are not followed, and the list's answer is read for its status
/// only.
pub async fn one_click_unsubscribe(http: &HttpClient, url: &str) -> Result<(), GmailError> {
    if !may_post(url) {
        return Err(Outcome::Rejected {
            status: 0,
            message: "one-click unsubscribe needs an https URL".to_owned(),
            reason: None,
        }
        .into());
    }
    let request = http
        .lists()
        .post(url)
        .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body("List-Unsubscribe=One-Click")
        .timeout(TIMEOUT);
    let response = http::send(request).await?;
    if response.status().is_success() {
        return Ok(());
    }
    // A redirect is a failure too: the list's other ways are still there.
    Err(ReadResponse::status_only(&response).to_error())
}

fn may_post(url: &str) -> bool {
    let Ok(url) = url::Url::parse(url) else {
        return false;
    };
    url.scheme() == "https"
        || (cfg!(debug_assertions)
            && url.scheme() == "http"
            && matches!(url.host_str(), Some("127.0.0.1" | "localhost")))
}
