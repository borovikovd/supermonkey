use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use sm_domain::{DraftId, GmailDraftId, GmailMessageId, Rev};
use sm_gmail::{Format, GmailError, Outcome, Priority};
use sm_store::{DraftRecord, Priority as WritePriority, StoreError};

use super::write::list_drafts;
use crate::Error;
use crate::drafts::lifecycle;
use crate::drafts::{GmailDraft, Reconcile, apply_gmail_draft, defer_if_open, drop_gone};
use crate::read::read;
use crate::workers::account::{
    AccountCtx, FetchQueue, LATE, OTHER_META, Work, draft_message, fetch_thread,
};
use sm_domain::AccountId;
use sm_store::ReadPool;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Occasion {
    Sync,
    Poll,
}

impl Occasion {
    const fn priorities(self) -> (Priority, Priority) {
        match self {
            Self::Sync => (Priority::P4, Priority::P5),
            Self::Poll => (Priority::P2, Priority::P2),
        }
    }
}

pub(crate) const fn stops_the_account(e: &GmailError) -> bool {
    matches!(
        e.outcome(),
        Outcome::Offline
            | Outcome::InvalidGrant
            | Outcome::Unauthorized
            | Outcome::RateLimited { .. }
    )
}

pub(crate) async fn reconcile(ctx: &AccountCtx, occasion: Occasion) -> Result<(), GmailError> {
    let (meta, _) = occasion.priorities();
    let started = started(&ctx.deps.pool, ctx.account).await;
    let listed = list_drafts(ctx, meta).await?;
    if occasion == Occasion::Sync {
        queue_threads(&ctx.queue, &listed);
    }
    let reconcile = Reconcile {
        account: ctx.account,
        live: Arc::new(listed.iter().map(|d| d.id.clone()).collect()),
        started,
        writing: Arc::clone(&ctx.lane.writing),
    };
    let Some(sorted) = sort_local(ctx, &reconcile).await else {
        return Ok(());
    };
    read_changed(ctx, listed, sorted, &reconcile, occasion).await
}

async fn read_changed(
    ctx: &AccountCtx,
    listed: Vec<sm_gmail::Draft>,
    mut sorted: Sorted,
    reconcile: &Reconcile,
    occasion: Occasion,
) -> Result<(), GmailError> {
    let (meta, body) = occasion.priorities();
    let account = ctx.account;
    let mut failed: Option<GmailError> = None;
    let mut skip = |e: GmailError| -> Result<(), GmailError> {
        if stops_the_account(&e) {
            return Err(e);
        }
        tracing::warn!(account = account.0, outcome = %e.outcome(), "couldn't read a Gmail draft; skipping it");
        if e.is_transient() {
            failed.get_or_insert(e);
        }
        Ok(())
    };
    for listed in listed {
        let read = match needs_read(ctx, &listed, &mut sorted, reconcile, meta).await {
            Ok(false) => continue,
            Ok(true) => read_draft(ctx, &listed.id, body, reconcile).await,
            Err(e) => Err(e),
        };
        if let Err(e) = read {
            skip(e)?;
        }
    }
    failed.map_or(Ok(()), Err)
}

async fn needs_read(
    ctx: &AccountCtx,
    listed: &sm_gmail::Draft,
    sorted: &mut Sorted,
    reconcile: &Reconcile,
    meta: Priority,
) -> Result<bool, GmailError> {
    if sorted.sending.contains(&listed.id) {
        return Ok(false);
    }
    let Some((local, message)) = sorted.known.remove(&listed.id) else {
        fetch_missing_thread(ctx, listed, meta).await?;
        return Ok(true);
    };
    if message.as_ref() == Some(&listed.message.id)
        || reconcile.writing.is_busy(&local)
        || sorted.unconfirmed.contains(&local)
    {
        return Ok(false);
    }
    Ok(!defer(ctx, local).await)
}

async fn fetch_missing_thread(
    ctx: &AccountCtx,
    listed: &sm_gmail::Draft,
    meta: Priority,
) -> Result<(), GmailError> {
    let thread = &listed.message.thread_id;
    if has_mail(&ctx.deps.pool, ctx.account, thread.clone()).await {
        return Ok(());
    }
    fetch_thread(ctx, thread, Format::Metadata, meta).await
}

async fn read_draft(
    ctx: &AccountCtx,
    id: &GmailDraftId,
    body: Priority,
    reconcile: &Reconcile,
) -> Result<(), GmailError> {
    match ctx.gmail.get_draft(body, id, Format::Full).await {
        Ok(full) => store(ctx, full, reconcile.clone()).await,
        Err(e) if *e.outcome() == Outcome::NotFound => {}
        Err(e) => return Err(e),
    }
    Ok(())
}

fn queue_threads(queue: &FetchQueue, listed: &[sm_gmail::Draft]) {
    for draft in listed {
        queue.push(
            OTHER_META,
            Work::Meta {
                thread: draft.message.thread_id.clone(),
                history: None,
                body: Some(LATE),
            },
        );
    }
}

pub(crate) async fn read_skipped(ctx: &AccountCtx, id: DraftId) {
    let account = ctx.account;
    let Some(gmail) = written_as(&ctx.deps.pool, account, id).await else {
        return;
    };
    let reconcile = Reconcile {
        account: ctx.account,
        live: Arc::default(),
        started: started(&ctx.deps.pool, ctx.account).await,
        writing: Arc::clone(&ctx.lane.writing),
    };
    let read = ctx
        .gmail
        .get_draft(Priority::P2, &gmail, Format::Full)
        .await;
    ctx.note(&read);
    let Ok(full) = read.inspect_err(|e| {
        tracing::info!(account = account.0, outcome = %e.outcome(), "couldn't read back a draft's Gmail change");
    }) else {
        return;
    };
    store(ctx, full, reconcile).await;
}

