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
}

#[test]
fn digest_of_threads_with_trashed_or_draft_messages() {
    let gone = Thread::new(vec![
        message(1, alice(), &["TRASH", "UNREAD"]),
        message(2, alice(), &["TRASH"]),
        draft(3),
    ]);
    let d = gone.digest().unwrap();
    assert_eq!((d.message_count, d.unread, d.has_live), (2, true, false));
    assert_eq!(d.sender, "Alice");
    assert_eq!(
        Thread::new(vec![draft(1)]).digest().unwrap().message_count,
        1
    );
    let read = Thread::new(vec![
        message(1, alice(), &["INBOX"]),
        message(2, alice(), &["TRASH", "UNREAD"]),
    ]);
    assert!(!read.digest().unwrap().unread);
}

#[test]
fn view_membership() {
    let thread = |labels: &[&str]| Thread::new(vec![message(1, alice(), labels)]);
    let (inbox, archived, trashed) = (
        thread(&["INBOX", "STARRED", "Label_1"]),
        thread(&["SENT"]),
        thread(&["TRASH", "INBOX"]),
    );
    let inbox = inbox.digest().unwrap();
    let archived = archived.digest().unwrap();
    let trashed = trashed.digest().unwrap();
    let label = View::Label {
        label: LabelId::from("Label_1"),
    };
    let cases: [(&View, &ThreadDigest<'_>, bool, bool); 18] = [
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
    let mixed = Thread::new(vec![
        message(1, alice(), &["INBOX"]),
        message(2, alice(), &["TRASH"]),
    ]);
    let mixed = mixed.digest().unwrap();
    assert!(View::Inbox.lists(&mixed, false) && View::Trash.lists(&mixed, false));
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

fn delta(kind: ModifyKind) -> LabelDelta {
    match kind {
        ModifyKind::Done => LabelDelta::new(LabelSet::default(), set(&["INBOX", "UNREAD"])),
        ModifyKind::Unread => LabelDelta::new(set(&["UNREAD"]), LabelSet::default()),
        ModifyKind::ReturnToInbox
        | ModifyKind::Snooze
        | ModifyKind::Trash
        | ModifyKind::Spam
        | ModifyKind::Star
        | ModifyKind::Unstar
        | ModifyKind::Read
        | ModifyKind::Label
        | ModifyKind::RemoveLabel => panic!("no test delta for {kind:?}"),
    }
    .unwrap()
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
    let gone = Thread::new(vec![
        message(1, alice(), &["TRASH"]),
        message(2, alice(), &["TRASH", "UNREAD"]),
    ]);
    let plan = plan_modify(&gone, &delta(ModifyKind::Unread));
    assert_eq!(plan.op_messages.len(), 2);
    assert_eq!(plan.changes, [change(1, &["UNREAD"], &[])]);
}

#[test]
fn revert_undoes_only_what_is_still_there() {
    let done = change(1, &["STARRED"], &["INBOX", "UNREAD"]);
    let now = done.apply(set(&["INBOX", "UNREAD", "Label_1"]));
    assert_eq!(now, set(&["Label_1", "STARRED"]));
    let back = done.reverted(&now).unwrap();
    assert_eq!(back, change(1, &["INBOX", "UNREAD"], &["STARRED"]));
    assert_eq!(back.apply(now), set(&["INBOX", "UNREAD", "Label_1"]));
    let partly = done.reverted(&set(&["INBOX", "Label_1"])).unwrap();
    assert_eq!(partly, change(1, &["UNREAD"], &[]));
    assert_eq!(done.reverted(&set(&["INBOX", "UNREAD"])), None);
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
    let flat = Router::new(splits(false, false, false), []);
    for m in [&list, &boss, &person, &promo] {
        assert_eq!(flat.route(m, &[]), Some(OTHER));
    }
}

#[test]
fn pins_beat_rules_sender_then_domain_then_label() {
    let labels = vec!["Clients", "Work"];
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
    let off = Router::new(splits(true, true, false), all.to_vec());
    assert_eq!(
        off.route(&m, &labels),
        Some(OTHER),
        "domain pin, as the sender pin's split is off"
    );
}

#[test]
fn several_label_pins_take_the_first_split_in_order() {
    let labels = vec!["Work", "Clients"];
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
fn gmail_draft_aim_by_in_reply_to_or_latest() {
    let t = Thread::new(vec![
        message(1, alice(), &["INBOX"]),
        message(3, alice(), &["INBOX"]),
        message(5, alice(), &["TRASH"]),
        draft(4),
    ]);
    let aim = gmail_draft_aim(&t, None, Millis(3500));
    assert_eq!(
        aim,
        DraftAim {
            target: Some(MessageId(3)),
            started_on_latest: true
        }
    );
    let early = gmail_draft_aim(&t, None, Millis(2000));
    assert_eq!(
        early,
        DraftAim {
            target: Some(MessageId(3)),
            started_on_latest: false
        }
    );
    let first = gmail_draft_aim(&t, Some(MessageId(1)), Millis(1500));
    assert_eq!(
        first,
        DraftAim {
            target: Some(MessageId(1)),
            started_on_latest: true
        }
    );
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
