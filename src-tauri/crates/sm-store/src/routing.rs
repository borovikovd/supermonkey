use std::collections::HashMap;
use std::collections::hash_map::Entry;

use rusqlite::named_params;
use sm_domain::{AccountId, Identity, LabelId, Router, SplitId, Thread, ThreadDigest, ThreadId};

use crate::StoreError;
use crate::changes::Table;
use crate::tx::{Reads, WriteTx, require};

pub(crate) struct Routing {
    router: Router,
    identity: Identity,
    user_labels: HashMap<LabelId, String>,
}

impl Routing {
    fn load(reads: Reads<'_>, account: AccountId) -> Result<Self, StoreError> {
        let identity = require(reads.accounts().identity(account)?, "account", &account)?;
        Ok(Self {
            router: reads.splits().router(account)?,
            identity,
            user_labels: reads.labels().user_label_names(account)?,
        })
    }

    fn place(&self, thread: &Thread, digest: &ThreadDigest<'_>) -> Option<SplitId> {
        let names: Vec<&str> = digest
            .labels
            .iter()
            .filter_map(|l| self.user_labels.get(l).map(String::as_str))
            .collect();
        self.router.place(thread, digest, &self.identity, &names)
    }
}

fn split_for(
    tx: &mut WriteTx<'_>,
    account: AccountId,
    thread: &Thread,
    digest: &ThreadDigest<'_>,
) -> Result<Option<SplitId>, StoreError> {
    if !digest.in_inbox() {
        return Ok(None);
    }
    let reads = tx.read();
    let routing = match tx.routing_cache().entry(account) {
        Entry::Occupied(e) => e.into_mut(),
        Entry::Vacant(e) => e.insert(Routing::load(reads, account)?),
    };
    Ok(routing.place(thread, digest))
}

pub(crate) fn derive_thread(tx: &mut WriteTx<'_>, thread: ThreadId) -> Result<(), StoreError> {
    let reads = tx.read();
    let Some(account) = reads.threads().account_of(thread)? else {
        return Ok(());
    };
    let messages = Thread::new(reads.messages().facts(thread)?);
    let Some(digest) = messages.digest() else {
        return delete_thread_row(tx, account, thread);
    };
    let split = split_for(tx, account, &messages, &digest)?;
    let rev = tx.stamp();
    let db = tx.db();
    db.execute(
        "store a thread's derived row",
        "UPDATE threads SET subject = :subject, snippet = :snippet, sender = :sender,
             sender_email = :sender_email, sort_time = :sort_time, message_count = :count,
             unread = :unread, has_attachments = :attachments, has_live = :live,
             in_inbox = :inbox, split_id = :split, rev = :rev
         WHERE id = :id",
        named_params! {
            ":id": thread,
            ":subject": digest.subject,
            ":snippet": digest.snippet,
            ":sender": digest.sender,
            ":sender_email": digest.sender_email,
            ":sort_time": digest.sort_time,
            ":count": digest.message_count,
            ":unread": digest.unread,
            ":attachments": digest.has_attachments,
            ":live": digest.has_live,
            ":inbox": digest.in_inbox(),
            ":split": split,
            ":rev": rev,
        },
    )?;
    db.execute(
        "clear a thread's labels",
        "DELETE FROM thread_labels WHERE thread_id = ?1",
        [thread],
    )?;
    for label in &digest.labels {
        db.execute(
            "store a thread's label",
            "INSERT INTO thread_labels (thread_id, account_id, label_id) VALUES (?1, ?2, ?3)",
            (thread, account, label),
        )?;
    }
    tx.record().thread(account, thread);
    Ok(())
}

pub(crate) fn delete_thread_row(
    tx: &mut WriteTx<'_>,
    account: AccountId,
    thread: ThreadId,
) -> Result<(), StoreError> {
    let rev = tx.stamp();
    let db = tx.db();
    let drafts = db.all(
        "find a thread's drafts",
        "SELECT id FROM drafts WHERE thread_id = ?1",
        [thread],
        |r| r.get("id"),
    )?;
    db.execute(
        "unlink a thread's drafts",
        "UPDATE drafts SET thread_id = NULL, rev = ?2 WHERE thread_id = ?1",
        (thread, rev),
    )?;
    let outbox = db.execute(
        "unlink a thread's outbox rows",
        "UPDATE outbox SET thread_id = NULL, rev = ?2 WHERE thread_id = ?1",
        (thread, rev),
    )?;
    db.execute(
        "delete a thread",
        "DELETE FROM threads WHERE id = ?1",
        [thread],
    )?;
    let changes = tx.record();
    changes.thread(account, thread);
    for draft in drafts {
        changes.draft(account, draft);
    }
    if outbox > 0 {
        changes.touch(account, Table::Outbox);
    }
    Ok(())
}

pub(crate) fn reroute_inbox(tx: &mut WriteTx<'_>, account: AccountId) -> Result<(), StoreError> {
    tx.forget_routing(account);
    let placed = tx.read().threads().inbox_splits(account)?;
    for (thread, old) in placed {
        let messages = Thread::new(tx.read().messages().facts(thread)?);
        let Some(digest) = messages.digest() else {
            continue;
        };
        let new = split_for(tx, account, &messages, &digest)?;
        if new != old {
            let rev = tx.stamp();
            tx.db().execute(
                "move a thread to another split",
                "UPDATE threads SET split_id = ?2, rev = ?3 WHERE id = ?1",
                (thread, new, rev),
            )?;
            tx.record().thread(account, thread);
        }
    }
    Ok(())
}
