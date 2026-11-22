use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use sm_domain::{DraftId, GmailDraftId, GmailMessageId, Rev};
use sm_gmail::{Format, GmailError, Outcome, Priority};
use sm_store::{Priority as WritePriority, StoreError};

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
    let (meta, body) = occasion.priorities();
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
    let account = ctx.account;
    let Some(Sorted {
        sending,
        mut known,
        unconfirmed,
    }) = sort_local(ctx, &reconcile).await
    else {
        return Ok(());
    };
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
        if sending.contains(&listed.id) {
            continue;
        }
        if let Some((local, message)) = known.remove(&listed.id) {
            if message.as_ref() == Some(&listed.message.id)
                || reconcile.writing.is_busy(&local)
                || unconfirmed.contains(&local)
            {
                continue;
            }
            if defer(ctx, local).await {
                continue;
            }
        } else if !has_mail(
            &ctx.deps.pool,
            ctx.account,
            listed.message.thread_id.clone(),
        )
        .await
            && let Err(e) =
                fetch_thread(ctx, &listed.message.thread_id, Format::Metadata, meta).await
        {
            skip(e)?;
            continue;
        }
        match ctx.gmail.get_draft(body, &listed.id, Format::Full).await {
            Ok(full) => store(ctx, full, reconcile.clone()).await,
            Err(e) if *e.outcome() == Outcome::NotFound => {}
            Err(e) => skip(e)?,
        }
    }
    failed.map_or(Ok(()), Err)
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
    let draft = super::write::load(&ctx.deps.pool, id).await;
    let gmail = match draft {
        Ok(Some(draft)) => lifecycle::gmail_draft(&draft).cloned(),
        Ok(None) => None,
        Err(e) => {
            tracing::warn!(account = account.0, error = %e, "couldn't read a draft to read back");
            None
        }
    };
    let Some(gmail) = gmail else {
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
    match read {
        Ok(full) => store(ctx, full, reconcile).await,
        Err(e) => {
            tracing::info!(account = account.0, outcome = %e.outcome(), "couldn't read back a draft's Gmail change");
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
    let parsed = tokio::task::spawn_blocking(move || {
        let thread = full.message.thread_id.clone();
        let (message, body) = draft_message(full.message);
        GmailDraft {
            id: full.id,
            thread,
            message,
            editor_html: body.editor_html,
            quote_html: body.quote_html,
        }
    })
    .await;
    let gmail = match parsed {
        Ok(gmail) => gmail,
        Err(e) => {
            tracing::error!(account = account.0, error = %e, "parsing a Gmail draft failed");
            return;
        }
    };
    let now = ctx.clock().now();
    let stored: Result<(), Error> = ctx
        .writer()
        .call(WritePriority::Sync, move |tx, state| {
            apply_gmail_draft(tx, state, gmail, &reconcile, now)
        })
        .await
        .map_err(|e: StoreError| Error::Store(e));
    if let Err(e) = stored {
        tracing::warn!(account = account.0, error = %e, "couldn't store a Gmail draft");
    }
}

struct Sorted {
    sending: HashSet<GmailDraftId>,
    known: HashMap<GmailDraftId, (DraftId, Option<GmailMessageId>)>,
    unconfirmed: HashSet<DraftId>,
}

async fn sort_local(ctx: &AccountCtx, reconcile: &Reconcile) -> Option<Sorted> {
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
    let (local, sending, unconfirmed) = match local {
        Ok(local) => local,
        Err(e) => {
            tracing::warn!(account = account.0, error = %e, "couldn't read the drafts to reconcile");
            return None;
        }
    };
    let mut sending: HashSet<GmailDraftId> = sending.into_iter().collect();
    let mut known: HashMap<GmailDraftId, (DraftId, Option<GmailMessageId>)> = HashMap::new();
    let mut gone = Vec::new();
    for record in local {
        let draft = record.draft;
        let Some(gmail) = lifecycle::gmail_draft(&draft).cloned() else {
            continue;
        };
        if record.in_outbox {
            sending.insert(gmail);
        } else if reconcile.live.contains(&gmail) {
            let message = lifecycle::gmail_message(&draft).cloned();
            known.insert(gmail, (draft.id, message));
        } else {
            gone.push(draft.id);
        }
    }
    if !gone.is_empty() {
        let r = reconcile.clone();
        let now = ctx.clock().now();
        let dropped = ctx
            .writer()
            .call(WritePriority::Sync, move |tx, state| {
                drop_gone(tx, state, gone, &r, now)
            })
            .await;
        if let Err(e) = dropped {
            tracing::warn!(account = account.0, error = %e, "couldn't drop drafts Gmail deleted");
        }
    }
    Some(Sorted {
        sending,
        known,
        unconfirmed,
    })
}
