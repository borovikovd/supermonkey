//! Draft rules (SPEC Reply composer, Drafts): the untouched check, and the
//! kind and target a draft first read from Gmail gets.

use serde::{Deserialize, Serialize};

use crate::{Addr, DraftKind, MessageId, Millis, Thread};

/// The fields of a draft that the untouched rule compares, stored as a
/// draft's defaults when its composer starts.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DraftContent {
    /// The From mailbox.
    pub from: Option<Addr>,
    /// The `To` mailboxes.
    pub to: Vec<Addr>,
    /// The `Cc` mailboxes.
    pub cc: Vec<Addr>,
    /// The `Bcc` mailboxes.
    pub bcc: Vec<Addr>,
    /// The subject.
    pub subject: String,
    /// The editor's HTML.
    pub body_html: String,
    /// The attachments' ids within the draft, in order.
    pub attachments: Vec<String>,
}

impl DraftContent {
    /// Whether a draft that started as `self` is untouched at `now`: the
    /// same From, recipients, subject, and attachments, and a body that is
    /// the same or blank. `body_is_blank` is whether `now`'s body has no
    /// text and no image, which only HTML parsing can tell.
    pub fn is_untouched_by<'a>(
        &self,
        now: DraftContentRef<'a, impl IntoIterator<Item = &'a str>>,
        body_is_blank: bool,
    ) -> bool {
        self.from.as_ref() == now.from
            && self.to == now.to
            && self.cc == now.cc
            && self.bcc == now.bcc
            && self.subject == now.subject
            && (body_is_blank || self.body_html == now.body_html)
            && self
                .attachments
                .iter()
                .map(String::as_str)
                .eq(now.attachments)
    }
}

/// A draft's [`DraftContent`] as it is now, borrowed from the draft so the
/// untouched rule compares without copying the body. `A` gives the
/// attachments' ids within the draft, in order.
#[derive(Debug, Clone)]
pub struct DraftContentRef<'a, A> {
    /// The From mailbox.
    pub from: Option<&'a Addr>,
    /// The `To` mailboxes.
    pub to: &'a [Addr],
    /// The `Cc` mailboxes.
    pub cc: &'a [Addr],
    /// The `Bcc` mailboxes.
    pub bcc: &'a [Addr],
    /// The subject.
    pub subject: &'a str,
    /// The editor's HTML.
    pub body_html: &'a str,
    /// The attachments' ids within the draft, in order.
    pub attachments: A,
}

impl<'a, A: IntoIterator<Item = &'a str>> DraftContentRef<'a, A> {
    /// This content, owned, with `body_html` for its body.
    pub fn with_body(self, body_html: String) -> DraftContent {
        DraftContent {
            from: self.from.cloned(),
            to: self.to.to_vec(),
            cc: self.cc.to_vec(),
            bcc: self.bcc.to_vec(),
            subject: self.subject.to_owned(),
            body_html,
            attachments: self.attachments.into_iter().map(str::to_owned).collect(),
        }
    }
}

/// The kind a draft gets when it first appears from Gmail, kept from then
/// on: new when it is alone in its thread with no `In-Reply-To`, forward when
/// its subject starts with `Fwd:` (any case), reply otherwise.
pub fn gmail_draft_kind(alone_in_thread: bool, has_in_reply_to: bool, subject: &str) -> DraftKind {
    let is_forward = subject
        .trim_start()
        .get(..4)
        .is_some_and(|p| p.eq_ignore_ascii_case("fwd:"));
    if alone_in_thread && !has_in_reply_to {
        DraftKind::New
    } else if is_forward {
        DraftKind::Forward
    } else {
        DraftKind::Reply
    }
}

/// Where a draft read from Gmail is aimed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DraftAim {
    /// The message it answers.
    pub target: Option<MessageId>,
    /// Counts as started on the thread's latest message, so `r` on the latest
    /// message resumes it.
    pub started_on_latest: bool,
}

/// A Gmail draft's target (SPEC Reply composer): the message its
/// `In-Reply-To` names (`by_in_reply_to`, resolved by the caller), else the
/// thread's latest message. It counts as started on the latest message when
/// its target is the newest non-draft message no newer than the draft.
pub fn gmail_draft_aim(
    thread: &Thread,
    by_in_reply_to: Option<MessageId>,
    draft_date: Millis,
) -> DraftAim {
    let target = by_in_reply_to.or_else(|| {
        thread
            .latest_live()
            .or_else(|| thread.non_drafts().next_back())
            .map(|m| m.id)
    });
    let newest_older = thread
        .non_drafts()
        .rev()
        .find(|m| m.date <= draft_date)
        .map(|m| m.id);
    DraftAim {
        target,
        started_on_latest: target.is_some() && target == newest_older,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `content`, borrowed as a draft lends it.
    fn now(content: &DraftContent) -> DraftContentRef<'_, impl Iterator<Item = &str>> {
        DraftContentRef {
            from: content.from.as_ref(),
            to: &content.to,
            cc: &content.cc,
            bcc: &content.bcc,
            subject: &content.subject,
            body_html: &content.body_html,
            attachments: content.attachments.iter().map(String::as_str),
        }
    }

    #[test]
    fn untouched_compares_every_field_but_a_blank_body() {
        let start = DraftContent {
            from: Some(Addr::new("me@x.io")),
            to: vec![Addr::new("a@a.io")],
            subject: "Re: hi".into(),
            attachments: vec!["src-1".into()],
            ..DraftContent::default()
        };
        assert!(start.is_untouched_by(now(&start), false));
        let typed = DraftContent {
            body_html: "<p>hello</p>".into(),
            ..start.clone()
        };
        assert!(!start.is_untouched_by(now(&typed), false));
        let cleared = DraftContent {
            body_html: "<p></p>".into(),
            ..start.clone()
        };
        assert!(start.is_untouched_by(now(&cleared), true));
        let edits = [
            DraftContent {
                from: None,
                ..start.clone()
            },
            DraftContent {
                to: vec![],
                ..start.clone()
            },
            DraftContent {
                cc: vec![Addr::new("c@c.io")],
                ..start.clone()
            },
            DraftContent {
                bcc: vec![Addr::new("b@b.io")],
                ..start.clone()
            },
            DraftContent {
                subject: "Re: hi!".into(),
                ..start.clone()
            },
            DraftContent {
                attachments: vec![],
                ..start.clone()
            },
        ];
        for edit in edits {
            assert!(!start.is_untouched_by(now(&edit), true), "{edit:?}");
        }
    }

    #[test]
    fn gmail_draft_kinds() {
        assert_eq!(gmail_draft_kind(true, false, "Hello"), DraftKind::New);
        assert_eq!(gmail_draft_kind(true, false, "Fwd: x"), DraftKind::New);
        assert_eq!(gmail_draft_kind(true, true, "Hello"), DraftKind::Reply);
        assert_eq!(
            gmail_draft_kind(false, false, "  FWD: x"),
            DraftKind::Forward
        );
        assert_eq!(gmail_draft_kind(false, true, "Re: x"), DraftKind::Reply);
        assert_eq!(gmail_draft_kind(false, true, "Fw: x"), DraftKind::Reply);
    }
}
