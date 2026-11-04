//! Writing a draft to Gmail: its MIME built with its files, then
//! `drafts.update`, or `drafts.create` with recovery (SPEC Gmail errors).

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use ub_domain::{Addr, DraftId, GmailDraftId, GmailThreadId};
use ub_gmail::{Format, GmailError, Outcome, PageToken, Priority};
use ub_mime::{InlinePart, OutAttachment, Outgoing};
use ub_store::{Draft, Priority as WritePriority, StoreError};

use super::WriteFailure;
use crate::Error;
use crate::workers::account::{AccountCtx, parse};
use crate::workers::attachments::{DownloadError, ensure};
use crate::workers::files::{attachments_dir, draft_dir, is_inside};

/// How many times a `drafts.create` with no clear answer is tried, each
/// after looking for the draft the last one may have made.
const CREATE_TRIES: usize = 3;

/// The draft as stored, or `None` when it is gone.
pub(crate) async fn load(ctx: &AccountCtx, id: &DraftId) -> Result<Option<Draft>, Error> {
    let id = id.clone();
    Ok(ctx
        .read(move |r| r.drafts().get(&id))
        .await?
        .map(|record| record.draft))
}

/// Writes the draft to Gmail: an update of its Gmail draft, or a create
/// when it has none or Gmail no longer has it (sent or deleted elsewhere).
/// A draft whose last create never got its answer stored (the app quit, or
/// the write was cancelled) may have a Gmail draft already: it is looked
/// for by its header first, and updated if found. Every write carries all
/// of the draft's attachments: `drafts.update` replaces the whole message.
pub(crate) async fn write(
    ctx: &AccountCtx,
    draft: &Draft,
) -> Result<ub_gmail::Draft, WriteFailure> {
    let raw = build_raw(ctx, draft).await?;
    let thread = gmail_thread(ctx, draft).await?;
    let existing = match &draft.gmail_draft_id {
        Some(id) => Some(id.clone()),
        None if draft.create_sent => recover_create(ctx, &draft.id).await?.map(|d| d.id),
        None => None,
    };
    if let Some(id) = existing {
        match ctx
            .gmail
            .update_draft(Priority::P1, &id, &raw, thread.as_ref())
            .await
        {
            Ok(written) => return Ok(written),
            Err(e) if *e.outcome() == Outcome::NotFound => {}
            Err(e) => return Err(e.into()),
        }
    }
    create(ctx, &draft.id, &raw, thread.as_ref()).await
}

/// The Gmail thread the draft goes on: its thread's, else the one its last
/// upsert returned.
async fn gmail_thread(
    ctx: &AccountCtx,
    draft: &Draft,
) -> Result<Option<GmailThreadId>, WriteFailure> {
    let Some(thread) = draft.thread else {
        return Ok(draft.gmail_thread_id.clone());
    };
    let row = ctx.read(move |r| r.threads().row(thread)).await?;
    Ok(row
        .map(|row| row.gmail_id)
        .or_else(|| draft.gmail_thread_id.clone()))
}

/// `drafts.create`, which is unsafe to repeat: the draft is marked as sent
/// to create first, so a create whose answer is lost to a quit is
/// recovered on the next write, and after one with no clear answer, it
/// looks for the draft it may have made before trying again.
async fn create(
    ctx: &AccountCtx,
    local: &DraftId,
    raw: &[u8],
    thread: Option<&GmailThreadId>,
) -> Result<ub_gmail::Draft, WriteFailure> {
    mark_create_sent(ctx, local).await?;
    for _ in 0..CREATE_TRIES {
        match ctx.gmail.create_draft(Priority::P1, raw, thread).await {
            Ok(created) => return Ok(created),
            Err(e)
                if matches!(
                    e.outcome(),
                    Outcome::NoResponse | Outcome::ServerError { .. }
                ) =>
            {
                if let Some(found) = recover_create(ctx, local).await? {
                    return Ok(found);
                }
            }
            Err(e) => return Err(e.into()),
        }
    }
    Err(GmailError::new(Outcome::NoResponse).into())
}

