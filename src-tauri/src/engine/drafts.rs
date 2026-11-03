//! Drafts on the Gmail side (SPEC Compose → Drafts, Gmail errors).
//!
//! Upserts replace the whole message, so every upsert carries all of the
//! draft's attachments. `drafts.create` is unsafe to repeat: every draft this
//! app writes carries `X-Ubermensch-Draft` with the local id, which is how a
//! create with no clear answer is recovered.

use crate::LogErr as _;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use rusqlite::{OptionalExtension, params};
use serde::Deserialize;
use ub_gmail::{Format, GmailError, Outcome, Priority, cost};

use super::attachments::DownloadError;
use super::ops::{Done, bad};
use super::{AccountRt, Engine};
use crate::db::{from_json, to_json};
use crate::model::{
    AccountId, Addr, AttachmentId, DraftAttachment, DraftId, DraftKind, MessageId, ThreadId,
};

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

pub fn load(e: &Engine, draft: &DraftId) -> Option<DraftRow> {
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
                        id: draft.clone(),
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

/// The Gmail draft id stored for a local draft, if it has one.
fn gmail_draft_id(e: &Engine, draft: &DraftId) -> Option<String> {
    e.reads
        .with(|c| {
            Ok(c.query_row(
                "SELECT gmail_draft_id FROM drafts WHERE id = ?1",
                [draft],
                |r| r.get(0),
            )
            .optional()?)
        })
        .inspect_err(|err| log::warn!("read gmail draft id: {err:#}"))
        .ok()
        .flatten()
        .flatten()
}

fn mime_addr(a: &Addr) -> ub_mime::Addr {
    ub_mime::Addr {
        name: a.name.clone(),
        email: a.email.clone(),
    }
}

/// Downloads an attachment for a draft write. Offline and transient failures
/// keep the write waiting; anything else rejects it.
async fn download(e: &Engine, attachment: AttachmentId) -> Result<PathBuf, GmailError> {
    super::attachments::ensure(e, attachment)
        .await
        .map_err(|err| match err.downcast_ref::<DownloadError>() {
            Some(DownloadError::NoAccount | DownloadError::Gmail(Outcome::Offline)) => {
                GmailError::new(Outcome::Offline)
            }
            Some(DownloadError::Gmail(
                Outcome::NoResponse | Outcome::RateLimited { .. } | Outcome::ServerError { .. },
            )) => GmailError::new(Outcome::NoResponse),
            _ => bad(err),
        })
}

/// A local file problem is not Gmail's; it surfaces as a rejection.
fn read_file(path: &Path) -> Result<Vec<u8>, GmailError> {
    std::fs::read(path).map_err(bad)
}

/// Bytes of every attachment, downloading those not cached yet.
async fn attachment_bytes(
    e: &Engine,
    d: &DraftRow,
) -> Result<Vec<ub_mime::OutAttachment>, GmailError> {
    let mut out = Vec::new();
    for a in &d.attachments {
        let data = match (&a.path, a.source) {
            (Some(p), _) if Path::new(p).exists() => read_file(Path::new(p))?,
            (_, Some(src)) => read_file(&download(e, src).await?)?,
            _ => continue,
        };
        out.push(ub_mime::OutAttachment {
            filename: a.filename.clone(),
            mime_type: a.mime_type.clone(),
            data,
        });
    }
    Ok(out)
}

/// Inline parts of the quoted message, for turning `data:` images in `html`
/// back into `cid:`.
async fn quoted_inline(
    e: &Engine,
    d: &DraftRow,
    html: &str,
) -> Result<Vec<ub_mime::InlinePart>, GmailError> {
    let Some(target) = d.target else {
        return Ok(vec![]);
    };
    if !html.contains("data:") {
        return Ok(vec![]);
    }
    let parts: Vec<(AttachmentId, String, String, String)> = e
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
        let path = download(e, id).await?;
        out.push(ub_mime::InlinePart {
            content_id: cid,
            mime_type: mime,
            filename: Some(name),
            data: read_file(&path)?,
        });
    }
    Ok(out)
}

