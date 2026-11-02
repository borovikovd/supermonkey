//! Drafts on the Gmail side (SPEC Compose → Drafts, Gmail errors).
//!
//! Upserts replace the whole message, so every upsert carries all of the
//! draft's attachments. `drafts.create` is unsafe to repeat: every draft this
//! app writes carries `X-Ubermensch-Draft` with the local id, which is how a
//! create with no clear answer is recovered.

use crate::LogErr as _;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use rusqlite::{OptionalExtension, params};
use ub_gmail::{Format, GmailError, Outcome, Priority, cost};

use super::ops::{Done, bad};
use super::{AccountRt, EngineRef};
use crate::db::{from_json, to_json};
use crate::model::{AccountId, Addr, DraftAttachment, DraftId, DraftKind, MessageId};

/// A draft as stored, for building its MIME.
pub struct DraftRow {
    pub id: DraftId,
    pub account: AccountId,
    pub from: Option<Addr>,
    pub to: Vec<Addr>,
    pub cc: Vec<Addr>,
    pub bcc: Vec<Addr>,
    pub subject: String,
    pub body_html: String,
    pub quote_html: Option<String>,
    pub attachments: Vec<DraftAttachment>,
    pub in_reply_to: Option<String>,
    pub references: String,
    pub gmail_draft_id: Option<String>,
    pub gmail_thread_id: Option<String>,
    pub target: Option<MessageId>,
    pub needs_parts: bool,
}

pub fn load(e: &EngineRef, draft: &str) -> Option<DraftRow> {
    e.reads
        .with(|c| {
            Ok(c.query_row(
                "SELECT d.account_id, d.from_json, d.to_json, d.cc_json, d.bcc_json, d.subject, d.body_html, d.quote_html,
                        d.attachments_json, d.in_reply_to, d.references_hdr, d.gmail_draft_id,
                        coalesce(t.gmail_id, d.gmail_thread_id), d.target_message_id, d.needs_parts
                 FROM drafts d LEFT JOIN threads t ON t.id = d.thread_id WHERE d.id = ?1",
                [draft],
                |r| {
                    Ok(DraftRow {
                        id: draft.to_string(),
                        account: r.get(0)?,
                        from: serde_json::from_str(&r.get::<_, String>(1)?).unwrap_or(None),
                        to: from_json(&r.get::<_, String>(2)?),
                        cc: from_json(&r.get::<_, String>(3)?),
                        bcc: from_json(&r.get::<_, String>(4)?),
                        subject: r.get(5)?,
                        body_html: r.get(6)?,
                        quote_html: r.get(7)?,
                        attachments: from_json(&r.get::<_, String>(8)?),
                        in_reply_to: r.get(9)?,
                        references: r.get(10)?,
                        gmail_draft_id: r.get(11)?,
                        gmail_thread_id: r.get(12)?,
                        target: r.get(13)?,
                        needs_parts: r.get(14)?,
                    })
                },
            )
            .optional()?)
        })
        .ok()
        .flatten()
}

fn mime_addr(a: &Addr) -> ub_mime::Addr {
    ub_mime::Addr {
        name: a.name.clone(),
        email: a.email.clone(),
    }
}

fn io(e: impl std::fmt::Display) -> GmailError {
    // A local file problem is not Gmail's; surface it as a rejection.
    bad(e)
}

/// Bytes of every attachment, downloading those not cached yet.
async fn attachment_bytes(
    e: &EngineRef,
    d: &DraftRow,
) -> Result<Vec<ub_mime::OutAttachment>, GmailError> {
    let mut out = Vec::new();
    for a in &d.attachments {
        let path = match (&a.path, a.source) {
            (Some(p), _) if std::path::Path::new(p).exists() => std::path::PathBuf::from(p),
            (_, Some(src)) => super::attachments::ensure(e, src)
                .await
                .map_err(|e| offline_or_bad(&e))?,
            _ => continue,
        };
        out.push(ub_mime::OutAttachment {
            filename: a.filename.clone(),
            mime_type: a.mime_type.clone(),
            data: std::fs::read(&path).map_err(io)?,
        });
    }
    Ok(out)
}

