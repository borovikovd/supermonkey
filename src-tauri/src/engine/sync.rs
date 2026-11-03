//! Connect sync, history polling, and full resync (SPEC Sync).

use crate::LogErr as _;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use rusqlite::{OptionalExtension, params};
use ub_gmail::{Format, GmailError, Outcome, Priority, ThreadListQuery, cost};

use super::ingest::{self, Incoming, PendingOps};
use super::work::{self, Work};
use super::{AccountRt, Engine};
use crate::db::to_json;
use crate::model::{AccountId, MessageId, SendAs, ThreadId};
use crate::writer::Effect;

const POLL: Duration = Duration::from_secs(45);
/// How soon to try again while offline or before the first connect succeeds.
const RETRY: Duration = Duration::from_secs(10);
const METADATA_HEADERS: &[&str] = &[
    "From",
    "To",
    "Cc",
    "Bcc",
    "Reply-To",
    "Subject",
    "Date",
    "Message-ID",
    "In-Reply-To",
    "References",
    "List-Id",
    "List-Unsubscribe",
    "X-Ubermensch-Draft",
];

/// An account's sync loop: connect if needed, then poll every 45 s, on focus,
/// and on wake from sleep.
pub async fn run(e: Arc<Engine>, rt: Arc<AccountRt>) {
    let connected: bool = e
        .reads
        .with(|c| {
            Ok(c.query_row(
                "SELECT connected FROM accounts WHERE id = ?1",
                [rt.id],
                |r| r.get(0),
            )?)
        })
        .unwrap_or(false);
    if connected {
        resume(&rt);
    } else {
        while !rt.is_stopped() {
            match connect(&e, &rt).await {
                Ok(()) => break,
                Err(err) => {
                    e.note_err(&rt, &err);
                    if matches!(err.outcome, Outcome::InvalidGrant) {
                        return;
                    }
                    rt.poll_kick.wait(RETRY).await;
                }
            }
        }
    }
    let mut last = crate::clock::now_ms();
    let mut polls = 0u32;
    while !rt.is_stopped() {
        let r = poll(&e, &rt).await;
        e.note(&rt, &r);
        if let Err(err) = &r {
            if matches!(err.outcome, Outcome::InvalidGrant) {
                return;
            }
            log::warn!("poll {}: {:?}", rt.email, err.outcome);
        }
        polls += 1;
        if polls.is_multiple_of(20) {
            refresh_send_as(&e, &rt).await.log_err("refresh_send_as");
        }
        let online = e.online.load(Ordering::Relaxed);
        rt.poll_kick.wait(if online { POLL } else { RETRY }).await;
        // A long gap in app time means the Mac slept: wake due snoozes before the
        // list shows. (Timers stand still while it sleeps; the wall clock does not.)
        let now = crate::clock::now_ms();
        if Duration::from_millis((now - last).unsigned_abs()) > POLL * 3 {
            super::snooze::wake_due(&e);
            rt.outbox_kick.kick();
        }
        last = now;
    }
}

/// The first `threads.list` page of a label or query: listed threads get
/// metadata at `meta`, then bodies at `body`.
fn first_page(label: Option<&str>, q: Option<&str>, meta: work::Slot, body: work::Slot) -> Work {
    Work::List {
        label: label.map(str::to_string),
        q: q.map(str::to_string),
        page: None,
        meta,
        body: Some(body),
        include_spam_trash: false,
    }
}

/// Lists Inbox, newest first, metadata then bodies (SPEC Sync).
fn queue_inbox(rt: &AccountRt) {
    rt.work.push(
        work::INBOX_META,
        first_page(Some("INBOX"), None, work::INBOX_META, work::INBOX_BODIES),
    );
}

/// Lists Starred and the last 90 days of Sent, behind Inbox.
fn queue_starred_and_sent(rt: &AccountRt) {
    rt.work.push(
        work::OTHER_META,
        first_page(Some("STARRED"), None, work::OTHER_META, work::LATE_BODIES),
    );
    rt.work.push(
        work::LATE_BODIES,
        first_page(
            None,
            Some("in:sent newer_than:90d"),
            work::LATE_BODIES,
            work::LATE_BODIES,
        ),
    );
}

