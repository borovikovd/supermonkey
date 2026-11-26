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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{LabelSet, MessageFacts, Millis, SystemLabel};

    fn defaults() -> DraftDefaults {
        DraftDefaults {
            from: Some(Addr::new("me@acme.test")),
            to: vec![Addr::new("bob@x.test")],
            cc: Vec::new(),
            bcc: Vec::new(),
            subject: "Re: Hi".to_owned(),
            body_html: String::new(),
            attachments: vec!["a1".to_owned()],
        }
    }

    #[test]
    fn draft_lifecycle_tracks_fresh_edit_waits_and_gmail_writes() {
        let defs = defaults();
        let mut lc = DraftLifecycle::fresh(defs.clone(), Some(DraftWait::TargetBody));
        assert!(!lc.is_kept(), "new draft starts fresh");
        assert!(
            lc.is_untouched(defs.content_ref(), ["a1"], true),
            "blank body matching defaults is untouched"
        );
        assert_eq!(
            lc.next_gmail_write(),
            GmailWrite::Create,
            "starts needing create"
        );

        assert!(
            lc.finish_wait(DraftWait::TargetBody),
            "matching wait clears"
        );
        assert!(
            !lc.finish_wait(DraftWait::TargetBody),
            "already cleared wait returns false"
        );

        let edited = DraftContentRef {
            body_html: "<p>Hello</p>",
            ..defs.content_ref()
        };
        lc.note_edit(edited, ["a1"], false);
        assert!(lc.is_kept(), "real edit promotes Fresh to Kept");
        assert!(
            !lc.is_untouched(defs.content_ref(), ["a1"], true),
            "Kept is never untouched"
        );

        lc.mark_create_sent();
        assert_eq!(
            lc.next_gmail_write(),
            GmailWrite::Recover,
            "in-flight create enters Recover"
        );
        lc.record_gmail_ids(
            GmailDraftId("gd1".to_owned()),
            Some(GmailMessageId("gm1".to_owned())),
            Some(GmailThreadId("gt1".to_owned())),
        );
        assert_eq!(
            lc.next_gmail_write(),
            GmailWrite::Update(GmailDraftId("gd1".to_owned())),
            "recorded id switches to Update"
        );
        lc.mark_gmail_change_skipped();
        assert!(lc.take_gmail_change_skipped(), "takes skipped flag once");
        assert!(!lc.take_gmail_change_skipped(), "second take is false");
        lc.clear_gmail_ids();
        assert_eq!(
            lc.next_gmail_write(),
            GmailWrite::Create,
            "cleared ids return to Create"
        );
    }

    fn sample_msg(id: i64, date: i64, hdr: &str) -> MessageFacts {
        MessageFacts {
            id: MessageId(id),
            gmail_id: GmailMessageId(format!("gm{id}")),
            date: Millis(date),
            from: Addr::new("alice@x.test"),
            to: vec![Addr::new("me@acme.test")],
            cc: Vec::new(),
            bcc: Vec::new(),
            reply_to: Vec::new(),
            subject: "Plan".to_owned(),
            snippet: String::new(),
            message_id_hdr: Some(hdr.to_owned()),
            in_reply_to: None,
            references: Vec::new(),
            is_list: false,
            is_draft: false,
            has_attachments: false,
            labels: std::iter::once(SystemLabel::Inbox.label_id()).collect::<LabelSet>(),
        }
    }

    #[test]
    fn gmail_draft_aim_resolves_reply_target_and_forward_kind() {
        let thread = Thread::new(vec![
            sample_msg(10, 100, "<M10@X.TEST>"),
            sample_msg(20, 200, "<m20@x.test>"),
        ]);

        assert_eq!(
            gmail_draft_aim(None, "New", None, &[]),
            DraftAim {
                kind: DraftKind::New,
                target: None,
            },
            "no thread yields New"
        );
        assert_eq!(
            gmail_draft_aim(Some(&thread), "Re: Plan", Some("m10@x.test"), &[]),
            DraftAim {
                kind: DraftKind::Reply,
                target: Some(MessageId(10)),
            },
            "In-Reply-To matches case-insensitively without angle brackets"
        );
        assert_eq!(
            gmail_draft_aim(
                Some(&thread),
                "Fwd: Plan",
                None,
                &["<missing@x.test>".to_owned(), "<m10@x.test>".to_owned()],
            ),
            DraftAim {
                kind: DraftKind::Forward,
                target: Some(MessageId(10)),
            },
            "References fallback picks newest matching header with Forward kind"
        );
        assert_eq!(
            gmail_draft_aim(Some(&thread), "Re: Plan", None, &[]),
            DraftAim {
                kind: DraftKind::Reply,
                target: Some(MessageId(20)),
            },
            "missing headers fall back to latest live message"
        );
    }
}
