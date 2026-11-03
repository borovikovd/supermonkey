//! Gmail search and the `threads.list` of Label, Trash, and Spam views
//! (SPEC Search, Views). Pages of 15 threads at priority 2.

use crate::LogErr as _;
use std::sync::Arc;
use std::time::Duration;

use rusqlite::params;
use ub_gmail::{Format, Priority, ThreadListQuery, cost};

use super::work::{self, Work};
use super::{AccountRt, Engine};
use crate::model::{AccountId, SubId, ThreadId};
use crate::subs::Sub;
use crate::writer::{Ctx, GmailListQuery};

const PAGE: u32 = 15;

pub fn cancel(e: &Engine, sub: &str) {
    let running = e.searches.lock().remove(sub);
    if let Some(h) = running {
        h.abort();
    }
}

pub fn start(
    e: &Arc<Engine>,
    sub: SubId,
    account: AccountId,
    query: GmailListQuery,
    page: Option<String>,
) {
    let Some(rt) = e.account(account) else {
        e.writer.sync(move |ctx| {
            settle(ctx, sub, |_| {});
            Ok(())
        });
        return;
    };
    // A newer query cancels the older one's queued fetches.
    cancel(e, &sub);
    if let Some(handle) = e.spawn("search", list_page(e.clone(), rt, sub.clone(), query, page)) {
        e.searches.lock().insert(sub, handle);
    }
}

/// Lists one page, fetches metadata for hits not cached at their historyId,
/// and adds the hits to the subscription.
async fn list_page(
    e: Arc<Engine>,
    rt: Arc<AccountRt>,
    sub: SubId,
    query: GmailListQuery,
    page: Option<String>,
) {
    if matches!(query, GmailListQuery::Search(_)) && page.is_none() {
        // The Gmail query runs once typing pauses for 300 ms.
        crate::clock::sleep(Duration::from_millis(300)).await;
    }
    let q = thread_list_query(query, page);
    let res = rt
        .sched
        .run(Priority::P2, cost::THREADS_LIST, rt.gmail.list_threads(&q))
        .await;
    e.note(&rt, &res);
    let (threads, next) = res
        .map(|r| (r.threads, r.next_page_token))
        .unwrap_or_default();
    let account = rt.id;
    // A hit already cached needs no fetch; others get metadata at priority 2.
    let fetches = threads
        .iter()
        .filter(|t| !super::sync::is_current(&e, account, &t.id, t.history_id))
        .map(|t| super::sync::fetch_thread(&e, &rt, &t.id, Format::Metadata, Priority::P2));
    for r in futures_util::future::join_all(fetches).await {
        r.log_err("search hit metadata");
    }
    for t in &threads {
        if super::sync::needs_bodies(&e, account, &t.id) {
            rt.work.push(
                work::LATE_BODIES,
                Work::Body {
                    thread: t.id.clone(),
                },
            );
        }
    }
    let gids: Vec<String> = threads.into_iter().map(|t| t.id).collect();
    let s = sub.clone();
    e.writer.sync(move |ctx| {
        let ids: Vec<ThreadId> = gids
            .iter()
            .filter_map(|g| {
                ctx.tx
                    .query_row(
                        "SELECT id FROM threads WHERE account_id = ?1 AND gmail_id = ?2",
                        params![account, g],
                        |r| r.get(0),
                    )
                    .ok()
            })
            .collect();
        settle(ctx, s, |x| {
            for id in ids {
                if !x.gmail_hits.contains(&id) {
                    x.gmail_hits.push(id);
                }
            }
            x.next_page = next;
        });
        Ok(())
    });
    e.searches.lock().remove(&sub);
}

fn thread_list_query(query: GmailListQuery, page: Option<String>) -> ThreadListQuery {
    match query {
        GmailListQuery::Search(q) => ThreadListQuery {
            q: Some(q),
            page_token: page,
            max_results: Some(PAGE),
            ..Default::default()
        },
        GmailListQuery::Label {
            label_gmail_id,
            include_spam_trash,
        } => ThreadListQuery {
            label_ids: vec![label_gmail_id],
            page_token: page,
            max_results: Some(PAGE),
            include_spam_trash,
            ..Default::default()
        },
    }
}

/// Ends the subscription's search after `update` records its results.
fn settle(ctx: &mut Ctx<'_>, sub: SubId, update: impl FnOnce(&mut Sub)) {
    if let Some(x) = ctx.subs.map.get_mut(&sub) {
        update(x);
        x.searching = false;
        ctx.touched.subs.insert(sub);
    }
}