/// After a restart: re-list Inbox (cheap; unchanged threads are skipped) and
/// finish any bodies the connect sync did not get to.
fn resume(rt: &AccountRt) {
    queue_inbox(rt);
}

pub async fn connect(e: &Engine, rt: &AccountRt) -> Result<(), GmailError> {
    e.set_status(rt.id, "syncing");
    let profile = rt
        .sched
        .run(Priority::P3, cost::GET_PROFILE, rt.gmail.get_profile())
        .await?;
    refresh_send_as(e, rt).await?;
    refresh_labels(e, rt).await?;
    let account = rt.id;
    let history = profile.history_id.to_string();
    // A retry after a failed attempt keeps that attempt's cursor: the lists
    // it already fetched predate any newer one, so polling from the newer
    // cursor would miss what changed in between.
    e.writer
        .call(move |ctx| {
            ctx.tx.execute(
                "UPDATE accounts SET history_id = coalesce(history_id, ?2), last_sync = ?3 WHERE id = ?1",
                params![account, history, ctx.now],
            )?;
            ctx.touch_summary(account);
            Ok(())
        })
        .await
        .log_err("connect");
    queue_inbox(rt);
    queue_starred_and_sent(rt);
    super::drafts::sync_list(e, rt).await?;
    rt.work.push(work::BACKFILL, Work::Backfill { page: None });
    // Only now: a launch before this point runs the connect sync again.
    e.writer
        .call(move |ctx| {
            ctx.tx
                .execute("UPDATE accounts SET connected = 1 WHERE id = ?1", [account])?;
            Ok(())
        })
        .await
        .log_err("connect");
    Ok(())
}

pub async fn refresh_send_as(e: &Engine, rt: &AccountRt) -> Result<(), GmailError> {
    let list = rt
        .sched
        .run(Priority::P3, cost::SEND_AS_LIST, rt.gmail.list_send_as())
        .await?;
    let send_as: Vec<SendAs> = list
        .into_iter()
        .map(|s| SendAs {
            email: s.send_as_email,
            name: s.display_name,
            is_default: s.is_default,
            is_primary: s.is_primary,
        })
        .collect();
    let name = send_as
        .iter()
        .find(|s| s.is_primary)
        .map(|s| s.name.clone())
        .filter(|n| !n.is_empty());
    let account = rt.id;
    e.writer.sync(move |ctx| {
        ctx.tx.execute(
            "UPDATE accounts SET send_as = ?2, display_name = coalesce(?3, display_name) WHERE id = ?1",
            params![account, to_json(&send_as), name],
        )?;
        ctx.touch_app();
        ctx.touch_summary(account);
        Ok(())
    });
    Ok(())
}

pub async fn refresh_labels(e: &Engine, rt: &AccountRt) -> Result<(), GmailError> {
    let mapped = Arc::clone(&rt.labels_mapped);
    let mapped_before = mapped.load(Ordering::SeqCst);
    let labels = rt
        .sched
        .run(Priority::P3, cost::LABELS_LIST, rt.gmail.list_labels())
        .await?;
    let account = rt.id;
    e
        .writer
        .call(move |ctx| {
            let gmail_ids: HashSet<&str> = labels.iter().map(|l| l.id.as_str()).collect();
            for l in &labels {
                let system = matches!(l.label_type, Some(ub_gmail::LabelType::System));
                // A label created here keeps its local id once mapped.
                let updated = ctx.tx.execute(
                    "UPDATE labels SET name = ?3, system = ?4 WHERE account_id = ?1 AND gmail_id = ?2",
                    params![account, l.id, l.name, system],
                )?;
                if updated == 0 {
                    ctx.tx.execute(
                        "INSERT OR IGNORE INTO labels (account_id, id, gmail_id, name, system) VALUES (?1, ?2, ?2, ?3, ?4)",
                        params![account, l.id, l.name, system],
                    )?;
                }
            }
            // Labels deleted on Gmail go too; locally created ones still waiting
            // stay. A label created here since the list was read is missing from
            // it, so then nothing goes until the next refresh.
            if mapped.load(Ordering::SeqCst) != mapped_before {
                ctx.touch_summary(account);
                return Ok(());
            }
            let local: Vec<(String, Option<String>)> = {
                let mut stmt = ctx.tx.prepare("SELECT id, gmail_id FROM labels WHERE account_id = ?1")?;
                stmt.query_map([account], |r| Ok((r.get(0)?, r.get(1)?)))?
                    .collect::<Result<_, _>>()?
            };
            for (id, gid) in local {
                if let Some(g) = gid
                    && !gmail_ids.contains(g.as_str())
                    && !crate::accounts::SYSTEM_LABELS.iter().any(|(s, _)| *s == id)
                {
                    ctx.tx
                        .execute("DELETE FROM labels WHERE account_id = ?1 AND id = ?2", params![account, id])?;
                }
            }
            ctx.touch_summary(account);
            Ok(())
        })
        .await
        .log_err("refresh_labels");
    Ok(())
}