async fn written_as(pool: &Arc<ReadPool>, account: AccountId, id: DraftId) -> Option<GmailDraftId> {
    match super::write::load(pool, id).await {
        Ok(draft) => draft.and_then(|d| lifecycle::gmail_draft(&d).cloned()),
        Err(e) => {
            tracing::warn!(account = account.0, error = %e, "couldn't read a draft to read back");
            None
        }
    }
}

async fn started(pool: &Arc<ReadPool>, account: AccountId) -> Rev {
    match read(pool, |r| r.meta().rev()).await {
        Ok(rev) => rev,
        Err(e) => {
            tracing::warn!(account = account.0, error = %e, "couldn't read the rev for a draft reconcile");
            Rev(i64::MIN)
        }
    }
}

async fn defer(ctx: &AccountCtx, id: DraftId) -> bool {
    let deferred = ctx
        .writer()
        .call(WritePriority::Sync, move |tx, state| {
            defer_if_open(tx, state, &id)
        })
        .await;
    deferred.unwrap_or_else(|e| {
        tracing::warn!(account = ctx.account.0, error = %e, "couldn't defer a draft's Gmail change");
        true
    })
}

async fn has_mail(
    pool: &Arc<ReadPool>,
    account: AccountId,
    thread: sm_domain::GmailThreadId,
) -> bool {
    let has = read(pool, move |r| {
        let Some(id) = r.threads().by_gmail_id(account, &thread)? else {
            return Ok(false);
        };
        Ok(r.messages().facts(id)?.iter().any(|m| !m.is_draft))
    })
    .await;
    has.unwrap_or_else(|e| {
        tracing::warn!(account = account.0, error = %e, "couldn't read a draft's thread");
        false
    })
}

async fn store(ctx: &AccountCtx, full: sm_gmail::Draft, reconcile: Reconcile) {
    let account = ctx.account;
    let parsed = tokio::task::spawn_blocking(move || gmail_draft(full)).await;
    let Ok(gmail) = parsed.inspect_err(|e| {
        tracing::error!(account = account.0, error = %e, "parsing a Gmail draft failed");
    }) else {
        return;
    };
    let now = ctx.clock().now();
    let stored: Result<(), Error> = ctx
        .writer()
        .call(WritePriority::Sync, move |tx, state| {
            apply_gmail_draft(tx, state, gmail, &reconcile, now)
        })
        .await
        .map_err(|e: StoreError| Error::Store(e));
    stored.unwrap_or_else(|e| {
        tracing::warn!(account = account.0, error = %e, "couldn't store a Gmail draft");
    });
}

fn gmail_draft(full: sm_gmail::Draft) -> GmailDraft {
    let thread = full.message.thread_id.clone();
    let (message, body) = draft_message(full.message);
    GmailDraft {
        id: full.id,
        thread,
        message,
        editor_html: body.editor_html,
        quote_html: body.quote_html,
    }
}

struct Sorted {
    sending: HashSet<GmailDraftId>,
    known: HashMap<GmailDraftId, (DraftId, Option<GmailMessageId>)>,
    unconfirmed: HashSet<DraftId>,
}

impl Sorted {
    fn sort(&mut self, local: Vec<DraftRecord>, live: &HashSet<GmailDraftId>) -> Vec<DraftId> {
        let mut gone = Vec::new();
        for record in local {
            let draft = record.draft;
            let Some(gmail) = lifecycle::gmail_draft(&draft).cloned() else {
                continue;
            };
            if record.in_outbox {
                self.sending.insert(gmail);
            } else if live.contains(&gmail) {
                let message = lifecycle::gmail_message(&draft).cloned();
                self.known.insert(gmail, (draft.id, message));
            } else {
                gone.push(draft.id);
            }
        }
        gone
    }
}

async fn sort_local(ctx: &AccountCtx, reconcile: &Reconcile) -> Option<Sorted> {
    let (local, sending, unconfirmed) = read_local(ctx).await?;
    let mut sorted = Sorted {
        sending: sending.into_iter().collect(),
        known: HashMap::new(),
        unconfirmed,
    };
    let gone = sorted.sort(local, &reconcile.live);
    if !gone.is_empty() {
        forget_gone(ctx, reconcile.clone(), gone).await;
    }
    Some(sorted)
}

async fn read_local(
    ctx: &AccountCtx,
) -> Option<(Vec<DraftRecord>, Vec<GmailDraftId>, HashSet<DraftId>)> {
    let account = ctx.account;
    let local = ctx
        .read(move |r| {
            let drafts = r.drafts().for_account(account)?;
            let sending = r.outbox().gmail_draft_ids(account)?;
            let unconfirmed: HashSet<DraftId> =
                r.ops().upserted_drafts(account)?.into_iter().collect();
            Ok((drafts, sending, unconfirmed))
        })
        .await;
    match local {
        Ok(local) => Some(local),
        Err(e) => {
            tracing::warn!(account = account.0, error = %e, "couldn't read the drafts to reconcile");
            None
        }
    }
}

async fn forget_gone(ctx: &AccountCtx, reconcile: Reconcile, gone: Vec<DraftId>) {
    let account = ctx.account;
    let now = ctx.clock().now();
    let dropped = ctx
        .writer()
        .call(WritePriority::Sync, move |tx, state| {
            drop_gone(tx, state, gone, &reconcile, now)
        })
        .await;
    if let Err(e) = dropped {
        tracing::warn!(account = account.0, error = %e, "couldn't drop drafts Gmail deleted");
    }
}
