//! The view lists' SQL agrees with `View::lists`, the domain's rule.

use ub_domain::{LabelId, Millis, Thread, ThreadId, View};

use crate::common::{Harness, fetched, msg};

#[test]
fn every_view_lists_what_the_domain_rule_says_in_order() {
    let h = Harness::start();
    let account = h.account("me@example.com");
    let other = h.account("other@example.com");
    // (labels of each message, snoozed): one thread each, newer as it goes.
    let shapes: &[(&[&[&str]], bool)] = &[
        (&[&["INBOX", "UNREAD"]], false),
        (&[&["INBOX"]], true),
        (&[&[]], false),
        (&[&[]], true),
        (&[&["STARRED"], &["INBOX"]], false),
        (&[&["SENT"]], false),
        (&[&["TRASH"]], false),
        (&[&["INBOX"], &["TRASH"]], false),
        (&[&["SPAM"]], false),
        (&[&["Label_1"]], false),
        (&[&["Label_1", "TRASH"]], false),
        (&[&["DRAFT"]], false),
    ];
    let mut threads: Vec<(ThreadId, bool)> = Vec::new();
    for (i, (messages, snoozed)) in shapes.iter().enumerate() {
        let n = i64::try_from(i).unwrap();
        let built: Vec<_> = messages
            .iter()
            .enumerate()
            .map(|(j, labels)| {
                let mut m = msg(
                    &format!("t{i}-m{j}"),
                    1_000 * n + i64::try_from(j).unwrap(),
                    "x@example.com",
                    labels,
                );
                m.is_draft = labels.contains(&"DRAFT");
                m
            })
            .collect();
        let gmail = format!("t{i}");
        let stored = h.write(move |tx| tx.threads().store(&fetched(account, &gmail, built)));
        let thread = stored.thread.unwrap();
        if *snoozed {
            h.write(move |tx| tx.threads().set_snooze(thread, Some(Millis(99_000 - n))));
        }
        threads.push((thread, *snoozed));
    }
    // Another account's mail never shows.
    h.write(move |tx| {
        tx.threads().store(&fetched(
            other,
            "x",
            vec![msg("x-m", 50, "y@example.com", &["INBOX", "STARRED"])],
        ))
    });

    let views = [
        View::Inbox,
        View::Done,
        View::AllMail,
        View::Snoozed,
        View::Starred,
        View::Sent,
        View::Trash,
        View::Spam,
        View::Label {
            label: LabelId::from("Label_1"),
        },
    ];
    for view in views {
        let mut expected: Vec<(Millis, ThreadId)> = Vec::new();
        for &(thread, snoozed) in &threads {
            let facts = h.read(move |r| r.messages().facts(thread));
            let digest = Thread::new(facts).digest().unwrap();
            if view.lists(&digest, snoozed) {
                let key = if view.sorts_by_snooze() {
                    h.read(move |r| r.threads().row(thread))
                        .unwrap()
                        .snooze_until
                        .unwrap()
                } else {
                    Millis(-digest.sort_time.0)
                };
                expected.push((key, thread));
            }
        }
        expected.sort();
        let expected: Vec<ThreadId> = expected.into_iter().map(|(_, t)| t).collect();
        let listed: Vec<ThreadId> = {
            let view = view.clone();
            h.read(move |r| r.threads().list(account, &view))
        }
        .into_iter()
        .map(|l| l.thread)
        .collect();
        assert_eq!(listed, expected, "{view:?}");
        assert!(
            !listed.is_empty(),
            "{view:?} lists something, so the check means something"
        );
    }
    assert!(
        h.read(move |r| r.threads().list(account, &View::Drafts))
            .is_empty(),
        "drafts, not threads"
    );
}