/// Whether the cached thread is already at Gmail's `history`, so a metadata
/// fetch would change nothing.
pub fn is_current(
    e: &Engine,
    account: AccountId,
    gmail_thread: &str,
    history: Option<u64>,
) -> bool {
    history.is_some() && cached_history(e, account, gmail_thread) == history
}

/// The cached thread's Gmail historyId, if any.
fn cached_history(e: &Engine, account: AccountId, gmail_thread: &str) -> Option<u64> {
    e.reads
        .with(|c| {
            Ok(c.query_row(
                "SELECT history_id FROM threads WHERE account_id = ?1 AND gmail_id = ?2",
                params![account, gmail_thread],
                |r| r.get::<_, Option<String>>(0),
            )
            .optional()?
            .flatten())
        })
        .ok()
        .flatten()
        .and_then(|h| h.parse().ok())
}

/// Gmail ids of the thread's messages whose body is stored.
fn stored_bodies(e: &Engine, account: AccountId, gmail_thread: &str) -> HashSet<String> {
    e.reads
        .with(|c| {
            let mut stmt = c.prepare_cached(
                "SELECT m.gmail_id FROM messages m JOIN threads t ON t.id = m.thread_id
                 WHERE t.account_id = ?1 AND t.gmail_id = ?2 AND m.has_body = 1",
            )?;
            let ids = stmt.query_map(params![account, gmail_thread], |r| r.get(0))?;
            Ok(ids.collect::<Result<_, _>>()?)
        })
        .inspect_err(|err| log::warn!("stored bodies of {gmail_thread}: {err:#}"))
        .unwrap_or_default()
}

/// Whether any message of the thread lacks its body (or the thread is unknown).
pub fn needs_bodies(e: &Engine, account: AccountId, gmail_thread: &str) -> bool {
    e.reads
        .with(|c| {
            Ok(c.query_row(
                "SELECT NOT EXISTS (SELECT 1 FROM threads WHERE account_id = ?1 AND gmail_id = ?2)
                    OR EXISTS (SELECT 1 FROM messages m JOIN threads t ON t.id = m.thread_id
                               WHERE t.account_id = ?1 AND t.gmail_id = ?2 AND m.has_body = 0)",
                params![account, gmail_thread],
                |r| r.get::<_, bool>(0),
            )?)
        })
        .unwrap_or(true)
}

/// Fetches inline `cid:` images for a full message, as `data:` URIs.
async fn inline_images(
    rt: &AccountRt,
    m: &ub_gmail::Message,
    prio: Priority,
) -> Result<HashMap<String, String>, GmailError> {
    let mut out = HashMap::new();
    for (cid, att, mime) in ingest::inline_parts_to_fetch(m) {
        let bytes = rt
            .sched
            .run(
                prio,
                cost::ATTACHMENTS_GET,
                rt.gmail.get_attachment(&m.id, &att),
            )
            .await?;
        out.insert(cid, ub_mime::data_uri(&mime, &bytes));
    }
    Ok(out)
}

