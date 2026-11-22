use std::collections::HashSet;
use std::path::{Path, PathBuf};

use sm_domain::{AccountId, Addr, DraftId, GmailDraftId, GmailThreadId};
use sm_gmail::{Format, GmailError, Outcome, PageToken, Priority};
use sm_mime::{InlinePart, OutAttachment, Outgoing};
use sm_store::{Draft, Priority as WritePriority, StoreError};

use super::WriteFailure;
use super::reconcile::stops_the_account;
use crate::Error;
use crate::drafts::lifecycle::{self, GmailWrite};
use crate::drafts::settle;
use crate::read::read;
use crate::workers::account::{AccountCtx, fetch_thread, parse};
use crate::workers::attachments::{DownloadError, ensure};
use crate::workers::files::{attachments_dir, draft_dir, is_inside};
use sm_store::ReadPool;
use std::sync::Arc;

const CREATE_TRIES: usize = 3;

pub(crate) async fn load(pool: &Arc<ReadPool>, id: DraftId) -> Result<Option<Draft>, Error> {
    Ok(read(pool, move |r| r.drafts().get(&id))
        .await?
        .map(|record| record.draft))
}

pub(crate) async fn write(
    ctx: &AccountCtx,
    draft: Draft,
) -> Result<Option<sm_gmail::Draft>, WriteFailure> {
    let draft = if lifecycle::awaits_target_body(&draft) {
        let Some(settled) = with_target_body(ctx, &draft).await? else {
            return Ok(None);
        };
        settled
    } else {
        draft
    };
    let (draft, raw) = build_raw(ctx, draft).await?;
    let thread = gmail_thread(&ctx.deps.pool, &draft).await?;
    let recovered;
    let existing = match lifecycle::gmail_write(&draft) {
        GmailWrite::Update(id) => Some(id),
        GmailWrite::Recover => {
            recovered = recover_create(ctx, &draft.id).await?.map(|d| d.id);
            recovered.as_ref()
        }
        GmailWrite::Create => None,
    };
    if let Some(id) = existing {
        match ctx
            .gmail
            .update_draft(Priority::P1, id, &raw, thread.as_ref())
            .await
        {
            Ok(written) => return Ok(Some(written)),
            Err(e) if *e.outcome() == Outcome::NotFound => {}
            Err(e) => return Err(e.into()),
        }
    }
    create(ctx, &draft.id, &raw, thread.as_ref()).await
}

async fn with_target_body(ctx: &AccountCtx, draft: &Draft) -> Result<Option<Draft>, WriteFailure> {
    if let Some(target) = draft.target {
        let thread = ctx
            .read(move |r| {
                let Some(message) = r.messages().get(target)? else {
                    return Ok(None);
                };
                if message.has_body {
                    return Ok(None);
                }
                Ok(r.threads().row(message.thread)?.map(|row| row.gmail_id))
            })
            .await?;
        if let Some(thread) = thread {
            fetch_thread(ctx, &thread, Format::Full, Priority::P1).await?;
        }
    }
    let id = draft.id.clone();
    let settled = ctx
        .writer()
        .call(WritePriority::Sync, move |tx, _| settle(tx, &id))
        .await
        .map_err(Error::from)?;
    Ok(settled)
}

async fn gmail_thread(
    pool: &Arc<ReadPool>,
    draft: &Draft,
) -> Result<Option<GmailThreadId>, WriteFailure> {
    let last = lifecycle::gmail_thread(draft).cloned();
    let Some(thread) = draft.thread else {
        return Ok(last);
    };
    let row = read(pool, move |r| r.threads().row(thread)).await?;
    Ok(row.map(|row| row.gmail_id).or(last))
}

async fn create(
    ctx: &AccountCtx,
    local: &DraftId,
    raw: &[u8],
    mut thread: Option<&GmailThreadId>,
) -> Result<Option<sm_gmail::Draft>, WriteFailure> {
    if !mark_create_sent(ctx, local.clone()).await? {
        return Ok(None);
    }
    for _ in 0..CREATE_TRIES {
        match ctx.gmail.create_draft(Priority::P1, raw, thread).await {
            Ok(created) => return Ok(Some(created)),
            Err(e) if *e.outcome() == Outcome::NotFound && thread.is_some() => thread = None,
            Err(e)
                if matches!(
                    e.outcome(),
                    Outcome::NoResponse | Outcome::ServerError { .. }
                ) =>
            {
                if let Some(found) = recover_create(ctx, local).await? {
                    return Ok(Some(found));
                }
            }
            Err(e) => return Err(e.into()),
        }
    }
    Err(GmailError::new(Outcome::NoResponse).into())
}

async fn mark_create_sent(ctx: &AccountCtx, local: DraftId) -> Result<bool, Error> {
    Ok(ctx
        .writer()
        .call(WritePriority::Sync, move |tx, _| {
            let Some(record) = tx.read().drafts().get(&local)? else {
                return Ok(false);
            };
            let mut draft = record.draft;
            if lifecycle::begin_create(&mut draft) {
                tx.drafts().put(&draft)?;
            }
            Ok::<_, StoreError>(true)
        })
        .await?)
}

async fn recover_create(
    ctx: &AccountCtx,
    local: &DraftId,
) -> Result<Option<sm_gmail::Draft>, WriteFailure> {
    if let Some(found) = stored_copy(ctx, local).await? {
        return Ok(Some(found));
    }
    let mut found = copies_of(ctx, local).await?.ours.into_iter();
    let keep = found.next();
    for extra in found {
        let deleted = ctx.gmail.delete_draft(Priority::P1, &extra.id).await;
        deleted.unwrap_or_else(|e| {
            tracing::warn!(account = ctx.account.0, outcome = %e.outcome(), "couldn't delete an extra copy of a draft");
        });
    }
    Ok(keep)
}

