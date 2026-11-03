//! Drafts on the writer side (SPEC Compose → Drafts, Addressing; UI state →
//! Actions). A fresh composer's first save carries only its kind and target;
//! Rust fills From, To, Cc, Subject, and the quote, and keeps them as the
//! draft's defaults for the untouched check.

use std::collections::HashSet;

use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

use crate::db::{from_json, queries, to_json};
use crate::model::{
    AccountId, Addr, AttachmentId, DraftAttachment, DraftId, DraftKind, DraftSave, MessageId,
    Millis, SendAs, ThreadId,
};
use crate::writer::{Ctx, Effect};

/// The fields a composer started with.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Defaults {
    pub from: Option<Addr>,
    pub to: Vec<Addr>,
    pub cc: Vec<Addr>,
    pub bcc: Vec<Addr>,
    pub subject: String,
    pub body_html: String,
    pub attachments: Vec<String>,
}

struct Target {
    id: MessageId,
    from: Addr,
    to: Vec<Addr>,
    cc: Vec<Addr>,
    reply_to: Vec<Addr>,
    subject: String,
    date: Millis,
    message_id: Option<String>,
    references: String,
    html: String,
}

fn target(conn: &Connection, id: MessageId) -> anyhow::Result<Option<Target>> {
    Ok(conn
        .query_row(
            "SELECT from_name, from_email, to_json, cc_json, reply_to_json, subject, internal_date, message_id_hdr,
                    references_hdr, coalesce(body_html, ''), coalesce(quote_html, ''), snippet, has_body
             FROM messages WHERE id = ?1",
            [id],
            |r| {
                let name: String = r.get(0)?;
                let body: String = r.get(9)?;
                let quote: String = r.get(10)?;
                let snippet: String = r.get(11)?;
                let has_body: bool = r.get(12)?;
                Ok(Target {
                    id,
                    from: Addr {
                        name: (!name.is_empty()).then_some(name),
                        email: r.get(1)?,
                    },
                    to: from_json(&r.get::<_, String>(2)?),
                    cc: from_json(&r.get::<_, String>(3)?),
                    reply_to: from_json(&r.get::<_, String>(4)?),
                    subject: r.get(5)?,
                    date: r.get(6)?,
                    message_id: r.get(7)?,
                    references: r.get(8)?,
                    html: if has_body {
                        format!("{body}{quote}")
                    } else {
                        html_escape::encode_text(&snippet).into_owned()
                    },
                })
            },
        )
        .optional()?)
}

fn mime_addr(a: &Addr) -> ub_mime::Addr {
    ub_mime::Addr {
        name: a.name.clone(),
        email: a.email.clone(),
    }
}

fn quote_source(t: &Target) -> ub_mime::QuoteSource {
    ub_mime::QuoteSource {
        from: mime_addr(&t.from),
        to: t.to.iter().map(mime_addr).collect(),
        cc: t.cc.iter().map(mime_addr).collect(),
        date_ms: t.date,
        subject: t.subject.clone(),
        html: t.html.clone(),
    }
}

pub fn tz_offset_minutes() -> i32 {
    chrono::Local::now().offset().local_minus_utc() / 60
}

fn send_as(conn: &Connection, account: AccountId) -> anyhow::Result<(Vec<SendAs>, Addr)> {
    let (email, name, json): (String, String, String) = conn.query_row(
        "SELECT email, display_name, send_as FROM accounts WHERE id = ?1",
        [account],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    let list: Vec<SendAs> = from_json(&json);
    let default = list
        .iter()
        .find(|s| s.is_default)
        .or_else(|| list.iter().find(|s| s.is_primary))
        .map_or(
            Addr {
                name: (!name.is_empty() && name != email).then_some(name),
                email,
            },
            send_as_addr,
        );
    Ok((list, default))
}

fn send_as_addr(s: &SendAs) -> Addr {
    Addr {
        name: (!s.name.is_empty()).then(|| s.name.clone()),
        email: s.email.clone(),
    }
}

fn same(a: &str, b: &str) -> bool {
    a.eq_ignore_ascii_case(b)
}

/// The addresses whose email is not yet in `seen`, first occurrence kept.
fn dedup<'a>(list: impl IntoIterator<Item = &'a Addr>, mut seen: HashSet<String>) -> Vec<Addr> {
    list.into_iter()
        .filter(|a| seen.insert(a.email.to_lowercase()))
        .cloned()
        .collect()
}

