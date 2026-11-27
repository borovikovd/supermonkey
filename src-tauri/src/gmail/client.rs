use std::net::IpAddr;
use std::sync::Arc;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use reqwest::Method;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use thiserror::Error;
use url::{Host, Url};

use crate::domain::{
    FilterId, GmailAttachmentId, GmailDraftId, GmailMessageId, GmailThreadId, HistoryId, LabelId,
    PageToken, SystemLabel,
};
use crate::mime::decode_base64_lenient;

use super::http::{GmailError, HttpClient, Outcome, ReadResponse};
use super::oauth::Tokens;
use super::quota::{
    AccountScheduler, COST_ATTACHMENT, COST_BATCH_MODIFY, COST_DRAFT_CREATE, COST_DRAFT_DELETE,
    COST_DRAFT_UPDATE, COST_DRAFTS_LIST, COST_FILTER_WRITE, COST_FILTERS_LIST, COST_HISTORY,
    COST_LABEL_CREATE, COST_LABELS_LIST, COST_PROFILE, COST_SEND, COST_SEND_AS, COST_THREADS_LIST,
    COST_TRASH, Priority, Units, cost_message_get, cost_thread_get,
};
use super::wire::{
    Draft, DraftPage, Filter, FilterList, Format, HistoryPage, Label, LabelList, Message, PartBody,
    Profile, SendAs, SendAsList, Thread, ThreadPage, ThreadQuery,
};

pub const GMAIL_BASE_URL: &str = "https://gmail.googleapis.com";

#[derive(Debug, Error)]
#[error("invalid Gmail base URL {url:?}")]
pub struct InvalidBaseUrl {
    pub url: String,
}

#[derive(Debug)]
pub struct Gmail {
    http: HttpClient,
    base_url: Url,
    tokens: Tokens,
    scheduler: Arc<AccountScheduler>,
}

struct Call<'a, B> {
    method: Method,
    segments: &'a [&'a str],
    query: &'a [(&'a str, &'a str)],
    body: Option<&'a B>,
    priority: Priority,
    cost: Units,
}

impl Gmail {
    pub fn new(
        http: HttpClient,
        base_url: &str,
        tokens: Tokens,
        scheduler: Arc<AccountScheduler>,
    ) -> Result<Self, InvalidBaseUrl> {
        let mut parsed = Url::parse(base_url).map_err(|_parse| InvalidBaseUrl {
            url: base_url.to_owned(),
        })?;
        if !matches!(parsed.scheme(), "http" | "https") || parsed.host().is_none() {
            return Err(InvalidBaseUrl {
                url: base_url.to_owned(),
            });
        }
        parsed.set_query(None);
        parsed.set_fragment(None);
        Ok(Self {
            http,
            base_url: parsed,
            tokens,
            scheduler,
        })
    }

    pub async fn get_profile(&self, priority: Priority) -> Result<Profile, GmailError> {
        self.json(Call {
            method: Method::GET,
            segments: &["profile"],
            query: &[],
            body: None::<&()>,
            priority,
            cost: COST_PROFILE,
        })
        .await
    }

    pub async fn list_send_as(&self, priority: Priority) -> Result<Vec<SendAs>, GmailError> {
        let list: SendAsList = self
            .json(Call {
                method: Method::GET,
                segments: &["settings", "sendAs"],
                query: &[],
                body: None::<&()>,
                priority,
                cost: COST_SEND_AS,
            })
            .await?;
        Ok(list.send_as)
    }

    pub async fn list_labels(&self, priority: Priority) -> Result<Vec<Label>, GmailError> {
        let list: LabelList = self
            .json(Call {
                method: Method::GET,
                segments: &["labels"],
                query: &[],
                body: None::<&()>,
                priority,
                cost: COST_LABELS_LIST,
            })
            .await?;
        Ok(list.labels)
    }

    pub async fn create_label(&self, name: &str, priority: Priority) -> Result<Label, GmailError> {
        let body = json!({
            "name": name,
            "labelListVisibility": "labelShow",
            "messageListVisibility": "show",
        });
        self.json(Call {
            method: Method::POST,
            segments: &["labels"],
            query: &[],
            body: Some(&body),
            priority,
            cost: COST_LABEL_CREATE,
        })
        .await
    }

    pub async fn list_filters(&self, priority: Priority) -> Result<Vec<Filter>, GmailError> {
        let list: FilterList = self
            .json(Call {
                method: Method::GET,
                segments: &["settings", "filters"],
                query: &[],
                body: None::<&()>,
                priority,
                cost: COST_FILTERS_LIST,
            })
            .await?;
        Ok(list.filter)
    }

