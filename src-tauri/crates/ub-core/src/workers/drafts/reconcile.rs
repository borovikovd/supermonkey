//! Reconciling local drafts with Gmail's (SPEC Drafts, Sync): drafts Gmail
//! deleted go, drafts the app does not know or whose message changed are
//! read back, and a change skipped while a composer was open is read when it
//! closes.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use ub_domain::{DraftId, GmailDraftId, GmailMessageId, Rev};
use ub_gmail::{Format, GmailError, Outcome, Priority};
use ub_store::{Priority as WritePriority, StoreError};

use super::write::list_drafts;
use crate::Error;
use crate::drafts::lifecycle;
use crate::drafts::{GmailDraft, Reconcile, apply_gmail_draft, defer_if_open, drop_gone};
use crate::read::read;
use crate::workers::account::{AccountCtx, LATE, OTHER_META, Work, draft_message, fetch_thread};
use ub_domain::AccountId;
use ub_store::ReadPool;

/// When a draft reconcile runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Occasion {
    /// The connect sync or a full resync: every Gmail draft's thread also
    /// gets its metadata, then bodies, in the background (SPEC Sync).
    Sync,
    /// A poll saw a draft change: only the drafts themselves are read.
    Poll,
}

impl Occasion {
    /// The priorities of its reads: listing and thread metadata, then
    /// drafts' bodies. A sync reads drafts behind the Inbox's metadata and
    /// their bodies later still, so a first sign-in shows the Inbox first; a
    /// poll's draft change is read at once (SPEC Sync).
    fn priorities(self) -> (Priority, Priority) {
        match self {
            Self::Sync => (Priority::P4, Priority::P5),
            Self::Poll => (Priority::P2, Priority::P2),
        }
    }
}

/// Reconciles the account's local drafts with `drafts.list`.
pub(crate) async fn reconcile(ctx: &AccountCtx, occasion: Occasion) -> Result<(), GmailError> {
    let (meta, body) = occasion.priorities();
    let started = started(&ctx.deps.pool, ctx.account).await;
    let listed = list_drafts(ctx, meta).await?;
    if occasion == Occasion::Sync {
        for draft in &listed {
            ctx.queue.push(
                OTHER_META,
                Work::Meta {
                    thread: draft.message.thread_id.clone(),
                    history: None,
                    body: Some(LATE),
                },
            );
        }
    }
    let reconcile = Reconcile {
        live: Arc::new(listed.iter().map(|d| d.id.clone()).collect()),
        started,
        writing: Arc::clone(&ctx.lane.writing),
    };
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
            return Ok(());
        }
    };
    // The outbox writes and sends its drafts itself, so Gmail's copy of one
    // is never read back over the version being sent.
    let mut sending: HashSet<GmailDraftId> = sending.into_iter().collect();
    // Gmail draft id to the local draft and its last message id, for the
    // drafts Gmail still has.
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
                drop_gone(tx, state, &gone, &r, now)
            })
            .await;
        if let Err(e) = dropped {
            tracing::warn!(account = account.0, error = %e, "couldn't drop drafts Gmail deleted");
        }
    }
    for listed in listed {
        if sending.contains(&listed.id) {
            continue;
        }
        if let Some((local, message)) = known.remove(&listed.id) {
            // Unchanged, or a local edit Gmail has not confirmed wins over it.
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
        {
            // A draft new to the app gets its kind and target from its
            // thread's messages, once: they must be stored first.
            fetch_thread(ctx, &listed.message.thread_id, Format::Metadata, meta).await?;
        }
        match ctx.gmail.get_draft(body, &listed.id, Format::Full).await {
            Ok(full) => store(ctx, full, reconcile.clone()).await,
            Err(e) if *e.outcome() == Outcome::NotFound => {}
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

/// Reads a Gmail change to the draft that was skipped while a composer had
/// it open: its composer closed without edits.
pub(crate) async fn read_skipped(ctx: &AccountCtx, id: &DraftId) {
    let account = ctx.account;
    let draft = super::write::load(&ctx.deps.pool, id.clone()).await;
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
    // Its own Gmail draft: no other copy is in question.
    let reconcile = Reconcile {
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

/// The last `rev` committed, before a reconcile's first request. Unknown,
/// every local draft counts as changed since, so none is touched.
async fn started(pool: &Arc<ReadPool>, account: AccountId) -> Rev {
    match read(pool, |r| r.meta().rev()).await {
        Ok(rev) => rev,
        Err(e) => {
            tracing::warn!(account = account.0, error = %e, "couldn't read the rev for a draft reconcile");
            Rev(i64::MIN)
        }
    }
}

/// Marks a known draft's Gmail change to read when its composer closes, if
/// one has it open. Returns whether it was open.
async fn defer(ctx: &AccountCtx, id: DraftId) -> bool {
    let deferred = ctx
        .writer()
        .call(WritePriority::Sync, move |tx, state| {
            defer_if_open(tx, state, &id)
        })
        .await;
    deferred.unwrap_or_else(|e| {
        // Only the write can fail, and only for an open draft: leave it.
        tracing::warn!(account = ctx.account.0, error = %e, "couldn't defer a draft's Gmail change");
        true
    })
}

/// Whether the Gmail thread's messages other than drafts are stored.
async fn has_mail(
    pool: &Arc<ReadPool>,
    account: AccountId,
    thread: ub_domain::GmailThreadId,
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

/// Parses a Gmail draft off the async threads and stores it.
async fn store(ctx: &AccountCtx, full: ub_gmail::Draft, reconcile: Reconcile) {
    let account = ctx.account;
    let parsed = tokio::task::spawn_blocking(move || {
        let (message, body) = draft_message(&full.message);
        GmailDraft {
            id: full.id,
            thread: full.message.thread_id,
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
            apply_gmail_draft(tx, state, account, gmail, &reconcile, now)
        })
        .await
        .map_err(|e: StoreError| Error::Store(e));
    if let Err(e) = stored {
        tracing::warn!(account = account.0, error = %e, "couldn't store a Gmail draft");
    }
}