/// A fresh composer's first save (SPEC UI state → Actions).
struct NewDraft<'a> {
    draft: &'a DraftId,
    account: AccountId,
    kind: DraftKind,
    reply_all: bool,
    thread: Option<ThreadId>,
    target: Option<MessageId>,
    started_on_latest: bool,
}

/// From, To, and Cc for a reply (SPEC Addressing).
fn reply_addressing(
    t: &Target,
    send_as: &[SendAs],
    default_from: Addr,
    mine: &HashSet<String>,
    reply_all: bool,
) -> (Addr, Vec<Addr>, Vec<Addr>) {
    if mine.contains(&t.from.email.to_lowercase()) {
        // Replying to my own message: send from its From, to its To.
        let cc = if reply_all {
            dedup(&t.cc, mine.clone())
        } else {
            vec![]
        };
        return (t.from.clone(), t.to.clone(), cc);
    }
    // From the sendAs address the message was sent to, else the default. With
    // two of them, To wins over Cc, and within a field the first one (Q3).
    let from =
        t.to.iter()
            .chain(&t.cc)
            .find_map(|a| send_as.iter().find(|s| same(&a.email, &s.email)))
            .map_or(default_from, send_as_addr);
    let to = if t.reply_to.is_empty() {
        vec![t.from.clone()]
    } else {
        t.reply_to.clone()
    };
    let cc = if reply_all {
        let mut seen = mine.clone();
        seen.extend(to.iter().map(|a| a.email.to_lowercase()));
        dedup(t.to.iter().chain(&t.cc), seen)
    } else {
        vec![]
    };
    (from, to, cc)
}