    pub async fn create_trash_filter(
        &self,
        from: &str,
        priority: Priority,
    ) -> Result<Filter, GmailError> {
        let body = json!({
            "criteria": { "from": from },
            "action": {
                "addLabelIds": [SystemLabel::Trash.id()],
                "removeLabelIds": [SystemLabel::Inbox.id()],
            },
        });
        self.json(Call {
            method: Method::POST,
            segments: &["settings", "filters"],
            query: &[],
            body: Some(&body),
            priority,
            cost: COST_FILTER_WRITE,
        })
        .await
    }

    pub async fn delete_filter(&self, id: &FilterId, priority: Priority) -> Result<(), GmailError> {
        self.empty(Call {
            method: Method::DELETE,
            segments: &["settings", "filters", id.as_str()],
            query: &[],
            body: None::<&()>,
            priority,
            cost: COST_FILTER_WRITE,
        })
        .await
    }

    pub async fn list_history(
        &self,
        start_history_id: HistoryId,
        page_token: Option<&PageToken>,
        priority: Priority,
    ) -> Result<HistoryPage, GmailError> {
        let start = start_history_id.to_string();
        let mut query = vec![("startHistoryId", start.as_str()), ("maxResults", "500")];
        if let Some(tok) = page_token {
            query.push(("pageToken", tok.as_str()));
        }
        self.json(Call {
            method: Method::GET,
            segments: &["history"],
            query: &query,
            body: None::<&()>,
            priority,
            cost: COST_HISTORY,
        })
        .await
    }

    pub async fn list_threads(
        &self,
        q: ThreadQuery<'_>,
        priority: Priority,
    ) -> Result<ThreadPage, GmailError> {
        let mut query: Vec<(&str, String)> = Vec::new();
        if let Some(text) = q.q
            && !text.is_empty()
        {
            query.push(("q", text.to_owned()));
        }
        if let Some(label) = q.label_id {
            query.push(("labelIds", label.0.clone()));
        }
        if q.include_spam_trash {
            query.push(("includeSpamTrash", "true".to_owned()));
        }
        if let Some(max) = q.max_results {
            query.push(("maxResults", max.to_string()));
        }
        if let Some(tok) = q.page_token {
            query.push(("pageToken", tok.0.clone()));
        }
        let pairs: Vec<(&str, &str)> = query.iter().map(|(k, v)| (*k, v.as_str())).collect();
        self.json(Call {
            method: Method::GET,
            segments: &["threads"],
            query: &pairs,
            body: None::<&()>,
            priority,
            cost: COST_THREADS_LIST,
        })
        .await
    }

    pub async fn get_thread(
        &self,
        id: &GmailThreadId,
        format: Format,
        priority: Priority,
    ) -> Result<Thread, GmailError> {
        let query = [("format", format.as_str())];
        self.json(Call {
            method: Method::GET,
            segments: &["threads", id.as_str()],
            query: &query,
            body: None::<&()>,
            priority,
            cost: cost_thread_get(format),
        })
        .await
    }

    pub async fn get_message(
        &self,
        id: &GmailMessageId,
        format: Format,
        priority: Priority,
    ) -> Result<Message, GmailError> {
        let query = [("format", format.as_str())];
        self.json(Call {
            method: Method::GET,
            segments: &["messages", id.as_str()],
            query: &query,
            body: None::<&()>,
            priority,
            cost: cost_message_get(format),
        })
        .await
    }

    pub async fn get_message_headers(
        &self,
        id: &GmailMessageId,
        headers: &[&str],
        priority: Priority,
    ) -> Result<Message, GmailError> {
        let mut query = vec![("format", "metadata")];
        for h in headers {
            query.push(("metadataHeaders", *h));
        }
        self.json(Call {
            method: Method::GET,
            segments: &["messages", id.as_str()],
            query: &query,
            body: None::<&()>,
            priority,
            cost: cost_message_get(Format::Metadata),
        })
        .await
    }

    pub async fn batch_modify(
        &self,
        ids: &[GmailMessageId],
        add: &[LabelId],
        remove: &[LabelId],
        priority: Priority,
    ) -> Result<(), GmailError> {
        let payload = json!({
            "ids": ids,
            "addLabelIds": add,
            "removeLabelIds": remove,
        });
        self.empty(Call {
            method: Method::POST,
            segments: &["messages", "batchModify"],
            query: &[],
            body: Some(&payload),
            priority,
            cost: COST_BATCH_MODIFY,
        })
        .await
    }

    pub async fn trash_message(
        &self,
        id: &GmailMessageId,
        priority: Priority,
    ) -> Result<Message, GmailError> {
        self.json(Call {
            method: Method::POST,
            segments: &["messages", id.as_str(), "trash"],
            query: &[],
            body: None::<&()>,
            priority,
            cost: COST_TRASH,
        })
        .await
    }

