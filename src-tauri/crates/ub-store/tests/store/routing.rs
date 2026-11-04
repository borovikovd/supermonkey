//! Splits and pins route Inbox threads, and their writes re-route.

use ub_domain::{AccountId, Pin, PinKind, SendAsAddress, SplitId, SplitKind, ThreadId, View};
use ub_store::SplitEdit;

use crate::common::{Harness, fetched, msg};

fn split(h: &Harness, account: AccountId, kind: SplitKind) -> SplitId {
    h.read(move |r| r.splits().all(account))
        .into_iter()
        .find(|s| s.kind == kind)
        .unwrap()
        .id
}

fn split_of(h: &Harness, thread: ThreadId) -> Option<SplitId> {
    h.read(move |r| r.threads().row(thread)).unwrap().split
}

fn store(
    h: &Harness,
    account: AccountId,
    gmail: &'static str,
    from: &'static str,
    labels: &'static [&'static str],
) -> ThreadId {
    h.write(move |tx| {
        tx.threads().store(&fetched(
            account,
            gmail,
            vec![msg(&format!("{gmail}-1"), 1_000, from, labels)],
        ))
    })
    .thread
    .unwrap()
}

#[test]
fn a_new_account_has_important_then_other() {
    let h = Harness::start();
    let account = h.account("me@example.com");
    let splits = h.read(move |r| r.splits().all(account));
    let kinds: Vec<_> = splits
        .iter()
        .map(|s| (s.name.as_str(), s.kind, s.enabled))
        .collect();
    assert_eq!(
        kinds,
        [
            ("Important", SplitKind::Important, true),
            ("Other", SplitKind::Other, true)
        ]
    );
    let labels = h.read(move |r| r.labels().all(account));
    assert!(labels.iter().any(|l| l.id.as_str() == "INBOX" && l.system));
}

#[test]
fn person_to_person_mail_is_important_and_the_rest_is_other() {
    let h = Harness::start();
    let account = h.account("me@example.com");
    let personal = store(&h, account, "t1", "alice@example.com", &["INBOX"]);
    let promo = store(
        &h,
        account,
        "t2",
        "shop@example.com",
        &["INBOX", "CATEGORY_PROMOTIONS"],
    );
    assert_eq!(
        split_of(&h, personal),
        Some(split(&h, account, SplitKind::Important))
    );
    assert_eq!(
        split_of(&h, promo),
        Some(split(&h, account, SplitKind::Other))
    );
    let archived = store(&h, account, "t3", "carol@example.com", &[]);
    assert_eq!(split_of(&h, archived), None, "no split outside the Inbox");
}

#[test]
fn a_pin_re_routes_and_stamps_the_threads_it_moves() {
    let h = Harness::start();
    let account = h.account("me@example.com");
    let alice = store(&h, account, "t1", "alice@example.com", &["INBOX"]);
    let bob = store(&h, account, "t2", "bob@example.com", &["INBOX"]);
    let other = split(&h, account, SplitKind::Other);
    let pin = Pin::new(PinKind::Domain, " Example.COM ", other).unwrap();
    let replaced = h.write(move |tx| tx.splits().set_pin(account, &pin));
    assert_eq!(replaced, None);
    let commit = h.last_commit();
    for thread in [alice, bob] {
        assert_eq!(split_of(&h, thread), Some(other));
        assert!(commit.changes.threads.contains(&thread));
        assert_eq!(
            h.read(move |r| r.threads().row(thread)).unwrap().rev,
            commit.rev
        );
    }
    assert_eq!(
        h.read(move |r| r.splits().pin(account, PinKind::Domain, "example.com")),
        Some(other)
    );

    // A sender pin beats the domain pin.
    let important = split(&h, account, SplitKind::Important);
    let sender = Pin::new(PinKind::Sender, "alice@example.com", important).unwrap();
    h.write(move |tx| tx.splits().set_pin(account, &sender));
    assert_eq!(split_of(&h, alice), Some(important));
    assert_eq!(split_of(&h, bob), Some(other));
    assert!(
        !h.last_commit().changes.threads.contains(&bob),
        "bob did not move"
    );

    h.write(move |tx| {
        tx.splits()
            .remove_pin(account, PinKind::Domain, "example.com")
    });
    assert_eq!(split_of(&h, bob), Some(important));
}