/// A message's attachments as draft attachments: a forward carries its
/// target's, and a draft read from Gmail carries its own.
pub fn message_attachments(
    conn: &Connection,
    message: MessageId,
) -> anyhow::Result<Vec<DraftAttachment>> {
    let mut stmt = conn.prepare_cached(
        "SELECT id, filename, mime_type, size, local_path FROM attachments WHERE message_id = ?1 AND inline = 0",
    )?;
    let rows = stmt.query_map([message], |r| {
        let id: AttachmentId = r.get(0)?;
        Ok(DraftAttachment {
            id: format!("src-{id}"),
            filename: r.get(1)?,
            mime_type: r.get(2)?,
            size: r.get(3)?,
            path: r.get(4)?,
            source: Some(id),
        })
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// Fills a fresh composer by the rules in SPEC Addressing.
fn create(ctx: &mut Ctx<'_>, d: &NewDraft<'_>) -> anyhow::Result<()> {
    let (send_as_list, default_from) = send_as(ctx.tx, d.account)?;
    let mine = queries::my_addresses(ctx.tx, d.account)?;
    let target = match d.target {
        Some(t) => target(ctx.tx, t)?,
        None => None,
    };
    let mut from = default_from;
    let mut to = Vec::new();
    let mut cc = Vec::new();
    let mut subject = String::new();
    let mut quote = None;
    let mut in_reply_to = None;
    let mut references = String::new();
    let mut attachments: Vec<DraftAttachment> = Vec::new();

    if let Some(t) = &target {
        in_reply_to.clone_from(&t.message_id);
        references = format!(
            "{} {}",
            t.references,
            t.message_id.as_deref().unwrap_or_default()
        )
        .trim()
        .to_string();
        match d.kind {
            DraftKind::Reply => {
                subject = ub_mime::reply_subject(&t.subject);
                quote = Some(ub_mime::reply_quote_html(
                    &quote_source(t),
                    tz_offset_minutes(),
                ));
                (from, to, cc) = reply_addressing(t, &send_as_list, from, &mine, d.reply_all);
            }
            DraftKind::Forward => {
                subject = ub_mime::forward_subject(&t.subject);
                quote = Some(ub_mime::forward_quote_html(
                    &quote_source(t),
                    tz_offset_minutes(),
                ));
                attachments = message_attachments(ctx.tx, t.id)?;
            }
            DraftKind::New => {}
        }
    }
    let defaults = Defaults {
        from: Some(from),
        to,
        cc,
        bcc: vec![],
        subject,
        body_html: String::new(),
        attachments: attachments.iter().map(|a| a.id.clone()).collect(),
    };
    ctx.tx.execute(
        "INSERT INTO drafts (id, account_id, kind, reply_all, thread_id, target_message_id, started_on_latest,
            started_fresh, from_json, to_json, cc_json, subject, quote_html, attachments_json, defaults_json,
            in_reply_to, references_hdr, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 1, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)",
        params![
            d.draft,
            d.account,
            d.kind.as_str(),
            d.reply_all,
            d.thread,
            d.target,
            d.started_on_latest,
            to_json(&defaults.from),
            to_json(&defaults.to),
            to_json(&defaults.cc),
            defaults.subject,
            quote,
            to_json(&attachments),
            to_json(&defaults),
            in_reply_to,
            references,
            ctx.now,
        ],
    )?;
    ctx.touch_draft(d.account, d.draft)?;
    if let Some(t) = d.thread {
        ctx.touch_thread(d.account, t)?;
    }
    Ok(())
}

pub fn account_of(
    conn: &Connection,
    draft: &DraftId,
) -> anyhow::Result<Option<(AccountId, Option<ThreadId>)>> {
    Ok(conn
        .query_row(
            "SELECT account_id, thread_id FROM drafts WHERE id = ?1",
            [draft],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?)
}

/// Queues one upsert per draft; the runner reads the draft's latest state.
pub fn queue_upsert(ctx: &mut Ctx<'_>, account: AccountId, draft: &DraftId) -> anyhow::Result<()> {
    let exists: bool = ctx.tx.query_row(
        "SELECT EXISTS (SELECT 1 FROM ops WHERE account_id = ?1 AND kind = 'draft_upsert'
                          AND json_extract(payload, '$.draft') = ?2)",
        params![account, draft],
        |r| r.get(0),
    )?;
    if !exists {
        crate::queue::push(
            ctx,
            account,
            None,
            "draft_upsert",
            &serde_json::json!({ "draft": draft }),
        )?;
    }
    Ok(())
}

fn in_outbox(conn: &Connection, draft: &DraftId) -> rusqlite::Result<bool> {
    conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM outbox WHERE draft_id = ?1)",
        [draft],
        |r| r.get(0),
    )
}

