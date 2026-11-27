use std::collections::HashSet;

use super::{
    Addr, GmailMessageId, Identity, LabelId, LabelSet, MessageId, Millis, SystemLabel, View,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageFacts {
    pub id: MessageId,
    pub gmail_id: GmailMessageId,
    pub date: Millis,
    pub from: Addr,
    pub to: Vec<Addr>,
    pub cc: Vec<Addr>,
    pub bcc: Vec<Addr>,
    pub reply_to: Vec<Addr>,
    pub subject: String,
    pub snippet: String,
    pub message_id_hdr: Option<String>,
    pub in_reply_to: Option<String>,
    pub references: Vec<String>,
    pub is_list: bool,
    pub is_draft: bool,
    pub has_attachments: bool,
    pub labels: LabelSet,
}

impl MessageFacts {
    #[must_use]
    pub fn has(&self, label: SystemLabel) -> bool {
        self.labels.has(label)
    }

    #[must_use]
    pub fn is_live(&self) -> bool {
        !self.is_draft && !self.has(SystemLabel::Trash) && !self.has(SystemLabel::Spam)
    }

    #[must_use]
    pub fn sender_display(&self) -> &str {
        self.from.display_name()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Thread(Vec<MessageFacts>);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadDigest<'a> {
    pub latest: &'a MessageFacts,
    pub subject: &'a str,
    pub snippet: &'a str,
    pub sender: &'a str,
    pub sender_email: &'a str,
    pub sort_time: Millis,
    pub message_count: u32,
    pub unread: bool,
    pub has_attachments: bool,
    pub has_live: bool,
    pub labels: LabelSet,
}

impl ThreadDigest<'_> {
    #[must_use]
    pub fn in_inbox(&self) -> bool {
        self.has_live && self.labels.has(SystemLabel::Inbox)
    }

    #[must_use]
    pub fn in_trash(&self) -> bool {
        self.labels.has(SystemLabel::Trash)
    }

    #[must_use]
    pub fn in_spam(&self) -> bool {
        self.labels.has(SystemLabel::Spam)
    }
}

impl Thread {
    #[must_use]
    pub fn new(mut messages: Vec<MessageFacts>) -> Self {
        messages.sort_by_key(|m| (m.date, m.id));
        Self(messages)
    }

    #[must_use]
    pub fn messages(&self) -> &[MessageFacts] {
        &self.0
    }

    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    #[must_use]
    pub fn latest_live(&self) -> Option<&MessageFacts> {
        self.0.iter().rev().find(|m| m.is_live())
    }

    #[must_use]
    pub fn latest_non_draft(&self) -> Option<&MessageFacts> {
        self.0.iter().rev().find(|m| !m.is_draft)
    }

    #[must_use]
    pub fn latest(&self) -> Option<&MessageFacts> {
        self.latest_live()
            .or_else(|| self.latest_non_draft())
            .or_else(|| self.0.last())
    }

    #[must_use]
    pub fn latest_from_someone_else(&self, identity: &Identity) -> Option<&MessageFacts> {
        self.0
            .iter()
            .rev()
            .find(|m| m.is_live() && !identity.is_mine(&m.from.email))
            .or_else(|| self.latest_live())
    }

    #[must_use]
    pub fn labels(&self) -> LabelSet {
        let mut out = LabelSet::new();
        for msg in &self.0 {
            if msg.is_live() {
                for label in &msg.labels {
                    if label.0 != SystemLabel::Draft.id() {
                        out.insert(label.clone());
                    }
                }
            } else if !msg.is_draft {
                if msg.has(SystemLabel::Trash) {
                    out.insert(SystemLabel::Trash.label_id());
                }
                if msg.has(SystemLabel::Spam) {
                    out.insert(SystemLabel::Spam.label_id());
                }
            }
        }
        out
    }

    #[must_use]
    pub fn digest(&self) -> Option<ThreadDigest<'_>> {
        let latest = self.latest()?;
        let live: Vec<&MessageFacts> = self.0.iter().filter(|m| m.is_live()).collect();
        let has_live = !live.is_empty();
        let non_drafts: Vec<&MessageFacts> = self.0.iter().filter(|m| !m.is_draft).collect();
        let counted = if has_live { &live } else { &non_drafts };
        let message_count = u32::try_from(counted.len()).unwrap_or(u32::MAX);
        let unread = counted.iter().any(|m| m.has(SystemLabel::Unread));
        let has_attachments = counted.iter().any(|m| m.has_attachments);
        let labels = self.labels();
        let subject = counted
            .iter()
            .find(|m| !m.subject.is_empty())
            .map_or(latest.subject.as_str(), |m| m.subject.as_str());
        Some(ThreadDigest {
            latest,
            subject,
            snippet: &latest.snippet,
            sender: latest.sender_display(),
            sender_email: &latest.from.email,
            sort_time: latest.date,
            message_count,
            unread,
            has_attachments,
            has_live,
            labels,
        })
    }

    fn has_inbox(&self) -> bool {
        self.0
            .iter()
            .any(|m| m.is_live() && m.has(SystemLabel::Inbox))
    }

    #[must_use]
    pub fn gained_inbox(before: Option<&Self>, after: &Self, identity: &Identity) -> bool {
        let had_inbox = before.is_some_and(Self::has_inbox);
        let now_inbox = after.has_inbox();
        if !now_inbox {
            return false;
        }
        if !had_inbox {
            let new_external = after.0.iter().any(|m| {
                m.is_live()
                    && m.has(SystemLabel::Inbox)
                    && !identity.is_mine(&m.from.email)
                    && before.is_none_or(|b| {
                        !b.0.iter()
                            .any(|old| old.gmail_id == m.gmail_id && old.has(SystemLabel::Inbox))
                    })
            });
            let existing_gained_inbox = before.is_some_and(|b| {
                after.0.iter().any(|m| {
                    m.is_live()
                        && m.has(SystemLabel::Inbox)
                        && b.0
                            .iter()
                            .any(|old| old.gmail_id == m.gmail_id && !old.has(SystemLabel::Inbox))
                })
            });
            return new_external || existing_gained_inbox;
        }
        after.0.iter().any(|m| {
            m.is_live()
                && m.has(SystemLabel::Inbox)
                && !identity.is_mine(&m.from.email)
                && before.is_none_or(|b| !b.0.iter().any(|old| old.gmail_id == m.gmail_id))
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GmailListing {
    Query(&'static str),
    Label {
        label: LabelId,
        include_spam_trash: bool,
    },
}

impl View {
    #[must_use]
    pub fn lists(&self, digest: &ThreadDigest<'_>, snoozed: bool) -> bool {
        match self {
            Self::Inbox => digest.in_inbox(),
            Self::Done => digest.has_live && !digest.in_inbox() && !snoozed,
            Self::AllMail => digest.has_live,
            Self::Snoozed => snoozed && digest.has_live && !digest.in_inbox(),
            Self::Starred => digest.has_live && digest.labels.has(SystemLabel::Starred),
            Self::Sent => digest.has_live && digest.labels.has(SystemLabel::Sent),
            Self::Drafts => false,
            Self::Trash => digest.in_trash(),
            Self::Spam => digest.in_spam(),
            Self::Label { label } => digest.has_live && digest.labels.contains(label.as_str()),
        }
    }

    #[must_use]
    pub const fn has_gmail_listing(&self) -> bool {
        matches!(
            self,
            Self::Done | Self::AllMail | Self::Trash | Self::Spam | Self::Label { .. }
        )
    }

    #[must_use]
    pub fn into_gmail_listing(self) -> Option<GmailListing> {
        match self {
            Self::Done => Some(GmailListing::Query("-in:inbox -in:draft")),
            Self::AllMail => Some(GmailListing::Query("-in:draft")),
            Self::Trash => Some(GmailListing::Label {
                label: SystemLabel::Trash.label_id(),
                include_spam_trash: true,
            }),
            Self::Spam => Some(GmailListing::Label {
                label: SystemLabel::Spam.label_id(),
                include_spam_trash: true,
            }),
            Self::Label { label } => Some(GmailListing::Label {
                label,
                include_spam_trash: false,
            }),
            Self::Inbox | Self::Snoozed | Self::Starred | Self::Sent | Self::Drafts => None,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ReplyTarget<'a> {
    pub from: &'a Addr,
    pub to: &'a [Addr],
    pub cc: &'a [Addr],
    pub reply_to: &'a [Addr],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Addressing {
    pub from: Addr,
    pub to: Vec<Addr>,
    pub cc: Vec<Addr>,
}

#[must_use]
pub fn addressing(target: ReplyTarget<'_>, identity: &Identity, reply_all: bool) -> Addressing {
    let from = pick_from(target, identity);
    let from_is_mine = identity.is_mine(&target.from.email);
    let mut seen: HashSet<String> = HashSet::new();
    let mut to = Vec::new();
    let mut cc = Vec::new();

    if !target.reply_to.is_empty() {
        push_unique(&mut to, target.reply_to, &mut seen, None);
    } else if from_is_mine {
        push_unique(&mut to, target.to, &mut seen, None);
        if to.is_empty() {
            push_unique(&mut to, std::slice::from_ref(target.from), &mut seen, None);
        }
    } else {
        push_unique(&mut to, std::slice::from_ref(target.from), &mut seen, None);
    }

    if reply_all {
        let exclude_mine = Some(identity);
        if !from_is_mine || !target.reply_to.is_empty() {
            push_unique(&mut cc, target.to, &mut seen, exclude_mine);
        }
        push_unique(&mut cc, target.cc, &mut seen, exclude_mine);
    }

    Addressing { from, to, cc }
}

fn pick_from(target: ReplyTarget<'_>, identity: &Identity) -> Addr {
    for addr in target
        .to
        .iter()
        .chain(target.cc)
        .chain(std::iter::once(target.from))
    {
        if let Some(found) = identity.send_as_for(&addr.email) {
            return found;
        }
    }
    identity.default_from()
}

fn push_unique(
    out: &mut Vec<Addr>,
    addrs: &[Addr],
    seen: &mut HashSet<String>,
    skip_mine: Option<&Identity>,
) {
    for addr in addrs {
        if skip_mine.is_some_and(|id| id.is_mine(&addr.email)) {
            continue;
        }
        let key = addr.key();
        if !key.is_empty() && seen.insert(key) {
            out.push(addr.clone());
        }
    }
}

#[must_use]
pub fn is_forward_subject(subject: &str) -> bool {
    let s = subject.trim_start();
    s.get(..4).is_some_and(|p| p.eq_ignore_ascii_case("fwd:"))
        || s.get(..3).is_some_and(|p| p.eq_ignore_ascii_case("fw:"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{
        LabelDelta, ModifyError, ModifyKind, SendAsAddress, SnoozeEdit, ThreadId, ThreadModify,
        plan_modify,
    };

    fn msg(id: i64, date: i64, from: &str, labels: &[LabelId]) -> MessageFacts {
        MessageFacts {
            id: MessageId(id),
            gmail_id: GmailMessageId(format!("gm{id}")),
            date: Millis(date),
            from: Addr::new(from),
            to: vec![Addr::new("me@acme.test")],
            cc: Vec::new(),
            bcc: Vec::new(),
            reply_to: Vec::new(),
            subject: format!("Subj {id}"),
            snippet: format!("Snip {id}"),
            message_id_hdr: Some(format!("<m{id}@acme.test>")),
            in_reply_to: None,
            references: Vec::new(),
            is_list: false,
            is_draft: false,
            has_attachments: false,
            labels: labels.iter().cloned().collect(),
        }
    }

    #[test]
    fn thread_latest_digest_and_views_follow_message_labels() {
        let mut draft = msg(3, 300, "me@acme.test", &[SystemLabel::Draft.label_id()]);
        draft.is_draft = true;
        let live = msg(
            1,
            100,
            "alice@x.test",
            &[
                SystemLabel::Inbox.label_id(),
                SystemLabel::Unread.label_id(),
                LabelId("L_WORK".to_owned()),
            ],
        );
        let trashed = msg(2, 200, "bob@x.test", &[SystemLabel::Trash.label_id()]);
        let thread = Thread::new(vec![draft, trashed, live]);

        let digest = thread.digest().expect("thread has a digest");
        assert_eq!(digest.latest.id, MessageId(1), "live beats trashed/draft");
        assert_eq!(digest.message_count, 1, "counts only live messages");
        assert!(digest.unread, "live unread propagates");
        assert!(View::Inbox.lists(&digest, false), "listed in Inbox");
        assert!(!View::Done.lists(&digest, false), "Inbox thread not Done");
        assert!(
            View::Trash.lists(&digest, false),
            "trashed msg puts in Trash"
        );
        assert!(
            View::AllMail.lists(&digest, false),
            "live thread in AllMail"
        );
        assert!(
            View::Label {
                label: LabelId("L_WORK".to_owned())
            }
            .lists(&digest, false),
            "custom label matches"
        );
        assert!(
            !View::Drafts.lists(&digest, false),
            "Drafts view lists drafts only"
        );
    }

    #[test]
    fn latest_from_someone_else_and_reply_addressing_respect_identity() {
        let identity = Identity::new(
            "me@acme.test",
            "Me",
            vec![SendAsAddress {
                email: "alias@acme.test".to_owned(),
                display_name: "Alias".to_owned(),
                primary: false,
            }],
        );
        let mut from_bob = msg(1, 100, "bob@x.test", &[SystemLabel::Inbox.label_id()]);
        from_bob.to = vec![Addr::new("alias@acme.test"), Addr::new("carol@x.test")];
        from_bob.cc = vec![Addr::new("dave@x.test"), Addr::new("me@acme.test")];
        let from_me = msg(2, 200, "me@acme.test", &[SystemLabel::Sent.label_id()]);
        let thread = Thread::new(vec![from_bob.clone(), from_me]);

        assert_eq!(
            thread.latest_from_someone_else(&identity).map(|m| m.id),
            Some(MessageId(1)),
            "skips own newer reply"
        );

        let reply_all = addressing(
            ReplyTarget {
                from: &from_bob.from,
                to: &from_bob.to,
                cc: &from_bob.cc,
                reply_to: &from_bob.reply_to,
            },
            &identity,
            true,
        );
        assert_eq!(
            reply_all.from.email, "alias@acme.test",
            "picks matching alias"
        );
        assert_eq!(
            reply_all.to,
            vec![Addr::new("bob@x.test")],
            "replies to sender"
        );
        assert_eq!(
            reply_all.cc,
            vec![Addr::new("carol@x.test"), Addr::new("dave@x.test")],
            "reply-all excludes own addresses"
        );
    }

    #[test]
    fn plan_modify_and_revert_handle_unread_isolation_and_undo() {
        assert_eq!(
            LabelDelta::new(
                vec![SystemLabel::Unread.label_id()],
                vec![SystemLabel::Inbox.label_id()],
            ),
            Err(ModifyError::UnreadMixed),
            "UNREAD cannot mix with other label edits"
        );

        let m1 = msg(1, 100, "a@x.test", &[SystemLabel::Inbox.label_id()]);
        let m2 = msg(2, 200, "b@x.test", &[SystemLabel::Inbox.label_id()]);
        let thread = Thread::new(vec![m1.clone(), m2]);

        let unread = LabelDelta::new(vec![SystemLabel::Unread.label_id()], Vec::new()).unwrap();
        let unread_change = plan_modify(
            ThreadModify {
                thread_id: ThreadId(7),
                thread: &thread,
                kind: ModifyKind::Unread,
                snooze_before: None,
                snooze_edit: None,
            },
            &unread,
        );
        assert_eq!(
            unread_change.messages.len(),
            1,
            "mark unread touches latest only"
        );
        assert_eq!(
            unread_change.messages[0].message,
            MessageId(2),
            "targets latest live"
        );

        let done = LabelDelta::new(Vec::new(), vec![SystemLabel::Inbox.label_id()]).unwrap();
        let mut done_change = plan_modify(
            ThreadModify {
                thread_id: ThreadId(7),
                thread: &thread,
                kind: ModifyKind::Done,
                snooze_before: Some(Millis(500)),
                snooze_edit: Some(SnoozeEdit::Clear),
            },
            &done,
        );
        assert_eq!(
            done_change.messages.len(),
            2,
            "archive touches every message"
        );
        assert!(done_change.restores_snooze(), "undo knows snooze changed");
        let mc = done_change.messages.remove(0);
        let after = mc.apply(&m1.labels);
        assert!(!after.has(SystemLabel::Inbox), "INBOX removed");
        let reverted = mc.reverted(&after).expect("revert restores removed label");
        assert!(
            reverted.apply(&after).has(SystemLabel::Inbox),
            "INBOX restored on undo"
        );
    }
}
