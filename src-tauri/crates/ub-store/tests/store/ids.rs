//! Ids the UI, the queue, or the history hold are never reused, even after
//! the newest row is deleted (AGENTS.md, Concurrency).

use ub_domain::{AccountId, DraftId, Millis, NoticeKind, ThreadId};
use ub_store::{NewNotice, NewOp, NewOutbox, OpKind};

use crate::common::{Harness, fetched, msg};

fn notice(account: AccountId) -> NewNotice {
    NewNotice {
        account,
        kind: NoticeKind::Info,
        message: "hello".to_owned(),
        draft: None,
        thread: None,
        created_at: Millis(1),
    }
}

#[test]
fn outbox_notice_and_op_ids_are_never_reused() {
    let h = Harness::start();
    let account = h.account("me@example.com");
    let draft = DraftId::from("d1");
    let send = NewOutbox {
        account,
        draft,
        thread: None,
        send_after: Millis(10),
    };
    let again = send.clone();
    let first_send = h.write(move |tx| tx.outbox().insert(&send));
    h.write(move |tx| tx.outbox().delete(first_send));
    let second_send = h.write(move |tx| tx.outbox().insert(&again));
    assert!(second_send > first_send, "{second_send} after {first_send}");

    let first_notice = h.write(move |tx| tx.notices().add(&notice(account)));
    h.write(move |tx| tx.notices().delete(first_notice));
    let second_notice = h.write(move |tx| tx.notices().add(&notice(account)));
    assert!(second_notice > first_notice);

    let push = move |tx: &mut ub_store::WriteTx<'_>| {
        tx.ops().push(&NewOp {
            account,
            action: None,
            kind: OpKind::Modify,
            draft: None,
            gmail_draft: None,
            payload: &vec![1, 2, 3],
            created_at: Millis(1),
        })
    };
    let first_op = h.write(push);
    assert_eq!(
        h.read(move |r| r.ops().get(first_op))
            .unwrap()
            .payload::<Vec<i32>>()
            .unwrap(),
        [1, 2, 3]
    );
    h.write(move |tx| tx.ops().delete(first_op));
    let second_op = h.write(push);
    assert!(second_op > first_op);
}

#[test]
fn thread_message_and_account_ids_are_never_reused() {
    let h = Harness::start();
    let first_account = h.account("a@example.com");
    let store = move |account: AccountId, gmail: &'static str| {
        move |tx: &mut ub_store::WriteTx<'_>| {
            tx.threads().store(&fetched(
                account,
                gmail,
                vec![msg(
                    &format!("{gmail}-m"),
                    1_000,
                    "x@example.com",
                    &["INBOX"],
                )],
            ))
        }
    };
    let first = h.write(store(first_account, "t1"));
    let thread: ThreadId = first.thread.unwrap();
    h.write(move |tx| tx.threads().delete(thread));
    assert!(h.read(move |r| r.threads().row(thread)).is_none());
    let second = h.write(store(first_account, "t2"));
    assert!(second.thread.unwrap() > thread);
    assert!(second.added[0] > first.added[0]);

    h.write(move |tx| tx.accounts().remove(first_account));
    let second_account = h.account("b@example.com");
    assert!(second_account > first_account);
    assert!(
        h.read(move |r| r.threads().list(first_account, &ub_domain::View::AllMail))
            .is_empty(),
        "the removed account's mail is gone"
    );
}