pub fn save(ctx: &mut Ctx<'_>, draft: &DraftId, save: DraftSave) -> anyhow::Result<()> {
    // A save that arrives after its draft was sent or discarded is dropped.
    if in_outbox(ctx.tx, draft)? {
        return Ok(());
    }
    match save {
        DraftSave::Create {
            account,
            kind,
            reply_all,
            thread,
            target,
            started_on_latest,
        } => {
            if account_of(ctx.tx, draft)?.is_some() {
                return Ok(());
            }
            create(
                ctx,
                &NewDraft {
                    draft,
                    account,
                    kind,
                    reply_all,
                    thread,
                    target,
                    started_on_latest,
                },
            )
        }
        DraftSave::Update { mut fields } => {
            let Some((account, thread)) = account_of(ctx.tx, draft)? else {
                return Ok(());
            };
            // The composer holds attachments as first loaded; a local copy
            // stored since (for example after the source left Gmail) is kept.
            let stored: Vec<DraftAttachment> = ctx
                .tx
                .query_row(
                    "SELECT attachments_json FROM drafts WHERE id = ?1",
                    [draft],
                    |r| r.get::<_, String>(0),
                )
                .map(|j| from_json(&j))?;
            for a in &mut fields.attachments {
                if a.path.is_none() {
                    a.path = stored
                        .iter()
                        .find(|s| s.id == a.id)
                        .and_then(|s| s.path.clone());
                }
            }
            ctx.tx.execute(
                // An edit after a skipped Gmail change wins over it (last writer wins),
                // so that change is no longer read on close.
                "UPDATE drafts SET from_json = ?2, to_json = ?3, cc_json = ?4, bcc_json = ?5, subject = ?6,
                    body_html = ?7, attachments_json = ?8, updated_at = ?9, pending_gmail_read = 0
                 WHERE id = ?1",
                params![
                    draft,
                    to_json(&fields.from),
                    to_json(&fields.to),
                    to_json(&fields.cc),
                    to_json(&fields.bcc),
                    fields.subject,
                    fields.body_html,
                    to_json(&fields.attachments),
                    ctx.now,
                ],
            )?;
            ctx.touch_draft(account, draft)?;
            if let Some(t) = thread {
                ctx.touch_thread(account, t)?;
            }
            if !is_untouched(ctx.tx, draft)? {
                queue_upsert(ctx, account, draft)?;
            }
            Ok(())
        }
    }
}

/// A draft's fields as stored (JSON columns unparsed), for the untouched check.
struct StoredFields {
    from: String,
    to: String,
    cc: String,
    bcc: String,
    subject: String,
    body: String,
    attachments: String,
    defaults: Option<String>,
}

/// Untouched: subject, body, and attachments as the composer started them,
/// and recipients and From the defaults.
pub fn is_untouched(conn: &Connection, draft: &DraftId) -> anyhow::Result<bool> {
    let row = conn
        .query_row(
            "SELECT from_json, to_json, cc_json, bcc_json, subject, body_html, attachments_json, defaults_json
             FROM drafts WHERE id = ?1",
            [draft],
            |r| {
                Ok(StoredFields {
                    from: r.get(0)?,
                    to: r.get(1)?,
                    cc: r.get(2)?,
                    bcc: r.get(3)?,
                    subject: r.get(4)?,
                    body: r.get(5)?,
                    attachments: r.get(6)?,
                    defaults: r.get(7)?,
                })
            },
        )
        .optional()?;
    let Some(stored) = row else {
        return Ok(false);
    };
    let Some(d) = stored
        .defaults
        .as_deref()
        .and_then(|d| serde_json::from_str::<Defaults>(d).ok())
    else {
        return Ok(false);
    };
    let from: Option<Addr> = serde_json::from_str(&stored.from).ok().flatten();
    let body = &stored.body;
    let atts: Vec<DraftAttachment> = from_json(&stored.attachments);
    let empty_body = ub_mime::html_to_text(body).trim().is_empty() && !body.contains("<img");
    Ok(from == d.from
        && from_json::<Vec<Addr>>(&stored.to) == d.to
        && from_json::<Vec<Addr>>(&stored.cc) == d.cc
        && from_json::<Vec<Addr>>(&stored.bcc) == d.bcc
        && stored.subject == d.subject
        && (empty_body || *body == d.body_html)
        && atts.iter().map(|a| &a.id).eq(&d.attachments))
}

