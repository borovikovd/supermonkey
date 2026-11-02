//! The Gmail REST client for one account.

use std::sync::Arc;
use std::time::Duration;

use base64::Engine;
use base64::engine::DecodePaddingMode;
use base64::engine::general_purpose::{GeneralPurpose, GeneralPurposeConfig, URL_SAFE};
use reqwest::{Method, StatusCode};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use crate::error::{self, GmailError, Outcome};
use crate::oauth::TokenSource;
use crate::types::{
    Draft, Format, Label, ListDraftsResponse, ListHistoryResponse, ListThreadsResponse, Message,
    MessagePartBody, Profile, SendAs, Thread,
};

/// Google's Gmail API base URL.
pub const GMAIL_BASE_URL: &str = "https://gmail.googleapis.com";

/// Accepts base64url with or without padding, as Gmail returns both.
const BASE64URL_LENIENT: GeneralPurpose = GeneralPurpose::new(
    &base64::alphabet::URL_SAFE,
    GeneralPurposeConfig::new().with_decode_padding_mode(DecodePaddingMode::Indifferent),
);

/// A reqwest client with the timeouts the error rule relies on: a connect timeout (Offline)
/// and a read timeout (`NoResponse`).
pub fn default_http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .read_timeout(Duration::from_secs(60))
        .build()
        .expect("reqwest client")
}

/// Parameters of `threads.list`.
#[derive(Debug, Clone, Default)]
pub struct ThreadListQuery {
    /// A Gmail search query.
    pub q: Option<String>,
    /// Threads must carry all of these labels.
    pub label_ids: Vec<String>,
    pub page_token: Option<String>,
    /// Gmail's default is 100, its maximum 500.
    pub max_results: Option<u32>,
    pub include_spam_trash: bool,
}

/// A Gmail client for one account.
///
/// Every call returns a [`GmailError`] classified by [`Outcome`]. A 401 refreshes the access
/// token once and retries inside the call. Other retries are the caller's: see
/// [`crate::retry`]. The client does not schedule or meter requests: wrap calls in
/// [`crate::AccountScheduler::run`] with the method's cost from [`crate::cost`].
#[derive(Clone)]
pub struct Gmail {
    base_url: String,
    account_email: String,
    http: reqwest::Client,
    tokens: Arc<dyn TokenSource>,
}

impl Gmail {
    /// A client for `account_email` against `base_url` ([`GMAIL_BASE_URL`] or the fake's).
    pub fn new(
        base_url: impl Into<String>,
        account_email: impl Into<String>,
        http: reqwest::Client,
        tokens: Arc<dyn TokenSource>,
    ) -> Self {
        Gmail {
            base_url: base_url.into().trim_end_matches('/').to_owned(),
            account_email: account_email.into(),
            http,
            tokens,
        }
    }

    /// The account this client acts for.
    pub fn account_email(&self) -> &str {
        &self.account_email
    }

    /// `users.getProfile`. Cost 1.
    pub async fn get_profile(&self) -> Result<Profile, GmailError> {
        self.get("profile", &[]).await
    }

