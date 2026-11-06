//! Drafts, the outbox, the undo history, notices, ops, labels, and settings.

use ub_domain::{
    AccountId, Addr, DraftContent, DraftId, DraftKind, LabelId, LabelSet, Millis, OutboxState,
    PinKind, Rev, SplitId, ThreadChange, ThreadId, UndoEntryId, UndoKind, View,
};
use ub_store::{Draft, DraftAttachment, DraftNeed, NewOutbox, StoreError, UNDO_KEPT, UndoAction};

use crate::common::{Harness, fetched, msg};

fn thread(h: &Harness, account: AccountId) -> ThreadId {
    h.write(move |tx| {
        tx.threads().store(&fetched(
            account,
            "t1",
            vec![msg("m1", 1_000, "alice@example.com", &["INBOX"])],
        ))
    })
    .thread
    .unwrap()
}

fn reply(account: AccountId, id: &str, thread: ThreadId, at: i64) -> Draft {
    let mut d = Draft::new(DraftId::from(id), account, DraftKind::Reply, Millis(at));
    d.thread = Some(thread);
    d.to = vec![Addr::named("Alice", "alice@example.com")];
    "Re: hi".clone_into(&mut d.subject);
    "<p>Sure</p>".clone_into(&mut d.body_html);
    "Sure".clone_into(&mut d.snippet);
    d.attachments = vec![DraftAttachment {
        id: "a1".to_owned(),
        filename: "x.pdf".to_owned(),
        mime_type: "application/pdf".to_owned(),
        size: 3,
        path: None,
        source: None,
        added: Some(Rev(4)),
    }];
    d.defaults = Some(DraftContent::default());
    d.needs = Some(DraftNeed::TargetBody);
    d
}

#[test]
fn a_draft_round_trips_and_stamps_its_thread() {
    let h = Harness::start();
    let account = h.account("me@example.com");
    let t = thread(&h, account);
    let draft = reply(account, "d1", t, 5);
    let written = draft.clone();
    h.write(move |tx| tx.drafts().put(&written));
    let commit = h.last_commit();
    let id = draft.id.clone();
    let stored = h.read(move |r| r.drafts().get(&id)).unwrap();
    assert_eq!(stored.draft, draft);
    assert_eq!(stored.rev, commit.rev);
    assert!(!stored.in_outbox);
    assert!(commit.changes.drafts.contains(&draft.id));
    assert!(commit.changes.draft_lists.contains(&account));
    assert!(
        commit.changes.threads.contains(&t),
        "a draft change stamps its thread"
    );
    assert_eq!(h.read(move |r| r.threads().row(t)).unwrap().rev, commit.rev);

    let other = h.account("other@example.com");
    let mut moved = draft;
    moved.account = other;
    let refused: Result<(), StoreError> = h
        .writer
        .call_blocking(ub_store::Priority::Ui, move |tx, _| tx.drafts().put(&moved));
    assert!(matches!(refused, Err(StoreError::Invalid(_))));
}

#[test]
fn a_draft_in_the_outbox_leaves_drafts_and_its_thread() {
    let h = Harness::start();
    let account = h.account("me@example.com");
    let t = thread(&h, account);
    let older = reply(account, "d-old", t, 5);
    let newer = reply(account, "d-new", t, 9);
    h.write(move |tx| {
        tx.drafts().put(&older)?;
        tx.drafts().put(&newer)
    });
    let ids = |h: &Harness| h.read(move |r| r.drafts().list(account));
    assert_eq!(
        ids(&h),
        [DraftId::from("d-new"), DraftId::from("d-old")],
        "newest edit first"
    );
    let conversation = h.read(move |r| r.threads().conversation(t, false)).unwrap();
    assert_eq!(conversation.drafts.len(), 2);

    let send = NewOutbox {
        account,
        draft: DraftId::from("d-new"),
        thread: Some(t),
        send_after: Millis(100),
    };
    let row = h.write(move |tx| tx.outbox().insert(&send));
    let commit = h.last_commit();
    assert!(commit.changes.outbox.contains(&row));
    assert!(commit.changes.threads.contains(&t));
    assert!(commit.changes.draft_lists.contains(&account));
    assert_eq!(ids(&h), [DraftId::from("d-old")]);
    let conversation = h.read(move |r| r.threads().conversation(t, false)).unwrap();
    assert_eq!(conversation.drafts.len(), 1);
    assert_eq!(conversation.outbox.len(), 1);
    let pending = &conversation.outbox[0];
    assert_eq!(pending.subject, "Re: hi");
    assert_eq!(pending.snippet, "Sure");
    assert_eq!(pending.outbox.state, OutboxState::Waiting);
    assert!(
        h.read(|r| r.drafts().get(&DraftId::from("d-new")))
            .unwrap()
            .in_outbox
    );

    // Sent, and the thread holds the sent message: the conversation shows
    // only the message.
    let mut delivered = pending.outbox.clone();
    delivered.state = OutboxState::Sent;
    delivered.sent_message_id = Some("m-sent".into());
    h.write(move |tx| tx.outbox().update(&delivered));
    assert_eq!(
        h.read(move |r| r.threads().conversation(t, false))
            .unwrap()
            .outbox
            .len(),
        1
    );
    h.write(move |tx| {
        tx.threads().store(&fetched(
            account,
            "t1",
            vec![
                msg("m1", 1_000, "alice@example.com", &["INBOX"]),
                msg("m-sent", 2_000, "me@example.com", &["SENT"]),
            ],
        ))
    });
    assert!(
        h.read(move |r| r.threads().conversation(t, false))
            .unwrap()
            .outbox
            .is_empty(),
        "the thread shows the sent message instead"
    );
    assert_eq!(
        h.read(move |r| r.outbox().for_account(account)).len(),
        1,
        "the toast still has it"
    );
}