    pub async fn send_message(
        &self,
        raw: &[u8],
        thread_id: Option<&GmailThreadId>,
        priority: Priority,
    ) -> Result<Message, GmailError> {
        let body = raw_message_json(raw, thread_id);
        self.json(Call {
            method: Method::POST,
            segments: &["messages", "send"],
            query: &[],
            body: Some(&body),
            priority,
            cost: COST_SEND,
        })
        .await
    }

    pub async fn get_attachment(
        &self,
        message_id: &GmailMessageId,
        attachment_id: &GmailAttachmentId,
        priority: Priority,
    ) -> Result<Vec<u8>, GmailError> {
        let part: PartBody = self
            .json(Call {
                method: Method::GET,
                segments: &[
                    "messages",
                    message_id.as_str(),
                    "attachments",
                    attachment_id.as_str(),
                ],
                query: &[],
                body: None::<&()>,
                priority,
                cost: COST_ATTACHMENT,
            })
            .await?;
        Ok(part
            .data
            .as_deref()
            .map(decode_base64_lenient)
            .unwrap_or_default())
    }

    pub async fn list_drafts(
        &self,
        page_token: Option<&PageToken>,
        priority: Priority,
    ) -> Result<DraftPage, GmailError> {
        let mut query = vec![("maxResults", "500")];
        if let Some(tok) = page_token {
            query.push(("pageToken", tok.as_str()));
        }
        self.json(Call {
            method: Method::GET,
            segments: &["drafts"],
            query: &query,
            body: None::<&()>,
            priority,
            cost: COST_DRAFTS_LIST,
        })
        .await
    }

    pub async fn get_draft(
        &self,
        id: &GmailDraftId,
        format: Format,
        priority: Priority,
    ) -> Result<Draft, GmailError> {
        let query = [("format", format.as_str())];
        self.json(Call {
            method: Method::GET,
            segments: &["drafts", id.as_str()],
            query: &query,
            body: None::<&()>,
            priority,
            cost: cost_message_get(format),
        })
        .await
    }

    pub async fn create_draft(
        &self,
        raw: &[u8],
        thread_id: Option<&GmailThreadId>,
        priority: Priority,
    ) -> Result<Draft, GmailError> {
        let body = json!({ "message": raw_message_json(raw, thread_id) });
        self.json(Call {
            method: Method::POST,
            segments: &["drafts"],
            query: &[],
            body: Some(&body),
            priority,
            cost: COST_DRAFT_CREATE,
        })
        .await
    }

    pub async fn update_draft(
        &self,
        id: &GmailDraftId,
        raw: &[u8],
        thread_id: Option<&GmailThreadId>,
        priority: Priority,
    ) -> Result<Draft, GmailError> {
        let body = json!({ "id": id, "message": raw_message_json(raw, thread_id) });
        self.json(Call {
            method: Method::PUT,
            segments: &["drafts", id.as_str()],
            query: &[],
            body: Some(&body),
            priority,
            cost: COST_DRAFT_UPDATE,
        })
        .await
    }

    pub async fn send_draft(
        &self,
        id: &GmailDraftId,
        priority: Priority,
    ) -> Result<Message, GmailError> {
        let body = json!({ "id": id });
        self.json(Call {
            method: Method::POST,
            segments: &["drafts", "send"],
            query: &[],
            body: Some(&body),
            priority,
            cost: COST_SEND,
        })
        .await
    }

    pub async fn delete_draft(
        &self,
        id: &GmailDraftId,
        priority: Priority,
    ) -> Result<(), GmailError> {
        self.empty(Call {
            method: Method::DELETE,
            segments: &["drafts", id.as_str()],
            query: &[],
            body: None::<&()>,
            priority,
            cost: COST_DRAFT_DELETE,
        })
        .await
    }

    fn endpoint(&self, segments: &[&str]) -> Result<Url, GmailError> {
        let mut url = self.base_url.clone();
        {
            let Ok(mut path) = url.path_segments_mut() else {
                return Err(GmailError::new(
                    Outcome::InvalidRequest("base URL can't take path segments".to_owned()),
                    "invalid Gmail base URL",
                ));
            };
            path.pop_if_empty();
            for fixed in ["gmail", "v1", "users", "me"] {
                path.push(fixed);
            }
            for seg in segments {
                path.push(seg);
            }
        }
        Ok(url)
    }

    async fn json<T: DeserializeOwned, B: Serialize + Sync>(
        &self,
        call: Call<'_, B>,
    ) -> Result<T, GmailError> {
        let resp = self.send(&call).await?;
        serde_json::from_str(&resp.body)
            .map_err(|e| GmailError::new(Outcome::NoResponse, format!("bad Gmail JSON: {e}")))
    }

