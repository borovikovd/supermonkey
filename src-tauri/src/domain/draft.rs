use serde::{Deserialize, Serialize};

use super::{
    Addr, DraftKind, DraftWait, GmailDraftId, GmailMessageId, GmailThreadId, MessageId, Thread,
    is_forward_subject,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DraftContentRef<'a> {
    pub from: Option<&'a Addr>,
    pub to: &'a [Addr],
    pub cc: &'a [Addr],
    pub bcc: &'a [Addr],
    pub subject: &'a str,
    pub body_html: &'a str,
}

impl DraftContentRef<'_> {
    #[must_use]
    pub fn matches_ignoring_body_ws<A, B>(
        self,
        other: Self,
        my_attachments: A,
        other_attachments: B,
        body_is_blank: bool,
    ) -> bool
    where
        A: IntoIterator,
        A::Item: AsRef<str>,
        B: IntoIterator,
        B::Item: AsRef<str>,
    {
        if self.from != other.from
            || self.to != other.to
            || self.cc != other.cc
            || self.bcc != other.bcc
            || self.subject != other.subject
            || !(self.body_html == other.body_html || (self.body_html.is_empty() && body_is_blank))
        {
            return false;
        }
        let mut a = my_attachments.into_iter();
        let mut b = other_attachments.into_iter();
        loop {
            match (a.next(), b.next()) {
                (Some(x), Some(y)) if x.as_ref() == y.as_ref() => {}
                (None, None) => return true,
                _ => return false,
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DraftDefaults {
    #[serde(default)]
    pub from: Option<Addr>,
    pub to: Vec<Addr>,
    pub cc: Vec<Addr>,
    pub bcc: Vec<Addr>,
    pub subject: String,
    pub body_html: String,
    pub attachments: Vec<String>,
}

impl DraftDefaults {
    #[must_use]
    pub fn content_ref(&self) -> DraftContentRef<'_> {
        DraftContentRef {
            from: self.from.as_ref(),
            to: &self.to,
            cc: &self.cc,
            bcc: &self.bcc,
            subject: &self.subject,
            body_html: &self.body_html,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DraftStage {
    Fresh { defaults: DraftDefaults },
    Kept,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GmailWrite {
    Update(GmailDraftId),
    Recover,
    Create,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DraftLifecycle {
    pub stage: DraftStage,
    pub waits_for: Option<DraftWait>,
    pub gmail_draft_id: Option<GmailDraftId>,
    pub gmail_message_id: Option<GmailMessageId>,
    pub gmail_thread_id: Option<GmailThreadId>,
    pub gmail_create_sent: bool,
    pub gmail_change_skipped: bool,
}

impl DraftLifecycle {
    #[must_use]
    pub const fn fresh(defaults: DraftDefaults, waits_for: Option<DraftWait>) -> Self {
        Self {
            stage: DraftStage::Fresh { defaults },
            waits_for,
            gmail_draft_id: None,
            gmail_message_id: None,
            gmail_thread_id: None,
            gmail_create_sent: false,
            gmail_change_skipped: false,
        }
    }

    #[must_use]
    pub const fn kept() -> Self {
        Self {
            stage: DraftStage::Kept,
            waits_for: None,
            gmail_draft_id: None,
            gmail_message_id: None,
            gmail_thread_id: None,
            gmail_create_sent: false,
            gmail_change_skipped: false,
        }
    }

    #[must_use]
    pub const fn is_kept(&self) -> bool {
        matches!(self.stage, DraftStage::Kept)
    }

    #[must_use]
    pub fn is_untouched<A>(
        &self,
        content: DraftContentRef<'_>,
        attachments: A,
        body_is_blank: bool,
    ) -> bool
    where
        A: IntoIterator,
        A::Item: AsRef<str>,
    {
        match &self.stage {
            DraftStage::Kept => false,
            DraftStage::Fresh { defaults } => defaults.content_ref().matches_ignoring_body_ws(
                content,
                defaults.attachments.iter().map(String::as_str),
                attachments,
                body_is_blank,
            ),
        }
    }

    pub fn keep(&mut self) {
        self.stage = DraftStage::Kept;
    }

    pub fn note_edit<A>(
        &mut self,
        content: DraftContentRef<'_>,
        attachments: A,
        body_is_blank: bool,
    ) where
        A: IntoIterator,
        A::Item: AsRef<str>,
    {
        if !self.is_untouched(content, attachments, body_is_blank) {
            self.stage = DraftStage::Kept;
        }
    }

    pub fn update_fresh_defaults(&mut self, defaults: DraftDefaults) {
        if let DraftStage::Fresh { defaults: slot } = &mut self.stage {
            *slot = defaults;
        }
    }

    pub fn finish_wait(&mut self, wait: DraftWait) -> bool {
        if self.waits_for == Some(wait) {
            self.waits_for = None;
            true
        } else {
            false
        }
    }

    #[must_use]
    pub fn next_gmail_write(&self) -> GmailWrite {
        if let Some(id) = &self.gmail_draft_id {
            GmailWrite::Update(id.clone())
        } else if self.gmail_create_sent {
            GmailWrite::Recover
        } else {
            GmailWrite::Create
        }
    }

    pub const fn mark_create_sent(&mut self) {
        self.gmail_create_sent = true;
    }

    pub fn record_gmail_ids(
        &mut self,
        draft_id: GmailDraftId,
        message_id: Option<GmailMessageId>,
        thread_id: Option<GmailThreadId>,
    ) {
        self.gmail_draft_id = Some(draft_id);
        self.gmail_message_id = message_id;
        if thread_id.is_some() {
            self.gmail_thread_id = thread_id;
        }
        self.gmail_create_sent = false;
    }

    pub fn clear_gmail_ids(&mut self) {
        self.gmail_draft_id = None;
        self.gmail_message_id = None;
        self.gmail_create_sent = false;
    }

    pub const fn mark_gmail_change_skipped(&mut self) {
        self.gmail_change_skipped = true;
    }

    pub const fn take_gmail_change_skipped(&mut self) -> bool {
        std::mem::replace(&mut self.gmail_change_skipped, false)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DraftAim {
    pub kind: DraftKind,
    pub target: Option<MessageId>,
}

#[must_use]
pub fn gmail_draft_kind(has_thread: bool, subject: &str, in_reply_to: Option<&str>) -> DraftKind {
    if !has_thread {
        return DraftKind::New;
    }
    if is_forward_subject(subject) && in_reply_to.is_none() {
        return DraftKind::Forward;
    }
    if is_forward_subject(subject) {
        return DraftKind::Forward;
    }
    DraftKind::Reply
}

#[must_use]
pub fn gmail_draft_aim(
    thread: Option<&Thread>,
    subject: &str,
    in_reply_to: Option<&str>,
    references: &[String],
) -> DraftAim {
    let Some(thread) = thread else {
        return DraftAim {
            kind: DraftKind::New,
            target: None,
        };
    };
    let kind = gmail_draft_kind(true, subject, in_reply_to);
    let target = in_reply_to
        .and_then(|id| find_by_header(thread, id))
        .or_else(|| {
            references
                .iter()
                .rev()
                .find_map(|id| find_by_header(thread, id))
        })
        .or_else(|| thread.latest_live().map(|m| m.id))
        .or_else(|| thread.latest_non_draft().map(|m| m.id));
    DraftAim { kind, target }
}

fn find_by_header(thread: &Thread, message_id: &str) -> Option<MessageId> {
    let want = message_id.trim().trim_matches(|c| c == '<' || c == '>');
    if want.is_empty() {
        return None;
    }
    thread.messages().iter().find_map(|m| {
        let hdr = m
            .message_id_hdr
            .as_deref()?
            .trim()
            .trim_matches(|c| c == '<' || c == '>');
        (!m.is_draft && hdr.eq_ignore_ascii_case(want)).then_some(m.id)
    })
}
