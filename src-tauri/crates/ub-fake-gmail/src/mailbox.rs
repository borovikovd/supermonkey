//! One user's mailbox: labels, messages grouped into threads, drafts and history.

use std::collections::{BTreeMap, HashMap};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE;
use serde_json::{Value, json};

use crate::mime;

/// Gmail's system labels.
pub(crate) const SYSTEM_LABELS: &[&str] = &[
    "INBOX",
    "SENT",
    "DRAFT",
    "SPAM",
    "TRASH",
    "STARRED",
    "UNREAD",
    "IMPORTANT",
    "CHAT",
    "CATEGORY_PERSONAL",
    "CATEGORY_SOCIAL",
    "CATEGORY_PROMOTIONS",
    "CATEGORY_UPDATES",
    "CATEGORY_FORUMS",
];

pub(crate) struct Label {
    pub id: String,
    pub name: String,
    pub system: bool,
}

pub(crate) struct Msg {
    pub id: String,
    pub thread_id: String,
    pub label_ids: Vec<String>,
    pub internal_date: u64,
    pub snippet: String,
    pub history_id: u64,
    pub payload: Value,
    pub headers: Vec<(String, String)>,
    pub raw: Vec<u8>,
    pub attachments: HashMap<String, Vec<u8>>,
}

enum Change {
    Added,
    Deleted,
    LabelsAdded(Vec<String>),
    LabelsRemoved(Vec<String>),
}

struct HistoryRecord {
    id: u64,
    message_id: String,
    thread_id: String,
    label_ids: Vec<String>,
    change: Change,
}

pub(crate) struct SendAsEntry {
    pub email: String,
    pub display_name: String,
    pub primary: bool,
}

pub(crate) struct Mailbox {
    pub email: String,
    pub send_as: Vec<SendAsEntry>,
    pub history_id: u64,
    /// `history.list` with an older start answers 404.
    pub history_floor: u64,
    pub labels: Vec<Label>,
    pub messages: BTreeMap<String, Msg>,
    /// Draft id to message id.
    pub drafts: BTreeMap<String, String>,
    history: Vec<HistoryRecord>,
    next_id: u64,
    clock_ms: u64,
}

/// Returned when something is not there.
pub(crate) struct Missing;

impl Mailbox {
    pub fn new(email: &str) -> Self {
        Mailbox {
            email: email.to_owned(),
            send_as: vec![SendAsEntry {
                email: email.to_owned(),
                display_name: String::new(),
                primary: true,
            }],
            history_id: 1000,
            history_floor: 0,
            labels: SYSTEM_LABELS
                .iter()
                .map(|&id| Label {
                    id: id.to_owned(),
                    name: id.to_owned(),
                    system: true,
                })
                .collect(),
            messages: BTreeMap::new(),
            drafts: BTreeMap::new(),
            history: Vec::new(),
            next_id: 0x18c0_0000_0000_0000,
            clock_ms: 1_700_000_000_000,
        }
    }

    /// A fresh Gmail-looking hex id.
    pub fn new_id(&mut self) -> String {
        self.next_id += 1;
        format!("{:x}", self.next_id)
    }

    /// A strictly increasing `internalDate`, a minute apart.
    pub fn next_date(&mut self) -> u64 {
        self.clock_ms += 60_000;
        self.clock_ms
    }

    pub fn label(&self, id: &str) -> Option<&Label> {
        self.labels.iter().find(|l| l.id == id)
    }

    /// Creates a user label, or `None` if the name is taken.
    pub fn create_label(&mut self, name: &str) -> Option<String> {
        if self
            .labels
            .iter()
            .any(|l| l.name.eq_ignore_ascii_case(name))
        {
            return None;
        }
        let id = format!("Label_{}", self.labels.len() - SYSTEM_LABELS.len() + 1);
        self.labels.push(Label {
            id: id.clone(),
            name: name.to_owned(),
            system: false,
        });
        Some(id)
    }

