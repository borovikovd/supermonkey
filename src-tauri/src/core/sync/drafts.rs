use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;

use uuid::Uuid;

use crate::domain::{
    AccountId, Addr, DraftId, DraftKind, DraftLifecycle, GmailDraftId, GmailMessageId,
    GmailThreadId, Millis, OpKind, Rev, gmail_draft_aim, gmail_draft_kind,
};
use crate::gmail::{Format, Gmail, GmailError, Outcome, Priority as GmailPriority};
use crate::mime::{CidImages, parse_message, snippet_text};
use crate::store::{
    DraftAttachmentItem, DraftRecord, DraftWrite, NewNotice, Priority as StorePriority, ReadPool,
    StoreError, WriteTx, Writer,
};

use super::super::drafts::{
    CarriedSource, DraftFiles, carried_sources_needing_download, sanitize_filename,
};
use super::super::events::Effect;
use super::super::writer_state::WriterState;
use super::ingest::current_rev;

#[derive(Debug)]
struct RemoteDraftData {
    gmail_draft_id: GmailDraftId,
    gmail_message_id: Option<GmailMessageId>,
    gmail_thread_id: Option<GmailThreadId>,
    sm_draft_id: Option<DraftId>,
    from: Option<Addr>,
    to: Vec<Addr>,
    cc: Vec<Addr>,
    bcc: Vec<Addr>,
    subject: String,
    body_html: String,
    quote_html: Option<String>,
    snippet: String,
    in_reply_to: Option<String>,
    references: Vec<String>,
    updated_at: Millis,
    attachments: Vec<DraftAttachmentItem>,
}

#[expect(
    clippy::too_many_lines,
    reason = "fetches full Gmail draft, resolves inline CID images, splits quote, and extracts carried attachments"
)]
async fn fetch_remote_draft(
    gmail: &Gmail,
    gid: &GmailDraftId,
    priority: GmailPriority,
) -> Result<Option<RemoteDraftData>, GmailError> {
    let draft = match gmail.get_draft(gid, Format::Full, priority).await {
        Ok(d) => d,
        Err(err) if err.outcome == Outcome::NotFound => return Ok(None),
        Err(err) => return Err(err),
    };
    let Some(msg) = draft.message else {
        return Ok(None);
    };
    let Some(payload) = msg.payload else {
        return Ok(None);
    };
    let parsed = parse_message(payload);
    let mut cids = CidImages::new();
    let mut attachments = Vec::new();
    for p in parsed.parts {
        if p.is_inline {
            if let Some(cid) = p.content_id.as_deref() {
                if let Some(bytes) = p.inline_data {
                    cids.insert(cid, p.mime_type, bytes);
                } else if let Some(att_id) = &p.attachment_id
                    && let Ok(bytes) = gmail.get_attachment(&msg.id, att_id, priority).await
                {
                    cids.insert(cid, p.mime_type, bytes);
                }
            }
        } else {
            attachments.push(DraftAttachmentItem {
                id: Uuid::new_v4().to_string(),
                filename: p.filename,
                mime_type: p.mime_type,
                size: p.size,
                path: None,
                source: None,
                gmail_att_id: p.attachment_id,
                added: None,
            });
        }
    }
    let body = parsed.raw_body.to_draft(&cids);
    let snippet = snippet_text(&body.html);
    Ok(Some(RemoteDraftData {
        gmail_draft_id: draft.id,
        gmail_message_id: Some(msg.id),
        gmail_thread_id: msg.thread_id,
        sm_draft_id: parsed.headers.draft_id,
        from: parsed.headers.from.into_iter().next(),
        to: parsed.headers.to,
        cc: parsed.headers.cc,
        bcc: parsed.headers.bcc,
        subject: parsed.headers.subject,
        body_html: body.html,
        quote_html: body.quote_html,
        snippet,
        in_reply_to: parsed.headers.in_reply_to,
        references: parsed.headers.references,
        updated_at: msg
            .internal_date
            .or(parsed.headers.date)
            .unwrap_or(Millis(0)),
        attachments,
    }))
}

