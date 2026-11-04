//! Unit tests for the mail model rules that span modules: threads and views,
//! label changes, split routing (with the cases ported from the old
//! `ub-rules` splits tests), and Gmail drafts.

use crate::query::MatchTarget;
use crate::query::Query;
use crate::test_support::{message, set};
use crate::*;

fn alice() -> Addr {
    Addr::named("Alice", "alice@a.io")
}

fn me() -> Identity {
    Identity::new("me@x.io".into(), "Me".into(), vec![])
}

fn draft(id: i64) -> MessageFacts {
    MessageFacts {
        is_draft: true,
        ..message(id, Addr::new("me@x.io"), &["DRAFT"])
    }
}

// --- threads and views ---------------------------------------------------

#[test]
fn latest_is_newest_live_then_newest_non_draft_then_draft() {
    let t = Thread::new(vec![
        message(2, alice(), &["INBOX"]),
        message(1, alice(), &["INBOX"]),
        message(3, alice(), &["TRASH"]),
        draft(4),
    ]);
    assert_eq!(t.messages()[0].id, MessageId(1), "sorted oldest first");
    assert_eq!(t.latest().map(|m| m.id), Some(MessageId(2)));
    let trashed = Thread::new(vec![
        message(1, alice(), &["TRASH"]),
        message(2, alice(), &["SPAM"]),
        draft(3),
    ]);
    assert_eq!(trashed.latest().map(|m| m.id), Some(MessageId(2)));
    let only_draft = Thread::new(vec![draft(5)]);
    assert_eq!(only_draft.latest().map(|m| m.id), Some(MessageId(5)));
    assert_eq!(Thread::new(vec![]).latest(), None);
    assert_eq!(Thread::new(vec![]).digest(), None);
}

#[test]
fn rules_read_the_latest_message_from_someone_else() {
    let mine = Addr::new("ME@x.io");
    let t = Thread::new(vec![
        message(1, alice(), &["INBOX"]),
        message(2, mine.clone(), &["SENT"]),
        message(3, Addr::new("bob@b.io"), &["TRASH"]),
    ]);
    assert_eq!(
        t.latest_from_someone_else(&me()).map(|m| m.id),
        Some(MessageId(1))
    );
    let all_mine = Thread::new(vec![
        message(1, mine.clone(), &["SENT"]),
        message(2, mine, &["SENT", "INBOX"]),
    ]);
    assert_eq!(
        all_mine.latest_from_someone_else(&me()).map(|m| m.id),
        Some(MessageId(2))
    );
}

#[test]
fn thread_labels_come_from_live_messages_plus_trash_and_spam() {
    let t = Thread::new(vec![
        message(1, alice(), &["INBOX", "Label_1"]),
        message(2, alice(), &["TRASH", "STARRED"]),
        message(3, alice(), &["SPAM"]),
        MessageFacts {
            labels: set(&["DRAFT", "Label_9"]),
            ..draft(4)
        },
    ]);
    assert_eq!(t.labels(), set(&["INBOX", "Label_1", "SPAM", "TRASH"]));
}

