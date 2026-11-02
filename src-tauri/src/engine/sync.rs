//! Connect sync, history polling, and full resync (SPEC Sync).

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use rusqlite::{OptionalExtension, params};
use ub_gmail::{Format, GmailError, Outcome, Priority, ThreadListQuery, cost};

use super::ingest::{self, Incoming, PendingOps};
use super::work::{self, Work};
use super::{AccountRt, EngineRef};
use crate::db::to_json;
use crate::model::*;
use crate::writer::Effect;

const POLL: Duration = Duration::from_secs(45);
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
pub async fn run(e: EngineRef, rt: Arc<AccountRt>) {
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
    if !connected {
        while !rt.is_stopped() {
            match connect(&e, &rt).await {
                Ok(()) => break,
                Err(err) => {
                    e.note(&rt, Err(&err));
                    if matches!(err.outcome, Outcome::InvalidGrant) {
                        return;
                    }
                    super::wait(&rt.poll_kick, 10_000).await;
                }
            }
        }
    } else {
        resume(&rt);
    }
    let mut last = Instant::now();
    let mut polls = 0u32;
    while !rt.is_stopped() {
        let r = poll(&e, &rt).await;
        e.note(&rt, r.as_ref().map(|_| ()));
        if let Err(err) = &r {
            if matches!(err.outcome, Outcome::InvalidGrant) {
                return;
            }
            log::warn!("poll {}: {:?}", rt.email, err.outcome);
        }
        polls += 1;
        if polls.is_multiple_of(20) {
            let _ = refresh_send_as(&e, &rt).await;
        }
        let offline = !e.online.load(std::sync::atomic::Ordering::Relaxed);
        super::wait(
            &rt.poll_kick,
            if offline {
                10_000
            } else {
                POLL.as_millis() as u64
            },
        )
        .await;
        // A long gap means the Mac slept: wake due snoozes before the list shows.
        if last.elapsed() > POLL * 3 {
            super::snooze::wake_due(&e);
            rt.outbox_kick.notify_one();
        }
        last = Instant::now();
    }
}

/// After a restart: re-list Inbox (cheap; unchanged threads are skipped) and
/// finish any bodies the connect sync did not get to.
fn resume(rt: &AccountRt) {
    rt.work.push(
        work::INBOX_META,
        Work::List {
            label: Some("INBOX".into()),
            q: None,
            page: None,
            meta: work::INBOX_META,
            body: Some(work::INBOX_BODIES),
            include_spam_trash: false,
        },
    );
}

pub async fn connect(e: &EngineRef, rt: &Arc<AccountRt>) -> Result<(), GmailError> {
    e.set_status(rt.id, "syncing");
    let profile = rt
        .sched
        .run(Priority::P3, cost::GET_PROFILE, rt.gmail.get_profile())
        .await?;
    refresh_send_as(e, rt).await?;
    refresh_labels(e, rt).await?;
    let account = rt.id;
    let history = profile.history_id.to_string();
    let _ = e
        .writer
        .call(move |ctx| {
            ctx.tx.execute(
                "UPDATE accounts SET history_id = ?2, connected = 1, last_sync = ?3 WHERE id = ?1",
                params![account, history, ctx.now],
            )?;
            ctx.touch_summary(account);
            Ok(())
        })
        .await;
    // Newest first, metadata then body (SPEC Sync).
    rt.work.push(
        work::INBOX_META,
        Work::List {
            label: Some("INBOX".into()),
            q: None,
            page: None,
            meta: work::INBOX_META,
            body: Some(work::INBOX_BODIES),
            include_spam_trash: false,
        },
    );
    rt.work.push(
        work::OTHER_META,
        Work::List {
            label: Some("STARRED".into()),
            q: None,
            page: None,
            meta: work::OTHER_META,
            body: Some(work::LATE_BODIES),
            include_spam_trash: false,
        },
    );
    rt.work.push(
        work::LATE_BODIES,
        Work::List {
            label: None,
            q: Some("in:sent newer_than:90d".into()),
            page: None,
            meta: work::LATE_BODIES,
            body: Some(work::LATE_BODIES),
            include_spam_trash: false,
        },
    );
    super::drafts::sync_list(e, rt).await?;
    rt.work.push(work::BACKFILL, Work::Backfill { page: None });
    Ok(())
}