    fn record(&mut self, msg_id: &str, change: Change) {
        self.history_id += 1;
        let id = self.history_id;
        let msg = self
            .messages
            .get_mut(msg_id)
            .expect("recorded message exists");
        msg.history_id = id;
        self.history.push(HistoryRecord {
            id,
            message_id: msg.id.clone(),
            thread_id: msg.thread_id.clone(),
            label_ids: msg.label_ids.clone(),
            change,
        });
    }

    /// Stores RFC 822 bytes as a new message and records `messagesAdded`.
    /// Without a thread id the message starts its own thread.
    pub fn insert_raw(
        &mut self,
        raw: Vec<u8>,
        thread_id: Option<String>,
        label_ids: Vec<String>,
        internal_date: Option<u64>,
    ) -> String {
        let id = self.new_id();
        let mut attachment_ids = Vec::new();
        let parsed = mime::parse(&raw, &mut || {
            let n = attachment_ids.len() + 1;
            let att = format!("ANGjdJ_{id}_{n}");
            attachment_ids.push(att.clone());
            att
        });
        let internal_date = internal_date.unwrap_or_else(|| self.next_date());
        let msg = Msg {
            id: id.clone(),
            thread_id: thread_id.unwrap_or_else(|| id.clone()),
            label_ids,
            internal_date,
            snippet: mime::snippet(&parsed.text),
            history_id: self.history_id,
            payload: parsed.payload,
            headers: parsed.headers,
            raw,
            attachments: parsed.attachments.into_iter().collect(),
        };
        self.messages.insert(id.clone(), msg);
        self.record(&id, Change::Added);
        id
    }

    /// Removes a message and records `messagesDeleted`.
    pub fn delete(&mut self, id: &str) -> Result<Msg, Missing> {
        if !self.messages.contains_key(id) {
            return Err(Missing);
        }
        self.record(id, Change::Deleted);
        Ok(self.messages.remove(id).expect("checked above"))
    }

    /// Adds and removes labels, recording only real changes.
    pub fn modify(&mut self, id: &str, add: &[String], remove: &[String]) -> Result<(), Missing> {
        let msg = self.messages.get_mut(id).ok_or(Missing)?;
        let added: Vec<String> = add
            .iter()
            .filter(|l| !msg.label_ids.contains(l))
            .cloned()
            .collect();
        msg.label_ids.extend(added.iter().cloned());
        let removed: Vec<String> = remove
            .iter()
            .filter(|l| msg.label_ids.contains(l) && !add.contains(l))
            .cloned()
            .collect();
        msg.label_ids.retain(|l| !removed.contains(l));
        if !added.is_empty() {
            self.record(id, Change::LabelsAdded(added));
        }
        if !removed.is_empty() {
            self.record(id, Change::LabelsRemoved(removed));
        }
        Ok(())
    }

    pub fn profile_json(&self) -> Value {
        let threads: std::collections::HashSet<&str> = self
            .messages
            .values()
            .map(|m| m.thread_id.as_str())
            .collect();
        json!({
            "emailAddress": self.email,
            "messagesTotal": self.messages.len(),
            "threadsTotal": threads.len(),
            "historyId": self.history_id.to_string(),
        })
    }

    pub fn labels_json(&self) -> Value {
        json!({ "labels": self.labels.iter().map(label_json).collect::<Vec<_>>() })
    }

    pub fn send_as_json(&self) -> Value {
        let entries: Vec<Value> = self
            .send_as
            .iter()
            .map(|s| {
                let mut entry = json!({
                    "sendAsEmail": s.email,
                    "displayName": s.display_name,
                    "replyToAddress": "",
                    "signature": "",
                    "isPrimary": s.primary,
                    "isDefault": s.primary,
                });
                if !s.primary {
                    entry["treatAsAlias"] = json!(true);
                    entry["verificationStatus"] = json!("accepted");
                }
                entry
            })
            .collect();
        json!({ "sendAs": entries })
    }

