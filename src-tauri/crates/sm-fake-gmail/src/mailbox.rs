//! One user's mailbox: labels, messages grouped into threads, drafts and history.

use std::collections::{BTreeMap, HashMap, HashSet};

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

pub(crate) struct HistoryRecord {
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
    /// `settings.filters`, oldest first.
    pub filters: Vec<Filter>,
    history: Vec<HistoryRecord>,
    next_id: u64,
    clock_ms: u64,
}

/// A Gmail filter: mail from `from` gets `add` and loses `remove`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Filter {
    /// Gmail's id of the filter.
    pub id: String,
    /// An address, or `@domain`.
    pub from: String,
    /// The label ids matching mail gets.
    pub add: Vec<String>,
    /// The label ids matching mail loses.
    pub remove: Vec<String>,
}

impl Filter {
    /// The filter as `settings.filters` serves it.
    #[must_use]
    pub fn json(&self) -> Value {
        json!({
            "id": self.id,
            "criteria": { "from": self.from },
            "action": { "addLabelIds": self.add, "removeLabelIds": self.remove },
        })
    }

    /// Gmail's `from:` match, as far as the app uses it: a whole address, or
    /// `@domain` matching every address there.
    fn matches(&self, sender: &str) -> bool {
        let sender = sender.to_ascii_lowercase();
        let from = self.from.to_ascii_lowercase();
        if from.starts_with('@') {
            sender.ends_with(&from)
        } else {
            sender == from
        }
    }
}

/// Returned when something is not there.
#[derive(Debug)]
pub(crate) struct Missing;

impl Label {
    pub(crate) fn json(&self) -> Value {
        if self.system {
            json!({ "id": self.id, "name": self.name, "type": "system" })
        } else {
            json!({
                "id": self.id,
                "name": self.name,
                "type": "user",
                "labelListVisibility": "labelShow",
                "messageListVisibility": "show",
            })
        }
    }
}

impl Msg {
    /// The message in the shape `format` asks for.
    pub(crate) fn json(&self, format: &str, metadata_headers: &[&str]) -> Value {
        let mut out = json!({
            "id": self.id,
            "threadId": self.thread_id,
            "labelIds": self.label_ids,
            "snippet": self.snippet,
            "historyId": self.history_id.to_string(),
            "internalDate": self.internal_date.to_string(),
            "sizeEstimate": self.raw.len(),
        });
        match format {
            "minimal" => {}
            "metadata" => {
                let headers: Vec<Value> = self
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
                    "mimeType": self.payload["mimeType"],
                    "filename": "",
                    "headers": headers,
                    "body": { "size": 0 },
                });
            }
            "raw" => out["raw"] = json!(URL_SAFE.encode(&self.raw)),
            _ => out["payload"] = self.payload.clone(),
        }
        out
    }
}

impl Mailbox {
    pub(crate) fn new(email: &str) -> Self {
        Self {
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
            filters: Vec::new(),
            history: Vec::new(),
            next_id: id_base(email),
            clock_ms: 1_700_000_000_000,
        }
    }

    /// Labels for incoming mail from `sender` after the mailbox's filters.
    pub(crate) fn filtered_labels(&self, sender: &str, mut labels: Vec<String>) -> Vec<String> {
        for f in self.filters.iter().filter(|f| f.matches(sender)) {
            labels.retain(|l| !f.remove.contains(l));
            for l in &f.add {
                if !labels.contains(l) {
                    labels.push(l.clone());
                }
            }
        }
        labels
    }

    /// A fresh Gmail-looking hex id, never one another mailbox hands out.
    pub(crate) fn new_id(&mut self) -> String {
        self.next_id += 1;
        format!("{:x}", self.next_id)
    }

    /// A strictly increasing `internalDate`, a minute apart.
    pub(crate) const fn next_date(&mut self) -> u64 {
        self.clock_ms += 60_000;
        self.clock_ms
    }

    pub(crate) fn label(&self, id: &str) -> Option<&Label> {
        self.labels.iter().find(|l| l.id == id)
    }