#[test]
fn digest_counts_and_flags() {
    let t = Thread::new(vec![
        MessageFacts {
            subject: "Hello".into(),
            has_attachments: true,
            ..message(1, alice(), &["INBOX"])
        },
        MessageFacts {
            snippet: "latest".into(),
            ..message(2, Addr::new("bob@b.io"), &["INBOX", "UNREAD"])
        },
        message(3, alice(), &["TRASH", "UNREAD"]),
        MessageFacts {
            has_attachments: true,
            ..draft(4)
        },
    ]);
    let d = t.digest().unwrap();
    assert_eq!(d.subject, "Hello");
    assert_eq!(d.snippet, "latest");
    assert_eq!(d.sender, "bob@b.io", "no name: the address");
    assert_eq!(d.sort_time, Millis(2000));
    assert_eq!(d.message_count, 2, "live messages only");
    assert!(d.unread);
    assert!(d.has_attachments);
    assert!(d.in_inbox() && d.in_trash() && !d.in_spam() && d.has_live);

    // With nothing live, the non-drafts count and decide unread.
    let gone = Thread::new(vec![
        message(1, alice(), &["TRASH", "UNREAD"]),
        message(2, alice(), &["TRASH"]),
        draft(3),
    ]);
    let d = gone.digest().unwrap();
    assert_eq!((d.message_count, d.unread, d.has_live), (2, true, false));
    assert_eq!(d.sender, "Alice");
    // A thread of one draft still counts one.
    assert_eq!(
        Thread::new(vec![draft(1)]).digest().unwrap().message_count,
        1
    );
    // A live read message hides an unread trashed one.
    let read = Thread::new(vec![
        message(1, alice(), &["INBOX"]),
        message(2, alice(), &["TRASH", "UNREAD"]),
    ]);
    assert!(!read.digest().unwrap().unread);
}

#[test]
fn view_membership() {
    let digest = |labels: &[&str]| {
        Thread::new(vec![message(1, alice(), labels)])
            .digest()
            .unwrap()
    };
    let inbox = digest(&["INBOX", "STARRED", "Label_1"]);
    let archived = digest(&["SENT"]);
    let trashed = digest(&["TRASH", "INBOX"]);
    let label = View::Label {
        label: LabelId::from("Label_1"),
    };
    let cases: [(&View, &ThreadDigest, bool, bool); 18] = [
        (&View::Inbox, &inbox, false, true),
        (&View::Inbox, &inbox, true, false),
        (&View::Done, &inbox, false, false),
        (&View::Done, &archived, false, true),
        (&View::Done, &archived, true, false),
        (&View::Done, &trashed, false, false),
        (&View::AllMail, &archived, true, true),
        (&View::AllMail, &trashed, false, false),
        (&View::Snoozed, &archived, true, true),
        (&View::Snoozed, &inbox, false, false),
        (&View::Starred, &inbox, false, true),
        (&View::Sent, &archived, false, true),
        (&View::Sent, &inbox, false, false),
        (&label, &inbox, true, true),
        (&View::Trash, &trashed, false, true),
        (&View::Trash, &inbox, false, false),
        (&View::Spam, &trashed, false, false),
        (&View::Drafts, &inbox, false, false),
    ];
    for (view, thread, snoozed, want) in cases {
        assert_eq!(
            view.lists(thread, snoozed),
            want,
            "{view:?} snoozed={snoozed} {:?}",
            thread.labels
        );
    }
    // A trashed message leaves the thread in the views its other messages put it in.
    let mixed = Thread::new(vec![
        message(1, alice(), &["INBOX"]),
        message(2, alice(), &["TRASH"]),
    ])
    .digest()
    .unwrap();
    assert!(View::Inbox.lists(&mixed, false) && View::Trash.lists(&mixed, false));
}

#[test]
fn gmail_listings() {
    assert_eq!(
        View::Done.gmail_listing(),
        Some(GmailListing::Query("-in:inbox -in:draft".into()))
    );
    assert_eq!(
        View::AllMail.gmail_listing(),
        Some(GmailListing::Query("-in:draft".into()))
    );
    assert_eq!(
        View::Spam.gmail_listing(),
        Some(GmailListing::Label {
            label: LabelId::from("SPAM"),
            include_spam_trash: true
        })
    );
    assert_eq!(
        View::Label {
            label: LabelId::from("Label_3")
        }
        .gmail_listing(),
        Some(GmailListing::Label {
            label: LabelId::from("Label_3"),
            include_spam_trash: false
        })
    );
    assert_eq!(View::Inbox.gmail_listing(), None);
    assert_eq!(
        GmailListing::search(" is:list from:a"),
        Some(GmailListing::Query("from:a".into()))
    );
    assert_eq!(GmailListing::search("  "), None);
}