/// Records, before a `drafts.create` goes out, that one did: committed
/// first, so no answer can be lost without the next write knowing.
async fn mark_create_sent(ctx: &AccountCtx, local: &DraftId) -> Result<(), Error> {
    let local = local.clone();
    ctx.writer()
        .call(WritePriority::Sync, move |tx, _| {
            if let Some(record) = tx.read().drafts().get(&local)?
                && !record.draft.create_sent
            {
                let mut draft = record.draft;
                draft.create_sent = true;
                tx.drafts().put(&draft)?;
            }
            Ok::<_, StoreError>(())
        })
        .await?;
    Ok(())
}

/// After a `drafts.create` with no clear answer (SPEC Gmail errors): if
/// sync has stored Gmail ids on the draft meanwhile, the create succeeded.
/// Otherwise every Gmail draft the app does not know is read
/// (`format=metadata`) for its `X-Ubermensch-Draft` header: one with this
/// draft's id is kept, and any other copies are deleted.
async fn recover_create(
    ctx: &AccountCtx,
    local: &DraftId,
) -> Result<Option<ub_gmail::Draft>, WriteFailure> {
    if let Some(stored) = load(ctx, local).await?.and_then(|d| d.gmail_draft_id) {
        let found = ctx
            .gmail
            .get_draft(Priority::P1, &stored, Format::Metadata)
            .await?;
        return Ok(Some(found));
    }
    let account = ctx.account;
    let known: HashSet<GmailDraftId> = ctx
        .read(move |r| r.drafts().for_account(account))
        .await?
        .into_iter()
        .filter_map(|record| record.draft.gmail_draft_id)
        .collect();
    let mut ours = Vec::new();
    for id in list_all(ctx)
        .await?
        .into_iter()
        .filter(|id| !known.contains(id))
    {
        let read = ctx
            .gmail
            .get_draft(Priority::P1, &id, Format::Metadata)
            .await;
        let read = match read {
            Ok(read) => read,
            Err(e) if *e.outcome() == Outcome::NotFound => continue,
            Err(e) => return Err(e.into()),
        };
        if parse(&read.message).headers.draft_id.as_ref() == Some(local) {
            ours.push(read);
        }
    }
    let mut found = ours.into_iter();
    let keep = found.next();
    for extra in found {
        if let Err(e) = ctx.gmail.delete_draft(Priority::P1, &extra.id).await {
            tracing::warn!(account = account.0, outcome = %e.outcome(), "couldn't delete an extra copy of a draft");
        }
    }
    Ok(keep)
}

/// The ids of every Gmail draft of the account.
pub(super) async fn list_all(ctx: &AccountCtx) -> Result<Vec<GmailDraftId>, GmailError> {
    Ok(list_drafts(ctx, Priority::P2)
        .await?
        .into_iter()
        .map(|d| d.id)
        .collect())
}

/// Every page of `drafts.list`.
pub(super) async fn list_drafts(
    ctx: &AccountCtx,
    priority: Priority,
) -> Result<Vec<ub_gmail::Draft>, GmailError> {
    let mut out = Vec::new();
    let mut page: Option<PageToken> = None;
    loop {
        let listed = ctx.gmail.list_drafts(priority, page.as_ref()).await?;
        out.extend(listed.drafts);
        match listed.next_page_token {
            Some(next) => page = Some(next),
            None => return Ok(out),
        }
    }
}

/// The draft's RFC 5322 bytes (SPEC Compose): its HTML and quote, its
/// attachments (downloading those not cached yet), and the quoted
/// message's `cid:` images turned back from `data:`.
async fn build_raw(ctx: &AccountCtx, draft: &Draft) -> Result<Vec<u8>, WriteFailure> {
    let attachments = attachment_bytes(ctx, draft).await?;
    let html = format!(
        "{}{}",
        draft.body_html,
        draft.quote_html.as_deref().unwrap_or_default()
    );
    let known = quoted_inline(ctx, draft, &html).await?;
    let from = draft
        .from
        .clone()
        .unwrap_or_else(|| Addr::new(String::new()));
    let date = ctx.clock().now();
    let id = draft.id.clone();
    let (to, cc, bcc) = (draft.to.clone(), draft.cc.clone(), draft.bcc.clone());
    let (subject, in_reply_to) = (draft.subject.clone(), draft.in_reply_to.clone());
    let references: Vec<String> = draft
        .references
        .split_whitespace()
        .map(str::to_owned)
        .collect();
    let built = tokio::task::spawn_blocking(move || {
        let (html, inline) = ub_mime::data_images_to_cid(&html, known);
        ub_mime::build_mime(&Outgoing {
            from: &from,
            to: &to,
            cc: &cc,
            bcc: &bcc,
            subject: &subject,
            html: &html,
            in_reply_to: in_reply_to.as_deref(),
            references: &references,
            draft_id: &id,
            attachments: &attachments,
            inline: &inline,
            date,
        })
    })
    .await
    .map_err(Error::from)?;
    built.map_err(|e| WriteFailure::Local(format!("The message can't be built: {e}")))
}

