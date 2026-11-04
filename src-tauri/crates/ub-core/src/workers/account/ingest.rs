//! Writing what Gmail said, on the writer thread, in small sync jobs
//! (ARCHITECTURE.md, section 5.3): fetched threads, history batches, and
//! threads Gmail no longer has.

use std::collections::{BTreeSet, HashMap};

use ub_domain::{
    AccountId, GmailThreadId, LabelId, LabelSet, MessageId, Millis, SystemLabel, Thread, ThreadId,
};
use ub_gmail::HistoryId;
use ub_store::{FetchedThread, Reads, StoreError, WriteTx};

use super::contacts::sightings;
use super::history::HistoryBatch;
use super::read::{ReadMessage, ReadThread};
use crate::effects::Effect;
use crate::ops::{LocalChanges, ReadTicket};
use crate::writer::WriterState;

/// What storing a thread did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Stored {
    /// The thread, or `None` when it is gone.
    pub(crate) thread: Option<ThreadId>,
    /// Messages the app did not have before.
    pub(crate) added: Vec<MessageId>,
}

/// Stores a thread read from Gmail (SPEC Sync): its messages replace what is
/// stored, labels included, with this app's changes still on their way
/// (`local`) kept on top, and messages it no longer has go. An answer older
/// than the stored thread (a slow read overtaken by a poll) keeps only the
/// bodies it brought, which never change.
pub(crate) fn store_thread(
    tx: &mut WriteTx<'_>,
    state: &mut WriterState,
    account: AccountId,
    read: ReadThread,
    local: &LocalChanges,
) -> Result<Stored, StoreError> {
    let reads = tx.read();
    let cached = reads.threads().by_gmail_id(account, &read.gmail_id)?;
    if let (Some(thread), Some(answer)) = (cached, read.history)
        && stored_history(reads, thread)?.is_some_and(|stored| answer < stored)
    {
        tracing::debug!(
            account = account.0,
            "a thread read was overtaken; keeping its bodies only"
        );
        store_bodies(tx, account, &read.messages)?;
        return Ok(Stored {
            thread: Some(thread),
            added: Vec::new(),
        });
    }
    let labels = Labels::load(reads, account)?;
    let mut unknown = false;
    let messages = read
        .messages
        .iter()
        .map(|m| {
            let mut message = m.message.clone();
            message.labels = labels.local(&message.labels, &mut unknown);
            local.apply(&message.gmail_id, &mut message.labels);
            message
        })
        .collect();
    let stored = tx.threads().store(&FetchedThread {
        account,
        gmail_id: read.gmail_id,
        history_id: read.history.map(|h| h.to_string()),
        messages,
        complete: true,
    })?;
    store_bodies(tx, account, &read.messages)?;
    count_contacts(tx, account, &read.messages)?;
    if let Some(thread) = stored.thread {
        wake_if_back_in_inbox(tx, thread, local)?;
    }
    if unknown {
        state.effect(Effect::RefreshLabels(account));
    }
    Ok(Stored {
        thread: stored.thread,
        added: stored.added,
    })
}

/// Stores the message of a Gmail draft read with `drafts.get` into its
/// thread, adding the thread if new, and leaves the thread's other messages
/// as they are. Its labels need no local changes on top: no action changes
/// a draft's. Returns the thread, or `None` when it is gone.
pub(crate) fn store_draft_message(
    tx: &mut WriteTx<'_>,
    state: &mut WriterState,
    account: AccountId,
    gmail_thread: &GmailThreadId,
    read: &ReadMessage,
) -> Result<Option<ThreadId>, StoreError> {
    let labels = Labels::load(tx.read(), account)?;
    let mut unknown = false;
    let mut message = read.message.clone();
    message.labels = labels.local(&message.labels, &mut unknown);
    let stored = tx.threads().store(&FetchedThread {
        account,
        gmail_id: gmail_thread.clone(),
        history_id: None,
        messages: vec![message],
        complete: false,
    })?;
    store_bodies(tx, account, std::slice::from_ref(read))?;
    if unknown {
        state.effect(Effect::RefreshLabels(account));
    }
    Ok(stored.thread)
}

/// Drops a thread Gmail no longer has: a 404 on a read of it.
pub(crate) fn drop_thread(
    tx: &mut WriteTx<'_>,
    account: AccountId,
    thread: &GmailThreadId,
) -> Result<(), StoreError> {
    if let Some(id) = tx.read().threads().by_gmail_id(account, thread)? {
        tx.threads().delete(id)?;
    }
    Ok(())
}

