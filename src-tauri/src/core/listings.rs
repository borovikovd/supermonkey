use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

use crate::domain::query::{Query, gmail_query};
use crate::domain::{
    AccountId, GmailListing, GmailThreadId, PageToken, SubId, SystemLabel, ThreadId,
};
use crate::gmail::{
    Backoff, Format, Gmail, GmailError, Outcome, Priority as GmailPriority, ThreadQuery,
};
use crate::ipc::SubKey;
use crate::store::{Priority as StorePriority, ReadPool, StoreError, Writer};

use super::network::Network;
use super::runtime::{Activity, Clock, Tasks};
use super::subs::ListingState;
use super::sync::AccountManager;
use super::sync::ingest::fetch_and_ingest_thread;
use super::writer_state::{SubOp, WriterState};

const LISTING_PAGE_SIZE: u32 = 15;
const SEARCH_DEBOUNCE: Duration = Duration::from_millis(300);

struct SubListing {
    account: AccountId,
    cancel: CancellationToken,
    more: Arc<Notify>,
}

#[derive(Clone)]
pub(crate) struct ListingManager {
    inner: Arc<ListingInner>,
}

struct ListingInner {
    accounts: AccountManager,
    pool: Arc<ReadPool>,
    writer: Writer<WriterState>,
    clock: Clock,
    network: Network,
    activity: Activity,
    tasks: Tasks,
    subs: Mutex<HashMap<SubId, SubListing>>,
}

impl std::fmt::Debug for ListingManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ListingManager").finish_non_exhaustive()
    }
}

impl ListingManager {
    #[expect(
        clippy::too_many_arguments,
        reason = "listing manager coordinates Gmail pagination, storage writes, clock debounce, and network state"
    )]
    #[must_use]
    pub(crate) fn new(
        accounts: AccountManager,
        pool: Arc<ReadPool>,
        writer: Writer<WriterState>,
        clock: Clock,
        network: Network,
        activity: Activity,
        tasks: Tasks,
    ) -> Self {
        Self {
            inner: Arc::new(ListingInner {
                accounts,
                pool,
                writer,
                clock,
                network,
                activity,
                tasks,
                subs: Mutex::new(HashMap::new()),
            }),
        }
    }

    pub(crate) fn subscribe(&self, sub: SubId, account: AccountId, key: SubKey) {
        self.unsubscribe(&sub);
        let cancel = self.inner.tasks.token().child_token();
        let more = Arc::new(Notify::new());
        self.inner.subs.lock().insert(
            sub.clone(),
            SubListing {
                account,
                cancel: cancel.clone(),
                more: Arc::clone(&more),
            },
        );
        let inner = Arc::clone(&self.inner);
        self.inner.tasks.spawn(async move {
            run_listing(inner, sub, account, key, more, cancel).await;
        });
    }

    pub(crate) fn more_results(&self, sub: &SubId) {
        if let Some(entry) = self.inner.subs.lock().get(sub) {
            entry.more.notify_one();
        }
    }

    pub(crate) fn unsubscribe(&self, sub: &SubId) {
        let entry = self.inner.subs.lock().remove(sub);
        if let Some(entry) = entry {
            entry.cancel.cancel();
        }
    }

    pub(crate) fn cancel_account(&self, account: AccountId) {
        self.inner.subs.lock().retain(|_, entry| {
            if entry.account == account {
                entry.cancel.cancel();
                false
            } else {
                true
            }
        });
    }

    pub(crate) fn reset(&self) {
        let all = std::mem::take(&mut *self.inner.subs.lock());
        for (_, entry) in all {
            entry.cancel.cancel();
        }
    }
}

enum RemoteQuery {
    Search { q: String, include_spam_trash: bool },
    View(GmailListing),
}