/// The bytes of every attachment of the draft: a file added to it, or a
/// stored attachment it carries, downloaded if needed. A file outside the
/// app's own folders is never read: its path came from the webview.
async fn attachment_bytes(
    ctx: &AccountCtx,
    draft: &Draft,
) -> Result<Vec<OutAttachment>, WriteFailure> {
    let mut out = Vec::with_capacity(draft.attachments.len());
    for a in &draft.attachments {
        let path = match (&a.path, a.source) {
            (Some(path), source) if kept_file(ctx, draft, path, source.is_some()).await => {
                PathBuf::from(path)
            }
            (_, Some(source)) => download(ctx, source).await?,
            (Some(_), None) => {
                return Err(WriteFailure::Local(format!(
                    "The file {} is gone",
                    a.filename
                )));
            }
            (None, None) => continue,
        };
        out.push(OutAttachment {
            filename: a.filename.clone(),
            mime_type: a.mime_type.clone(),
            data: read_file(path).await?,
        });
    }
    Ok(out)
}

/// Whether `path` is a file the app keeps for this draft: one added to it,
/// or, for an attachment it carries, one downloaded for the account.
async fn kept_file(ctx: &AccountCtx, draft: &Draft, path: &str, carried: bool) -> bool {
    let path = PathBuf::from(path);
    let added = draft_dir(&ctx.deps.data_dir, &draft.id);
    let downloaded = carried.then(|| attachments_dir(&ctx.deps.data_dir, draft.account));
    tokio::task::spawn_blocking(move || {
        is_inside(&path, &added) || downloaded.is_some_and(|dir| is_inside(&path, &dir))
    })
    .await
    .unwrap_or(false)
}

/// The quoted message's inline parts, when the HTML may show one as a
/// `data:` image.
async fn quoted_inline(
    ctx: &AccountCtx,
    draft: &Draft,
    html: &str,
) -> Result<Vec<InlinePart>, WriteFailure> {
    let Some(target) = draft.target else {
        return Ok(Vec::new());
    };
    if !html.contains("data:") {
        return Ok(Vec::new());
    }
    let parts = ctx.read(move |r| r.messages().attachments(target)).await?;
    let mut out = Vec::new();
    for part in parts {
        let Some(content_id) = part.content_id else {
            continue;
        };
        let path = download(ctx, part.id).await?;
        out.push(InlinePart {
            content_id,
            mime_type: part.mime_type,
            filename: Some(part.filename),
            data: read_file(path).await?,
        });
    }
    Ok(out)
}

/// Downloads a stored attachment for a draft write. Offline and transient
/// failures leave the write to run again; one Gmail no longer has, or that
/// is not the account's, rejects it.
pub(super) async fn download(
    ctx: &AccountCtx,
    attachment: ub_domain::AttachmentId,
) -> Result<PathBuf, WriteFailure> {
    match ensure(ctx, attachment).await {
        Ok(path) => Ok(path),
        Err(DownloadError::Gone) => Err(WriteFailure::Local("An attachment is gone".to_owned())),
        Err(DownloadError::Stopped) => Err(GmailError::new(Outcome::Offline).into()),
        Err(DownloadError::Gmail(e)) if *e.outcome() == Outcome::NotFound => Err(
            WriteFailure::Local("An attachment is gone from Gmail".to_owned()),
        ),
        Err(DownloadError::Gmail(e)) => Err(e.into()),
        Err(DownloadError::Core(e)) => Err(e.into()),
    }
}

/// A file's bytes. A file that can't be read rejects the write.
async fn read_file(path: PathBuf) -> Result<Vec<u8>, WriteFailure> {
    let read = tokio::task::spawn_blocking(move || std::fs::read(Path::new(&path)))
        .await
        .map_err(Error::from)?;
    read.map_err(|e| WriteFailure::Local(format!("A file can't be read: {e}")))
}
