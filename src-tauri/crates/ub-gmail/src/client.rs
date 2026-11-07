//! The Gmail REST client for one account.

use std::borrow::Cow;
use std::sync::Arc;

use base64::Engine as _;
use base64::engine::DecodePaddingMode;
use base64::engine::general_purpose::{GeneralPurpose, GeneralPurposeConfig, URL_SAFE};
use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};
use reqwest::{Method, StatusCode};
use serde::Deserialize;
use serde_json::{Value, json};
use tracing::Instrument as _;
use ub_domain::{GmailDraftId, GmailMessageId, GmailThreadId, LabelId};

use crate::http::{self, HttpClient, ReadResponse};
use crate::ids::{FilterId, GmailAttachmentId, HistoryId, PageToken};
use crate::oauth::{AccessToken, Tokens};
use crate::outcome::{GmailError, Outcome};
use crate::quota::{AccountScheduler, Priority, Units, cost};
use crate::wire::{
    Draft, DraftPage, Filter, Format, HistoryPage, Label, Message, PartBody, Profile, SendAs,
    Thread, ThreadPage, ThreadQuery,
};

/// Google's Gmail API base URL.
pub const GMAIL_BASE_URL: &str = "https://gmail.googleapis.com";

/// Accepts base64url with or without padding: Gmail sends both.
const BASE64URL_LENIENT: GeneralPurpose = GeneralPurpose::new(
    &base64::alphabet::URL_SAFE,
    GeneralPurposeConfig::new().with_decode_padding_mode(DecodePaddingMode::Indifferent),
);

/// Characters an id keeps in a path segment; everything else is escaped, so
/// an id can never reach another path or the query.
const SEGMENT: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'_')
    .remove(b'.')
    .remove(b'~');

/// The base URL given to [`Gmail::new`] is not an `http` or `https` URL.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum InvalidBaseUrl {
    /// It is not a URL.
    #[error("the Gmail base URL {url:?} does not parse")]
    Unparsable {
        /// The URL as given.
        url: String,
        /// Why it does not parse.
        #[source]
        source: url::ParseError,
    },
    /// It is a URL of another scheme.
    #[error("the Gmail base URL {0:?} is not an http or https URL")]
    NotHttp(String),
}

/// A Gmail client for one account.
///
/// Every call waits its turn in the account's [`AccountScheduler`] at the
/// priority given, paying the method's published quota cost, so a call can't
/// skip the scheduler or pay the wrong cost. A 401 refreshes the access token
/// once and retries inside the call. Other retries are the caller's: see
/// [`crate::retry`]. Errors carry an [`Outcome`].
///
/// Cheap to clone: clones share the tokens and the scheduler.
#[derive(Clone)]
pub struct Gmail {
    /// `{base}/gmail/v1/users/me`.
    api: Arc<str>,
    http: HttpClient,
    tokens: Arc<Tokens>,
    scheduler: AccountScheduler,
}

/// One request, before it is sent.
struct Call<'a> {
    api: &'static str,
    priority: Priority,
    cost: Units,
    method: Method,
    path: String,
    query: Vec<(&'static str, Cow<'a, str>)>,
    body: Option<Value>,
}

impl<'a> Call<'a> {
    fn new(
        api: &'static str,
        priority: Priority,
        cost: Units,
        method: Method,
        segments: &[&str],
    ) -> Self {
        let path = segments
            .iter()
            .map(|s| utf8_percent_encode(s, SEGMENT).to_string())
            .collect::<Vec<_>>()
            .join("/");
        Self {
            api,
            priority,
            cost,
            method,
            path,
            query: Vec::new(),
            body: None,
        }
    }

    fn query(mut self, name: &'static str, value: impl Into<Cow<'a, str>>) -> Self {
        self.query.push((name, value.into()));
        self
    }

    fn query_opt(self, name: &'static str, value: Option<impl Into<Cow<'a, str>>>) -> Self {
        match value {
            Some(value) => self.query(name, value),
            None => self,
        }
    }

    fn body(mut self, body: Value) -> Self {
        self.body = Some(body);
        self
    }
}