#[expect(
    clippy::too_many_arguments,
    clippy::too_many_lines,
    clippy::cognitive_complexity,
    reason = "pagination loop handles search debounce, offline pause/resume, auth failures, and MoreResults"
)]
async fn run_listing(
    inner: Arc<ListingInner>,
    sub: SubId,
    account: AccountId,
    key: SubKey,
    more: Arc<Notify>,
    cancel: CancellationToken,
) {
    let remote_query = match key {
        SubKey::Search { query, .. } => {
            let sleep = inner.clock.sleep(SEARCH_DEBOUNCE);
            tokio::pin!(sleep);
            tokio::select! {
                biased;
                () = cancel.cancelled() => return,
                () = &mut sleep => {}
            }
            let gmail_q = gmail_query(&query);
            if gmail_q.trim().is_empty() {
                set_listing_state(&inner.writer, sub, ListingState::default()).await;
                return;
            }
            let parsed = Query::parse_search(&query, &inner.clock.local_now());
            RemoteQuery::Search {
                q: gmail_q,
                include_spam_trash: parsed.looks_in_trash(),
            }
        }
        SubKey::List { view, .. } => match view.into_gmail_listing() {
            Some(listing) => RemoteQuery::View(listing),
            None => return,
        },
        SubKey::App
        | SubKey::Account { .. }
        | SubKey::Conversation { .. }
        | SubKey::Composer { .. } => return,
    };

    let mut page_token: Option<PageToken> = None;
    let mut remote_hits: Vec<ThreadId> = Vec::new();
    let mut seen_hits: HashSet<ThreadId> = HashSet::new();
    let mut online_rx = inner.network.subscribe();
    let mut backoff = Backoff::new();

    loop {
        if cancel.is_cancelled() {
            return;
        }
        if !inner.network.is_online() {
            set_listing_state(
                &inner.writer,
                sub.clone(),
                ListingState {
                    remote_hits: remote_hits.clone(),
                    searching: false,
                    more: page_token.is_some(),
                },
            )
            .await;
            if !wait_for_online(&mut online_rx, &cancel).await {
                return;
            }
            continue;
        }

        let Some(slot) = inner.accounts.slot(account) else {
            return;
        };
        let Some((grant_gen, gmail)) = slot.gmail() else {
            set_listing_state(
                &inner.writer,
                sub,
                ListingState {
                    remote_hits,
                    searching: false,
                    more: false,
                },
            )
            .await;
            return;
        };

        let busy = inner.activity.enter();
        set_listing_state(
            &inner.writer,
            sub.clone(),
            ListingState {
                remote_hits: remote_hits.clone(),
                searching: true,
                more: page_token.is_some(),
            },
        )
        .await;

        let Some(loaded) = cancel
            .run_until_cancelled(load_listing_page(
                account,
                &gmail,
                &inner.pool,
                &inner.writer,
                &inner.clock,
                &remote_query,
                page_token.as_ref(),
            ))
            .await
        else {
            return;
        };

        let page = match loaded {
            Ok(p) => p,
            Err(err) if matches!(err.outcome, Outcome::InvalidGrant | Outcome::Unauthorized) => {
                slot.mark_auth_failed(grant_gen).await;
                return;
            }
            Err(err) if err.outcome == Outcome::Offline || !inner.network.is_online() => {
                inner.network.lost_if_unreachable(&err).await;
                set_listing_state(
                    &inner.writer,
                    sub.clone(),
                    ListingState {
                        remote_hits: remote_hits.clone(),
                        searching: false,
                        more: page_token.is_some(),
                    },
                )
                .await;
                drop(busy);
                if !wait_for_online(&mut online_rx, &cancel).await {
                    return;
                }
                continue;
            }
            Err(err) if err.is_transient() => {
                drop(busy);
                let pause = inner.clock.sleep(backoff.next_delay(err.retry_after()));
                tokio::select! {
                    biased;
                    () = cancel.cancelled() => return,
                    () = pause => {}
                }
                continue;
            }
            Err(_) => {
                set_listing_state(
                    &inner.writer,
                    sub,
                    ListingState {
                        remote_hits,
                        searching: false,
                        more: page_token.is_some(),
                    },
                )
                .await;
                return;
            }
        };
        backoff.reset();

        for tid in page.hits {
            if seen_hits.insert(tid) {
                remote_hits.push(tid);
            }
        }

        page_token = page.next;
        let has_more = page_token.is_some();
        if !has_more {
            set_listing_state(
                &inner.writer,
                sub,
                ListingState {
                    remote_hits,
                    searching: false,
                    more: false,
                },
            )
            .await;
            drop(busy);
            return;
        }
        set_listing_state(
            &inner.writer,
            sub.clone(),
            ListingState {
                remote_hits: remote_hits.clone(),
                searching: false,
                more: true,
            },
        )
        .await;
        drop(busy);
        tokio::select! {
            biased;
            () = cancel.cancelled() => return,
            () = more.notified() => {}
        }
    }
}