#[test]
fn the_undo_history_keeps_the_newest_entries_and_finds_sends() {
    let h = Harness::start();
    let account = h.account("me@example.com");
    let total = i64::from(UNDO_KEPT) + 5;
    h.write(move |tx| {
        for n in 1..=total {
            let action = UndoAction::Mail {
                threads: vec![ThreadChange {
                    thread: ThreadId(n),
                    changes: Vec::new(),
                    snooze_before: None,
                    snooze_after: Some(Millis(n)),
                }],
            };
            tx.undo()
                .record(account, UndoEntryId(n), "Marked done", &action)?;
        }
        Ok(())
    });
    let entries = h.read(move |r| r.undo().entries(account));
    assert_eq!(entries.len(), usize::try_from(UNDO_KEPT).unwrap());
    assert_eq!(entries[0].id, UndoEntryId(6), "the oldest went");
    assert_eq!(entries.last().unwrap().action.threads(), [ThreadId(total)]);
    assert!(
        h.read(move |r| r.undo().get(account, UndoEntryId(1)))
            .is_none()
    );

    let send = NewOutbox {
        account,
        draft: DraftId::from("d1"),
        thread: None,
        send_after: Millis(1),
    };
    let row = h.write(move |tx| tx.outbox().insert(&send));
    let pin = UndoAction::Pin {
        pin_kind: PinKind::Sender,
        value: "a@example.com".to_owned(),
        split: SplitId(1),
        replaced: None,
    };
    h.write(move |tx| {
        tx.undo().record(
            account,
            UndoEntryId(100),
            "Sent",
            &UndoAction::Send { outbox: row },
        )?;
        tx.undo()
            .record(account, UndoEntryId(101), "Moved to Other", &pin)
    });
    let outbox = h.read(move |r| r.outbox().get(row)).unwrap();
    assert_eq!(outbox.undo_entry, Some(UndoEntryId(100)));
    let kinds: Vec<UndoKind> = h
        .read(move |r| r.undo().entries(account))
        .iter()
        .rev()
        .take(2)
        .map(|e| e.action.kind())
        .collect();
    assert_eq!(kinds, [UndoKind::Pin, UndoKind::Send]);
    assert!(
        h.read(|r| r.undo().for_sent_rows()).is_empty(),
        "nothing sent yet"
    );
    let mut delivered = outbox.outbox;
    delivered.state = OutboxState::Sent;
    h.write(move |tx| tx.outbox().update(&delivered));
    assert_eq!(h.read(|r| r.undo().for_sent_rows()), [UndoEntryId(100)]);
    h.write(move |tx| tx.undo().delete_for_outbox(row));
    assert_eq!(
        h.read(move |r| r.outbox().get(row)).unwrap().undo_entry,
        None
    );
}

