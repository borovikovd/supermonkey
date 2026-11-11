//! Writing what Gmail said, on the writer thread, in small sync jobs
//! (ARCHITECTURE.md, section 5.3): fetched threads, history batches, and
//! threads Gmail no longer has.

use std::collections::{BTreeSet, HashMap};

use sm_domain::{
    AccountId, GmailMessageId, GmailThreadId, LabelId, LabelSet, MessageId, Millis, SystemLabel,
    Thread, ThreadId,
};
use sm_gmail::HistoryId;
use sm_store::{FetchedThread, NewBody, NewMessage, Reads, StoreError, WriteTx};

use super::contacts::sightings;
use super::history::HistoryBatch;
use super::read::{ReadMessage, ReadThread};
use crate::drafts;
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
        store_bodies(tx, account, read.messages.iter().map(ReadMessage::brought))?;
        return Ok(Stored {
            thread: Some(thread),
            added: Vec::new(),
        });
    }
    let labels = Labels::load(reads, account)?;
    let mut unknown = false;
    // Each message moves into the thread to store. What the steps after
    // need of it stays, in the same order: its body, and whether Gmail has
    // it in Spam, before this app's changes go on top of its labels.
    let mut rest = Vec::with_capacity(read.messages.len());
    let messages = read
        .messages
        .into_iter()
        .map(|m| {
            let ReadMessage { mut message, body } = m;
            rest.push((body, message.labels.has(SystemLabel::Spam)));
            message.labels = labels.local(&message.labels, &mut unknown);
            local.apply(&message.gmail_id, &mut message.labels);
            message
        })
        .collect();
    let fetched = FetchedThread {
        account,
        gmail_id: read.gmail_id,
        history_id: read.history,
        messages,
        complete: true,
    };
    let stored = tx.threads().store(&fetched)?;
    let each = || fetched.messages.iter().zip(&rest);
    let brought = each().map(|(m, (body, _))| (&m.gmail_id, body.as_ref()));
    store_bodies(tx, account, brought)?;
    let not_spam = each().filter(|(_, (_, in_spam))| !in_spam).map(|(m, _)| m);
    count_contacts(tx, account, not_spam)?;
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
    store_bodies(tx, account, [read.brought()])?;
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

/// What one poll read, for [`apply_history`].
pub(crate) struct Polled<'a> {
    pub(crate) account: AccountId,
    /// Its history records, gathered.
    pub(crate) batch: &'a HistoryBatch,
    /// The threads read for messages the app did not know; `None` when the
    /// thread is gone.
    pub(crate) read: Vec<(GmailThreadId, Option<ReadThread>)>,
    /// Threads with messages the app did not know whose read failed: left
    /// to the fetch queue, they keep their stored history, so nothing takes
    /// them for current before that read lands.
    pub(crate) deferred: &'a [GmailThreadId],
    /// Where the account's history cursor moves.
    pub(crate) cursor: HistoryId,
    pub(crate) now: Millis,
    /// The poll's, taken before it read history.
    pub(crate) ticket: &'a ReadTicket,
}

/// One poll's history, applied in one transaction (SPEC Sync): deletes, the
/// threads read for messages the app did not know (`None` when the thread
/// is gone), then label changes for the messages it knows, with this app's
/// changes still on their way kept on top; a thread stored meanwhile by a
/// read newer than every record naming it keeps its labels. Then each
/// thread's history moves
/// up to the newest record that named it, so an older read counts as
/// older; the snooze rule runs; and the account's cursor moves. Returns the
/// messages added.
pub(crate) fn apply_history(
    tx: &mut WriteTx<'_>,
    state: &mut WriterState,
    polled: Polled<'_>,
) -> Result<Vec<MessageId>, StoreError> {
    let Polled {
        account,
        batch,
        read,
        deferred,
        cursor,
        now,
        ticket,
    } = polled;
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
    let mut refetched = BTreeSet::new();
    for (thread, answer) in read {
        match answer {
            Some(answer) => {
                let stored = store_thread(tx, state, account, answer, &local)?;
                touched.extend(stored.thread);
                added.extend(stored.added);
            }
            None => drop_thread(tx, account, &thread)?,
        }
        refetched.insert(thread);
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
        // A thread fetched while this history was on its way (an opened
        // conversation) holds newer labels than these records: keep them.
        if let Some(newest) = batch.newest.get(thread)
            && stored_history(reads, record.thread)?.is_some_and(|stored| stored > *newest)
        {
            continue;
        }
        let mut set = labels.local(gmail_labels, &mut unknown);
        local.apply(message, &mut set);
        if set != record.labels {
            touched.insert(record.thread);
            changed.push((record.id, set));
        }
    }
    tx.messages().set_labels(&changed)?;
    for (thread, newest) in batch.newest.iter().filter(|(t, _)| !deferred.contains(t)) {
        let Some(id) = tx.read().threads().by_gmail_id(account, thread)? else {
            continue;
        };
        if stored_history(tx.read(), id)?.is_none_or(|stored| stored < *newest) {
            tx.threads().set_history_id(id, *newest)?;
        }
    }
    for &thread in &touched {
        wake_if_back_in_inbox(tx, thread, &local)?;
    }
    tx.accounts().set_history(account, cursor, now)?;
    if unknown {
        state.effect(Effect::RefreshLabels(account));
    }
    Ok(added)
}

/// The stored thread's Gmail `historyId`.
fn stored_history(reads: Reads<'_>, thread: ThreadId) -> Result<Option<HistoryId>, StoreError> {
    Ok(reads.threads().row(thread)?.and_then(|row| row.history_id))
}

/// Stores the bodies a read brought, each with its message's Gmail id, for
/// messages that have none yet, and fills the drafts that quote one's
/// snippet.
fn store_bodies<'a>(
    tx: &mut WriteTx<'_>,
    account: AccountId,
    brought: impl IntoIterator<Item = (&'a GmailMessageId, Option<&'a NewBody>)>,
) -> Result<(), StoreError> {
    for (message, body) in brought {
        let Some(body) = body else {
            continue;
        };
        let Some(record) = tx.read().messages().by_gmail_id(account, message)? else {
            continue;
        };
        if !record.has_body {
            tx.messages().store_body(record.id, body)?;
            drafts::complete(tx, record.id)?;
        }
    }
    Ok(())
}

/// Counts the addresses of messages not in drafts or spam for recipient
/// autocomplete, each message once (SPEC Recipients and autocomplete).
/// `not_spam` are the read's messages Gmail does not have in Spam.
fn count_contacts<'a>(
    tx: &mut WriteTx<'_>,
    account: AccountId,
    not_spam: impl IntoIterator<Item = &'a NewMessage>,
) -> Result<(), StoreError> {
    let Some(me) = tx.read().accounts().identity(account)? else {
        return Ok(());
    };
    for m in not_spam {
        if m.is_draft {
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