    /// A message in the shape `format` asks for.
    pub fn message_json(&self, msg: &Msg, format: &str, metadata_headers: &[String]) -> Value {
        let mut out = json!({
            "id": msg.id,
            "threadId": msg.thread_id,
            "labelIds": msg.label_ids,
            "snippet": msg.snippet,
            "historyId": msg.history_id.to_string(),
            "internalDate": msg.internal_date.to_string(),
            "sizeEstimate": msg.raw.len(),
        });
        match format {
            "minimal" => {}
            "metadata" => {
                let headers: Vec<Value> = msg
                    .headers
                    .iter()
                    .filter(|(name, _)| {
                        metadata_headers.is_empty()
                            || metadata_headers
                                .iter()
                                .any(|h| h.eq_ignore_ascii_case(name))
                    })
                    .map(|(name, value)| json!({ "name": name, "value": value }))
                    .collect();
                out["payload"] = json!({
                    "partId": "",
                    "mimeType": msg.payload["mimeType"],
                    "filename": "",
                    "headers": headers,
                    "body": { "size": 0 },
                });
            }
            "raw" => out["raw"] = json!(URL_SAFE.encode(&msg.raw)),
            _ => out["payload"] = msg.payload.clone(),
        }
        out
    }

    fn thread_messages(&self, thread_id: &str) -> Vec<&Msg> {
        let mut messages: Vec<&Msg> = self
            .messages
            .values()
            .filter(|m| m.thread_id == thread_id)
            .collect();
        messages.sort_by(|a, b| (a.internal_date, &a.id).cmp(&(b.internal_date, &b.id)));
        messages
    }

    pub fn thread_json(&self, thread_id: &str, format: &str, headers: &[String]) -> Option<Value> {
        let messages = self.thread_messages(thread_id);
        let last = messages.last()?;
        Some(json!({
            "id": thread_id,
            "snippet": last.snippet,
            "historyId": messages.iter().map(|m| m.history_id).max().unwrap_or_default().to_string(),
            "messages": messages.iter().map(|m| self.message_json(m, format, headers)).collect::<Vec<_>>(),
        }))
    }

    /// Thread ids matching `q` and `label_ids`, newest first.
    pub fn search_threads(
        &self,
        q: &str,
        label_ids: &[String],
        include_spam_trash: bool,
    ) -> Vec<String> {
        let query = Query::parse(q, self);
        let wants_spam_trash = include_spam_trash
            || query.wants_spam_trash
            || label_ids.iter().any(|l| l == "SPAM" || l == "TRASH");
        let mut latest: HashMap<&str, u64> = HashMap::new();
        for msg in self.messages.values() {
            let date = latest.entry(&msg.thread_id).or_default();
            *date = (*date).max(msg.internal_date);
        }
        let mut hits: Vec<&str> = Vec::new();
        for msg in self.messages.values() {
            let hidden = msg.label_ids.iter().any(|l| l == "SPAM" || l == "TRASH");
            if (hidden && !wants_spam_trash)
                || !label_ids.iter().all(|l| msg.label_ids.contains(l))
                || !query.matches(msg)
                || hits.contains(&msg.thread_id.as_str())
            {
                continue;
            }
            hits.push(&msg.thread_id);
        }
        hits.sort_by(|a, b| latest[b].cmp(&latest[a]).then(b.cmp(a)));
        hits.into_iter().map(str::to_owned).collect()
    }

    pub fn thread_summary_json(&self, thread_id: &str) -> Value {
        let messages = self.thread_messages(thread_id);
        json!({
            "id": thread_id,
            "snippet": messages.last().map(|m| m.snippet.clone()).unwrap_or_default(),
            "historyId": messages.iter().map(|m| m.history_id).max().unwrap_or_default().to_string(),
        })
    }

    /// History records after `start`, or `Missing` if `start` is older than the floor.
    pub fn history_after(&self, start: u64) -> Result<Vec<Value>, Missing> {
        if start < self.history_floor {
            return Err(Missing);
        }
        Ok(self
            .history
            .iter()
            .filter(|r| r.id > start)
            .map(history_json)
            .collect())
    }
}