fn offline_or_bad(err: &anyhow::Error) -> GmailError {
    let s = err.to_string();
    if s.contains("Offline") || s.contains("offline") {
        GmailError::new(Outcome::Offline)
    } else if s.contains("NoResponse") || s.contains("RateLimited") || s.contains("ServerError") {
        GmailError::new(Outcome::NoResponse)
    } else {
        bad(s)
    }
}

/// Inline parts of the quoted message, for turning `data:` images back into `cid:`.
async fn quoted_inline(
    e: &EngineRef,
    d: &DraftRow,
) -> Result<Vec<ub_mime::InlinePart>, GmailError> {
    let html = format!(
        "{}{}",
        d.body_html,
        d.quote_html.clone().unwrap_or_default()
    );
    let Some(target) = d.target else {
        return Ok(vec![]);
    };
    if !html.contains("data:") {
        return Ok(vec![]);
    }
    let parts: Vec<(i64, String, String, String)> = e
        .reads
        .with(|c| {
            let mut stmt = c.prepare(
                "SELECT id, content_id, mime_type, filename FROM attachments WHERE message_id = ?1 AND content_id IS NOT NULL",
            )?;
            let rows = stmt.query_map([target], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?;
            Ok(rows.collect::<Result<_, _>>()?)
        })
        .unwrap_or_default();
    let mut out = Vec::new();
    for (id, cid, mime, name) in parts {
        let path = super::attachments::ensure(e, id)
            .await
            .map_err(|e| offline_or_bad(&e))?;
        out.push(ub_mime::InlinePart {
            content_id: cid,
            mime_type: mime,
            filename: Some(name),
            data: std::fs::read(path).map_err(io)?,
        });
    }
    Ok(out)
}

/// Builds the draft's MIME with mail-builder (via ub-mime).
pub async fn build_raw(e: &EngineRef, d: &DraftRow) -> Result<Vec<u8>, GmailError> {
    let attachments = attachment_bytes(e, d).await?;
    let known = quoted_inline(e, d).await?;
    let html = format!(
        "{}{}",
        d.body_html,
        d.quote_html.clone().unwrap_or_default()
    );
    let (html, inline) = ub_mime::data_images_to_cid(&html, &known);
    let from = d.from.clone().unwrap_or(Addr {
        name: None,
        email: String::new(),
    });
    let domain = from
        .email
        .split('@')
        .nth(1)
        .unwrap_or("ubermensch.local")
        .to_string();
    Ok(ub_mime::build_mime(&ub_mime::Outgoing {
        from: mime_addr(&from),
        to: d.to.iter().map(mime_addr).collect(),
        cc: d.cc.iter().map(mime_addr).collect(),
        bcc: d.bcc.iter().map(mime_addr).collect(),
        subject: d.subject.clone(),
        html,
        in_reply_to: d.in_reply_to.clone(),
        references: d
            .references
            .split_whitespace()
            .map(str::to_string)
            .collect(),
        ub_draft_id: d.id.clone(),
        attachments,
        inline,
        date_ms: crate::db::now_ms(),
        message_id_domain: domain,
    }))
}

/// Writes the draft to Gmail with `drafts.create` or `drafts.update`.
/// Returns the Gmail draft.
pub async fn write(
    e: &EngineRef,
    rt: &Arc<AccountRt>,
    d: &DraftRow,
) -> Result<ub_gmail::Draft, GmailError> {
    let raw = build_raw(e, d).await?;
    let thread = d.gmail_thread_id.as_deref();
    if let Some(id) = &d.gmail_draft_id {
        match rt
            .sched
            .run(
                Priority::P1,
                cost::DRAFTS_UPDATE,
                rt.gmail.update_draft(id, &raw, thread),
            )
            .await
        {
            Ok(g) => return Ok(g),
            // Sent or deleted elsewhere: create a new one below.
            Err(err) if matches!(err.outcome, Outcome::NotFound) => {}
            Err(err) => return Err(err),
        }
    }
    create_with_recovery(e, rt, d, &raw).await
}

async fn create_with_recovery(
    e: &EngineRef,
    rt: &Arc<AccountRt>,
    d: &DraftRow,
    raw: &[u8],
) -> Result<ub_gmail::Draft, GmailError> {
    let thread = d.gmail_thread_id.as_deref();
    for _ in 0..3 {
        match rt
            .sched
            .run(
                Priority::P1,
                cost::DRAFTS_CREATE,
                rt.gmail.create_draft(raw, thread),
            )
            .await
        {
            Ok(g) => return Ok(g),
            Err(err)
                if matches!(
                    err.outcome,
                    Outcome::NoResponse | Outcome::ServerError { .. }
                ) =>
            {
                if let Some(found) = recover_create(e, rt, &d.id).await? {
                    return Ok(found);
                }
            }
            Err(err) => return Err(err),
        }
    }
    Err(GmailError::new(Outcome::NoResponse))
}

/// After a `drafts.create` with no clear answer: did sync already store Gmail
/// ids, or does Gmail hold a draft with this local id? Extra copies are deleted.
async fn recover_create(
    e: &EngineRef,
    rt: &Arc<AccountRt>,
    local: &str,
) -> Result<Option<ub_gmail::Draft>, GmailError> {
    if let Some(d) = load(e, local)
        && let Some(gid) = d.gmail_draft_id
    {
        let g = rt
            .sched
            .run(
                Priority::P1,
                cost::drafts_get(Format::Metadata),
                rt.gmail.get_draft(&gid, Format::Metadata),
            )
            .await?;
        return Ok(Some(g));
    }
    let known: HashSet<String> = e
        .reads
        .with(|c| {
            let mut stmt = c.prepare("SELECT gmail_draft_id FROM drafts WHERE account_id = ?1 AND gmail_draft_id IS NOT NULL")?;
            let rows = stmt.query_map([rt.id], |r| r.get(0))?;
            Ok(rows.collect::<Result<_, _>>()?)
        })
        .unwrap_or_default();
    let mut ours: Vec<ub_gmail::Draft> = Vec::new();
    for gid in list_all(rt)
        .await?
        .into_iter()
        .map(|d| d.id)
        .filter(|id| !known.contains(id))
    {
        let g = rt
            .sched
            .run(
                Priority::P1,
                cost::drafts_get(Format::Metadata),
                rt.gmail.get_draft(&gid, Format::Metadata),
            )
            .await?;
        let header = g
            .message
            .payload
            .as_ref()
            .map(ub_mime::parse_gmail_payload)
            .and_then(|p| p.ub_draft_id);
        if header.as_deref() == Some(local) {
            ours.push(g);
        }
    }
    let mut it = ours.into_iter();
    let keep = it.next();
    for extra in it {
        rt.sched
            .run(
                Priority::P1,
                cost::DRAFTS_DELETE,
                rt.gmail.delete_draft(&extra.id),
            )
            .await
            .log_err("recover_create");
    }
    Ok(keep)
}

async fn list_all(rt: &Arc<AccountRt>) -> Result<Vec<ub_gmail::Draft>, GmailError> {
    let mut out = Vec::new();
    let mut page: Option<String> = None;
    loop {
        let res = rt
            .sched
            .run(
                Priority::P2,
                cost::DRAFTS_LIST,
                rt.gmail.list_drafts(page.as_deref()),
            )
            .await?;
        out.extend(res.drafts);
        match res.next_page_token {
            Some(p) => page = Some(p),
            None => return Ok(out),
        }
    }
}

/// Stores the Gmail ids an upsert returned.
pub async fn store_ids(e: &EngineRef, local: &str, g: &ub_gmail::Draft) {
    let (id, msg, thread) = (
        g.id.clone(),
        g.message.id.clone(),
        g.message.thread_id.clone(),
    );
    let local = local.to_string();
    e
        .writer
        .call(move |ctx| {
            let account: Option<i64> = ctx
                .tx
                .query_row("SELECT account_id FROM drafts WHERE id = ?1", [&local], |r| r.get(0))
                .optional()?;
            let Some(account) = account else { return Ok(()) };
            // Another live Gmail draft for the same local draft is an extra copy.
            ctx.tx.execute(
                "UPDATE drafts SET gmail_draft_id = ?2, gmail_message_id = ?3, gmail_thread_id = ?4 WHERE id = ?1",
                params![local, id, msg, thread],
            )?;
            ctx.touch_draft(account, &local)?;
            Ok(())
        })
        .await
        .log_err("store_ids");
}

pub async fn execute_op(
    e: &EngineRef,
    rt: &Arc<AccountRt>,
    kind: &str,
    payload: &str,
) -> Result<Done, GmailError> {
    let v: serde_json::Value = serde_json::from_str(payload).map_err(bad)?;
    match kind {
        "draft_upsert" => {
            let local = v["draft"].as_str().unwrap_or_default();
            let Some(d) = load(e, local) else {
                return Ok(Done::Finished);
            };
            let in_outbox = e
                .reads
                .with(|c| {
                    Ok(c.query_row(
                        "SELECT EXISTS (SELECT 1 FROM outbox WHERE draft_id = ?1)",
                        [local],
                        |r| r.get(0),
                    )?)
                })
                .unwrap_or(false);
            if in_outbox {
                return Ok(Done::Finished);
            }
            if d.needs_parts {
                download_parts_now(e, rt, local).await?;
            }
            let Some(d) = load(e, local) else {
                return Ok(Done::Finished);
            };
            let g = write(e, rt, &d).await?;
            store_ids(e, local, &g).await;
            Ok(Done::Finished)
        }
        "draft_delete" => {
            let gid = v["gmail_draft"].as_str().unwrap_or_default();
            match rt
                .sched
                .run(
                    Priority::P1,
                    cost::DRAFTS_DELETE,
                    rt.gmail.delete_draft(gid),
                )
                .await
            {
                Ok(()) => Ok(Done::Finished),
                Err(err) if matches!(err.outcome, Outcome::NotFound) => Ok(Done::Finished),
                Err(err) => Err(err),
            }
        }
        "trash_message" => {
            let id = v["message"].as_str().unwrap_or_default();
            rt.sched
                .run(
                    Priority::P1,
                    cost::MESSAGES_TRASH,
                    rt.gmail.trash_message(id),
                )
                .await?;
            Ok(Done::Finished)
        }
        other => Err(bad(format!("unknown op {other}"))),
    }
}

/// Downloads a Gmail draft's attachments and inline parts before its first
/// upsert (SPEC Addressing).
async fn download_parts_now(
    e: &EngineRef,
    _rt: &Arc<AccountRt>,
    draft: &str,
) -> Result<(), GmailError> {
    let Some(d) = load(e, draft) else {
        return Ok(());
    };
    let mut atts = d.attachments.clone();
    for a in &mut atts {
        if a.path.is_none()
            && let Some(src) = a.source
        {
            let p = super::attachments::ensure(e, src)
                .await
                .map_err(|e| offline_or_bad(&e))?;
            a.path = Some(p.to_string_lossy().to_string());
        }
    }
    let local = draft.to_string();
    e.writer
        .call(move |ctx| {
            ctx.tx.execute(
                "UPDATE drafts SET attachments_json = ?2, needs_parts = 0 WHERE id = ?1",
                params![local, to_json(&atts)],
            )?;
            Ok(())
        })
        .await
        .log_err("download_parts_now");
    Ok(())
}

pub async fn download_parts(e: EngineRef, rt: Arc<AccountRt>, draft: DraftId) {
    let _ = download_parts_now(&e, &rt, &draft).await;
}

/// A Gmail change skipped while a composer had the draft open, read when that
/// composer closes without edits.
pub async fn read_skipped(e: EngineRef, rt: Arc<AccountRt>, draft: DraftId) {
    let Some(d) = load(&e, &draft) else { return };
    let Some(gid) = d.gmail_draft_id else { return };
    if let Ok(g) = rt
        .sched
        .run(
            Priority::P2,
            cost::drafts_get(Format::Full),
            rt.gmail.get_draft(&gid, Format::Full),
        )
        .await
    {
        store_gmail_draft(&e, &rt, g).await;
    }
}

/// Drafts deleted on Gmail (for example sent from a phone) are deleted here,
/// unless a composer has one open: then it keeps its text, loses its Gmail
/// id, and a notice says so (SPEC Drafts).
fn drop_gone(
    ctx: &mut crate::writer::Ctx<'_>,
    account: AccountId,
    gone: Vec<String>,
) -> anyhow::Result<()> {
    for d in gone {
        if ctx.subs.is_draft_open(&d) {
            // The composer stays; the next upsert uses drafts.create.
            ctx.tx.execute(
                "UPDATE drafts SET gmail_draft_id = NULL, gmail_message_id = NULL WHERE id = ?1",
                [&d],
            )?;
            ctx.tx.execute(
                "INSERT INTO notices (account_id, kind, message, draft_id, created_at)
                 VALUES (?1, 'draft_gone', 'Draft was sent or deleted elsewhere', ?2, ?3)",
                params![account, d, ctx.now],
            )?;
            ctx.touch_draft(account, &d)?;
        } else {
            let thread: Option<i64> = ctx
                .tx
                .query_row("SELECT thread_id FROM drafts WHERE id = ?1", [&d], |r| {
                    r.get(0)
                })
                .optional()?
                .flatten();
            ctx.tx.execute("DELETE FROM drafts WHERE id = ?1", [&d])?;
            ctx.touched.drafts.insert(d);
            ctx.touched.draft_lists.insert(account);
            if let Some(t) = thread {
                crate::threads::refresh(ctx, t)?;
            }
        }
        ctx.touch_summary(account);
    }
    Ok(())
}

/// Reconciles local drafts with `drafts.list` (SPEC Drafts).
pub async fn sync_list(e: &EngineRef, rt: &Arc<AccountRt>) -> Result<(), GmailError> {
    let gmail = list_all(rt).await?;
    let local: Vec<(String, Option<String>, Option<String>)> = e
        .reads
        .with(|c| {
            let mut stmt = c.prepare(
                "SELECT id, gmail_draft_id, gmail_message_id FROM drafts d WHERE account_id = ?1
                   AND NOT EXISTS (SELECT 1 FROM outbox o WHERE o.draft_id = d.id)",
            )?;
            let rows = stmt.query_map([rt.id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
            Ok(rows.collect::<Result<_, _>>()?)
        })
        .unwrap_or_default();
    let by_gmail: HashMap<String, (String, Option<String>)> = local
        .iter()
        .filter_map(|(id, g, m)| g.clone().map(|g| (g, (id.clone(), m.clone()))))
        .collect();
    let live: HashSet<String> = gmail.iter().map(|d| d.id.clone()).collect();

    // Drafts deleted on Gmail (for example sent from a phone).
    let gone: Vec<String> = local
        .iter()
        .filter(|(_, g, _)| g.as_ref().is_some_and(|g| !live.contains(g)))
        .map(|(id, _, _)| id.clone())
        .collect();
    if !gone.is_empty() {
        let account = rt.id;
        e.writer
            .call(move |ctx| drop_gone(ctx, account, gone))
            .await
            .log_err("sync_list");
    }

    // Drafts this app does not know, or whose message changed, are read again.
    for g in gmail {
        let changed = match by_gmail.get(&g.id) {
            None => true,
            Some((_, msg)) => msg.as_deref() != Some(g.message.id.as_str()),
        };
        if !changed {
            continue;
        }
        if let Some((local, _)) = by_gmail.get(&g.id) {
            let open = {
                let l = local.clone();
                e.writer
                    .call(move |ctx| Ok(ctx.subs.is_draft_open(&l)))
                    .await
                    .unwrap_or(false)
            };
            if open {
                let l = local.clone();
                e.writer
                    .call(move |ctx| {
                        ctx.tx.execute(
                            "UPDATE drafts SET pending_gmail_read = 1 WHERE id = ?1",
                            [&l],
                        )?;
                        Ok(())
                    })
                    .await
                    .log_err("sync_list");
                continue;
            }
        }
        match rt
            .sched
            .run(
                Priority::P2,
                cost::drafts_get(Format::Full),
                rt.gmail.get_draft(&g.id, Format::Full),
            )
            .await
        {
            Ok(full) => store_gmail_draft(e, rt, full).await,
            Err(err) if matches!(err.outcome, Outcome::NotFound) => {}
            Err(err) => return Err(err),
        }
    }
    Ok(())
}

/// The message a draft read from Gmail is aimed at: the one its `In-Reply-To`
/// names, else the thread's latest. It counts as started on the latest message
/// when its target was the newest non-draft message older than the draft.
fn gmail_draft_target(
    ctx: &crate::writer::Ctx<'_>,
    thread: crate::model::ThreadId,
    others: &[crate::threads::Msg],
    m: &super::ingest::Incoming,
) -> anyhow::Result<(Option<MessageId>, bool)> {
    let by_reply = match &m.in_reply_to {
        Some(irt) => ctx
            .tx
            .query_row(
                "SELECT id FROM messages WHERE thread_id = ?1 AND message_id_hdr = ?2 AND is_draft = 0",
                params![thread, irt.trim_matches(['<', '>'])],
                |r| r.get::<_, i64>(0),
            )
            .optional()?,
        None => None,
    };
    let target = by_reply.or_else(|| crate::threads::latest(others).map(|x| x.id));
    let newest_older = others
        .iter()
        .rev()
        .find(|x| x.date <= m.internal_date)
        .map(|x| x.id);
    Ok((target, target.is_some() && target == newest_older))
}

/// A draft's kind is decided once, when it first appears from Gmail: new
/// when alone in its thread with no `In-Reply-To`, forward for `Fwd:`, else reply.
fn gmail_draft_kind(alone: bool, m: &super::ingest::Incoming) -> DraftKind {
    if alone && m.in_reply_to.is_none() {
        DraftKind::New
    } else if m.subject.trim_start().to_lowercase().starts_with("fwd:") {
        DraftKind::Forward
    } else {
        DraftKind::Reply
    }
}

/// The stored message row for a Gmail draft's message, and the local draft
/// it belongs to (by Gmail draft id or by its `X-Ubermensch-Draft` header).
fn gmail_draft_rows(
    ctx: &crate::writer::Ctx<'_>,
    account: AccountId,
    m: &super::ingest::Incoming,
    gmail_draft: &str,
) -> anyhow::Result<(Option<i64>, Option<String>)> {
    let msg_row = ctx
        .tx
        .query_row(
            "SELECT id FROM messages WHERE account_id = ?1 AND gmail_id = ?2",
            params![account, m.gmail_id],
            |r| r.get(0),
        )
        .optional()?;
    let existing = ctx
        .tx
        .query_row(
            "SELECT id FROM drafts WHERE account_id = ?1 AND (gmail_draft_id = ?2 OR id = ?3)",
            params![
                account,
                gmail_draft,
                m.ub_draft_id.clone().unwrap_or_default()
            ],
            |r| r.get(0),
        )
        .optional()?;
    Ok((msg_row, existing))
}

/// Creates or updates the local draft for a Gmail draft. Its kind is decided
/// once, when it first appears: new, forward (`Fwd:`), or reply.
pub async fn store_gmail_draft(e: &EngineRef, rt: &Arc<AccountRt>, g: ub_gmail::Draft) {
    let m = super::ingest::parse(&g.message, true, &HashMap::new());
    let raw_html = g
        .message
        .payload
        .as_ref()
        .map(ub_mime::parse_gmail_payload)
        .map(|p| {
            p.html
                .unwrap_or_else(|| ub_mime::text_to_html(&p.text.unwrap_or_default()))
        })
        .unwrap_or_default();
    let (above, quote) = ub_mime::split_gmail_quote(&raw_html);
    let body = ub_mime::sanitize_html(&above, &HashMap::new());
    let quote = quote.map(|q| ub_mime::sanitize_html(&q, &HashMap::new()));
    let account = rt.id;
    let gmail_draft = g.id.clone();
    e
        .writer
        .call(move |ctx| {
            // The thread and message rows for the draft (and its siblings).
            let pending = super::ingest::PendingOps::load(ctx.tx, account)?;
            let thread = super::ingest::store_thread(ctx, account, &m.gmail_thread_id, None, std::slice::from_ref(&m), false, &pending)?;
            let (msg_row, existing) = gmail_draft_rows(ctx, account, &m, &gmail_draft)?;
            let others: Vec<crate::threads::Msg> =
                crate::threads::messages(ctx.tx, thread)?.into_iter().filter(|x| !x.is_draft).collect();
            let (target, started_on_latest) = gmail_draft_target(ctx, thread, &others, &m)?;
            let atts = match msg_row {
                Some(row) => crate::drafts::message_attachments(ctx, row)?,
                None => vec![],
            };
            let from = m.from.clone();
            if let Some(local) = existing {
                ctx.tx.execute(
                    "UPDATE drafts SET from_json = ?2, to_json = ?3, cc_json = ?4, bcc_json = ?5, subject = ?6,
                        body_html = ?7, quote_html = ?8, attachments_json = ?9, gmail_draft_id = ?10,
                        gmail_message_id = ?11, gmail_thread_id = ?12, thread_id = ?13, pending_gmail_read = 0,
                        needs_parts = ?14, updated_at = ?15
                     WHERE id = ?1",
                    params![
                        local,
                        to_json(&from),
                        to_json(&m.to),
                        to_json(&m.cc),
                        to_json(&m.bcc),
                        m.subject,
                        body,
                        quote,
                        to_json(&atts),
                        gmail_draft,
                        m.gmail_id,
                        m.gmail_thread_id,
                        thread,
                        !atts.is_empty(),
                        m.internal_date,
                    ],
                )?;
                ctx.touch_draft(account, &local)?;
            } else {
                let kind = gmail_draft_kind(others.is_empty(), &m);
                let local = m.ub_draft_id.clone().unwrap_or_else(|| format!("g-{gmail_draft}"));
                ctx.tx.execute(
                    "INSERT INTO drafts (id, account_id, kind, thread_id, target_message_id, started_on_latest,
                        from_json, to_json, cc_json, bcc_json, subject, body_html, quote_html, attachments_json,
                        in_reply_to, references_hdr, gmail_draft_id, gmail_message_id, gmail_thread_id, needs_parts,
                        updated_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21)",
                    params![
                        local,
                        account,
                        kind.as_str(),
                        if kind == DraftKind::New { None } else { Some(thread) },
                        if kind == DraftKind::New { None } else { target },
                        started_on_latest,
                        to_json(&from),
                        to_json(&m.to),
                        to_json(&m.cc),
                        to_json(&m.bcc),
                        m.subject,
                        body,
                        quote,
                        to_json(&atts),
                        m.in_reply_to,
                        m.references.join(" "),
                        gmail_draft,
                        m.gmail_id,
                        m.gmail_thread_id,
                        !atts.is_empty(),
                        m.internal_date,
                    ],
                )?;
                ctx.touch_draft(account, &local)?;
            }
            ctx.touch_thread(account, thread)?;
            Ok(())
        })
        .await
        .log_err("store_gmail_draft");
}