/// Fetches a thread and parses it, ready for `ingest::store_thread`.
/// Messages in `stored` already have their body, which never changes: they
/// are parsed without it, so their inline images are not fetched again.
pub async fn get_thread(
    rt: &AccountRt,
    gmail_thread: &str,
    format: Format,
    prio: Priority,
    stored: &HashSet<String>,
) -> Result<(Option<u64>, Vec<Incoming>), GmailError> {
    let headers: &[&str] = if format == Format::Metadata {
        METADATA_HEADERS
    } else {
        &[]
    };
    let thread = rt
        .sched
        .run(
            prio,
            cost::threads_get(format),
            rt.gmail.get_thread(gmail_thread, format, headers),
        )
        .await?;
    let full = format == Format::Full;
    let mut msgs = Vec::with_capacity(thread.messages.len());
    for m in &thread.messages {
        let body = full && !stored.contains(&m.id);
        let cids = if body {
            inline_images(rt, m, prio).await?
        } else {
            HashMap::new()
        };
        msgs.push(ingest::parse(m, body, cids));
    }
    Ok((thread.history_id, msgs))
}

/// Fetches and stores a thread. A 404 means it is gone: drop it, no toast.
pub async fn fetch_thread(
    e: &Engine,
    rt: &AccountRt,
    gmail_thread: &str,
    format: Format,
    prio: Priority,
) -> Result<Option<ThreadId>, GmailError> {
    let account = rt.id;
    let gid = gmail_thread.to_string();
    let read = rt.landed.read();
    let stored = if format == Format::Full {
        stored_bodies(e, account, gmail_thread)
    } else {
        HashSet::new()
    };
    match get_thread(rt, gmail_thread, format, prio, &stored).await {
        Ok((history, msgs)) => {
            let stored = e
                .writer
                .call(move |ctx| {
                    let pending = PendingOps::load(ctx.tx, account, read.landed())?;
                    let t =
                        ingest::store_thread(ctx, account, &gid, history, &msgs, true, &pending)?;
                    super::snooze::apply_rule(ctx, &[t], &pending)?;
                    Ok(t)
                })
                .await
                .inspect_err(|err| log::warn!("store thread {gmail_thread}: {err:#}"));
            Ok(stored.ok())
        }
        Err(err) if matches!(err.outcome, Outcome::NotFound) => {
            e.writer
                .call(move |ctx| drop_thread(ctx, account, &gid))
                .await
                .log_err("fetch_thread");
            Ok(None)
        }
        Err(err) => Err(err),
    }
}

pub fn drop_thread(
    ctx: &mut crate::writer::Ctx<'_>,
    account: AccountId,
    gmail_thread: &str,
) -> anyhow::Result<()> {
    let Some(t) = crate::db::queries::thread_by_gmail(ctx.tx, account, gmail_thread)? else {
        return Ok(());
    };
    let ids: Vec<MessageId> = {
        let mut stmt = ctx
            .tx
            .prepare("SELECT id FROM messages WHERE thread_id = ?1")?;
        stmt.query_map([t], |r| r.get(0))?
            .collect::<Result<_, _>>()?
    };
    for id in ids {
        ingest::delete_message(ctx, id)?;
    }
    crate::threads::refresh(ctx, t)
}

/// A thread the user opens, or a prefetched neighbor: both priority 2.
pub async fn fetch_opened(e: Arc<Engine>, rt: Arc<AccountRt>, thread: ThreadId) {
    let gid: Option<String> = e
        .reads
        .with(|c| {
            Ok(c.query_row(
                "SELECT gmail_id FROM threads WHERE id = ?1",
                [thread],
                |r| r.get(0),
            )
            .optional()?)
        })
        .ok()
        .flatten();
    let Some(gid) = gid else { return };
    if !needs_bodies(&e, rt.id, &gid) {
        return;
    }
    let r = fetch_thread(&e, &rt, &gid, Format::Full, Priority::P2).await;
    e.note(&rt, &r);
}