#[test]
fn saving_splits_places_custom_ones_first_and_routes_by_their_rules() {
    let h = Harness::start();
    let account = h.account("me@example.com");
    let news = store(&h, account, "t1", "news@paper.com", &["INBOX"]);
    let alice = store(&h, account, "t2", "alice@example.com", &["INBOX"]);
    let important = split(&h, account, SplitKind::Important);
    let other = split(&h, account, SplitKind::Other);
    let edit = |id, name: &str, kind, query: &str| SplitEdit {
        id,
        name: name.to_owned(),
        kind,
        query: query.to_owned(),
        enabled: true,
        hide_when_empty: true,
    };
    let edits = vec![
        edit(important, "Important", SplitKind::Important, ""),
        edit(SplitId(0), " Papers ", SplitKind::Custom, "from:paper.com"),
        edit(other, "Other", SplitKind::Other, ""),
    ];
    let ids = h.write(move |tx| tx.splits().save(account, &edits, &[]));
    let papers = ids[1];
    assert_eq!(ids, [important, papers, other]);
    let names: Vec<_> = h
        .read(move |r| r.splits().all(account))
        .into_iter()
        .map(|s| s.name)
        .collect();
    assert_eq!(
        names,
        ["Papers", "Important", "Other"],
        "custom first, trimmed"
    );
    assert_eq!(split_of(&h, news), Some(papers));
    let inbox: Vec<_> = h
        .read(move |r| r.threads().list(account, &View::Inbox))
        .into_iter()
        .map(|l| (l.thread, l.split))
        .collect();
    assert_eq!(
        inbox,
        [(news, Some(papers)), (alice, Some(important))],
        "by split order"
    );

    // Deleting the custom split lets its mail fall through.
    let keep = vec![
        edit(important, "Important", SplitKind::Important, ""),
        edit(other, "Other", SplitKind::Other, ""),
    ];
    h.write(move |tx| tx.splits().save(account, &keep, &[papers]));
    assert_eq!(split_of(&h, news), Some(important));
}

#[test]
fn your_own_reply_does_not_move_a_thread_and_send_as_counts_as_you() {
    let h = Harness::start();
    let account = h.account("me@example.com");
    let thread = h
        .write(move |tx| {
            tx.threads().store(&fetched(
                account,
                "t1",
                vec![
                    msg("m1", 1_000, "alice@example.com", &["INBOX"]),
                    msg(
                        "m2",
                        2_000,
                        "alias@corp.io",
                        &["INBOX", "CATEGORY_PROMOTIONS"],
                    ),
                ],
            ))
        })
        .thread
        .unwrap();
    let important = split(&h, account, SplitKind::Important);
    let other = split(&h, account, SplitKind::Other);
    assert_eq!(
        split_of(&h, thread),
        Some(other),
        "alias@corp.io is someone else yet"
    );
    let send_as = vec![SendAsAddress {
        email: "alias@corp.io".to_owned(),
        name: String::new(),
        is_default: false,
        is_primary: false,
    }];
    h.write(move |tx| tx.accounts().set_profile(account, "Me", &send_as));
    assert_eq!(
        split_of(&h, thread),
        Some(important),
        "now the reply is mine"
    );
    assert!(h.last_commit().changes.threads.contains(&thread));
}

#[test]
fn a_label_pin_matches_the_threads_user_label_names() {
    let h = Harness::start();
    let account = h.account("me@example.com");
    h.write(move |tx| {
        tx.labels().sync_from_gmail(
            account,
            &[ub_store::GmailLabel {
                gmail_id: "Label_7".into(),
                name: "Clients".to_owned(),
                system: false,
            }],
        )
    });
    let thread = store(
        &h,
        account,
        "t1",
        "alice@example.com",
        &["INBOX", "Label_7"],
    );
    let other = split(&h, account, SplitKind::Other);
    let pin = Pin::new(PinKind::Label, "clients", other).unwrap();
    h.write(move |tx| tx.splits().set_pin(account, &pin));
    assert_eq!(split_of(&h, thread), Some(other));
    // Renaming the label on Gmail re-routes: the pin no longer matches.
    h.write(move |tx| {
        tx.labels().sync_from_gmail(
            account,
            &[ub_store::GmailLabel {
                gmail_id: "Label_7".into(),
                name: "Customers".to_owned(),
                system: false,
            }],
        )
    });
    assert_eq!(
        split_of(&h, thread),
        Some(split(&h, account, SplitKind::Important))
    );
}