async fn stored_copy(
    ctx: &AccountCtx,
    local: &DraftId,
) -> Result<Option<sm_gmail::Draft>, WriteFailure> {
    let Some(stored) = load(&ctx.deps.pool, local.clone())
        .await?
        .and_then(|d| lifecycle::gmail_draft(&d).cloned())
    else {
        return Ok(None);
    };
    match ctx
        .gmail
        .get_draft(Priority::P1, &stored, Format::Metadata)
        .await
    {
        Ok(found) => Ok(Some(found)),
        Err(e) if *e.outcome() == Outcome::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

pub(super) struct Copies {
    pub(super) ours: Vec<sm_gmail::Draft>,
    pub(super) unread: Option<GmailError>,
}

pub(super) async fn copies_of(ctx: &AccountCtx, local: &DraftId) -> Result<Copies, WriteFailure> {
    let account = ctx.account;
    let known: HashSet<GmailDraftId> = ctx
        .read(move |r| r.drafts().for_account(account))
        .await?
        .into_iter()
        .filter_map(|record| lifecycle::gmail_draft(&record.draft).cloned())
        .collect();
    let mut copies = Copies {
        ours: Vec::new(),
        unread: None,
    };
    for id in list_all(ctx)
        .await?
        .into_iter()
        .filter(|id| !known.contains(id))
    {
        let Some(read) = read_copy(ctx, &id, &mut copies.unread).await? else {
            continue;
        };
        if parse(&read.message).headers.draft_id.as_ref() == Some(local) {
            copies.ours.push(read);
        }
    }
    Ok(copies)
}

async fn read_copy(
    ctx: &AccountCtx,
    id: &GmailDraftId,
    unread: &mut Option<GmailError>,
) -> Result<Option<sm_gmail::Draft>, WriteFailure> {
    let read = ctx
        .gmail
        .get_draft(Priority::P1, id, Format::Metadata)
        .await;
    match read {
        Ok(read) => Ok(Some(read)),
        Err(e) if *e.outcome() == Outcome::NotFound => Ok(None),
        Err(e) if stops_the_account(&e) => Err(e.into()),
        Err(e) => {
            skip_copy(ctx.account, e, unread);
            Ok(None)
        }
    }
}

fn skip_copy(account: AccountId, e: GmailError, unread: &mut Option<GmailError>) {
    tracing::warn!(account = account.0, outcome = %e.outcome(), "couldn't read a Gmail draft while looking for copies; skipping it");
    if e.outcome().is_transient() {
        unread.get_or_insert(e);
    }
}

pub(super) async fn list_all(ctx: &AccountCtx) -> Result<Vec<GmailDraftId>, GmailError> {
    Ok(list_drafts(ctx, Priority::P2)
        .await?
        .into_iter()
        .map(|d| d.id)
        .collect())
}

pub(super) async fn list_drafts(
    ctx: &AccountCtx,
    priority: Priority,
) -> Result<Vec<sm_gmail::Draft>, GmailError> {
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

async fn build_raw(ctx: &AccountCtx, draft: Draft) -> Result<(Draft, Vec<u8>), WriteFailure> {
    let attachments = attachment_bytes(ctx, &draft).await?;
    let html = format!(
        "{}{}",
        draft.body_html,
        draft.quote_html.as_deref().unwrap_or_default()
    );
    let known = quoted_inline(ctx, &draft, &html).await?;
    let date = ctx.clock().now();
    let references: Vec<String> = draft
        .references
        .split_whitespace()
        .map(str::to_owned)
        .collect();
    let (draft, built) = tokio::task::spawn_blocking(move || {
        let (html, inline) = sm_mime::data_images_to_cid(&html, known);
        let nobody = Addr::new(String::new());
        let built = sm_mime::build_mime(&Outgoing {
            from: draft.from.as_ref().unwrap_or(&nobody),
            to: &draft.to,
            cc: &draft.cc,
            bcc: &draft.bcc,
            subject: &draft.subject,
            html: &html,
            in_reply_to: draft.in_reply_to.as_deref(),
            references: &references,
            draft_id: &draft.id,
            attachments: &attachments,
            inline: &inline,
            date,
        });
        (draft, built)
    })
    .await
    .map_err(Error::from)?;
    let raw = built.map_err(|e| WriteFailure::Local(format!("The message can't be built: {e}")))?;
    Ok((draft, raw))
}

async fn attachment_bytes(
    ctx: &AccountCtx,
    draft: &Draft,
) -> Result<Vec<OutAttachment>, WriteFailure> {
    let mut out = Vec::with_capacity(draft.attachments.len());
    for a in &draft.attachments {
        let path = match (&a.path, a.source) {
            (Some(path), source)
                if kept_file(&ctx.deps.data_dir, draft, path, source.is_some()).await =>
            {
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

async fn kept_file(data_dir: &Path, draft: &Draft, path: &str, carried: bool) -> bool {
    let path = PathBuf::from(path);
    let added = draft_dir(data_dir, &draft.id);
    let downloaded = carried.then(|| attachments_dir(data_dir, draft.account));
    tokio::task::spawn_blocking(move || {
        is_inside(&path, &added) || downloaded.is_some_and(|dir| is_inside(&path, &dir))
    })
    .await
    .unwrap_or(false)
}

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

pub(super) async fn download(
    ctx: &AccountCtx,
    attachment: sm_domain::AttachmentId,
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

async fn read_file(path: PathBuf) -> Result<Vec<u8>, WriteFailure> {
    let read = tokio::task::spawn_blocking(move || std::fs::read(Path::new(&path)))
        .await
        .map_err(Error::from)?;
    read.map_err(|e| WriteFailure::Local(format!("A file can't be read: {e}")))
}