/// Closing a composer (second Esc, switching, quitting). A draft started fresh
/// and untouched is discarded, locally and on Gmail.
pub fn close(ctx: &mut Ctx<'_>, draft: &DraftId) -> anyhow::Result<()> {
    let row: Option<(AccountId, bool, bool)> = ctx
        .tx
        .query_row(
            "SELECT account_id, started_fresh, pending_gmail_read FROM drafts WHERE id = ?1",
            [draft],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    let Some((account, fresh, pending_read)) = row else {
        return Ok(());
    };
    if in_outbox(ctx.tx, draft)? {
        return Ok(());
    }
    let untouched = is_untouched(ctx.tx, draft)?;
    if fresh && untouched {
        return discard(ctx, draft);
    }
    ctx.tx
        .execute("UPDATE drafts SET started_fresh = 0 WHERE id = ?1", [draft])?;
    ctx.touch_draft(account, draft)?;
    // A Gmail change skipped while this composer was open is read now. Edits
    // saved after it cleared the flag.
    if pending_read {
        ctx.effect(Effect::ReadDraft {
            account,
            draft: draft.clone(),
        });
    }
    Ok(())
}

/// Deletes the local draft and the Gmail draft.
pub fn discard(ctx: &mut Ctx<'_>, draft: &DraftId) -> anyhow::Result<()> {
    let row: Option<(AccountId, Option<ThreadId>, Option<String>)> = ctx
        .tx
        .query_row(
            "SELECT account_id, thread_id, gmail_draft_id FROM drafts WHERE id = ?1",
            [draft],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    let Some((account, thread, gmail)) = row else {
        return Ok(());
    };
    ctx.tx.execute(
        "DELETE FROM ops WHERE kind = 'draft_upsert' AND json_extract(payload, '$.draft') = ?1",
        [draft],
    )?;
    ctx.tx
        .execute("DELETE FROM drafts WHERE id = ?1", [draft])?;
    ctx.touched.drafts.insert(draft.clone());
    ctx.touched.draft_lists.insert(account);
    ctx.touch_summary(account);
    if let Some(t) = thread {
        crate::threads::touch_if_exists(ctx, account, t)?;
    }
    if let Some(g) = gmail {
        crate::queue::push(
            ctx,
            account,
            None,
            "draft_delete",
            &serde_json::json!({ "gmail_draft": g }),
        )?;
    }
    // Remove local Gmail copies of the draft message.
    let draft_msgs: Vec<MessageId> = {
        let mut stmt = ctx.tx.prepare(
            "SELECT id FROM messages WHERE account_id = ?1 AND ub_draft_id = ?2 AND is_draft = 1",
        )?;
        stmt.query_map(params![account, draft], |r| r.get(0))?
            .collect::<Result<_, _>>()?
    };
    for m in draft_msgs {
        crate::engine::ingest::delete_message(ctx, m)?;
    }
    if let Some(t) = thread {
        crate::threads::refresh(ctx, t)?;
    }
    let _ = std::fs::remove_dir_all(draft_dir(draft));
    Ok(())
}

/// Where a draft's added files are kept until sent or discarded.
pub fn draft_dir(draft: &DraftId) -> std::path::PathBuf {
    let name = draft.as_str().replace(['/', '.'], "_");
    match DRAFT_FILES.get() {
        Some(base) => base.join(name),
        None => std::env::temp_dir().join(name),
    }
}

pub static DRAFT_FILES: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();

/// On launch: apply the untouched rule to drafts whose started-fresh flag is
/// still set (e.g. after a crash).
pub fn on_launch(ctx: &mut Ctx<'_>) -> anyhow::Result<()> {
    let fresh: Vec<DraftId> = {
        let mut stmt = ctx
            .tx
            .prepare("SELECT id FROM drafts WHERE started_fresh = 1")?;
        stmt.query_map([], |r| r.get(0))?
            .collect::<Result<_, _>>()?
    };
    for d in fresh {
        close(ctx, &d)?;
    }
    Ok(())
}

/// Gmail rejected a draft op.
pub fn on_rejected(
    ctx: &mut Ctx<'_>,
    account: AccountId,
    kind: &str,
    payload: &str,
) -> anyhow::Result<()> {
    let v: serde_json::Value = serde_json::from_str(payload).unwrap_or_default();
    if kind == "draft_upsert"
        && let Ok(d) = DraftId::deserialize(&v["draft"])
    {
        ctx.touch_draft(account, &d)?;
    }
    Ok(())
}