#[test]
fn snooze_rule_ignores_trash_and_pending_ops() {
    let t = Thread::new(vec![
        message(1, alice(), &["TRASH", "INBOX"]),
        message(2, alice(), &["INBOX"]),
    ]);
    assert!(t.gained_inbox(|_| false));
    assert!(
        !t.gained_inbox(|m| m.id == MessageId(2)),
        "message 2's INBOX is our own pending change"
    );
    let archived = Thread::new(vec![message(1, alice(), &["SENT"])]);
    assert!(!archived.gained_inbox(|_| false));
}

// --- label changes -------------------------------------------------------

fn delta(kind: ModifyKind) -> LabelDelta {
    LabelDelta::preset(kind).unwrap()
}

fn change(id: i64, added: &[&str], removed: &[&str]) -> MessageChange {
    MessageChange {
        message: MessageId(id),
        added: set(added),
        removed: set(removed),
    }
}

#[test]
fn done_removes_inbox_and_unread_from_every_non_draft() {
    let t = Thread::new(vec![
        message(1, alice(), &["INBOX", "UNREAD"]),
        message(2, alice(), &["INBOX"]),
        message(3, alice(), &["SENT"]),
        draft(4),
    ]);
    let plan = plan_modify(&t, &delta(ModifyKind::Done));
    let ops: Vec<i64> = plan.op_messages.iter().map(|m| m.id.0).collect();
    assert_eq!(ops, [1, 2, 3], "every non-draft, changed or not");
    assert_eq!(
        plan.changes,
        [
            change(1, &[], &["INBOX", "UNREAD"]),
            change(2, &[], &["INBOX"])
        ]
    );
}

#[test]
fn mark_unread_touches_the_latest_live_message_only() {
    let t = Thread::new(vec![
        message(1, alice(), &["INBOX"]),
        message(2, alice(), &["INBOX"]),
        message(3, alice(), &["TRASH"]),
        draft(4),
    ]);
    let plan = plan_modify(&t, &delta(ModifyKind::Unread));
    let ops: Vec<i64> = plan.op_messages.iter().map(|m| m.id.0).collect();
    assert_eq!(ops, [2]);
    assert_eq!(plan.changes, [change(2, &["UNREAD"], &[])]);
    // With nothing live, every non-draft message.
    let gone = Thread::new(vec![
        message(1, alice(), &["TRASH"]),
        message(2, alice(), &["TRASH", "UNREAD"]),
    ]);
    let plan = plan_modify(&gone, &delta(ModifyKind::Unread));
    assert_eq!(plan.op_messages.len(), 2);
    assert_eq!(plan.changes, [change(1, &["UNREAD"], &[])]);
}

#[test]
fn deltas_are_checked_and_presets_match_the_spec() {
    assert_eq!(
        LabelDelta::new(set(&["UNREAD", "STARRED"]), set(&[])),
        Err(ModifyError::UnreadWithOtherLabels)
    );
    assert_eq!(
        LabelDelta::new(set(&["UNREAD"]), set(&["INBOX"])),
        Err(ModifyError::UnreadWithOtherLabels)
    );
    let both = LabelDelta::new(set(&["Label_1"]), set(&["Label_1", "INBOX"])).unwrap();
    assert_eq!(
        both.remove(),
        &set(&["INBOX"]),
        "a label in both is only added"
    );
    let trash = delta(ModifyKind::Trash);
    assert_eq!(
        (trash.add(), trash.remove()),
        (&set(&["TRASH"]), &set(&["INBOX", "SPAM"]))
    );
    let spam = delta(ModifyKind::Spam);
    assert_eq!(
        (spam.add(), spam.remove()),
        (&set(&["SPAM"]), &set(&["INBOX", "TRASH"]))
    );
    assert_eq!(delta(ModifyKind::Snooze).remove(), &set(&["INBOX"]));
    assert_eq!(delta(ModifyKind::ReturnToInbox).add(), &set(&["INBOX"]));
    assert!(LabelDelta::preset(ModifyKind::Label).is_none());
}