async fn list_all_draft_stubs(
    gmail: &Gmail,
    priority: GmailPriority,
) -> Result<Vec<crate::gmail::Draft>, GmailError> {
    let mut remote_stubs = Vec::new();
    let mut page_token = None;
    loop {
        let page = gmail.list_drafts(page_token.as_ref(), priority).await?;
        remote_stubs.extend(page.drafts);
        if page.next_page_token.is_none() {
            break;
        }
        page_token = page.next_page_token;
    }
    Ok(remote_stubs)
}

fn should_skip_remote_draft(
    pool: &ReadPool,
    account: AccountId,
    stub: &crate::gmail::Draft,
) -> (bool, Option<Rev>) {
    pool.with(|db| {
        let ops = db.all_ops(account)?;
        let deleting = ops.iter().any(|op| {
            op.kind == OpKind::DraftDelete && op.gmail_draft_id.as_ref() == Some(&stub.id)
        });
        let outbox_ids = db.outbox_gmail_draft_ids(account)?;
        if deleting || outbox_ids.contains(&stub.id) {
            return Ok::<_, StoreError>((true, None));
        }
        let local = db.draft_by_gmail(account, &stub.id)?;
        if let Some(d) = local {
            let msg_matches = stub
                .message
                .as_ref()
                .zip(d.lifecycle.gmail_message_id.as_ref())
                .is_some_and(|(m, gid)| m.id == *gid);
            let has_upsert = db.has_draft_upsert(&d.id)?;
            return Ok((msg_matches || has_upsert, Some(d.rev)));
        }
        Ok((false, None))
    })
    .unwrap_or((false, None))
}

pub(crate) async fn reconcile_drafts_with_gmail(
    account: AccountId,
    gmail: &Gmail,
    pool: &ReadPool,
    writer: &Writer<WriterState>,
    priority: GmailPriority,
) -> Result<Vec<GmailThreadId>, GmailError> {
    let start_rev = current_rev(pool);
    let remote_stubs = list_all_draft_stubs(gmail, priority).await?;

    let remote_ids: Arc<HashSet<GmailDraftId>> =
        Arc::new(remote_stubs.iter().map(|d| d.id.clone()).collect());
    let mut missing_threads = Vec::new();
    let mut duplicates_to_delete = Vec::new();

    for stub in &remote_stubs {
        let before_rev = current_rev(pool);
        let (skip, existing_rev) = should_skip_remote_draft(pool, account, stub);
        if skip {
            continue;
        }

        let Some(remote) = fetch_remote_draft(gmail, &stub.id, priority).await? else {
            continue;
        };

        let remote_ids_ref = Arc::clone(&remote_ids);
        let res = writer
            .call(StorePriority::Sync, move |tx, state| {
                ingest_remote_draft_tx(
                    tx,
                    state,
                    account,
                    remote,
                    &remote_ids_ref,
                    before_rev,
                    existing_rev,
                )
            })
            .await;

        if let Ok(action) = res {
            match action {
                RemoteDraftAction::None => {}
                RemoteDraftAction::NeedThread(gtid) => missing_threads.push(gtid),
                RemoteDraftAction::DeleteDuplicate(gid) => duplicates_to_delete.push(gid),
            }
        }
    }

    for dup in duplicates_to_delete {
        if let Err(err) = gmail.delete_draft(&dup, priority).await
            && err.outcome != Outcome::NotFound
        {
            super::super::log_err::<(), _>("delete duplicate Gmail draft", Err(err));
        }
    }

    let remote_set = remote_ids;
    super::super::log_err(
        "prune deleted remote drafts",
        writer
            .call(StorePriority::Sync, move |tx, state| {
                prune_deleted_remote_drafts(tx, state, account, &remote_set, start_rev)
            })
            .await,
    );

    Ok(missing_threads)
}

