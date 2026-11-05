//! Message writes re-derive their thread's row, labels, and existence.

use ub_domain::{
    AccountId, DraftId, DraftKind, LabelSet, MessageChange, Millis, SplitKind, ThreadId, View,
};
use ub_store::{Draft, NewBody};

use crate::common::{Harness, fetched, msg};

fn labels(ids: &[&str]) -> LabelSet {
    ids.iter().copied().collect()
}

fn split_of(h: &Harness, account: AccountId, kind: SplitKind) -> ub_domain::SplitId {
    h.read(move |r| r.splits().all(account))
        .into_iter()
        .find(|s| s.kind == kind)
        .unwrap()
        .id
}

/// A two-message Inbox thread: Alice's unread message, then Bob's.
fn two_messages(h: &Harness, account: AccountId) -> ThreadId {
    let mut first = msg("m1", 1_000, "alice@example.com", &["INBOX", "UNREAD"]);
    "Plans".clone_into(&mut first.subject);
    let mut second = msg("m2", 2_000, "bob@example.com", &["INBOX"]);
    "Re: Plans".clone_into(&mut second.subject);
    h.write(move |tx| {
        tx.threads()
            .store(&fetched(account, "t1", vec![first, second]))
    })
    .thread
    .unwrap()
}

#[test]
fn a_stored_thread_gets_its_row_from_its_messages() {
    let h = Harness::start();
    let account = h.account("me@example.com");
    let thread = two_messages(&h, account);
    let row = h.read(move |r| r.threads().row(thread)).unwrap();
    assert_eq!(row.subject, "Plans", "the first message's subject");
    assert_eq!(
        row.sender_email, "bob@example.com",
        "the latest message's sender"
    );
    assert_eq!(row.snippet, "Snippet of m2");
    assert_eq!(row.sort_time, Millis(2_000));
    assert_eq!(row.message_count, 2);
    assert!(row.unread, "a live message is unread");
    assert!(row.in_inbox && row.has_live);
    assert_eq!(row.labels, labels(&["INBOX", "UNREAD"]));
    assert_eq!(row.split, Some(split_of(&h, account, SplitKind::Important)));
    let inbox = h.read(move |r| r.threads().list(account, &View::Inbox));
    assert_eq!(inbox.iter().map(|l| l.thread).collect::<Vec<_>>(), [thread]);
}

#[test]
fn a_label_change_re_derives_and_stamps_the_thread() {
    let h = Harness::start();
    let account = h.account("me@example.com");
    let thread = two_messages(&h, account);
    let first = h
        .read(move |r| r.messages().for_thread(thread))
        .into_iter()
        .next()
        .unwrap();
    let change = MessageChange {
        message: first.id,
        added: LabelSet::new(),
        removed: labels(&["UNREAD"]),
    };
    h.write(move |tx| tx.messages().apply(&[change]));
    let commit = h.last_commit();
    let row = h.read(move |r| r.threads().row(thread)).unwrap();
    assert!(!row.unread);
    assert_eq!(row.labels, labels(&["INBOX"]));
    assert_eq!(row.rev, commit.rev);
    assert!(commit.changes.threads.contains(&thread));
    assert!(commit.changes.messages.contains(&first.id));
}

#[test]
fn trashing_the_latest_message_moves_the_row_to_the_one_before() {
    let h = Harness::start();
    let account = h.account("me@example.com");
    let thread = two_messages(&h, account);
    let second = h.read(move |r| r.messages().for_thread(thread))[1].id;
    h.write(move |tx| tx.messages().set_labels(&[(second, labels(&["TRASH"]))]));
    let row = h.read(move |r| r.threads().row(thread)).unwrap();
    assert_eq!(
        row.sender_email, "alice@example.com",
        "the latest live message"
    );
    assert_eq!(row.message_count, 1, "live messages only");
    assert_eq!(row.labels, labels(&["INBOX", "TRASH", "UNREAD"]));
    for view in [View::Inbox, View::Trash] {
        let listed = h.read(|r| r.threads().list(account, &view));
        assert_eq!(listed.len(), 1, "still listed in {view:?}");
    }
}

#[test]
fn a_complete_refetch_deletes_messages_gmail_no_longer_has() {
    let h = Harness::start();
    let account = h.account("me@example.com");
    let thread = two_messages(&h, account);
    let stored = h.write(move |tx| {
        tx.threads().store(&fetched(
            account,
            "t1",
            vec![msg("m1", 1_000, "alice@example.com", &["INBOX"])],
        ))
    });
    assert_eq!(stored.thread, Some(thread));
    assert_eq!(stored.removed.len(), 1);
    assert!(stored.added.is_empty(), "m1 was stored before");
    let messages = h.read(move |r| r.messages().for_thread(thread));
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].gmail_id.as_str(), "m1");
    let row = h.read(move |r| r.threads().row(thread)).unwrap();
    assert_eq!(row.sender_email, "alice@example.com");
    assert!(!row.unread, "the refetch replaced the labels");
}