/// One poll's history, applied in one transaction (SPEC Sync): deletes, the
/// threads read for messages the app did not know (`None` when the thread
/// is gone), then label changes for the messages it knows, with this app's
/// changes still on their way kept on top. Then each thread's history moves
/// up to the newest record that named it, so an older read counts as
/// older; the snooze rule runs; and the cursor moves to `cursor`. `ticket`
/// is the poll's, taken before it read history. Returns the messages added.
#[expect(
    clippy::too_many_arguments,
    reason = "one poll's parts, gathered by the poller"
)]
pub(crate) fn apply_history(
    tx: &mut WriteTx<'_>,
    state: &mut WriterState,
    account: AccountId,
    batch: &HistoryBatch,
    read: Vec<(GmailThreadId, Option<ReadThread>)>,
    cursor: HistoryId,
    now: Millis,
    ticket: &ReadTicket,
) -> Result<Vec<MessageId>, StoreError> {
    let local = LocalChanges::load(tx.read(), account, ticket.landed())?;
    let mut touched = BTreeSet::new();
    let mut gone = Vec::new();
    for message in &batch.deleted {
        if let Some(record) = tx.read().messages().by_gmail_id(account, message)? {
            touched.insert(record.thread);
            gone.push(record.id);
        }
    }
    tx.messages().delete(&gone)?;
    let mut added = Vec::new();
    let refetched: BTreeSet<GmailThreadId> = read.iter().map(|(id, _)| id.clone()).collect();
    for (thread, answer) in read {
        match answer {
            Some(answer) => {
                let stored = store_thread(tx, state, account, answer, &local)?;
                touched.extend(stored.thread);
                added.extend(stored.added);
            }
            None => drop_thread(tx, account, &thread)?,
        }
    }
    let reads = tx.read();
    let labels = Labels::load(reads, account)?;
    let mut unknown = false;
    let mut changed = Vec::new();
    for (message, (thread, gmail_labels)) in &batch.labels {
        if refetched.contains(thread) || batch.deleted.contains(message) {
            continue;
        }
        let Some(record) = reads.messages().by_gmail_id(account, message)? else {
            continue;
        };
        let mut set = labels.local(gmail_labels, &mut unknown);
        local.apply(message, &mut set);
        if set != record.labels {
            touched.insert(record.thread);
            changed.push((record.id, set));
        }
    }
    tx.messages().set_labels(&changed)?;
    for (thread, newest) in &batch.newest {
        let Some(id) = tx.read().threads().by_gmail_id(account, thread)? else {
            continue;
        };
        if stored_history(tx.read(), id)?.is_none_or(|stored| stored < *newest) {
            tx.threads().set_history_id(id, &newest.to_string())?;
        }
    }
    for &thread in &touched {
        wake_if_back_in_inbox(tx, thread, &local)?;
    }
    tx.accounts()
        .set_history(account, &cursor.to_string(), now)?;
    if unknown {
        state.effect(Effect::RefreshLabels(account));
    }
    Ok(added)
}

/// The stored thread's Gmail `historyId`.
fn stored_history(reads: Reads<'_>, thread: ThreadId) -> Result<Option<HistoryId>, StoreError> {
    Ok(reads
        .threads()
        .row(thread)?
        .and_then(|row| row.history_id)
        .and_then(|h| h.parse().ok())
        .map(HistoryId))
}

/// Stores the bodies a read brought for messages that have none yet.
fn store_bodies(
    tx: &mut WriteTx<'_>,
    account: AccountId,
    messages: &[ReadMessage],
) -> Result<(), StoreError> {
    for m in messages {
        let Some(body) = &m.body else {
            continue;
        };
        let Some(record) = tx
            .read()
            .messages()
            .by_gmail_id(account, &m.message.gmail_id)?
        else {
            continue;
        };
        if !record.has_body {
            tx.messages().store_body(record.id, body)?;
        }
    }
    Ok(())
}

/// Counts the addresses of messages not in drafts or spam for recipient
/// autocomplete, each message once (SPEC Recipients and autocomplete).
fn count_contacts(
    tx: &mut WriteTx<'_>,
    account: AccountId,
    messages: &[ReadMessage],
) -> Result<(), StoreError> {
    let Some(me) = tx.read().accounts().identity(account)? else {
        return Ok(());
    };
    for m in messages {
        let m = &m.message;
        if m.is_draft || m.labels.has(SystemLabel::Spam) {
            continue;
        }
        if tx.read().contacts().is_counted(account, &m.gmail_id)? {
            continue;
        }
        tx.contacts()
            .count(account, &m.gmail_id, &sightings(m, &me))?;
    }
    Ok(())
}

/// The snooze rule (SPEC Snooze, Sync): a snoozed thread that has gained
/// `INBOX` from outside this app loses its snooze.
fn wake_if_back_in_inbox(
    tx: &mut WriteTx<'_>,
    thread: ThreadId,
    local: &LocalChanges,
) -> Result<(), StoreError> {
    let Some(row) = tx.read().threads().row(thread)? else {
        return Ok(());
    };
    if row.snooze_until.is_none() {
        return Ok(());
    }
    let facts = Thread::new(tx.read().messages().facts(thread)?);
    if facts.gained_inbox(|m| local.touches(&m.gmail_id)) {
        tx.threads().set_snooze(thread, None)?;
    }
    Ok(())
}

/// The account's label table, to map Gmail's label ids to local ones.
struct Labels(HashMap<LabelId, LabelId>);

impl Labels {
    fn load(reads: Reads<'_>, account: AccountId) -> Result<Self, StoreError> {
        Ok(Self(reads.labels().local_ids(account)?))
    }

    /// Gmail's labels as local ids. An id the table lacks is kept as it is,
    /// and sets `unknown`, so the label list is read again.
    fn local(&self, gmail: &LabelSet, unknown: &mut bool) -> LabelSet {
        gmail
            .iter()
            .map(|id| {
                self.0.get(id).cloned().unwrap_or_else(|| {
                    *unknown = true;
                    id.clone()
                })
            })
            .collect()
    }
}