pub async fn refresh_send_as(e: &EngineRef, rt: &Arc<AccountRt>) -> Result<(), GmailError> {
    let list = rt
        .sched
        .run(Priority::P3, cost::SEND_AS_LIST, rt.gmail.list_send_as())
        .await?;
    let send_as: Vec<SendAs> = list
        .iter()
        .map(|s| SendAs {
            email: s.send_as_email.clone(),
            name: s.display_name.clone(),
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

pub async fn refresh_labels(e: &EngineRef, rt: &Arc<AccountRt>) -> Result<(), GmailError> {
    let labels = rt
        .sched
        .run(Priority::P3, cost::LABELS_LIST, rt.gmail.list_labels())
        .await?;
    let account = rt.id;
    let _ = e
        .writer
        .call(move |ctx| {
            let gmail_ids: HashSet<String> = labels.iter().map(|l| l.id.clone()).collect();
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
            // Labels deleted on Gmail go too; locally created ones still waiting stay.
            let local: Vec<(String, Option<String>)> = {
                let mut stmt = ctx.tx.prepare("SELECT id, gmail_id FROM labels WHERE account_id = ?1")?;
                stmt.query_map([account], |r| Ok((r.get(0)?, r.get(1)?)))?
                    .collect::<Result<_, _>>()?
            };
            for (id, gid) in local {
                if let Some(g) = gid
                    && !gmail_ids.contains(&g)
                    && !crate::accounts::SYSTEM_LABELS.iter().any(|(s, _)| *s == id)
                {
                    ctx.tx
                        .execute("DELETE FROM labels WHERE account_id = ?1 AND id = ?2", params![account, id])?;
                }
            }
            ctx.touch_summary(account);
            Ok(())
        })
        .await;
    Ok(())
}

/// The cached thread's Gmail historyId, if any.
pub fn cached_history(e: &EngineRef, account: AccountId, gmail_thread: &str) -> Option<u64> {
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

/// Whether any message of the thread lacks its body (or the thread is unknown).
pub fn needs_bodies(e: &EngineRef, account: AccountId, gmail_thread: &str) -> bool {
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
pub async fn get_thread(
    rt: &AccountRt,
    gmail_thread: &str,
    format: Format,
    prio: Priority,
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
        let cids = if full {
            inline_images(rt, m, prio).await?
        } else {
            HashMap::new()
        };
        msgs.push(ingest::parse(m, full, &cids));
    }
    Ok((thread.history_id, msgs))
}

/// Fetches and stores a thread. A 404 means it is gone: drop it, no toast.
pub async fn fetch_thread(
    e: &EngineRef,
    rt: &Arc<AccountRt>,
    gmail_thread: &str,
    format: Format,
    prio: Priority,
) -> Result<Option<ThreadId>, GmailError> {
    let account = rt.id;
    let gid = gmail_thread.to_string();
    match get_thread(rt, gmail_thread, format, prio).await {
        Ok((history, msgs)) => {
            let r = e
                .writer
                .call(move |ctx| {
                    let pending = PendingOps::load(ctx.tx, account)?;
                    let t =
                        ingest::store_thread(ctx, account, &gid, history, &msgs, true, &pending)?;
                    super::snooze::apply_rule(ctx, &[t], &pending)?;
                    Ok(t)
                })
                .await;
            Ok(r.ok())
        }
        Err(err) if matches!(err.outcome, Outcome::NotFound) => {
            let _ = e
                .writer
                .call(move |ctx| drop_thread(ctx, account, &gid))
                .await;
            Ok(None)
        }
        Err(err) => Err(err),
    }
}

pub fn drop_thread(
    ctx: &mut crate::writer::Ctx,
    account: AccountId,
    gmail_thread: &str,
) -> anyhow::Result<()> {
    let Some(t) = crate::db::queries::thread_by_gmail(ctx.tx, account, gmail_thread)? else {
        return Ok(());
    };
    let ids: Vec<i64> = {
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

/// A thread the user opens (priority 2), or a prefetched neighbor.
pub async fn fetch_opened(e: EngineRef, rt: Arc<AccountRt>, thread: ThreadId, _open: bool) {
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
    e.note(&rt, r.as_ref().map(|_| ()));
}

/// One poll of `users.history.list` from the stored cursor.
pub async fn poll(e: &EngineRef, rt: &Arc<AccountRt>) -> Result<(), GmailError> {
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

    let mut records = Vec::new();
    let mut page: Option<String> = None;
    let latest;
    loop {
        match rt
            .sched
            .run(
                Priority::P3,
                cost::HISTORY_LIST,
                rt.gmail.list_history(start, page.as_deref()),
            )
            .await
        {
            Ok(res) => {
                records.extend(res.history);
                if res.next_page_token.is_none() {
                    latest = res.history_id;
                    break;
                }
                page = res.next_page_token;
            }
            Err(err) if matches!(err.outcome, Outcome::NotFound) => {
                return full_resync(e, rt).await;
            }
            Err(err) => return Err(err),
        }
    }
    if records.is_empty() {
        if latest != start {
            store_cursor(e, rt.id, latest).await;
        }
        return Ok(());
    }

    // Net label sets per message (the last record wins), adds, and deletes.
    let mut label_sets: HashMap<String, (String, Vec<String>)> = HashMap::new();
    let mut added: Vec<ub_gmail::Message> = Vec::new();
    let mut deleted: HashSet<String> = HashSet::new();
    for h in &records {
        for m in &h.messages_added {
            added.push(m.message.clone());
            label_sets.insert(
                m.message.id.clone(),
                (m.message.thread_id.clone(), m.message.label_ids.clone()),
            );
        }
        for c in h.labels_added.iter().chain(&h.labels_removed) {
            label_sets.insert(
                c.message.id.clone(),
                (c.message.thread_id.clone(), c.message.label_ids.clone()),
            );
        }
        for m in &h.messages_deleted {
            deleted.insert(m.message.id.clone());
        }
    }
    let known: HashSet<String> = {
        let ids: Vec<String> = label_sets.keys().cloned().collect();
        e.reads
            .with(|c| {
                let mut stmt = c.prepare_cached(
                    "SELECT 1 FROM messages WHERE account_id = ?1 AND gmail_id = ?2",
                )?;
                Ok(ids
                    .into_iter()
                    .filter(|id| stmt.exists(params![rt.id, id]).unwrap_or(false))
                    .collect())
            })
            .unwrap_or_default()
    };
    // Threads with new or unknown messages get a metadata fetch at priority 3.
    let mut fetch: HashSet<String> = HashSet::new();
    for (msg, (thread, _)) in &label_sets {
        if !known.contains(msg) && !deleted.contains(msg) {
            fetch.insert(thread.clone());
        }
    }
    let mut fetched: Vec<(String, Option<u64>, Vec<Incoming>)> = Vec::new();
    for t in &fetch {
        match get_thread(rt, t, Format::Metadata, Priority::P3).await {
            Ok((h, msgs)) => fetched.push((t.clone(), h, msgs)),
            Err(err) if matches!(err.outcome, Outcome::NotFound) => {
                fetched.push((t.clone(), None, vec![]))
            }
            Err(err) => return Err(err),
        }
    }

    let account = rt.id;
    let new_ids: HashSet<String> = added.iter().map(|m| m.id.clone()).collect();
    let fetch_threads: HashSet<String> = fetch.clone();
    let res = e
        .writer
        .call(move |ctx| {
            let pending = PendingOps::load(ctx.tx, account)?;
            let mut touched: Vec<ThreadId> = Vec::new();
            for gid in &deleted {
                let row: Option<(i64, i64)> = ctx
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
            for (gid, h, msgs) in &fetched {
                if msgs.is_empty() {
                    drop_thread(ctx, account, gid)?;
                } else {
                    touched.push(ingest::store_thread(ctx, account, gid, *h, msgs, true, &pending)?);
                }
            }
            for (msg, (thread, labels)) in &label_sets {
                if fetch_threads.contains(thread) || deleted.contains(msg) {
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
            // The snooze rule, judged on net labels after the whole batch.
            super::snooze::apply_rule(ctx, &touched, &pending)?;
            ctx.tx.execute(
                "UPDATE accounts SET history_id = ?2, last_sync = ?3 WHERE id = ?1",
                params![account, latest.to_string(), ctx.now],
            )?;
            super::notify::from_poll(ctx, account, &new_ids)?;
            ctx.effect(Effect::Badge);
            Ok(fetch_threads)
        })
        .await;
    // Bodies of new messages, at priority 4 ahead of the rest.
    if let Ok(threads) = res {
        for t in threads {
            rt.work.push(work::POLL_BODIES, Work::Body { thread: t });
        }
    }
    Ok(())
}

async fn store_cursor(e: &EngineRef, account: AccountId, history: u64) {
    let _ = e
        .writer
        .call(move |ctx| {
            ctx.tx.execute(
                "UPDATE accounts SET history_id = ?2, last_sync = ?3 WHERE id = ?1",
                params![account, history.to_string(), ctx.now],
            )?;
            Ok(())
        })
        .await;
}

/// `history.list` returned 404: the cursor is too old (SPEC Sync, full resync).
pub async fn full_resync(e: &EngineRef, rt: &Arc<AccountRt>) -> Result<(), GmailError> {
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
                label_ids: label.iter().map(|s| s.to_string()).collect(),
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

    // 3. Cached threads missing from the list are dropped only if unchanged
    // since step 1 and Gmail confirms with a 404.
    let cached: Vec<(String, Option<String>, i64)> = e
        .reads
        .with(|c| {
            let mut stmt =
                c.prepare("SELECT gmail_id, history_id, rev FROM threads WHERE account_id = ?1")?;
            let rows = stmt.query_map([rt.id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
            Ok(rows.collect::<Result<_, _>>()?)
        })
        .unwrap_or_default();
    for (gid, hist, rev) in &cached {
        match listed.get(gid) {
            None if *rev <= started_rev => {
                let r = rt
                    .sched
                    .run(
                        Priority::P4,
                        cost::threads_get(Format::Minimal),
                        rt.gmail.get_thread(gid, Format::Minimal, &[]),
                    )
                    .await;
                if let Err(err) = r
                    && matches!(err.outcome, Outcome::NotFound)
                {
                    let account = rt.id;
                    let gid = gid.clone();
                    let _ = e
                        .writer
                        .call(move |ctx| drop_thread(ctx, account, &gid))
                        .await;
                }
            }
            // 4. Refetch metadata, which replaces labels, where historyId changed.
            Some(h) if hist.as_deref().and_then(|x| x.parse::<u64>().ok()) != *h => {
                rt.work.push(
                    work::OTHER_META,
                    Work::Meta {
                        thread: gid.clone(),
                        history: *h,
                        body: None,
                    },
                );
            }
            _ => {}
        }
    }
    // New threads the connect sync would fetch: starred and recent sent.
    rt.work.push(
        work::OTHER_META,
        Work::List {
            label: Some("STARRED".into()),
            q: None,
            page: None,
            meta: work::OTHER_META,
            body: Some(work::LATE_BODIES),
            include_spam_trash: false,
        },
    );
    rt.work.push(
        work::LATE_BODIES,
        Work::List {
            label: None,
            q: Some("in:sent newer_than:90d".into()),
            page: None,
            meta: work::LATE_BODIES,
            body: Some(work::LATE_BODIES),
            include_spam_trash: false,
        },
    );
    // 5. Drafts.
    super::drafts::sync_list(e, rt).await?;
    // 6. The snooze rule runs as each thread's metadata is stored.
    Ok(())
}