/// Every `history.list` record since `start` and the newest historyId, or
/// None when Gmail answers 404 because the cursor is too old.
async fn list_history_since(
    rt: &AccountRt,
    start: u64,
) -> Result<Option<(Vec<ub_gmail::History>, u64)>, GmailError> {
    let mut records = Vec::new();
    let mut page: Option<String> = None;
    loop {
        let res = rt
            .sched
            .run(
                Priority::P3,
                cost::HISTORY_LIST,
                rt.gmail.list_history(start, page.as_deref()),
            )
            .await;
        match res {
            Ok(res) => {
                records.extend(res.history);
                match res.next_page_token {
                    Some(next) => page = Some(next),
                    None => return Ok(Some((records, res.history_id))),
                }
            }
            Err(err) if matches!(err.outcome, Outcome::NotFound) => return Ok(None),
            Err(err) => return Err(err),
        }
    }
}

/// A poll's history, reduced: each message's latest label set (the last
/// record wins), the messages added, and the messages deleted.
struct HistoryBatch {
    /// Gmail message id → (Gmail thread id, label ids).
    label_sets: HashMap<String, (String, Vec<String>)>,
    added: HashSet<String>,
    deleted: HashSet<String>,
    /// Draft versions are new messages with DRAFT; deletes may be old versions.
    draft_change: bool,
    /// Gmail thread id → the newest history record that names it.
    newest: HashMap<String, u64>,
}

impl HistoryBatch {
    fn from_records(records: Vec<ub_gmail::History>) -> HistoryBatch {
        let mut batch = HistoryBatch {
            label_sets: HashMap::new(),
            added: HashSet::new(),
            deleted: HashSet::new(),
            draft_change: false,
            newest: HashMap::new(),
        };
        let mut newest_by_thread: HashMap<String, u64> = HashMap::new();
        for h in records {
            let mut newest = |thread: &str| {
                let n = newest_by_thread.entry(thread.to_string()).or_default();
                *n = (*n).max(h.id);
            };
            for m in h.messages_added {
                let m = m.message;
                newest(&m.thread_id);
                batch.draft_change |= m.label_ids.iter().any(|l| l == "DRAFT");
                batch.added.insert(m.id.clone());
                batch.label_sets.insert(m.id, (m.thread_id, m.label_ids));
            }
            for c in h.labels_added.into_iter().chain(h.labels_removed) {
                let m = c.message;
                newest(&m.thread_id);
                batch.label_sets.insert(m.id, (m.thread_id, m.label_ids));
            }
            for m in h.messages_deleted {
                newest(&m.message.thread_id);
                batch.deleted.insert(m.message.id);
            }
        }
        batch.draft_change |= !batch.deleted.is_empty();
        batch.newest = newest_by_thread;
        batch
    }
}