#[expect(
    clippy::too_many_arguments,
    reason = "reloads a single draft from Gmail after an open composer closed untouched"
)]
pub(crate) async fn reload_single_draft_from_gmail(
    account: AccountId,
    draft_id: DraftId,
    gmail_draft_id: &GmailDraftId,
    gmail: &Gmail,
    pool: &ReadPool,
    writer: &Writer<WriterState>,
    priority: GmailPriority,
) {
    let before_rev = current_rev(pool);
    let Ok(Some(remote)) = fetch_remote_draft(gmail, gmail_draft_id, priority).await else {
        return;
    };
    super::super::log_err(
        "reload draft from Gmail",
        writer
            .call(StorePriority::Sync, move |tx, state| {
                let Some(local) = tx.read().draft(&draft_id)? else {
                    return Ok::<_, StoreError>(());
                };
                if local.in_outbox
                    || local.rev > before_rev
                    || tx.read().has_draft_upsert(&local.id)?
                    || state.registry.is_draft_open(&local.id)
                {
                    return Ok(());
                }
                apply_remote_draft_update(tx, account, Some(local), remote)?;
                Ok(())
            })
            .await,
    );
}

enum RemoteDraftAction {
    None,
    NeedThread(GmailThreadId),
    DeleteDuplicate(GmailDraftId),
}

#[expect(
    clippy::too_many_arguments,
    clippy::too_many_lines,
    reason = "checks delete/outbox guards, duplicate X-Supermonkey-Draft, open composer conflict, and local edits"
)]
fn ingest_remote_draft_tx(
    tx: &mut WriteTx<'_>,
    state: &WriterState,
    account: AccountId,
    remote: RemoteDraftData,
    all_remote_ids: &HashSet<GmailDraftId>,
    before_rev: Rev,
    existing_rev: Option<Rev>,
) -> Result<RemoteDraftAction, StoreError> {
    if tx.read().account(account)?.is_none() {
        return Ok(RemoteDraftAction::None);
    }

    let ops = tx.read().all_ops(account)?;
    let deleting = ops.iter().any(|op| {
        op.kind == OpKind::DraftDelete
            && (op.gmail_draft_id.as_ref() == Some(&remote.gmail_draft_id)
                || remote
                    .sm_draft_id
                    .as_ref()
                    .is_some_and(|sid| op.draft_id.as_ref() == Some(sid)))
    });
    if deleting {
        return Ok(RemoteDraftAction::None);
    }

    let outbox_ids = tx.read().outbox_gmail_draft_ids(account)?;
    if outbox_ids.contains(&remote.gmail_draft_id) {
        return Ok(RemoteDraftAction::None);
    }

    let existing_by_gid = tx.read().draft_by_gmail(account, &remote.gmail_draft_id)?;
    let existing_by_sm = match &remote.sm_draft_id {
        Some(sid) => tx.read().draft(sid)?,
        None => None,
    };

    if let Some(local) = &existing_by_sm
        && let Some(local_gid) = &local.lifecycle.gmail_draft_id
        && *local_gid != remote.gmail_draft_id
        && all_remote_ids.contains(local_gid)
    {
        return Ok(RemoteDraftAction::DeleteDuplicate(remote.gmail_draft_id));
    }

    let local = existing_by_gid.or(existing_by_sm);
    if let Some(mut rec) = local {
        if rec.in_outbox || tx.read().has_draft_upsert(&rec.id)? {
            return Ok(RemoteDraftAction::None);
        }
        if rec.rev > before_rev || existing_rev.is_some_and(|r| rec.rev > r) {
            return Ok(RemoteDraftAction::None);
        }
        if state.registry.is_draft_open(&rec.id) {
            let changed = rec.body_html != remote.body_html
                || rec.subject != remote.subject
                || rec.to != remote.to
                || rec.cc != remote.cc
                || rec.bcc != remote.bcc;
            if changed {
                rec.lifecycle.mark_gmail_change_skipped();
                rec.lifecycle.record_gmail_ids(
                    remote.gmail_draft_id,
                    remote.gmail_message_id,
                    remote.gmail_thread_id,
                );
                tx.put_draft(&rec.as_write())?;
            }
            return Ok(RemoteDraftAction::None);
        }
        let need_thread = apply_remote_draft_update(tx, account, Some(rec), remote)?;
        return Ok(need_thread.map_or(RemoteDraftAction::None, RemoteDraftAction::NeedThread));
    }

    let need_thread = apply_remote_draft_update(tx, account, None, remote)?;
    Ok(need_thread.map_or(RemoteDraftAction::None, RemoteDraftAction::NeedThread))
}

