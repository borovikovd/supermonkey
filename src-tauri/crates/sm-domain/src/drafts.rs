use serde::{Deserialize, Serialize};

use crate::{Addr, DraftKind, MessageId, Millis, Thread};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DraftContent {
    pub from: Option<Addr>,
    pub to: Vec<Addr>,
    pub cc: Vec<Addr>,
    pub bcc: Vec<Addr>,
    pub subject: String,
    pub body_html: String,
    pub attachments: Vec<String>,
}

impl DraftContent {
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

#[derive(Debug, Clone)]
pub struct DraftContentRef<'a, A> {
    pub from: Option<&'a Addr>,
    pub to: &'a [Addr],
    pub cc: &'a [Addr],
    pub bcc: &'a [Addr],
    pub subject: &'a str,
    pub body_html: &'a str,
    pub attachments: A,
}

impl<'a, A: IntoIterator<Item = &'a str>> DraftContentRef<'a, A> {
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

#[must_use]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DraftAim {
    pub target: Option<MessageId>,
    pub started_on_latest: bool,
}

#[must_use]
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