impl Gmail {
    /// A client for the account `tokens` belong to, against `base_url`
    /// ([`GMAIL_BASE_URL`], or the fake's), scheduled by `scheduler`.
    pub fn new(
        base_url: &str,
        http: HttpClient,
        tokens: Arc<Tokens>,
        scheduler: AccountScheduler,
    ) -> Result<Self, InvalidBaseUrl> {
        let parsed = url::Url::parse(base_url).map_err(|source| InvalidBaseUrl::Unparsable {
            url: base_url.to_owned(),
            source,
        })?;
        if !matches!(parsed.scheme(), "http" | "https") {
            return Err(InvalidBaseUrl::NotHttp(base_url.to_owned()));
        }
        let api = format!("{}/gmail/v1/users/me", base_url.trim_end_matches('/'));
        Ok(Self {
            api: api.into(),
            http,
            tokens,
            scheduler,
        })
    }

    /// The account's scheduler, for work that should share its slots.
    pub fn scheduler(&self) -> &AccountScheduler {
        &self.scheduler
    }

    /// `users.getProfile`: the address and the current history cursor. Cost 1.
    pub async fn get_profile(&self, priority: Priority) -> Result<Profile, GmailError> {
        let call = Call::new(
            "getProfile",
            priority,
            cost::GET_PROFILE,
            Method::GET,
            &["profile"],
        );
        self.json(call).await
    }