#[expect(
    clippy::too_many_lines,
    reason = "maps search, query view, and label view listings to Gmail threads.list parameters"
)]
async fn fetch_listing_page(
    gmail: &Gmail,
    pool: &ReadPool,
    account: AccountId,
    query: &RemoteQuery,
    page_token: Option<&PageToken>,
) -> Result<crate::gmail::ThreadPage, GmailError> {
    match query {
        RemoteQuery::Search {
            q,
            include_spam_trash,
        } => {
            gmail
                .list_threads(
                    ThreadQuery {
                        q: Some(q.as_str()),
                        label_id: None,
                        include_spam_trash: *include_spam_trash,
                        max_results: Some(LISTING_PAGE_SIZE),
                        page_token,
                    },
                    GmailPriority::P3,
                )
                .await
        }
        RemoteQuery::View(GmailListing::Query(q)) => {
            gmail
                .list_threads(
                    ThreadQuery {
                        q: (!q.is_empty()).then_some(q),
                        label_id: None,
                        include_spam_trash: false,
                        max_results: Some(LISTING_PAGE_SIZE),
                        page_token,
                    },
                    GmailPriority::P3,
                )
                .await
        }
        RemoteQuery::View(GmailListing::Label {
            label,
            include_spam_trash,
        }) => {
            let owned_gid;
            let wire_label = if SystemLabel::is_system_id(label.as_str()) {
                label
            } else {
                let found = pool
                    .with(|db| db.label_by_id(account, label))
                    .ok()
                    .flatten()
                    .and_then(|l| l.gmail_id);
                let Some(gid) = found else {
                    return Ok(crate::gmail::ThreadPage::default());
                };
                owned_gid = gid;
                &owned_gid
            };
            gmail
                .list_threads(
                    ThreadQuery {
                        q: None,
                        label_id: Some(wire_label),
                        include_spam_trash: *include_spam_trash,
                        max_results: Some(LISTING_PAGE_SIZE),
                        page_token,
                    },
                    GmailPriority::P3,
                )
                .await
        }
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "resolves cached thread or fetches full thread from Gmail with clock and writer"
)]
async fn resolve_or_fetch_hit(
    account: AccountId,
    gmail: &Gmail,
    pool: &ReadPool,
    writer: &Writer<WriterState>,
    clock: &Clock,
    gtid: &GmailThreadId,
) -> Result<Option<ThreadId>, GmailError> {
    if let Some(tid) = pool
        .with(|db| db.thread_by_gmail(account, gtid))
        .ok()
        .flatten()
    {
        return Ok(Some(tid));
    }
    fetch_and_ingest_thread(
        account,
        gmail,
        pool,
        writer,
        clock,
        None,
        gtid,
        Format::Metadata,
        GmailPriority::P3,
        false,
    )
    .await
    .map(|r| r.thread_id)
}

struct LoadedPage {
    hits: Vec<ThreadId>,
    next: Option<PageToken>,
}

#[expect(
    clippy::too_many_arguments,
    reason = "lists one Gmail page and resolves each hit with the storage, clock, and writer the fetch needs"
)]
async fn load_listing_page(
    account: AccountId,
    gmail: &Gmail,
    pool: &ReadPool,
    writer: &Writer<WriterState>,
    clock: &Clock,
    query: &RemoteQuery,
    page_token: Option<&PageToken>,
) -> Result<LoadedPage, GmailError> {
    let page = fetch_listing_page(gmail, pool, account, query, page_token).await?;
    let mut hits = Vec::with_capacity(page.threads.len());
    for tref in page.threads {
        match resolve_or_fetch_hit(account, gmail, pool, writer, clock, &tref.id).await {
            Ok(Some(tid)) => hits.push(tid),
            Err(err)
                if err.is_transient()
                    || matches!(err.outcome, Outcome::InvalidGrant | Outcome::Unauthorized) =>
            {
                return Err(err);
            }
            Ok(None) | Err(_) => {}
        }
    }
    Ok(LoadedPage {
        hits,
        next: page.next_page_token,
    })
}

async fn set_listing_state(writer: &Writer<WriterState>, sub: SubId, listing: ListingState) {
    super::log_err(
        "update listing state",
        writer
            .call(StorePriority::Sync, move |_tx, state| {
                state.stage_sub(SubOp::SetListing(sub, listing));
                Ok::<_, StoreError>(())
            })
            .await,
    );
}

async fn wait_for_online(
    rx: &mut tokio::sync::watch::Receiver<bool>,
    cancel: &CancellationToken,
) -> bool {
    loop {
        if *rx.borrow_and_update() {
            return true;
        }
        tokio::select! {
            biased;
            () = cancel.cancelled() => return false,
            res = rx.changed() => {
                if res.is_err() {
                    return false;
                }
            }
        }
    }
}
