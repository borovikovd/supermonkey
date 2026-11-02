//! Gmail search and the `threads.list` of Label, Trash, and Spam views
//! (SPEC Search, Views). Pages of 15 threads at priority 2.

use std::time::Duration;

use rusqlite::params;
use ub_gmail::{Format, Priority, ThreadListQuery, cost};

use super::EngineRef;
use super::work::{self, Work};
use crate::model::*;
use crate::writer::GmailListQuery;

const PAGE: u32 = 15;

pub fn cancel(e: &EngineRef, sub: &str) {
    if let Some(h) = e.searches.lock().remove(sub) {
        h.abort();
    }
}

pub fn start(
    e: EngineRef,
    sub: SubId,
    account: AccountId,
    query: GmailListQuery,
    page: Option<String>,
) {
    let Some(rt) = e.account(account) else {
        let s = sub.clone();
        e.writer.sync(move |ctx| {
            if let Some(x) = ctx.subs.map.get_mut(&s) {
                x.searching = false;
                ctx.touched.subs.insert(s);
            }
            Ok(())
        });
        return;
    };
    // A newer query cancels the older one's queued fetches.
    cancel(&e, &sub);
    let e2 = e.clone();
    let s = sub.clone();
    let handle = tauri::async_runtime::spawn(async move {
        if matches!(query, GmailListQuery::Search(_)) && page.is_none() {
            // The Gmail query runs once typing pauses for 300 ms.
            tokio::time::sleep(Duration::from_millis(300)).await;
        }
        let q = match &query {
            GmailListQuery::Search(q) => ThreadListQuery {
                q: Some(q.clone()),
                page_token: page,
                max_results: Some(PAGE),
                ..Default::default()
            },
            GmailListQuery::Label {
                label_gmail_id,
                include_spam_trash,
            } => ThreadListQuery {
                label_ids: vec![label_gmail_id.clone()],
                page_token: page,
                max_results: Some(PAGE),
                include_spam_trash: *include_spam_trash,
                ..Default::default()
            },
        };
        let res = rt
            .sched
            .run(Priority::P2, cost::THREADS_LIST, rt.gmail.list_threads(&q))
            .await;
        e2.note(&rt, res.as_ref().map(|_| ()));
        let (threads, next) = match res {
            Ok(r) => (r.threads, r.next_page_token),
            Err(_) => (vec![], None),
        };
        // A hit already cached needs no fetch; others get metadata at priority 2.
        let fetches = threads.iter().filter(|t| {
            let cached = super::sync::cached_history(&e2, account, &t.id);
            cached.is_none() || cached != t.history_id
        });
        let futs = fetches
            .map(|t| super::sync::fetch_thread(&e2, &rt, &t.id, Format::Metadata, Priority::P2));
        let _ = futures_util::future::join_all(futs).await;
        for t in &threads {
            if super::sync::needs_bodies(&e2, account, &t.id) {
                rt.work.push(
                    work::LATE_BODIES,
                    Work::Body {
                        thread: t.id.clone(),
                    },
                );
            }
        }
        let gids: Vec<String> = threads.into_iter().map(|t| t.id).collect();
        let sub2 = s.clone();
        e2.writer.sync(move |ctx| {
            let mut ids = Vec::new();
            for g in &gids {
                if let Ok(id) = ctx.tx.query_row(
                    "SELECT id FROM threads WHERE account_id = ?1 AND gmail_id = ?2",
                    params![account, g],
                    |r| r.get::<_, i64>(0),
                ) {
                    ids.push(id);
                }
            }
            if let Some(x) = ctx.subs.map.get_mut(&sub2) {
                for id in ids {
                    if !x.gmail_hits.contains(&id) {
                        x.gmail_hits.push(id);
                    }
                }
                x.next_page = next;
                x.searching = false;
                ctx.touched.subs.insert(sub2);
            }
            Ok(())
        });
        e2.searches.lock().remove(&s);
    });
    e.searches.lock().insert(sub, handle);
}