pub(crate) fn label_json(label: &Label) -> Value {
    if label.system {
        json!({ "id": label.id, "name": label.name, "type": "system" })
    } else {
        json!({
            "id": label.id,
            "name": label.name,
            "type": "user",
            "labelListVisibility": "labelShow",
            "messageListVisibility": "show",
        })
    }
}

fn history_json(r: &HistoryRecord) -> Value {
    let message = json!({ "id": r.message_id, "threadId": r.thread_id, "labelIds": r.label_ids });
    let mut out = json!({
        "id": r.id.to_string(),
        "messages": [{ "id": r.message_id, "threadId": r.thread_id }],
    });
    match &r.change {
        Change::Added => out["messagesAdded"] = json!([{ "message": message }]),
        Change::Deleted => out["messagesDeleted"] = json!([{ "message": message }]),
        Change::LabelsAdded(ids) => {
            out["labelsAdded"] = json!([{ "message": message, "labelIds": ids }]);
        }
        Change::LabelsRemoved(ids) => {
            out["labelsRemoved"] = json!([{ "message": message, "labelIds": ids }]);
        }
    }
    out
}

/// A tiny subset of Gmail search: `in:`/`label:`/`is:` label filters, `from:`, `to:`,
/// `subject:`, `-` negation, and bare words matched against subject, from and snippet.
/// Other operators are ignored.
struct Query {
    terms: Vec<(bool, Term)>,
    wants_spam_trash: bool,
}

enum Term {
    Label(String),
    Header(&'static str, String),
    Word(String),
}

impl Query {
    fn parse(q: &str, mailbox: &Mailbox) -> Query {
        let mut query = Query {
            terms: Vec::new(),
            wants_spam_trash: false,
        };
        for token in q.split_whitespace() {
            let (negated, token) = match token.strip_prefix('-') {
                Some(rest) => (true, rest),
                None => (false, token),
            };
            let token = token.trim_matches('"').to_lowercase();
            let term = match token.split_once(':') {
                Some(("in" | "label", "anywhere")) => {
                    query.wants_spam_trash = true;
                    continue;
                }
                Some(("in" | "label" | "is", name)) => {
                    if !negated && (name == "spam" || name == "trash") {
                        query.wants_spam_trash = true;
                    }
                    match name {
                        "read" => {
                            query.terms.push((!negated, Term::Label("UNREAD".into())));
                            continue;
                        }
                        _ => Term::Label(resolve_label(name, mailbox)),
                    }
                }
                Some(("from", v)) => Term::Header("From", v.to_owned()),
                Some(("to", v)) => Term::Header("To", v.to_owned()),
                Some(("subject", v)) => Term::Header("Subject", v.to_owned()),
                Some(_) => continue,
                None if token.is_empty() => continue,
                None => Term::Word(token),
            };
            query.terms.push((negated, term));
        }
        query
    }

    fn matches(&self, msg: &Msg) -> bool {
        self.terms.iter().all(|(negated, term)| {
            let hit = match term {
                Term::Label(id) => msg.label_ids.contains(id),
                Term::Header(name, value) => mime::header(&msg.headers, name)
                    .is_some_and(|h| h.to_lowercase().contains(value)),
                Term::Word(word) => ["Subject", "From"]
                    .iter()
                    .filter_map(|h| mime::header(&msg.headers, h))
                    .chain([msg.snippet.as_str()])
                    .any(|text| text.to_lowercase().contains(word)),
            };
            hit != *negated
        })
    }
}

/// A query label name to a label id. Unknown names give an id nothing carries.
fn resolve_label(name: &str, mailbox: &Mailbox) -> String {
    let system = match name {
        "drafts" | "draft" => Some("DRAFT"),
        "sent" => Some("SENT"),
        "important" => Some("IMPORTANT"),
        _ => None,
    };
    if let Some(id) = system {
        return id.to_owned();
    }
    let dashed = |s: &str| s.to_lowercase().replace([' ', '/'], "-");
    mailbox
        .labels
        .iter()
        .find(|l| dashed(&l.name) == dashed(name) || l.id.eq_ignore_ascii_case(name))
        .map_or_else(|| format!("unknown:{name}"), |l| l.id.clone())
}