#[test]
fn revert_undoes_only_what_is_still_there() {
    let done = change(1, &["STARRED"], &["INBOX", "UNREAD"]);
    let now = done.apply(&set(&["INBOX", "UNREAD", "Label_1"]));
    assert_eq!(now, set(&["Label_1", "STARRED"]));
    let back = done.reverted(&now).unwrap();
    assert_eq!(back, change(1, &["INBOX", "UNREAD"], &["STARRED"]));
    assert_eq!(back.apply(&now), set(&["INBOX", "UNREAD", "Label_1"]));
    // INBOX put back and STARRED removed elsewhere since: only UNREAD is left to revert.
    let partly = done.reverted(&set(&["INBOX", "Label_1"])).unwrap();
    assert_eq!(partly, change(1, &["UNREAD"], &[]));
    assert_eq!(done.reverted(&set(&["INBOX", "UNREAD"])), None);
}

#[test]
fn snooze_after_and_its_restore() {
    let at = Some(Millis(5));
    assert_eq!(snooze_after(None, ModifyKind::Done, at), None);
    assert_eq!(snooze_after(None, ModifyKind::Trash, at), None);
    assert_eq!(snooze_after(None, ModifyKind::Spam, at), None);
    assert_eq!(snooze_after(None, ModifyKind::Star, at), at);
    assert_eq!(
        snooze_after(Some(SnoozeEdit::Set(Millis(9))), ModifyKind::Snooze, at),
        Some(Millis(9))
    );
    assert_eq!(
        snooze_after(Some(SnoozeEdit::Clear), ModifyKind::ReturnToInbox, at),
        None
    );
    let tc = ThreadChange {
        thread: ThreadId(1),
        changes: vec![],
        snooze_before: at,
        snooze_after: None,
    };
    assert!(tc.restores_snooze(None));
    assert!(!tc.restores_snooze(Some(Millis(7))), "snoozed again since");
    let unchanged = ThreadChange {
        snooze_after: at,
        ..tc
    };
    assert!(!unchanged.restores_snooze(at));
}

// --- split routing ---------------------------------------------------------

#[test]
fn person_to_person() {
    let p2p = |labels: &[&str], is_list| is_person_to_person(&set(labels), is_list);
    assert!(p2p(&[], false));
    assert!(p2p(&["INBOX", "UNREAD"], false));
    assert!(p2p(&["INBOX", "CATEGORY_PERSONAL"], false));
    assert!(p2p(&["category_personal"], false));
    assert!(!p2p(&["INBOX"], true));
    assert!(!p2p(&["CATEGORY_PERSONAL"], true));
    for c in [
        "CATEGORY_PROMOTIONS",
        "CATEGORY_SOCIAL",
        "CATEGORY_UPDATES",
        "CATEGORY_FORUMS",
    ] {
        assert!(!p2p(&["INBOX", c], false), "{c}");
        assert!(!p2p(&["CATEGORY_PERSONAL", c], false), "{c}");
    }
    // An unknown category without CATEGORY_PERSONAL is not "no category".
    assert!(!p2p(&["CATEGORY_PURCHASES"], false));
    assert!(!p2p(&["category_purchases"], false));
    // User label ids are not categories.
    assert!(p2p(&["Label_12"], false));
}

#[test]
fn starters_parse_and_match() {
    let names: Vec<_> = STARTER_SPLITS.iter().map(|s| s.name).collect();
    assert_eq!(names, ["Newsletters", "Promotions", "Social", "Updates"]);

    let mut m = MessageFacts {
        is_list: true,
        ..message(
            1,
            Addr::named("Shop", "deals@shop.com"),
            &["INBOX", "CATEGORY_PROMOTIONS"],
        )
    };
    let hits = |m: &MessageFacts| -> Vec<&str> {
        STARTER_SPLITS
            .iter()
            .filter(|s| {
                Query::parse_rule(s.query)
                    .unwrap()
                    .matches(&MatchTarget::rule(m, &[]))
            })
            .map(|s| s.name)
            .collect()
    };
    assert_eq!(hits(&m), ["Newsletters", "Promotions"]);
    m.is_list = false;
    m.labels = set(&["CATEGORY_SOCIAL"]);
    assert_eq!(hits(&m), ["Social"]);
    m.labels = set(&["CATEGORY_UPDATES"]);
    assert_eq!(hits(&m), ["Updates"]);
}