/// Writes one poll's history in a single transaction: deletes, fetched
/// threads, and label changes, with pending ops re-applied on top. Then the
/// snooze rule on net labels, the new cursor, and notifications. A thread in
/// `fetched` with no messages is gone from Gmail.
fn apply_history(
    ctx: &mut crate::writer::Ctx<'_>,
    account: AccountId,
    batch: &HistoryBatch,
    fetched: &[(String, Option<u64>, Vec<Incoming>)],
    latest: u64,
    focused: bool,
    read: &ingest::Read,
) -> anyhow::Result<()> {
    let pending = PendingOps::load(ctx.tx, account, read.landed())?;
    let refetched: HashSet<&str> = fetched.iter().map(|(gid, ..)| gid.as_str()).collect();
    let mut touched: Vec<ThreadId> = Vec::new();
    for gid in &batch.deleted {
        let row: Option<(MessageId, ThreadId)> = ctx
            .tx
            .query_row(
                "SELECT id, thread_id FROM messages WHERE account_id = ?1 AND gmail_id = ?2",
                params![account, gid],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        if let Some((id, t)) = row {
            ingest::delete_message(ctx, id)?;
            touched.push(t);
        }
    }
    for (gid, h, msgs) in fetched {
        if msgs.is_empty() {
            drop_thread(ctx, account, gid)?;
        } else {
            touched.push(ingest::store_thread(
                ctx, account, gid, *h, msgs, true, &pending,
            )?);
        }
    }
    for (msg, (thread, labels)) in &batch.label_sets {
        if refetched.contains(thread.as_str()) || batch.deleted.contains(msg) {
            continue;
        }
        if let Some(t) = ingest::set_labels_from_gmail(ctx, account, msg, labels, &pending)? {
            touched.push(t);
        }
    }
    touched.sort_unstable();
    touched.dedup();
    for &t in &touched {
        crate::threads::refresh(ctx, t)?;
    }
    for (gid, &h) in &batch.newest {
        if let Some(t) = crate::db::queries::thread_by_gmail(ctx.tx, account, gid)? {
            ingest::advance_history(ctx, t, h)?;
        }
    }
    // The snooze rule, judged on net labels after the whole batch.
    super::snooze::apply_rule(ctx, &touched, &pending)?;
    ctx.tx.execute(
        "UPDATE accounts SET history_id = ?2, last_sync = ?3 WHERE id = ?1",
        params![account, latest.to_string(), ctx.now],
    )?;
    super::notify::from_poll(ctx, account, &batch.added, focused)?;
    ctx.effect(Effect::Badge);
    Ok(())
}

/// One poll of `users.history.list` from the stored cursor.
pub async fn poll(e: &Arc<Engine>, rt: &AccountRt) -> Result<(), GmailError> {
    let start: Option<u64> = e
        .reads
        .with(|c| {
            Ok(c.query_row(
                "SELECT history_id FROM accounts WHERE id = ?1",
                [rt.id],
                |r| r.get::<_, Option<String>>(0),
            )?)
        })
        .ok()
        .flatten()
        .and_then(|h| h.parse().ok());
    let Some(start) = start else { return Ok(()) };

    let read = rt.landed.read();
    let Some((records, latest)) = list_history_since(rt, start).await? else {
        // The cursor is too old.
        return full_resync(e, rt).await;
    };
    if records.is_empty() {
        if latest != start {
            store_cursor(e, rt.id, latest).await;
        }
        return Ok(());
    }
    let batch = HistoryBatch::from_records(records);
    let draft_change = batch.draft_change;
    let known: HashSet<&str> = e
        .reads
        .with(|c| {
            let mut stmt =
                c.prepare_cached("SELECT 1 FROM messages WHERE account_id = ?1 AND gmail_id = ?2")?;
            Ok(batch
                .label_sets
                .keys()
                .map(String::as_str)
                .filter(|id| stmt.exists(params![rt.id, id]).unwrap_or(false))
                .collect())
        })
        .unwrap_or_default();
    // Threads with new or unknown messages get a metadata fetch at priority 3.
    let fetch: HashSet<String> = batch
        .label_sets
        .iter()
        .filter(|(msg, _)| !known.contains(msg.as_str()) && !batch.deleted.contains(*msg))
        .map(|(_, (thread, _))| thread.clone())
        .collect();
    // Concurrently: the scheduler runs several requests at once.
    let none = HashSet::new();
    let answers = futures_util::future::join_all(
        fetch
            .iter()
            .map(|t| get_thread(rt, t, Format::Metadata, Priority::P3, &none)),
    )
    .await;
    let mut fetched: Vec<(String, Option<u64>, Vec<Incoming>)> = Vec::new();
    for (t, answer) in fetch.iter().zip(answers) {
        match answer {
            Ok((h, msgs)) => fetched.push((t.clone(), h, msgs)),
            Err(err) if matches!(err.outcome, Outcome::NotFound) => {
                fetched.push((t.clone(), None, vec![]));
            }
            Err(err) => return Err(err),
        }
    }

    let account = rt.id;
    let eng = Arc::clone(e);
    let applied = e
        .writer
        .call(move |ctx| {
            // Read at apply time: whether the window shows the open thread now.
            let focused = eng.unfocused_since.load(Ordering::Relaxed) == 0;
            apply_history(ctx, account, &batch, &fetched, latest, focused, &read)
        })
        .await;
    match applied {
        // Bodies of new messages, at priority 4 ahead of the rest.
        Ok(()) => {
            for t in fetch {
                rt.work.push(work::POLL_BODIES, Work::Body { thread: t });
            }
        }
        Err(err) => log::warn!("apply history for {}: {err:#}", rt.email),
    }
    // Draft messages changed: reconcile drafts (SPEC Drafts).
    if draft_change {
        super::drafts::sync_list(e, rt).await?;
    }
    Ok(())
}

async fn store_cursor(e: &Engine, account: AccountId, history: u64) {
    e.writer
        .call(move |ctx| {
            ctx.tx.execute(
                "UPDATE accounts SET history_id = ?2, last_sync = ?3 WHERE id = ?1",
                params![account, history.to_string(), ctx.now],
            )?;
            Ok(())
        })
        .await
        .log_err("store_cursor");
}

/// Full resync steps 3 and 4: drop cached threads Gmail no longer has (only
/// if unchanged since the resync started and Gmail confirms with a 404), and
/// refetch metadata where the historyId changed.
async fn reconcile_cached(
    e: &Engine,
    rt: &AccountRt,
    listed: &HashMap<String, Option<u64>>,
    started_rev: i64,
) -> Result<(), GmailError> {
    let cached: Vec<(String, Option<String>, i64)> = e
        .reads
        .with(|c| {
            let mut stmt =
                c.prepare("SELECT gmail_id, history_id, rev FROM threads WHERE account_id = ?1")?;
            let rows = stmt.query_map([rt.id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
            Ok(rows.collect::<Result<_, _>>()?)
        })
        .unwrap_or_default();
    for (gid, hist, rev) in cached {
        match listed.get(&gid) {
            None if rev <= started_rev => {
                let r = rt
                    .sched
                    .run(
                        Priority::P4,
                        cost::threads_get(Format::Minimal),
                        rt.gmail.get_thread(&gid, Format::Minimal, &[]),
                    )
                    .await;
                if let Err(err) = r
                    && matches!(err.outcome, Outcome::NotFound)
                {
                    let account = rt.id;
                    e.writer
                        .call(move |ctx| drop_thread(ctx, account, &gid))
                        .await
                        .log_err("reconcile_cached");
                }
            }
            // 4. Refetch metadata, which replaces labels, where historyId changed.
            Some(h) if hist.as_deref().and_then(|x| x.parse::<u64>().ok()) != *h => {
                rt.work.push(
                    work::OTHER_META,
                    Work::Meta {
                        thread: gid,
                        history: *h,
                        body: None,
                    },
                );
            }
            _ => {}
        }
    }
    Ok(())
}

/// `history.list` returned 404: the cursor is too old (SPEC Sync, full resync).
pub async fn full_resync(e: &Engine, rt: &AccountRt) -> Result<(), GmailError> {
    log::info!("full resync for {}", rt.email);
    // 1. A fresh historyId first, so polling resumes at once.
    let profile = rt
        .sched
        .run(Priority::P3, cost::GET_PROFILE, rt.gmail.get_profile())
        .await?;
    let started_rev: i64 = e
        .reads
        .with(|c| Ok(c.query_row("SELECT value FROM meta WHERE key = 'rev'", [], |r| r.get(0))?))
        .unwrap_or(0);
    store_cursor(e, rt.id, profile.history_id).await;

    // 2. Every thread id with its historyId, Inbox first.
    let mut listed: HashMap<String, Option<u64>> = HashMap::new();
    for label in [Some("INBOX"), None] {
        let mut page = None;
        loop {
            let q = ThreadListQuery {
                q: None,
                label_ids: label.iter().map(ToString::to_string).collect(),
                page_token: page,
                max_results: Some(500),
                include_spam_trash: true,
            };
            let res = rt
                .sched
                .run(Priority::P4, cost::THREADS_LIST, rt.gmail.list_threads(&q))
                .await?;
            for t in res.threads {
                let inbox = label.is_some();
                if inbox && !listed.contains_key(&t.id) {
                    rt.work.push(
                        work::INBOX_META,
                        Work::Meta {
                            thread: t.id.clone(),
                            history: t.history_id,
                            body: Some(work::INBOX_BODIES),
                        },
                    );
                }
                listed.insert(t.id, t.history_id);
            }
            match res.next_page_token {
                Some(p) => page = Some(p),
                None => break,
            }
        }
    }

    reconcile_cached(e, rt, &listed, started_rev).await?;
    // New threads the connect sync would fetch: starred and recent sent.
    queue_starred_and_sent(rt);
    // 5. Drafts.
    super::drafts::sync_list(e, rt).await?;
    // 6. The snooze rule runs as each thread's metadata is stored.
    Ok(())
}