#[test]
fn a_thread_whose_last_message_goes_is_deleted_and_its_draft_kept() {
    let h = Harness::start();
    let account = h.account("me@example.com");
    let thread = two_messages(&h, account);
    let draft_id = DraftId::from("d1");
    let mut draft = Draft::new(draft_id.clone(), account, DraftKind::Reply, Millis(5));
    draft.thread = Some(thread);
    h.write(move |tx| tx.drafts().put(&draft));

    let stored = h.write(move |tx| tx.threads().store(&fetched(account, "t1", Vec::new())));
    let commit = h.last_commit();
    assert_eq!(stored.thread, None);
    assert_eq!(stored.removed.len(), 2);
    assert!(h.read(move |r| r.threads().row(thread)).is_none());
    assert!(
        h.read(move |r| r.threads().list(account, &View::Inbox))
            .is_empty(),
        "left the Inbox"
    );
    let id = draft_id.clone();
    let kept = h.read(move |r| r.drafts().get(&id)).unwrap();
    assert_eq!(kept.draft.thread, None, "unlinked, not deleted");
    assert_eq!(kept.rev, commit.rev, "and stamped");
    assert!(commit.changes.drafts.contains(&draft_id));
    assert!(commit.changes.threads.contains(&thread));
}

#[test]
fn a_thread_of_only_drafts_is_in_no_mail_view() {
    let h = Harness::start();
    let account = h.account("me@example.com");
    let mut draft = msg("d1", 1_000, "me@example.com", &["DRAFT"]);
    draft.is_draft = true;
    let thread = h
        .write(move |tx| tx.threads().store(&fetched(account, "t9", vec![draft])))
        .thread
        .unwrap();
    let row = h.read(move |r| r.threads().row(thread)).unwrap();
    assert!(!row.has_live);
    assert!(row.labels.is_empty(), "DRAFT is never a thread label");
    for view in [View::Inbox, View::Done, View::AllMail] {
        assert!(
            h.read(|r| r.threads().list(account, &view)).is_empty(),
            "{view:?}"
        );
    }
}

#[test]
fn a_new_body_keeps_downloaded_attachments_by_part() {
    let h = Harness::start();
    let account = h.account("me@example.com");
    let thread = two_messages(&h, account);
    let message = h.read(move |r| r.messages().for_thread(thread))[0].id;
    let part = |id: &str| ub_store::NewAttachment {
        part_id: id.to_owned(),
        filename: format!("{id}.pdf"),
        mime_type: "application/pdf".to_owned(),
        size: 10,
        gmail_attachment_id: Some(format!("att-{id}")),
        content_id: None,
        is_inline: false,
    };
    let body = NewBody {
        html: Some("<p>Hi</p>".to_owned()),
        quote_html: None,
        text: Some("Hi".to_owned()),
        is_plain: false,
        attachments: vec![part("1"), part("2")],
    };
    let again = body.clone();
    h.write(move |tx| tx.messages().store_body(message, &body));
    let first = h.read(move |r| r.messages().attachments(message));
    assert_eq!(first.len(), 2);
    let downloaded = first[0].id;
    h.write(move |tx| tx.messages().set_attachment_path(downloaded, "/tmp/1.pdf"));
    // The same body again, now without part 2.
    let mut again = again;
    again.attachments.truncate(1);
    h.write(move |tx| tx.messages().store_body(message, &again));
    let after = h.read(move |r| r.messages().attachments(message));
    assert_eq!(after.len(), 1);
    assert_eq!(after[0].id, downloaded, "same id");
    assert_eq!(
        after[0].local_path.as_deref(),
        Some("/tmp/1.pdf"),
        "same file"
    );
    let bodies = h.read(move |r| r.messages().bodies(&[message, ub_domain::MessageId(999)]));
    assert_eq!(bodies.len(), 1);
    assert_eq!(bodies[0].html.as_deref(), Some("<p>Hi</p>"));
    assert!(h.read(move |r| r.messages().get(message)).unwrap().has_body);
}

#[test]
fn a_conversation_hides_trash_outside_trash_views_and_drafts_always() {
    let h = Harness::start();
    let account = h.account("me@example.com");
    let mut draft = msg("m3", 3_000, "me@example.com", &["DRAFT"]);
    draft.is_draft = true;
    let thread = h
        .write(move |tx| {
            tx.threads().store(&fetched(
                account,
                "t1",
                vec![
                    msg("m1", 1_000, "alice@example.com", &["INBOX"]),
                    msg("m2", 2_000, "bob@example.com", &["TRASH"]),
                    draft,
                ],
            ))
        })
        .thread
        .unwrap();
    let ids = |trash: bool| {
        h.read(move |r| r.threads().conversation(thread, trash))
            .unwrap()
            .messages
            .into_iter()
            .map(|m| m.message.gmail_id.0)
            .collect::<Vec<_>>()
    };
    assert_eq!(ids(false), ["m1"]);
    assert_eq!(ids(true), ["m1", "m2"]);
    assert!(
        h.read(|r| r.threads().conversation(ThreadId(404), false))
            .is_none()
    );
}

#[test]
fn snoozing_lists_the_thread_in_snoozed_and_out_of_the_inbox() {
    let h = Harness::start();
    let account = h.account("me@example.com");
    let thread = two_messages(&h, account);
    h.write(move |tx| tx.threads().set_snooze(thread, Some(Millis(9_000))));
    assert!(h.last_commit().changes.threads.contains(&thread));
    assert!(
        h.read(move |r| r.threads().list(account, &View::Inbox))
            .is_empty(),
        "snoozed"
    );
    assert_eq!(
        h.read(move |r| r.threads().list(account, &View::Snoozed))
            .len(),
        1
    );
    assert_eq!(h.read(|r| r.threads().next_snooze()), Some(Millis(9_000)));
    assert!(
        h.read(|r| r.threads().snoozes_due(Millis(8_999)))
            .is_empty(),
        "not due yet"
    );
    assert_eq!(
        h.read(|r| r.threads().snoozes_due(Millis(9_000))),
        [(account, thread)]
    );
}