const NEWS: SplitId = SplitId(1);
const VIP: SplitId = SplitId(2);
const IMPORTANT: SplitId = SplitId(3);
const OTHER: SplitId = SplitId(4);

fn splits(news_on: bool, vip_on: bool, important_on: bool) -> Vec<Split> {
    let split = |id, kind, query: &str, enabled| Split {
        id,
        kind,
        query: query.into(),
        enabled,
    };
    vec![
        split(NEWS, SplitKind::Custom, "is:list", news_on),
        split(VIP, SplitKind::Custom, "from:boss", vip_on),
        split(IMPORTANT, SplitKind::Important, "", important_on),
        split(OTHER, SplitKind::Other, "", true),
    ]
}

fn pin(kind: PinKind, value: &str, split: SplitId) -> Pin {
    Pin::new(kind, value, split).unwrap()
}

#[test]
fn routing_falls_through_rules_important_and_other() {
    let router = Router::new(splits(true, true, true), []);
    let person = message(1, alice(), &["INBOX"]);
    let list = MessageFacts {
        is_list: true,
        ..person.clone()
    };
    let boss = message(2, Addr::new("boss@corp.io"), &["INBOX", "CATEGORY_UPDATES"]);
    let promo = message(3, alice(), &["INBOX", "CATEGORY_PROMOTIONS"]);
    assert_eq!(router.route(&list, &[]), Some(NEWS), "custom splits first");
    assert_eq!(router.route(&boss, &[]), Some(VIP));
    assert_eq!(router.route(&person, &[]), Some(IMPORTANT));
    assert_eq!(router.route(&promo, &[]), Some(OTHER));
    // Turned off: skipped, falling through.
    let flat = Router::new(splits(false, false, false), []);
    for m in [&list, &boss, &person, &promo] {
        assert_eq!(flat.route(m, &[]), Some(OTHER));
    }
}

#[test]
fn blank_custom_rules_match_nothing() {
    let router = Router::new(
        [
            Split {
                id: NEWS,
                kind: SplitKind::Custom,
                query: "  ".into(),
                enabled: true,
            },
            Split {
                id: OTHER,
                kind: SplitKind::Other,
                query: String::new(),
                enabled: true,
            },
        ],
        [],
    );
    assert_eq!(
        router.route(&message(1, alice(), &["INBOX"]), &[]),
        Some(OTHER)
    );
}

#[test]
fn pins_beat_rules_sender_then_domain_then_label() {
    let labels = vec!["Clients".to_owned(), "Work".to_owned()];
    let m = MessageFacts {
        is_list: true,
        ..message(1, Addr::new("Alice@A.io"), &["INBOX"])
    };
    let all = [
        pin(PinKind::Label, "work", VIP),
        pin(PinKind::Domain, "A.IO", OTHER),
        pin(PinKind::Sender, " alice@a.io ", IMPORTANT),
    ];
    let router = |pins: &[Pin]| Router::new(splits(true, true, true), pins.to_vec());
    assert_eq!(router(&all).route(&m, &labels), Some(IMPORTANT));
    assert_eq!(router(&all[..2]).route(&m, &labels), Some(OTHER));
    assert_eq!(router(&all[..1]).route(&m, &labels), Some(VIP));
    assert_eq!(router(&[]).route(&m, &labels), Some(NEWS));
    // A pin to a split that is off is skipped, falling through to the next rule.
    let off = Router::new(splits(true, true, false), all.to_vec());
    assert_eq!(
        off.route(&m, &labels),
        Some(OTHER),
        "domain pin, as the sender pin's split is off"
    );
}