/// Builds the draft's MIME with mail-builder (via ub-mime).
pub async fn build_raw(e: &Engine, d: &DraftRow) -> Result<Vec<u8>, GmailError> {
    let attachments = attachment_bytes(e, d).await?;
    let html = format!(
        "{}{}",
        d.body_html,
        d.quote_html.as_deref().unwrap_or_default()
    );
    let known = quoted_inline(e, d, &html).await?;
    let (html, inline) = ub_mime::data_images_to_cid(&html, known);
    let from = d.from.as_ref().map_or_else(
        || ub_mime::Addr {
            name: None,
            email: String::new(),
        },
        mime_addr,
    );
    let domain = from
        .email
        .split('@')
        .nth(1)
        .unwrap_or("ubermensch.local")
        .to_string();
    Ok(ub_mime::build_mime(&ub_mime::Outgoing {
        from,
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
        ub_draft_id: d.id.to_string(),
        attachments,
        inline,
        date_ms: crate::clock::now_ms(),
        message_id_domain: domain,
    }))
}

/// Writes the draft to Gmail with `drafts.create` or `drafts.update`.
/// Returns the Gmail draft.
pub async fn write(
    e: &Engine,
    rt: &AccountRt,
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
    e: &Engine,
    rt: &AccountRt,
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
    e: &Engine,
    rt: &AccountRt,
    local: &DraftId,
) -> Result<Option<ub_gmail::Draft>, GmailError> {
    if let Some(gid) = gmail_draft_id(e, local) {
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
        if header.as_deref() == Some(local.as_str()) {
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

async fn list_all(rt: &AccountRt) -> Result<Vec<ub_gmail::Draft>, GmailError> {
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

/// Stores the Gmail ids an upsert returned. With `orphan` (the account), a
/// draft discarded while its upsert was in flight has the Gmail draft that
/// upsert made deleted too, or sync would bring the draft back.
pub async fn store_ids(
    e: &Engine,
    local: &DraftId,
    g: &ub_gmail::Draft,
    orphan: Option<AccountId>,
) {
    let (id, msg, thread) = (
        g.id.clone(),
        g.message.id.clone(),
        g.message.thread_id.clone(),
    );
    let local = local.clone();
    e
        .writer
        .call(move |ctx| {
            let account: Option<AccountId> = ctx
                .tx
                .query_row("SELECT account_id FROM drafts WHERE id = ?1", [&local], |r| r.get(0))
                .optional()?;
            let Some(account) = account else {
                return match orphan {
                    Some(owner) => delete_gmail_draft(ctx, owner, &id),
                    None => Ok(()),
                };
            };
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

/// Whether a delete of this Gmail draft is waiting in the queue.
fn delete_queued(
    ctx: &crate::writer::Ctx<'_>,
    account: AccountId,
    gmail_draft: &str,
) -> rusqlite::Result<bool> {
    ctx.tx.query_row(
        "SELECT EXISTS (SELECT 1 FROM ops WHERE account_id = ?1 AND kind = 'draft_delete'
                          AND json_extract(payload, '$.gmail_draft') = ?2)",
        params![account, gmail_draft],
        |r| r.get(0),
    )
}

/// Queues the delete of a Gmail draft no local draft owns, once.
fn delete_gmail_draft(
    ctx: &mut crate::writer::Ctx<'_>,
    account: AccountId,
    gmail_draft: &str,
) -> anyhow::Result<()> {
    if !delete_queued(ctx, account, gmail_draft)? {
        crate::queue::push(
            ctx,
            account,
            None,
            "draft_delete",
            &serde_json::json!({ "gmail_draft": gmail_draft }),
        )?;
    }
    Ok(())
}

pub async fn execute_op(
    e: &Engine,
    rt: &AccountRt,
    kind: &str,
    payload: &str,
) -> Result<Done, GmailError> {
    let v: serde_json::Value = serde_json::from_str(payload).map_err(bad)?;
    match kind {
        "draft_upsert" => {
            let Ok(local) = DraftId::deserialize(&v["draft"]) else {
                return Ok(Done::Finished);
            };
            let Some(writing) = e.draft_writes.begin(&local) else {
                // The outbox is writing it, and sends the final version itself.
                return if in_outbox(e, &local) {
                    Ok(Done::Finished)
                } else {
                    Err(GmailError::new(Outcome::NoResponse))
                };
            };
            let r = upsert(e, rt, &local).await;
            drop(writing);
            // Sent meanwhile: the outbox waited for this write to end.
            if in_outbox(e, &local) {
                rt.outbox_kick.kick();
            }
            r
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

/// Writes the draft's latest state to Gmail, unless the outbox has taken it.
async fn upsert(e: &Engine, rt: &AccountRt, local: &DraftId) -> Result<Done, GmailError> {
    if in_outbox(e, local) {
        return Ok(Done::Finished);
    }
    let Some(d) = load(e, local) else {
        return Ok(Done::Finished);
    };
    if d.needs_parts {
        download_parts_now(e, local).await?;
    }
    let Some(d) = load(e, local) else {
        return Ok(Done::Finished);
    };
    let g = write(e, rt, &d).await?;
    store_ids(e, local, &g, Some(rt.id)).await;
    Ok(Done::Finished)
}

fn in_outbox(e: &Engine, draft: &DraftId) -> bool {
    e.reads
        .with(|c| Ok(crate::drafts::in_outbox(c, draft)?))
        .inspect_err(|err| log::warn!("read outbox for draft {draft}: {err:#}"))
        .unwrap_or(false)
}

/// Drafts being written to Gmail now, by an upsert or by the outbox. Two writes
/// of one draft never overlap: an upsert still in flight when the send's time
/// comes would otherwise land after the outbox's write, as an extra Gmail draft
/// or as an older version under the one being sent.
#[derive(Default)]
pub struct DraftWrites(parking_lot::Mutex<HashSet<DraftId>>);

impl DraftWrites {
    /// Starts writing `draft`, or `None` while another write of it runs.
    pub fn begin(self: &Arc<Self>, draft: &DraftId) -> Option<DraftWrite> {
        self.0.lock().insert(draft.clone()).then(|| DraftWrite {
            writes: Arc::clone(self),
            draft: draft.clone(),
        })
    }

    pub fn busy(&self, draft: &DraftId) -> bool {
        self.0.lock().contains(draft)
    }
}

/// A write in progress; it ends when dropped.
pub struct DraftWrite {
    writes: Arc<DraftWrites>,
    draft: DraftId,
}

impl Drop for DraftWrite {
    fn drop(&mut self) {
        self.writes.0.lock().remove(&self.draft);
    }
}

/// Downloads a Gmail draft's attachments and inline parts before its first
/// upsert (SPEC Addressing).
async fn download_parts_now(e: &Engine, draft: &DraftId) -> Result<(), GmailError> {
    let Some(DraftRow {
        attachments: mut atts,
        ..
    }) = load(e, draft)
    else {
        return Ok(());
    };
    for a in &mut atts {
        if a.path.is_none()
            && let Some(src) = a.source
        {
            let p = download(e, src).await?;
            a.path = Some(p.to_string_lossy().into_owned());
        }
    }
    let local = draft.clone();
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

pub async fn download_parts(e: Arc<Engine>, draft: DraftId) {
    download_parts_now(&e, &draft)
        .await
        .log_err("download draft parts");
}

/// A Gmail change skipped while a composer had the draft open, read when that
/// composer closes without edits.
pub async fn read_skipped(e: Arc<Engine>, rt: Arc<AccountRt>, draft: DraftId) {
    let Some(gid) = gmail_draft_id(&e, &draft) else {
        return;
    };
    // The draft's own Gmail draft: no other copy is in question.
    let reconcile = Reconcile::start(&e, Arc::default());
    let read = rt
        .sched
        .run(
            Priority::P2,
            cost::drafts_get(Format::Full),
            rt.gmail.get_draft(&gid, Format::Full),
        )
        .await;
    match read {
        Ok(g) => store_gmail_draft(&e, &rt, g, reconcile).await,
        Err(err) => log::warn!("read skipped draft change: {:?}", err.outcome),
    }
}

/// What Gmail said about the account's drafts, and since when. Gmail's
/// answers are only as new as the request: a local draft written while one
/// was in flight, or still being written, is left for the next reconcile.
#[derive(Clone)]
pub struct Reconcile {
    /// The account's Gmail draft ids, as `drafts.list` gave them.
    live: Arc<HashSet<String>>,
    /// The last `rev` committed before the first request.
    started: i64,
    writes: Arc<DraftWrites>,
}

impl Reconcile {
    /// Call before the first request.
    fn start(e: &Engine, live: Arc<HashSet<String>>) -> Reconcile {
        let started = e
            .reads
            .with(|c| {
                Ok(c.query_row("SELECT value FROM meta WHERE key = 'rev'", [], |r| {
                    r.get(0)
                })?)
            })
            .inspect_err(|err| log::warn!("read rev for a draft reconcile: {err:#}"))
            // Unknown: every local draft counts as changed, so none is touched.
            .unwrap_or(i64::MIN);
        Reconcile {
            live,
            started,
            writes: Arc::clone(&e.draft_writes),
        }
    }

    /// Whether the local draft changed or is being written since the start.
    fn moved(&self, ctx: &crate::writer::Ctx<'_>, draft: &DraftId) -> rusqlite::Result<bool> {
        if self.writes.busy(draft) {
            return Ok(true);
        }
        let rev: Option<i64> = ctx
            .tx
            .query_row("SELECT rev FROM drafts WHERE id = ?1", [draft], |r| {
                r.get(0)
            })
            .optional()?;
        Ok(rev.is_some_and(|rev| rev > self.started))
    }
}

/// Drafts deleted on Gmail (for example sent from a phone) are deleted here,
/// unless a composer has one open: then it keeps its text, loses its Gmail
/// id, and a notice says so (SPEC Drafts). A draft that got a Gmail draft the
/// list could not know about yet is not gone.
fn drop_gone(
    ctx: &mut crate::writer::Ctx<'_>,
    account: AccountId,
    gone: Vec<DraftId>,
    reconcile: &Reconcile,
) -> anyhow::Result<()> {
    for d in gone {
        let gmail: Option<String> = ctx
            .tx
            .query_row(
                "SELECT gmail_draft_id FROM drafts WHERE id = ?1",
                [&d],
                |r| r.get(0),
            )
            .optional()?
            .flatten();
        let still_gone = gmail.is_some_and(|g| !reconcile.live.contains(&g));
        if !still_gone || reconcile.moved(ctx, &d)? {
            continue;
        }
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
            let thread: Option<ThreadId> = ctx
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
pub async fn sync_list(e: &Engine, rt: &AccountRt) -> Result<(), GmailError> {
    let mut reconcile = Reconcile::start(e, Arc::default());
    let gmail = list_all(rt).await?;
    reconcile.live = Arc::new(gmail.iter().map(|d| d.id.clone()).collect());
    let (local, outbox): (Vec<LocalDraft>, HashSet<String>) = e
        .reads
        .with(|c| {
            let mut stmt = c.prepare(
                "SELECT id, gmail_draft_id, gmail_message_id,
                        EXISTS (SELECT 1 FROM outbox o WHERE o.draft_id = d.id)
                 FROM drafts d WHERE account_id = ?1",
            )?;
            let rows = stmt.query_map([rt.id], |r| {
                Ok(LocalDraft {
                    id: r.get(0)?,
                    gmail: r.get(1)?,
                    message: r.get(2)?,
                    in_outbox: r.get(3)?,
                })
            })?;
            let local = rows.collect::<Result<_, _>>()?;
            let mut stmt = c.prepare(
                "SELECT gmail_draft_id FROM outbox WHERE account_id = ?1 AND gmail_draft_id IS NOT NULL",
            )?;
            let outbox = stmt
                .query_map([rt.id], |r| r.get(0))?
                .collect::<Result<_, _>>()?;
            Ok((local, outbox))
        })
        .unwrap_or_default();
    // Gmail draft id → (local id, Gmail message id), for drafts Gmail still has.
    let mut by_gmail: HashMap<String, (DraftId, Option<String>)> = HashMap::new();
    // Drafts deleted on Gmail (for example sent from a phone).
    let mut gone: Vec<DraftId> = Vec::new();
    // The outbox writes and sends its drafts itself, so Gmail's copy of one is
    // never read back over the version being sent.
    let mut sending = outbox;
    for LocalDraft {
        id,
        gmail: g,
        message: m,
        in_outbox,
    } in local
    {
        let Some(g) = g else { continue };
        if in_outbox {
            sending.insert(g);
        } else if reconcile.live.contains(g.as_str()) {
            by_gmail.insert(g, (id, m));
        } else {
            gone.push(id);
        }
    }
    if !gone.is_empty() {
        let account = rt.id;
        let r = reconcile.clone();
        e.writer
            .call(move |ctx| drop_gone(ctx, account, gone, &r))
            .await
            .log_err("sync_list");
    }

    // Drafts this app does not know, or whose message changed, are read again.
    for g in gmail {
        if sending.contains(&g.id) {
            continue;
        }
        if let Some((local, msg)) = by_gmail.remove(&g.id) {
            if msg.as_deref() == Some(g.message.id.as_str()) || e.draft_writes.busy(&local) {
                continue;
            }
            if defer_if_open(e, local).await {
                continue;
            }
        } else if !thread_has_mail(e, rt.id, &g.message.thread_id) {
            // A draft new to this app gets its kind and target from the
            // thread's messages once, so they must be stored first.
            super::sync::fetch_thread(e, rt, &g.message.thread_id, Format::Metadata, Priority::P2)
                .await?;
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
            Ok(full) => store_gmail_draft(e, rt, full, reconcile.clone()).await,
            Err(err) if matches!(err.outcome, Outcome::NotFound) => {}
            Err(err) => return Err(err),
        }
    }
    Ok(())
}

/// Whether a Gmail thread's messages other than drafts are stored.
fn thread_has_mail(e: &Engine, account: AccountId, gmail_thread: &str) -> bool {
    e.reads
        .with(|c| {
            Ok(c.query_row(
                "SELECT EXISTS (SELECT 1 FROM threads t JOIN messages m ON m.thread_id = t.id
                                WHERE t.account_id = ?1 AND t.gmail_id = ?2 AND m.is_draft = 0)",
                params![account, gmail_thread],
                |r| r.get(0),
            )?)
        })
        .inspect_err(|err| log::warn!("read thread {gmail_thread}: {err:#}"))
        .unwrap_or(false)
}

/// A local draft as `sync_list` reconciles it.
struct LocalDraft {
    id: DraftId,
    gmail: Option<String>,
    message: Option<String>,
    in_outbox: bool,
}

/// A composer has the draft open: mark the Gmail change for `read_skipped`
/// instead of overwriting the text being edited.
async fn defer_if_open(e: &Engine, draft: DraftId) -> bool {
    e.writer
        .call(move |ctx| {
            if !ctx.subs.is_draft_open(&draft) {
                return Ok(false);
            }
            ctx.tx.execute(
                "UPDATE drafts SET pending_gmail_read = 1 WHERE id = ?1",
                [&draft],
            )?;
            Ok(true)
        })
        .await
        .inspect_err(|err| log::warn!("defer draft read: {err:#}"))
        // Only the update can fail, and only for an open draft: leave it alone.
        .unwrap_or(true)
}

/// The message a draft read from Gmail is aimed at: the one its `In-Reply-To`
/// names, else the thread's latest. It counts as started on the latest message
/// when its target was the newest non-draft message older than the draft.
fn gmail_draft_target(
    ctx: &crate::writer::Ctx<'_>,
    thread: ThreadId,
    others: &[crate::threads::Msg],
    m: &super::ingest::Incoming,
) -> anyhow::Result<(Option<MessageId>, bool)> {
    let by_reply = match &m.in_reply_to {
        Some(irt) => ctx
            .tx
            .query_row(
                "SELECT id FROM messages WHERE thread_id = ?1 AND message_id_hdr = ?2 AND is_draft = 0",
                params![thread, irt.trim_matches(['<', '>'])],
                |r| r.get::<_, MessageId>(0),
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

/// The local draft a Gmail draft belongs to, with the Gmail draft id stored on
/// it: by Gmail draft id, else by its `X-Ubermensch-Draft` header.
fn local_for(
    ctx: &crate::writer::Ctx<'_>,
    account: AccountId,
    m: &super::ingest::Incoming,
    gmail_draft: &str,
) -> rusqlite::Result<Option<(DraftId, Option<String>)>> {
    ctx.tx
        .query_row(
            "SELECT id, gmail_draft_id FROM drafts WHERE account_id = ?1 AND (gmail_draft_id = ?2 OR id = ?3)
             ORDER BY gmail_draft_id IS ?2 DESC LIMIT 1",
            params![account, gmail_draft, m.ub_draft_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
}

/// A Gmail draft carrying the id of a local draft that has no Gmail draft, or
/// whose Gmail draft is gone: this app wrote it, so it is that draft. Its
/// Gmail ids are stored; the local text stays, as it is never older.
fn adopt_gmail_draft(
    ctx: &mut crate::writer::Ctx<'_>,
    account: AccountId,
    local: &DraftId,
    d: &FetchedDraft,
) -> anyhow::Result<()> {
    ctx.tx.execute(
        "UPDATE drafts SET gmail_draft_id = ?2, gmail_message_id = ?3, gmail_thread_id = ?4 WHERE id = ?1",
        params![local, d.id, d.m.gmail_id, d.m.gmail_thread_id],
    )?;
    ctx.touch_draft(account, local)?;
    Ok(())
}

/// The stored message row for a Gmail draft's message.
fn message_row(
    ctx: &crate::writer::Ctx<'_>,
    account: AccountId,
    m: &super::ingest::Incoming,
) -> rusqlite::Result<Option<MessageId>> {
    ctx.tx
        .query_row(
            "SELECT id FROM messages WHERE account_id = ?1 AND gmail_id = ?2",
            params![account, m.gmail_id],
            |r| r.get(0),
        )
        .optional()
}

/// A draft read from Gmail: its message, and its HTML split at the quote.
struct FetchedDraft {
    id: String,
    m: super::ingest::Incoming,
    body: String,
    quote: Option<String>,
}

impl FetchedDraft {
    fn parse(g: &ub_gmail::Draft) -> FetchedDraft {
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
        FetchedDraft {
            id: g.id.clone(),
            m: super::ingest::parse(&g.message, true, HashMap::new()),
            body: ub_mime::sanitize_html(above, &HashMap::new()),
            quote: quote.map(|q| ub_mime::sanitize_html(q, &HashMap::new())),
        }
    }
}

/// Creates or updates the local draft for a Gmail draft. Its kind is decided
/// once, when it first appears: new, forward (`Fwd:`), or reply. `live` holds
/// the account's Gmail draft ids, as `drafts.list` just gave them.
pub async fn store_gmail_draft(
    e: &Engine,
    rt: &AccountRt,
    g: ub_gmail::Draft,
    reconcile: Reconcile,
) {
    let d = FetchedDraft::parse(&g);
    let account = rt.id;
    e.writer
        .call(move |ctx| apply_gmail_draft(ctx, account, &d, &reconcile))
        .await
        .log_err("store_gmail_draft");
}

fn apply_gmail_draft(
    ctx: &mut crate::writer::Ctx<'_>,
    account: AccountId,
    d: &FetchedDraft,
    reconcile: &Reconcile,
) -> anyhow::Result<()> {
    let m = &d.m;
    // Discarded here, and its delete has not reached Gmail yet.
    if delete_queued(ctx, account, &d.id)? {
        return Ok(());
    }
    // A write of the draft it names is in flight: its answer settles it.
    if m.ub_draft_id
        .as_ref()
        .is_some_and(|local| reconcile.writes.busy(local))
    {
        return Ok(());
    }
    let existing = local_for(ctx, account, m, &d.id)?;
    if let Some((local, gmail)) = &existing {
        // The outbox sends its own version; Gmail's copy may be older.
        if crate::drafts::in_outbox(ctx.tx, local)? || reconcile.moved(ctx, local)? {
            return Ok(());
        }
        // Found by its header (SPEC Gmail errors): an extra copy when the
        // local draft already has a live Gmail draft, else that draft.
        if gmail.as_deref() != Some(d.id.as_str()) {
            return match gmail {
                Some(other) if reconcile.live.contains(other) => {
                    delete_gmail_draft(ctx, account, &d.id)
                }
                _ => adopt_gmail_draft(ctx, account, local, d),
            };
        }
        // Opened since the list was read: read once its composer closes.
        if ctx.subs.is_draft_open(local) {
            ctx.tx.execute(
                "UPDATE drafts SET pending_gmail_read = 1 WHERE id = ?1",
                [local],
            )?;
            return Ok(());
        }
    }
    let existing = existing.map(|(local, _)| local);
    // The thread and message rows for the draft (and its siblings).
    // Actions don't change a draft's labels, so no read to guard.
    let pending = super::ingest::PendingOps::load(ctx.tx, account, Vec::new())?;
    let thread = super::ingest::store_thread(
        ctx,
        account,
        &m.gmail_thread_id,
        None,
        std::slice::from_ref(m),
        false,
        &pending,
    )?;
    let atts = match message_row(ctx, account, m)? {
        Some(row) => crate::drafts::message_attachments(ctx.tx, row)?,
        None => vec![],
    };
    if let Some(local) = existing {
        ctx.tx.execute(
            "UPDATE drafts SET from_json = ?2, to_json = ?3, cc_json = ?4, bcc_json = ?5, subject = ?6,
                body_html = ?7, quote_html = ?8, attachments_json = ?9, gmail_draft_id = ?10,
                gmail_message_id = ?11, gmail_thread_id = ?12, thread_id = ?13, pending_gmail_read = 0,
                needs_parts = ?14, updated_at = ?15
             WHERE id = ?1",
            params![
                local,
                to_json(&m.from),
                to_json(&m.to),
                to_json(&m.cc),
                to_json(&m.bcc),
                m.subject,
                d.body,
                d.quote,
                to_json(&atts),
                d.id,
                m.gmail_id,
                m.gmail_thread_id,
                thread,
                !atts.is_empty(),
                m.internal_date,
            ],
        )?;
        ctx.touch_draft(account, &local)?;
    } else {
        insert_gmail_draft(ctx, account, d, thread, &atts)?;
    }
    ctx.touch_thread(account, thread)?;
    Ok(())
}

/// A Gmail draft this app has no local draft for.
fn insert_gmail_draft(
    ctx: &mut crate::writer::Ctx<'_>,
    account: AccountId,
    d: &FetchedDraft,
    thread: ThreadId,
    atts: &[DraftAttachment],
) -> anyhow::Result<()> {
    let m = &d.m;
    let others: Vec<crate::threads::Msg> = crate::threads::messages(ctx.tx, thread)?
        .into_iter()
        .filter(|x| !x.is_draft)
        .collect();
    let (target, started_on_latest) = gmail_draft_target(ctx, thread, &others, m)?;
    let kind = gmail_draft_kind(others.is_empty(), m);
    let local = m
        .ub_draft_id
        .clone()
        .unwrap_or_else(|| DraftId(format!("g-{}", d.id)));
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
            to_json(&m.from),
            to_json(&m.to),
            to_json(&m.cc),
            to_json(&m.bcc),
            m.subject,
            d.body,
            d.quote,
            to_json(&atts),
            m.in_reply_to,
            m.references.join(" "),
            d.id,
            m.gmail_id,
            m.gmail_thread_id,
            !atts.is_empty(),
            m.internal_date,
        ],
    )?;
    ctx.touch_draft(account, &local)?;
    Ok(())
}