    /// `users.settings.sendAs.list`. Cost 1.
    pub async fn list_send_as(&self) -> Result<Vec<SendAs>, GmailError> {
        #[derive(serde::Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Response {
            #[serde(default)]
            send_as: Vec<SendAs>,
        }
        let response: Response = self.get("settings/sendAs", &[]).await?;
        Ok(response.send_as)
    }

    /// `users.labels.list`. Cost 1.
    pub async fn list_labels(&self) -> Result<Vec<Label>, GmailError> {
        #[derive(serde::Deserialize)]
        struct Response {
            #[serde(default)]
            labels: Vec<Label>,
        }
        let response: Response = self.get("labels", &[]).await?;
        Ok(response.labels)
    }

    /// `users.labels.create`, shown in the label list and message list. Cost 5.
    /// A name that exists gives [`Outcome::Conflict`].
    pub async fn create_label(&self, name: &str) -> Result<Label, GmailError> {
        let body = json!({
            "name": name,
            "labelListVisibility": "labelShow",
            "messageListVisibility": "show",
        });
        self.json(Method::POST, "labels", &[], Some(&body)).await
    }

    /// `users.history.list` from `start_history_id`, 500 records a page. Cost 2.
    /// A cursor that is too old gives [`Outcome::NotFound`].
    pub async fn list_history(
        &self,
        start_history_id: u64,
        page_token: Option<&str>,
    ) -> Result<ListHistoryResponse, GmailError> {
        let mut query = vec![
            ("startHistoryId", start_history_id.to_string()),
            ("maxResults", "500".to_owned()),
        ];
        if let Some(token) = page_token {
            query.push(("pageToken", token.to_owned()));
        }
        self.get("history", &query).await
    }

    /// `users.threads.list`. Cost 10.
    pub async fn list_threads(
        &self,
        params: &ThreadListQuery,
    ) -> Result<ListThreadsResponse, GmailError> {
        let mut query = Vec::new();
        if let Some(q) = &params.q {
            query.push(("q", q.clone()));
        }
        for id in &params.label_ids {
            query.push(("labelIds", id.clone()));
        }
        if let Some(token) = &params.page_token {
            query.push(("pageToken", token.clone()));
        }
        if let Some(max) = params.max_results {
            query.push(("maxResults", max.to_string()));
        }
        if params.include_spam_trash {
            query.push(("includeSpamTrash", "true".to_owned()));
        }
        self.get("threads", &query).await
    }

    /// `users.threads.get`. With [`Format::Metadata`], `metadata_headers` limits the headers
    /// returned (empty means all). Cost 40, or 120 with [`Format::Full`].
    pub async fn get_thread(
        &self,
        id: &str,
        format: Format,
        metadata_headers: &[&str],
    ) -> Result<Thread, GmailError> {
        let mut query = vec![("format", format.as_str().to_owned())];
        for header in metadata_headers {
            query.push(("metadataHeaders", (*header).to_owned()));
        }
        self.get(&format!("threads/{id}"), &query).await
    }

    /// `users.messages.get`. Cost 20, or 60 with [`Format::Full`] or [`Format::Raw`].
    pub async fn get_message(&self, id: &str, format: Format) -> Result<Message, GmailError> {
        self.get(
            &format!("messages/{id}"),
            &[("format", format.as_str().to_owned())],
        )
        .await
    }

    /// `users.messages.attachments.get`, decoded to bytes. Cost 20.
    pub async fn get_attachment(
        &self,
        message_id: &str,
        attachment_id: &str,
    ) -> Result<Vec<u8>, GmailError> {
        let body: MessagePartBody = self
            .get(
                &format!("messages/{message_id}/attachments/{attachment_id}"),
                &[],
            )
            .await?;
        BASE64URL_LENIENT
            .decode(body.data.unwrap_or_default())
            .map_err(|e| {
                GmailError::with_detail(Outcome::NoResponse, format!("attachment data: {e}"))
            })
    }

    /// `users.messages.batchModify`. Cost 50. One unknown id fails the whole call with
    /// [`Outcome::NotFound`] without saying which.
    pub async fn batch_modify(
        &self,
        ids: &[String],
        add_label_ids: &[String],
        remove_label_ids: &[String],
    ) -> Result<(), GmailError> {
        let body = json!({
            "ids": ids,
            "addLabelIds": add_label_ids,
            "removeLabelIds": remove_label_ids,
        });
        self.send(Method::POST, "messages/batchModify", &[], Some(&body))
            .await?;
        Ok(())
    }

    /// `users.messages.trash`. Cost 20.
    pub async fn trash_message(&self, id: &str) -> Result<Message, GmailError> {
        self.json(Method::POST, &format!("messages/{id}/trash"), &[], None)
            .await
    }

    /// `users.drafts.list`, 500 a page. Cost 5.
    pub async fn list_drafts(
        &self,
        page_token: Option<&str>,
    ) -> Result<ListDraftsResponse, GmailError> {
        let mut query = vec![("maxResults", "500".to_owned())];
        if let Some(token) = page_token {
            query.push(("pageToken", token.to_owned()));
        }
        self.get("drafts", &query).await
    }

    /// `users.drafts.get`. Cost 20, or 60 with [`Format::Full`] or [`Format::Raw`].
    pub async fn get_draft(&self, id: &str, format: Format) -> Result<Draft, GmailError> {
        self.get(
            &format!("drafts/{id}"),
            &[("format", format.as_str().to_owned())],
        )
        .await
    }

    /// `users.drafts.create` from RFC 822 bytes. Cost 10. Not safe to repeat after
    /// [`Outcome::NoResponse`] or a 5xx: see SPEC, Gmail errors.
    pub async fn create_draft(
        &self,
        raw_rfc822: &[u8],
        thread_id: Option<&str>,
    ) -> Result<Draft, GmailError> {
        let body = json!({ "message": draft_message(raw_rfc822, thread_id) });
        self.json(Method::POST, "drafts", &[], Some(&body)).await
    }

    /// `users.drafts.update`: replaces the draft's message (its message id changes). Cost 15.
    pub async fn update_draft(
        &self,
        id: &str,
        raw_rfc822: &[u8],
        thread_id: Option<&str>,
    ) -> Result<Draft, GmailError> {
        let body = json!({ "id": id, "message": draft_message(raw_rfc822, thread_id) });
        self.json(Method::PUT, &format!("drafts/{id}"), &[], Some(&body))
            .await
    }

    /// `users.drafts.send`. Returns the sent message. Cost 100. After
    /// [`Outcome::NoResponse`] or a 5xx, settle with [`Gmail::get_draft`]: a 404 means sent.
    pub async fn send_draft(&self, id: &str) -> Result<Message, GmailError> {
        let body = json!({ "id": id });
        self.json(Method::POST, "drafts/send", &[], Some(&body))
            .await
    }

    /// `users.drafts.delete`. Cost 10.
    pub async fn delete_draft(&self, id: &str) -> Result<(), GmailError> {
        self.send(Method::DELETE, &format!("drafts/{id}"), &[], None)
            .await?;
        Ok(())
    }

    async fn get<T: DeserializeOwned>(
        &self,
        path: &str,
        query: &[(&str, String)],
    ) -> Result<T, GmailError> {
        self.json(Method::GET, path, query, None).await
    }

    async fn json<T: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        query: &[(&str, String)],
        body: Option<&Value>,
    ) -> Result<T, GmailError> {
        let response = self.send(method, path, query, body).await?;
        let bytes = response
            .bytes()
            .await
            .map_err(|e| error::from_transport(&e))?;
        // A 2xx whose body does not parse is no clear answer.
        serde_json::from_slice(&bytes).map_err(|e| {
            GmailError::with_detail(Outcome::NoResponse, format!("response body: {e}"))
        })
    }

    /// Sends one request, refreshing the token and retrying once on 401, and classifies
    /// any non-2xx status.
    async fn send(
        &self,
        method: Method,
        path: &str,
        query: &[(&str, String)],
        body: Option<&Value>,
    ) -> Result<reqwest::Response, GmailError> {
        let url = format!("{}/gmail/v1/users/me/{path}", self.base_url);
        let token = self.tokens.access_token().await?;
        let mut response = self.attempt(&method, &url, query, body, &token).await?;
        if response.status() == StatusCode::UNAUTHORIZED {
            let token = self.tokens.refresh_after_401(&token).await?;
            response = self.attempt(&method, &url, query, body, &token).await?;
        }
        let status = response.status();
        if status.is_success() {
            return Ok(response);
        }
        let headers = response.headers().clone();
        let text = response
            .text()
            .await
            .map_err(|e| error::from_transport(&e))?;
        Err(error::from_status(status, &headers, &text))
    }

    async fn attempt(
        &self,
        method: &Method,
        url: &str,
        query: &[(&str, String)],
        body: Option<&Value>,
        token: &str,
    ) -> Result<reqwest::Response, GmailError> {
        let mut request = self
            .http
            .request(method.clone(), url)
            .bearer_auth(token)
            .query(query);
        if let Some(body) = body {
            request = request.json(body);
        }
        request.send().await.map_err(|e| error::from_transport(&e))
    }
}

fn draft_message(raw_rfc822: &[u8], thread_id: Option<&str>) -> Value {
    let mut message = json!({ "raw": URL_SAFE.encode(raw_rfc822) });
    if let Some(thread_id) = thread_id {
        message["threadId"] = json!(thread_id);
    }
    message
}
