//! Which new mail notifies (SPEC Notifications), for the case the E2E suite
//! can't reach: a poll that reads a whole thread for one changed message
//! stores its older messages too, and only mail history reported as added
//! notifies.

use std::collections::BTreeSet;

use ub_domain::{GmailMessageId, GmailThreadId};
use ub_store::{FetchedThread, Priority, StoreError};

use super::{Harness, NEWEST, eventually, message, seed};

#[tokio::test]
async fn only_messages_history_reported_added_notify() {
    let h = Harness::start().await;
    let (account, _) = h.write(seed).await;
    let added = h
        .write(move |tx| {
            let stored = tx.threads().store(&FetchedThread {
                account,
                gmail_id: GmailThreadId::from("t-old"),
                history_id: Some("2".to_owned()),
                messages: vec![
                    message("old", NEWEST - 1, "pat@example.org", &["INBOX", "UNREAD"]),
                    message("new", NEWEST, "pat@example.org", &["INBOX", "UNREAD"]),
                ],
                complete: true,
            })?;
            Ok::<_, StoreError>(stored.added)
        })
        .await;
    assert_eq!(added.len(), 2);
    let reported = BTreeSet::from([GmailMessageId::from("new")]);
    h.app
        .writer()
        .call(Priority::Sync, move |tx, state| {
            crate::notify::new_mail(tx.read(), state, account, &added, &reported, true)
        })
        .await
        .unwrap();
    eventually("a notification is posted", || {
        !h.platform.notifications.lock().is_empty()
    })
    .await;
    let posted = h.platform.notifications.lock().clone();
    assert_eq!(
        posted.len(),
        1,
        "the older message stored with it is not new mail"
    );
    assert_eq!(posted[0].body, "Subject new\nSnippet new");
}