    /// `settings.sendAs.list`: every address the account sends as. Cost 1.
    pub async fn list_send_as(&self, priority: Priority) -> Result<Vec<SendAs>, GmailError> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct List {
            #[serde(default)]
            send_as: Vec<SendAs>,
        }
        let call = Call::new(
            "sendAs.list",
            priority,
            cost::SEND_AS_LIST,
            Method::GET,
            &["settings", "sendAs"],
        );
        Ok(self.json::<List>(call).await?.send_as)
    }

    /// `labels.list`. Cost 1.
    pub async fn list_labels(&self, priority: Priority) -> Result<Vec<Label>, GmailError> {
        #[derive(Deserialize)]
        struct List {
            #[serde(default)]
            labels: Vec<Label>,
        }
        let call = Call::new(
            "labels.list",
            priority,
            cost::LABELS_LIST,
            Method::GET,
            &["labels"],
        );
        Ok(self.json::<List>(call).await?.labels)
    }

    /// `labels.create`, shown in the label list and the message list. A name
    /// that exists is [`Outcome::Conflict`]. Cost 5.
    pub async fn create_label(&self, priority: Priority, name: &str) -> Result<Label, GmailError> {
        let call = Call::new(
            "labels.create",
            priority,
            cost::LABELS_CREATE,
            Method::POST,
            &["labels"],
        )
        .body(json!({
            "name": name,
            "labelListVisibility": "labelShow",
            "messageListVisibility": "show",
        }));
        self.json(call).await
    }

    /// `history.list` after `start`, 500 records a page. A cursor too old
    /// for Gmail to keep is [`Outcome::NotFound`]. Cost 2.
    pub async fn list_history(
        &self,
        priority: Priority,
        start: HistoryId,
        page: Option<&PageToken>,
    ) -> Result<HistoryPage, GmailError> {
        let call = Call::new(
            "history.list",
            priority,
            cost::HISTORY_LIST,
            Method::GET,
            &["history"],
        )
        .query("startHistoryId", start.to_string())
        .query("maxResults", "500")
        .query_opt("pageToken", page.map(PageToken::as_str));
        self.json(call).await
    }

    /// `threads.list`. Cost 10.
    pub async fn list_threads(
        &self,
        priority: Priority,
        query: &ThreadQuery,
    ) -> Result<ThreadPage, GmailError> {
        let mut call = Call::new(
            "threads.list",
            priority,
            cost::THREADS_LIST,
            Method::GET,
            &["threads"],
        )
        .query_opt("q", query.q.as_deref())
        .query_opt(
            "pageToken",
            query.page_token.as_ref().map(PageToken::as_str),
        )
        .query_opt("maxResults", query.max_results.map(|n| n.to_string()));
        for label in &query.label_ids {
            call = call.query("labelIds", label.as_str());
        }
        if query.include_spam_trash {
            call = call.query("includeSpamTrash", "true");
        }
        self.json(call).await
    }

    /// `threads.get`. With [`Format::Metadata`], `headers` limits the headers
    /// returned (empty means all). Cost 40, or 120 with [`Format::Full`].
    pub async fn get_thread(
        &self,
        priority: Priority,
        id: &GmailThreadId,
        format: Format,
        headers: &[&str],
    ) -> Result<Thread, GmailError> {
        let mut call = Call::new(
            "threads.get",
            priority,
            cost::threads_get(format),
            Method::GET,
            &["threads", id.as_str()],
        )
        .query("format", format.as_str());
        for &header in headers {
            call = call.query("metadataHeaders", header);
        }
        self.json(call).await
    }

    /// `messages.get`. Cost 20, or 60 with [`Format::Full`] or [`Format::Raw`].
    pub async fn get_message(
        &self,
        priority: Priority,
        id: &GmailMessageId,
        format: Format,
    ) -> Result<Message, GmailError> {
        let call = Call::new(
            "messages.get",
            priority,
            cost::messages_get(format),
            Method::GET,
            &["messages", id.as_str()],
        )
        .query("format", format.as_str());
        self.json(call).await
    }

    /// `messages.get` with `format=metadata` and only the named headers.
    /// Cost 20.
    pub async fn get_message_headers(
        &self,
        priority: Priority,
        id: &GmailMessageId,
        headers: &[&str],
    ) -> Result<Message, GmailError> {
        let mut call = Call::new(
            "messages.get",
            priority,
            cost::messages_get(Format::Metadata),
            Method::GET,
            &["messages", id.as_str()],
        )
        .query("format", Format::Metadata.as_str());
        for &header in headers {
            call = call.query("metadataHeaders", header);
        }
        self.json(call).await
    }

    /// `messages.batchModify`. One unknown id fails the whole call with
    /// [`Outcome::NotFound`], without saying which. Cost 50.
    pub async fn batch_modify(
        &self,
        priority: Priority,
        ids: &[GmailMessageId],
        add: &[LabelId],
        remove: &[LabelId],
    ) -> Result<(), GmailError> {
        let call = Call::new(
            "messages.batchModify",
            priority,
            cost::MESSAGES_BATCH_MODIFY,
            Method::POST,
            &["messages", "batchModify"],
        )
        .body(json!({ "ids": ids, "addLabelIds": add, "removeLabelIds": remove }));
        self.empty(call).await
    }

    /// `messages.trash`. Cost 20.
    pub async fn trash_message(
        &self,
        priority: Priority,
        id: &GmailMessageId,
    ) -> Result<Message, GmailError> {
        let call = Call::new(
            "messages.trash",
            priority,
            cost::MESSAGES_TRASH,
            Method::POST,
            &["messages", id.as_str(), "trash"],
        );
        self.json(call).await
    }

    /// `messages.send` of RFC 822 bytes, outside any draft. Cost 100.
    pub async fn send_message(
        &self,
        priority: Priority,
        rfc822: &[u8],
    ) -> Result<Message, GmailError> {
        let call = Call::new(
            "messages.send",
            priority,
            cost::MESSAGES_SEND,
            Method::POST,
            &["messages", "send"],
        )
        .body(json!({ "raw": URL_SAFE.encode(rfc822) }));
        self.json(call).await
    }

    /// `messages.attachments.get`, decoded to bytes. Cost 20.
    pub async fn get_attachment(
        &self,
        priority: Priority,
        message: &GmailMessageId,
        attachment: &GmailAttachmentId,
    ) -> Result<Vec<u8>, GmailError> {
        let call = Call::new(
            "attachments.get",
            priority,
            cost::ATTACHMENTS_GET,
            Method::GET,
            &[
                "messages",
                message.as_str(),
                "attachments",
                attachment.as_str(),
            ],
        );
        let body: PartBody = self.json(call).await?;
        BASE64URL_LENIENT
            .decode(body.data.unwrap_or_default())
            .map_err(|e| {
                GmailError::with_detail(Outcome::NoResponse, format!("attachment data: {e}"))
            })
    }

    /// `drafts.list`, 500 a page. Each draft's message has only its id and
    /// thread id. Cost 5.
    pub async fn list_drafts(
        &self,
        priority: Priority,
        page: Option<&PageToken>,
    ) -> Result<DraftPage, GmailError> {
        let call = Call::new(
            "drafts.list",
            priority,
            cost::DRAFTS_LIST,
            Method::GET,
            &["drafts"],
        )
        .query("maxResults", "500")
        .query_opt("pageToken", page.map(PageToken::as_str));
        self.json(call).await
    }

    /// `drafts.get`. Cost 20, or 60 with [`Format::Full`] or [`Format::Raw`].
    pub async fn get_draft(
        &self,
        priority: Priority,
        id: &GmailDraftId,
        format: Format,
    ) -> Result<Draft, GmailError> {
        let call = Call::new(
            "drafts.get",
            priority,
            cost::messages_get(format),
            Method::GET,
            &["drafts", id.as_str()],
        )
        .query("format", format.as_str());
        self.json(call).await
    }

    /// `drafts.create` of RFC 822 bytes, in `thread` when given. Not safe to
    /// repeat after no response or a 5xx: see SPEC Gmail errors. Cost 10.
    pub async fn create_draft(
        &self,
        priority: Priority,
        rfc822: &[u8],
        thread: Option<&GmailThreadId>,
    ) -> Result<Draft, GmailError> {
        let call = Call::new(
            "drafts.create",
            priority,
            cost::DRAFTS_CREATE,
            Method::POST,
            &["drafts"],
        )
        .body(json!({ "message": draft_message(rfc822, thread) }));
        self.json(call).await
    }

    /// `drafts.update`: replaces the draft's message, whose id changes.
    /// Cost 15.
    pub async fn update_draft(
        &self,
        priority: Priority,
        id: &GmailDraftId,
        rfc822: &[u8],
        thread: Option<&GmailThreadId>,
    ) -> Result<Draft, GmailError> {
        let call = Call::new(
            "drafts.update",
            priority,
            cost::DRAFTS_UPDATE,
            Method::PUT,
            &["drafts", id.as_str()],
        )
        .body(json!({ "id": id, "message": draft_message(rfc822, thread) }));
        self.json(call).await
    }

    /// `drafts.send`: sends the draft and returns the sent message. After no
    /// response or a 5xx, settle with [`Gmail::get_draft`]: a 404 means sent.
    /// Cost 100.
    pub async fn send_draft(
        &self,
        priority: Priority,
        id: &GmailDraftId,
    ) -> Result<Message, GmailError> {
        let call = Call::new(
            "drafts.send",
            priority,
            cost::DRAFTS_SEND,
            Method::POST,
            &["drafts", "send"],
        )
        .body(json!({ "id": id }));
        self.json(call).await
    }

    /// `drafts.delete`. Cost 10.
    pub async fn delete_draft(
        &self,
        priority: Priority,
        id: &GmailDraftId,
    ) -> Result<(), GmailError> {
        let call = Call::new(
            "drafts.delete",
            priority,
            cost::DRAFTS_DELETE,
            Method::DELETE,
            &["drafts", id.as_str()],
        );
        self.empty(call).await
    }

    /// `settings.filters.list`. Needs [`crate::GMAIL_SETTINGS_SCOPE`]. Cost 1.
    pub async fn list_filters(&self, priority: Priority) -> Result<Vec<Filter>, GmailError> {
        #[derive(Deserialize)]
        struct List {
            #[serde(default)]
            filter: Vec<Filter>,
        }
        let call = Call::new(
            "filters.list",
            priority,
            cost::FILTERS_LIST,
            Method::GET,
            &["settings", "filters"],
        );
        Ok(self.json::<List>(call).await?.filter)
    }

    /// `settings.filters.create`: mail from `from` (an address or `@domain`)
    /// skips the Inbox and goes to Trash. Needs
    /// [`crate::GMAIL_SETTINGS_SCOPE`]. Cost 5.
    pub async fn create_trash_filter(
        &self,
        priority: Priority,
        from: &str,
    ) -> Result<Filter, GmailError> {
        let call = Call::new(
            "filters.create",
            priority,
            cost::FILTERS_CREATE,
            Method::POST,
            &["settings", "filters"],
        )
        .body(json!({
            "criteria": { "from": from },
            "action": { "addLabelIds": ["TRASH"], "removeLabelIds": ["INBOX"] },
        }));
        self.json(call).await
    }

    /// `settings.filters.delete`. Needs [`crate::GMAIL_SETTINGS_SCOPE`].
    /// Cost 5.
    pub async fn delete_filter(&self, priority: Priority, id: &FilterId) -> Result<(), GmailError> {
        let call = Call::new(
            "filters.delete",
            priority,
            cost::FILTERS_DELETE,
            Method::DELETE,
            &["settings", "filters", id.as_str()],
        );
        self.empty(call).await
    }

    async fn json<T: for<'de> Deserialize<'de>>(&self, call: Call<'_>) -> Result<T, GmailError> {
        let api = call.api;
        self.execute(call).await?.json(api)
    }

    async fn empty(&self, call: Call<'_>) -> Result<(), GmailError> {
        self.execute(call).await.map(drop)
    }

    /// Runs a call in its own span, logging a failure.
    async fn execute(&self, call: Call<'_>) -> Result<ReadResponse, GmailError> {
        let span = tracing::debug_span!("gmail", api = call.api);
        async {
            let result = self.send_call(&call).await;
            if let Err(err) = &result {
                tracing::debug!(outcome = %err.outcome(), detail = err.detail(), "Gmail call failed");
            }
            result
        }
        .instrument(span)
        .await
    }

    /// Waits for the scheduler, sends, refreshes the token and retries once
    /// on a 401, and classifies a status that is not a success.
    async fn send_call(&self, call: &Call<'_>) -> Result<ReadResponse, GmailError> {
        let _permit = self.scheduler.acquire(call.priority, call.cost).await;
        let url = format!("{}/{}", self.api, call.path);
        let token = self.tokens.access_token().await.map_err(not_sent)?;
        let mut response = self.attempt(call, &url, &token).await?;
        if response.status() == StatusCode::UNAUTHORIZED {
            tracing::debug!("401; refreshing the access token");
            let token = self
                .tokens
                .refresh_after_401(&token)
                .await
                .map_err(not_sent)?;
            response = self.attempt(call, &url, &token).await?;
        }
        let read = ReadResponse::read(response).await?;
        if read.status.is_success() {
            Ok(read)
        } else {
            Err(read.to_error())
        }
    }

    async fn attempt(
        &self,
        call: &Call<'_>,
        url: &str,
        token: &AccessToken,
    ) -> Result<reqwest::Response, GmailError> {
        let mut request = self
            .http
            .inner()
            .request(call.method.clone(), url)
            .bearer_auth(token.secret())
            .query(&call.query);
        if let Some(body) = &call.body {
            request = request.json(body);
        }
        http::send(request).await
    }
}

/// A token refresh that failed before the call went out (again): Gmail did
/// nothing, so the call is safe to replay (SPEC Gmail errors), whatever the
/// token endpoint answered. A refused grant and a rejected request keep
/// their outcome: the account must sign in again, or the client is wrong.
fn not_sent(err: GmailError) -> GmailError {
    match err.outcome() {
        Outcome::Offline | Outcome::InvalidGrant | Outcome::Rejected { .. } => err,
        outcome => {
            let detail = format!("token refresh: {outcome}");
            GmailError::with_detail(Outcome::Offline, detail)
        }
    }
}

fn draft_message(rfc822: &[u8], thread: Option<&GmailThreadId>) -> Value {
    let mut message = json!({ "raw": URL_SAFE.encode(rfc822) });
    if let Some(thread) = thread {
        message["threadId"] = json!(thread);
    }
    message
}