#[expect(
    clippy::too_many_lines,
    reason = "merges remote draft metadata, target aim, lifecycle state, and existing local attachment paths"
)]
fn apply_remote_draft_update(
    tx: &mut WriteTx<'_>,
    account: AccountId,
    existing: Option<DraftRecord>,
    remote: RemoteDraftData,
) -> Result<Option<GmailThreadId>, StoreError> {
    let mut need_thread = None;
    let thread_id = match &remote.gmail_thread_id {
        Some(gtid) => {
            let found = tx.read().thread_by_gmail(account, gtid)?;
            if found.is_none() {
                need_thread = Some(gtid.clone());
            }
            found
        }
        None => None,
    };

    let (mut kind, target, started_on_latest) = if let Some(tid) = thread_id {
        let thread = tx.read().thread_facts(tid)?;
        let aim = gmail_draft_aim(
            Some(&thread),
            &remote.subject,
            remote.in_reply_to.as_deref(),
            &remote.references,
        );
        let on_latest = match aim.target {
            Some(target_id) => {
                let target_date = thread
                    .messages()
                    .iter()
                    .find(|m| m.id == target_id)
                    .map(|m| m.date);
                thread.latest_live().map(|m| m.id) == Some(target_id)
                    || target_date.is_some_and(|tdate| {
                        !thread
                            .messages()
                            .iter()
                            .any(|m| m.is_live() && m.date > tdate && m.date <= remote.updated_at)
                    })
            }
            None => false,
        };
        (aim.kind, aim.target, on_latest)
    } else if remote.gmail_thread_id.is_some() {
        let k = gmail_draft_kind(true, &remote.subject, remote.in_reply_to.as_deref());
        (k, None, false)
    } else {
        (DraftKind::New, None, false)
    };

    let (id, reply_all, mut lifecycle, prev_atts) = if let Some(prev) = existing {
        if prev.thread == thread_id {
            kind = prev.kind;
        }
        (prev.id, prev.reply_all, prev.lifecycle, prev.attachments)
    } else {
        let id = remote
            .sm_draft_id
            .unwrap_or_else(|| DraftId(Uuid::new_v4().to_string()));
        (id, false, DraftLifecycle::kept(), Vec::new())
    };
    lifecycle.keep();
    lifecycle.gmail_change_skipped = false;
    lifecycle.record_gmail_ids(
        remote.gmail_draft_id,
        remote.gmail_message_id,
        remote.gmail_thread_id,
    );

    let attachments: Vec<DraftAttachmentItem> = remote
        .attachments
        .into_iter()
        .map(|mut item| {
            if let Some(prev) = prev_atts
                .iter()
                .find(|p| p.filename == item.filename && p.size == item.size && p.path.is_some())
            {
                item.id.clone_from(&prev.id);
                item.path.clone_from(&prev.path);
                item.source = prev.source;
            }
            item
        })
        .collect();

    tx.put_draft(&DraftWrite {
        id: &id,
        account,
        kind,
        reply_all,
        thread: thread_id,
        target,
        started_on_latest,
        from: remote.from.as_ref(),
        to: &remote.to,
        cc: &remote.cc,
        bcc: &remote.bcc,
        subject: &remote.subject,
        body_html: &remote.body_html,
        snippet: &remote.snippet,
        quote_html: remote.quote_html.as_deref(),
        attachments: &attachments,
        in_reply_to: remote.in_reply_to.as_deref(),
        references: &remote.references,
        updated_at: remote.updated_at,
        lifecycle: &lifecycle,
    })?;
    Ok(need_thread)
}