#[test]
fn a_local_label_merges_into_the_gmail_label_with_its_name() {
    let h = Harness::start();
    let account = h.account("me@example.com");
    let local = LabelId::from("ui-1");
    let gmail = LabelId::from("Label_9");
    let id = local.clone();
    h.write(move |tx| tx.labels().create_local(account, &id, "Work"));
    let labeled = msg("m1", 1_000, "alice@example.com", &["INBOX", "ui-1"]);
    let labeled_thread = h
        .write(move |tx| tx.threads().store(&fetched(account, "t1", vec![labeled])))
        .thread
        .unwrap();
    // Gmail already had "Work" as Label_9.
    h.write(move |tx| {
        tx.labels().sync_from_gmail(
            account,
            &[ub_store::GmailLabel {
                gmail_id: "Label_9".into(),
                name: "Work".to_owned(),
                system: false,
            }],
        )
    });
    let (from, to) = (local.clone(), gmail.clone());
    let kept = h.write(move |tx| tx.labels().assign_gmail_id(account, &from, &to));
    assert_eq!(kept, gmail);
    assert!(
        h.read(|r| r.labels().get(account, &local)).is_none(),
        "the local label went"
    );
    let row = h.read(|r| r.threads().row(labeled_thread)).unwrap();
    assert_eq!(
        row.labels,
        ["INBOX", "Label_9"].into_iter().collect::<LabelSet>()
    );
    let listed = h.read(|r| r.threads().list(account, &View::Label { label: gmail }));
    assert_eq!(listed.len(), 1);

    // Without a clash, the local id stays and gains the Gmail id.
    let other = LabelId::from("ui-2");
    let id = other.clone();
    h.write(move |tx| tx.labels().create_local(account, &id, "Fresh"));
    let id = other.clone();
    let kept = h.write(move |tx| {
        tx.labels()
            .assign_gmail_id(account, &id, &LabelId::from("Label_10"))
    });
    assert_eq!(kept, other);
    assert_eq!(
        h.read(move |r| r.labels().by_gmail_id(account, &LabelId::from("Label_10"))),
        Some(other)
    );
}

#[test]
fn settings_are_global_or_per_account() {
    let h = Harness::start();
    let account = h.account("me@example.com");
    h.write(move |tx| {
        tx.settings().set(AccountId::NONE, "remote_images", "off")?;
        tx.settings().set(account, "muted", "on")
    });
    let commit = h.last_commit();
    assert!(commit.changes.app, "a global setting changes the app state");
    assert!(commit.changes.summaries.contains(&account));
    assert_eq!(
        h.read(|r| r.settings().get(AccountId::NONE, "remote_images"))
            .as_deref(),
        Some("off")
    );
    assert_eq!(h.read(move |r| r.settings().all(account)).len(), 1);
    h.write(move |tx| tx.accounts().remove(account));
    assert!(
        h.read(move |r| r.settings().all(account)).is_empty(),
        "removed with the account"
    );
}

#[test]
fn contacts_count_each_message_once_and_keep_the_latest_name() {
    let h = Harness::start();
    let account = h.account("me@example.com");
    let seen = |name: &str, at: i64, on_list: bool| ub_store::Sighting {
        email: "Ann.Lee@Example.com".to_owned(),
        name: name.to_owned(),
        sent: 1,
        received: 0,
        on_list,
        at: Millis(at),
    };
    let first = vec![seen("Ann", 10, true)];
    let counted = h.write(move |tx| tx.contacts().count(account, &"g1".into(), &first));
    assert!(counted);
    let again = vec![seen("Ann", 10, true)];
    let counted = h.write(move |tx| tx.contacts().count(account, &"g1".into(), &again));
    assert!(!counted, "a message counts once");
    let later = vec![seen("Ann Lee", 20, false), seen("", 30, true)];
    h.write(move |tx| tx.contacts().count(account, &"g2".into(), &later));

    let found = h.read(move |r| r.contacts().candidates(account, "ann.l"));
    assert_eq!(found.len(), 1);
    let ann = &found[0];
    assert_eq!(ann.email, "ann.lee@example.com", "stored lowercased");
    assert_eq!(ann.name, "Ann Lee", "the latest name that is not empty");
    assert_eq!(ann.stats.sent, 3);
    assert!(!ann.stats.list_only, "seen once off a list");
    assert_eq!(ann.stats.last_contact, Millis(30));
    assert!(h.read(move |r| r.contacts().is_counted(account, &"g2".into())));
    assert!(
        h.read(move |r| r.contacts().candidates(account, "100%"))
            .is_empty(),
        "LIKE wildcards in the query are literal"
    );
}