    async fn empty<B: Serialize + Sync>(&self, call: Call<'_, B>) -> Result<(), GmailError> {
        let _ = self.send(&call).await?;
        Ok(())
    }

    async fn send<B: Serialize + Sync>(
        &self,
        call: &Call<'_, B>,
    ) -> Result<ReadResponse, GmailError> {
        let url = self.endpoint(call.segments)?;
        let token = self.tokens.access_token().await.map_err(not_sent)?;
        let _permit = self.scheduler.acquire(call.priority, call.cost).await;

        let first = self.dispatch_once(call, &url, token.expose()).await?;
        if first.status.as_u16() == 401 {
            let refreshed = self
                .tokens
                .refresh_after_401(token.expose())
                .await
                .map_err(not_sent)?;
            let second = self.dispatch_once(call, &url, refreshed.expose()).await?;
            return if second.status.is_success() {
                Ok(second)
            } else {
                Err(second.to_error())
            };
        }
        if first.status.is_success() {
            Ok(first)
        } else {
            Err(first.to_error())
        }
    }

    async fn dispatch_once<B: Serialize + Sync>(
        &self,
        call: &Call<'_, B>,
        url: &Url,
        token: &str,
    ) -> Result<ReadResponse, GmailError> {
        let body =
            call.body.map(serde_json::to_vec).transpose().map_err(|e| {
                GmailError::new(Outcome::InvalidRequest(e.to_string()), e.to_string())
            })?;
        let mut req = self
            .http
            .google_json(call.method.clone(), url.clone(), body)
            .bearer_auth(token);
        if !call.query.is_empty() {
            req = req.query(call.query);
        }
        let response = req
            .send()
            .await
            .map_err(|e| GmailError::from_transport(&e))?;
        ReadResponse::read(response).await
    }
}

fn raw_message_json(raw: &[u8], thread_id: Option<&GmailThreadId>) -> Value {
    let encoded = URL_SAFE_NO_PAD.encode(raw);
    match thread_id {
        Some(tid) => json!({ "raw": encoded, "threadId": tid }),
        None => json!({ "raw": encoded }),
    }
}

fn not_sent(err: GmailError) -> GmailError {
    if matches!(
        err.outcome,
        Outcome::NoResponse | Outcome::ServerError { .. }
    ) {
        GmailError::new(Outcome::Offline, err.detail)
    } else {
        err
    }
}

pub async fn one_click_unsubscribe(http: &HttpClient, raw_url: &str) -> Result<(), GmailError> {
    let parsed = Url::parse(raw_url).map_err(|_parse| {
        GmailError::new(
            Outcome::InvalidRequest("invalid unsubscribe URL".to_owned()),
            "invalid unsubscribe URL",
        )
    })?;
    if !may_post_unsubscribe(&parsed) {
        return Err(GmailError::new(
            Outcome::InvalidRequest("refused unsafe unsubscribe URL".to_owned()),
            "unsubscribe link must use HTTPS",
        ));
    }
    let response = http
        .lists()
        .post(parsed)
        .header("content-type", "application/x-www-form-urlencoded")
        .body("List-Unsubscribe=One-Click")
        .send()
        .await
        .map_err(|e| GmailError::from_transport(&e))?;
    let status = response.status();
    if status.is_success() {
        Ok(())
    } else {
        Err(GmailError::new(
            Outcome::Rejected {
                status: status.as_u16(),
                reason: None,
                message: format!("unsubscribe endpoint returned HTTP {}", status.as_u16()),
            },
            format!("unsubscribe endpoint returned HTTP {}", status.as_u16()),
        ))
    }
}

fn may_post_unsubscribe(url: &Url) -> bool {
    let is_local = is_loopback_or_private(url);
    match url.scheme() {
        "https" => cfg!(debug_assertions) || !is_local,
        "http" => cfg!(debug_assertions) && is_local,
        _ => false,
    }
}

fn is_loopback_or_private(url: &Url) -> bool {
    match url.host() {
        Some(Host::Domain(host)) => {
            let h = host.to_ascii_lowercase();
            h == "localhost"
                || h.rsplit('.').next() == Some("localhost")
                || h.rsplit('.').next() == Some("local")
        }
        Some(Host::Ipv4(ip)) => is_private_ip(IpAddr::V4(ip)),
        Some(Host::Ipv6(ip)) => is_private_ip(IpAddr::V6(ip)),
        None => true,
    }
}

const fn is_private_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_broadcast()
        }
        IpAddr::V6(v6) => {
            v6.is_loopback()
                || v6.is_unspecified()
                || (v6.segments()[0] & 0xfe00) == 0xfc00
                || (v6.segments()[0] & 0xffc0) == 0xfe80
        }
    }
}