fn prune_deleted_remote_drafts(
    tx: &mut WriteTx<'_>,
    state: &mut WriterState,
    account: AccountId,
    remote_ids: &HashSet<GmailDraftId>,
    start_rev: Rev,
) -> Result<(), StoreError> {
    let locals = tx.read().drafts_for_account(account)?;
    for mut local in locals {
        if local.in_outbox || local.rev > start_rev {
            continue;
        }
        let Some(gid) = &local.lifecycle.gmail_draft_id else {
            continue;
        };
        if remote_ids.contains(gid) || tx.read().has_draft_upsert(&local.id)? {
            continue;
        }
        if state.registry.is_draft_open(&local.id) {
            local.lifecycle.clear_gmail_ids();
            local.lifecycle.gmail_change_skipped = false;
            tx.put_draft(&local.as_write())?;
            tx.add_notice(&NewNotice {
                account,
                kind: crate::domain::NoticeKind::DraftGone,
                message: "Draft was sent or deleted elsewhere",
                draft: Some(&local.id),
                thread: local.thread,
            })?;
        } else {
            tx.delete_draft(&local.id)?;
            state.emit(Effect::RemoveDraftFiles(local.id));
        }
    }
    Ok(())
}

#[expect(
    clippy::too_many_arguments,
    clippy::too_many_lines,
    reason = "downloads carried message and Gmail-draft attachments to disk and persists local paths"
)]
pub(crate) async fn download_carried_draft_attachments(
    account: AccountId,
    draft_id: &DraftId,
    gmail: &Gmail,
    pool: &ReadPool,
    writer: &Writer<WriterState>,
    files: &DraftFiles,
    priority: GmailPriority,
) -> Result<(), GmailError> {
    let Some(draft) = pool.with(|db| db.draft(draft_id)).ok().flatten() else {
        return Ok(());
    };
    if draft.account != account {
        return Ok(());
    }
    let missing = carried_sources_needing_download(draft.attachments);
    if missing.is_empty() {
        return Ok(());
    }

    let mut downloaded: Vec<(String, String)> = Vec::new();
    for (att_id, source, filename) in missing {
        let fetched = match source {
            CarriedSource::MessageAttachment(aid) => {
                let info = pool.with(|db| db.attachment(aid)).ok().flatten();
                let Some((_, gmid, rec)) = info else {
                    continue;
                };
                if let Some(existing) = rec.local_path
                    && Path::new(&existing).exists()
                {
                    downloaded.push((att_id, existing));
                    continue;
                }
                let Some(gatt_id) = rec.gmail_att_id else {
                    continue;
                };
                gmail.get_attachment(&gmid, &gatt_id, priority).await?
            }
            CarriedSource::GmailDraftAttachment(gatt_id) => {
                let Some(gmid) = &draft.lifecycle.gmail_message_id else {
                    continue;
                };
                gmail.get_attachment(gmid, &gatt_id, priority).await?
            }
        };
        let clean = sanitize_filename(&filename);
        let Ok(path) = files.store_carried(draft_id, &att_id, &clean, &fetched) else {
            continue;
        };
        downloaded.push((att_id, path.to_string_lossy().into_owned()));
    }

    if downloaded.is_empty() {
        return Ok(());
    }
    let did = draft.id;
    super::super::log_err(
        "store downloaded carried attachment paths",
        writer
            .call(StorePriority::Sync, move |tx, _state| {
                let Some(mut rec) = tx.read().draft(&did)? else {
                    return Ok::<_, StoreError>(());
                };
                let mut by_id: HashMap<String, String> = downloaded.into_iter().collect();
                let mut changed = false;
                for att in &mut rec.attachments {
                    if att.path.is_none()
                        && let Some(p) = by_id.remove(&att.id)
                    {
                        att.path = Some(p);
                        changed = true;
                    }
                }
                if changed {
                    tx.put_draft(&rec.as_write())?;
                }
                Ok(())
            })
            .await,
    );
    Ok(())
}