    /// Creates a user label, or `None` if the name is taken.
    pub(crate) fn create_label(&mut self, name: &str) -> Option<String> {
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
    pub(crate) fn insert_raw(
        &mut self,
        raw: Vec<u8>,
        thread_id: Option<String>,
        label_ids: Vec<String>,
        internal_date: Option<u64>,
    ) -> String {
        let id = self.new_id();
        let mut attachments = 0;
        let parsed = mime::parse(&raw, &mut || {
            attachments += 1;
            format!("ANGjdJ_{id}_{attachments}")
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
    pub(crate) fn delete(&mut self, id: &str) -> Result<(), Missing> {
        if !self.messages.contains_key(id) {
            return Err(Missing);
        }
        self.record(id, Change::Deleted);
        self.messages.remove(id);
        Ok(())
    }

    /// Stores `raw` as a new draft's message (`DRAFT`), in `thread_id` or a new thread.
    /// Returns the draft id.
    pub(crate) fn create_draft(
        &mut self,
        raw: Vec<u8>,
        thread_id: Option<String>,
        date: Option<u64>,
    ) -> String {
        let message_id = self.insert_raw(raw, thread_id, vec!["DRAFT".into()], date);
        let draft_id = format!("r-{}", self.new_id());
        self.drafts.insert(draft_id.clone(), message_id);
        draft_id
    }

    /// Replaces a draft's message, keeping its thread unless `thread_id` names another.
    /// Returns the new message id.
    pub(crate) fn update_draft(
        &mut self,
        draft_id: &str,
        raw: Vec<u8>,
        thread_id: Option<String>,
        date: Option<u64>,
    ) -> Result<String, Missing> {
        let old = self.drafts.get(draft_id).cloned().ok_or(Missing)?;
        let thread_id = thread_id.unwrap_or_else(|| self.messages[&old].thread_id.clone());
        self.delete(&old)?;
        let message_id = self.insert_raw(raw, Some(thread_id), vec!["DRAFT".into()], date);
        self.drafts.insert(draft_id.to_owned(), message_id.clone());
        Ok(message_id)
    }

    /// Sends a draft: the draft and its message go, and a `SENT` message with `raw` (or the
    /// draft's own bytes) joins the thread. Returns the sent message id.
    pub(crate) fn send_draft(
        &mut self,
        draft_id: &str,
        raw: Option<Vec<u8>>,
        date: Option<u64>,
    ) -> Result<String, Missing> {
        let old = self.drafts.remove(draft_id).ok_or(Missing)?;
        let raw = raw.unwrap_or_else(|| self.messages[&old].raw.clone());
        let thread_id = self.messages[&old].thread_id.clone();
        self.delete(&old)?;
        Ok(self.insert_raw(raw, Some(thread_id), vec!["SENT".into()], date))
    }

    /// Deletes a draft and its message.
    pub(crate) fn delete_draft(&mut self, draft_id: &str) -> Result<(), Missing> {
        let message = self.drafts.remove(draft_id).ok_or(Missing)?;
        self.delete(&message)
    }

    /// Adds and removes labels, recording only real changes.
    pub(crate) fn modify<L: AsRef<str>>(
        &mut self,
        id: &str,
        add: &[L],
        remove: &[L],
    ) -> Result<(), Missing> {
        let msg = self.messages.get_mut(id).ok_or(Missing)?;
        let has = |labels: &[String], l: &str| labels.iter().any(|x| x == l);
        let added: Vec<String> = add
            .iter()
            .map(AsRef::as_ref)
            .filter(|l| !has(&msg.label_ids, l))
            .map(str::to_owned)
            .collect();
        msg.label_ids.extend(added.iter().cloned());
        let removed: Vec<String> = remove
            .iter()
            .map(AsRef::as_ref)
            .filter(|&l| has(&msg.label_ids, l) && !add.iter().any(|a| a.as_ref() == l))
            .map(str::to_owned)
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

    pub(crate) fn profile_json(&self) -> Value {
        let threads: HashSet<&str> = self
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

    pub(crate) fn labels_json(&self) -> Value {
        json!({ "labels": self.labels.iter().map(Label::json).collect::<Vec<_>>() })
    }

    pub(crate) fn send_as_json(&self) -> Value {
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

    /// A thread's messages, oldest first.
    pub(crate) fn thread_messages(&self, thread_id: &str) -> Vec<&Msg> {
        let mut messages: Vec<&Msg> = self
            .messages
            .values()
            .filter(|m| m.thread_id == thread_id)
            .collect();
        messages.sort_by(|a, b| (a.internal_date, &a.id).cmp(&(b.internal_date, &b.id)));
        messages
    }

    pub(crate) fn thread_json(
        &self,
        thread_id: &str,
        format: &str,
        headers: &[&str],
    ) -> Option<Value> {
        let messages = self.thread_messages(thread_id);
        if messages.is_empty() {
            return None;
        }
        let mut out = thread_summary_json(thread_id, &messages);
        out["messages"] = messages.iter().map(|m| m.json(format, headers)).collect();
        Some(out)
    }

    /// Thread ids matching `q` and `label_ids`, newest first. `now_ms` is what
    /// `newer_than:` and `older_than:` count back from.
    pub(crate) fn search_threads(
        &self,
        q: &str,
        label_ids: &[&str],
        include_spam_trash: bool,
        now_ms: u64,
    ) -> Vec<&str> {
        let query = Query::parse(q, self, now_ms);
        let wants_spam_trash = include_spam_trash
            || query.wants_spam_trash
            || label_ids.iter().any(|&l| l == "SPAM" || l == "TRASH");
        let mut latest: HashMap<&str, u64> = HashMap::new();
        for msg in self.messages.values() {
            let date = latest.entry(&msg.thread_id).or_default();
            *date = (*date).max(msg.internal_date);
        }
        let mut hits: Vec<&str> = self
            .messages
            .values()
            .filter(|msg| {
                let hidden = msg.label_ids.iter().any(|l| l == "SPAM" || l == "TRASH");
                (!hidden || wants_spam_trash)
                    && label_ids
                        .iter()
                        .all(|l| msg.label_ids.iter().any(|x| x == l))
                    && query.matches(msg)
            })
            .map(|msg| msg.thread_id.as_str())
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();
        hits.sort_by(|a, b| latest[b].cmp(&latest[a]).then(b.cmp(a)));
        hits
    }

    pub(crate) fn thread_summary_json(&self, thread_id: &str) -> Value {
        thread_summary_json(thread_id, &self.thread_messages(thread_id))
    }

    /// History records after `start`, or `Missing` if `start` is older than the floor.
    pub(crate) fn history_after(&self, start: u64) -> Result<Vec<&HistoryRecord>, Missing> {
        if start < self.history_floor {
            return Err(Missing);
        }
        Ok(self.history.iter().filter(|r| r.id > start).collect())
    }
}

/// `id`, `snippet` and `historyId` of a thread whose messages are `messages`, oldest first.
fn thread_summary_json(thread_id: &str, messages: &[&Msg]) -> Value {
    json!({
        "id": thread_id,
        "snippet": messages.last().map(|m| m.snippet.as_str()).unwrap_or_default(),
        "historyId": messages.iter().map(|m| m.history_id).max().unwrap_or_default().to_string(),
    })
}

impl HistoryRecord {
    pub(crate) fn json(&self) -> Value {
        let message = json!({ "id": self.message_id, "threadId": self.thread_id, "labelIds": self.label_ids });
        let mut out = json!({
            "id": self.id.to_string(),
            "messages": [{ "id": self.message_id, "threadId": self.thread_id }],
        });
        match &self.change {
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
}

/// A tiny subset of Gmail search: `in:`/`label:`/`is:` label filters, `from:`, `to:`,
/// `subject:`, `newer_than:`/`older_than:` (`d`, `m`, `y`), `-` negation, and bare words
/// matched against subject, from and snippet. Other operators are ignored.
struct Query {
    terms: Vec<(bool, Term)>,
    wants_spam_trash: bool,
}

enum Term {
    Label(String),
    Header(&'static str, String),
    Word(String),
    /// `internalDate` at or after this.
    Since(u64),
    /// `internalDate` before this.
    Before(u64),
}

/// `7d`, `2m`, `1y` in milliseconds (a month is 30 days, a year 365).
fn age_ms(text: &str) -> Option<u64> {
    let unit = text.chars().last()?;
    let n: u64 = text[..text.len() - unit.len_utf8()].parse().ok()?;
    let days = match unit {
        'd' => n,
        'm' => n * 30,
        'y' => n * 365,
        _ => return None,
    };
    Some(days * 86_400_000)
}

impl Query {
    fn parse(q: &str, mailbox: &Mailbox, now_ms: u64) -> Self {
        let mut query = Self {
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
                Some(("newer_than", v)) => match age_ms(v) {
                    Some(age) => Term::Since(now_ms.saturating_sub(age)),
                    None => continue,
                },
                Some(("older_than", v)) => match age_ms(v) {
                    Some(age) => Term::Before(now_ms.saturating_sub(age)),
                    None => continue,
                },
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
                Term::Since(t) => msg.internal_date >= *t,
                Term::Before(t) => msg.internal_date < *t,
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

/// Where a mailbox's ids start. Real Gmail ids are unique across mailboxes,
/// so each fake mailbox counts in its own range, fixed by its address (FNV-1a).
fn id_base(email: &str) -> u64 {
    let hash = email.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    });
    0x18c0_0000_0000_0000 | ((hash & 0xffff) << 32)
}