#[test]
fn several_label_pins_take_the_first_split_in_order() {
    let labels = vec!["Work".to_owned(), "Clients".to_owned()];
    let m = message(1, alice(), &["INBOX"]);
    let router = Router::new(
        splits(true, true, true),
        [
            pin(PinKind::Label, "work", IMPORTANT),
            pin(PinKind::Label, "clients", VIP),
        ],
    );
    assert_eq!(router.route(&m, &labels), Some(VIP));
}

#[test]
fn pins_need_a_value() {
    assert!(Pin::new(PinKind::Sender, "   ", OTHER).is_none());
    assert_eq!(pin(PinKind::Label, " Work ", OTHER).value(), "work");
    assert_eq!(email_domain("a@B.io").as_deref(), Some("b.io"));
    assert_eq!(email_domain("\"a@b\"@c.io").as_deref(), Some("c.io"));
    assert_eq!(email_domain("nobody"), None);
    assert_eq!(email_domain("a@"), None);
}

#[test]
fn place_routes_inbox_threads_by_someone_else_s_message() {
    let router = Router::new(
        splits(true, true, true),
        [pin(PinKind::Sender, "alice@a.io", VIP)],
    );
    let t = Thread::new(vec![
        message(1, alice(), &["INBOX"]),
        message(2, Addr::new("me@x.io"), &["INBOX", "SENT"]),
    ]);
    let digest = t.digest().unwrap();
    assert_eq!(
        router.place(&t, &digest, &me(), &[]),
        Some(VIP),
        "my own reply does not move it"
    );
    let archived = Thread::new(vec![message(1, alice(), &["SENT"])]);
    assert_eq!(
        router.place(&archived, &archived.digest().unwrap(), &me(), &[]),
        None
    );
}

#[test]
fn split_positions() {
    assert_eq!(split_position(SplitKind::Custom, 0), 10);
    assert_eq!(split_position(SplitKind::Custom, 2), 30);
    assert_eq!(split_position(SplitKind::Important, 5), IMPORTANT_POSITION);
    assert_eq!(split_position(SplitKind::Other, 0), OTHER_POSITION);
    assert!(split_position(SplitKind::Custom, u32::MAX) < IMPORTANT_POSITION);
}

// --- Gmail drafts ----------------------------------------------------------

#[test]
fn gmail_draft_aim_by_in_reply_to_or_latest() {
    let t = Thread::new(vec![
        message(1, alice(), &["INBOX"]),
        message(3, alice(), &["INBOX"]),
        message(5, alice(), &["TRASH"]),
        draft(4),
    ]);
    // No In-Reply-To match: the latest message, and it is the newest non-draft
    // no newer than a draft written at 3.5 s.
    let aim = gmail_draft_aim(&t, None, Millis(3500));
    assert_eq!(
        aim,
        DraftAim {
            target: Some(MessageId(3)),
            started_on_latest: true
        }
    );
    // Written before message 3 arrived: not started on the latest.
    let early = gmail_draft_aim(&t, None, Millis(2000));
    assert_eq!(
        early,
        DraftAim {
            target: Some(MessageId(3)),
            started_on_latest: false
        }
    );
    // Aimed at message 1 by In-Reply-To, written right after it.
    let first = gmail_draft_aim(&t, Some(MessageId(1)), Millis(1500));
    assert_eq!(
        first,
        DraftAim {
            target: Some(MessageId(1)),
            started_on_latest: true
        }
    );
    // The trashed message 5 is newer than the draft written at 6 s, so the
    // draft did not start on the latest message.
    let after_trash = gmail_draft_aim(&t, None, Millis(6000));
    assert!(!after_trash.started_on_latest);
    let empty = gmail_draft_aim(&Thread::new(vec![draft(1)]), None, Millis(9));
    assert_eq!(
        empty,
        DraftAim {
            target: None,
            started_on_latest: false
        }
    );
}
