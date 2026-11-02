//! Drafts on the writer side (SPEC Compose → Drafts, Addressing; UI state →
//! Actions). A fresh composer's first save carries only its kind and target;
//! Rust fills From, To, Cc, Subject, and the quote, and keeps them as the
//! draft's defaults for the untouched check.

use std::collections::HashSet;

use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};

use crate::db::{from_json, queries, to_json};
use crate::model::*;
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
    date: i64,
    message_id: Option<String>,
    references: String,
    html: String,
}

fn target(ctx: &Ctx, id: MessageId) -> anyhow::Result<Option<Target>> {
    Ok(ctx
        .tx
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

fn send_as(ctx: &Ctx, account: AccountId) -> anyhow::Result<(Vec<SendAs>, Addr)> {
    let (email, name, json): (String, String, String) = ctx.tx.query_row(
        "SELECT email, display_name, send_as FROM accounts WHERE id = ?1",
        [account],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    let list: Vec<SendAs> = from_json(&json);
    let default = list
        .iter()
        .find(|s| s.is_default)
        .or_else(|| list.iter().find(|s| s.is_primary))
        .map(|s| Addr {
            name: (!s.name.is_empty()).then(|| s.name.clone()),
            email: s.email.clone(),
        })
        .unwrap_or(Addr {
            name: (!name.is_empty() && name != email).then_some(name),
            email,
        });
    Ok((list, default))
}

fn same(a: &str, b: &str) -> bool {
    a.eq_ignore_ascii_case(b)
}

fn dedup(list: Vec<Addr>, skip: &HashSet<String>) -> Vec<Addr> {
    let mut seen = skip.clone();
    list.into_iter()
        .filter(|a| seen.insert(a.email.to_lowercase()))
        .collect()
}

/// Fills a fresh composer by the rules in SPEC Addressing.
#[allow(clippy::too_many_arguments)]
fn create(
    ctx: &mut Ctx,
    draft: &str,
    account: AccountId,
    kind: DraftKind,
    reply_all: bool,
    thread: Option<ThreadId>,
    target_id: Option<MessageId>,
    started_on_latest: bool,
) -> anyhow::Result<()> {
    let (list, default_from) = send_as(ctx, account)?;
    let mine = queries::my_addresses(ctx.tx, account)?;
    let target = match target_id {
        Some(t) => target(ctx, t)?,
        None => None,
    };
    let mut from = default_from.clone();
    let mut to = Vec::new();
    let mut cc = Vec::new();
    let mut subject = String::new();
    let mut quote = None;
    let mut in_reply_to = None;
    let mut references = String::new();
    let mut attachments: Vec<DraftAttachment> = Vec::new();

    if let Some(t) = &target {
        let sent_by_me = mine.contains(&t.from.email.to_lowercase());
        in_reply_to = t.message_id.clone();
        references = format!(
            "{} {}",
            t.references,
            t.message_id.clone().unwrap_or_default()
        )
        .trim()
        .to_string();
        match kind {
            DraftKind::Reply => {
                subject = ub_mime::reply_subject(&t.subject);
                quote = Some(ub_mime::reply_quote_html(
                    &quote_source(t),
                    tz_offset_minutes(),
                ));
                if sent_by_me {
                    // Replying to my own message: send from its From, to its To.
                    from = t.from.clone();
                    to = t.to.clone();
                    if reply_all {
                        cc = dedup(t.cc.clone(), &mine);
                    }
                } else {
                    // From the sendAs address the message was sent to, else the default.
                    if let Some(s) = list
                        .iter()
                        .find(|s| t.to.iter().chain(&t.cc).any(|a| same(&a.email, &s.email)))
                    {
                        from = Addr {
                            name: (!s.name.is_empty()).then(|| s.name.clone()),
                            email: s.email.clone(),
                        };
                    }
                    to = if t.reply_to.is_empty() {
                        vec![t.from.clone()]
                    } else {
                        t.reply_to.clone()
                    };
                    if reply_all {
                        let mut skip = mine.clone();
                        skip.extend(to.iter().map(|a| a.email.to_lowercase()));
                        cc = dedup(t.to.iter().chain(&t.cc).cloned().collect(), &skip);
                    }
                }
            }
            DraftKind::Forward => {
                subject = ub_mime::forward_subject(&t.subject);
                quote = Some(ub_mime::forward_quote_html(
                    &quote_source(t),
                    tz_offset_minutes(),
                ));
                // Forward carries the target's attachments.
                let mut stmt = ctx.tx.prepare_cached(
                    "SELECT id, filename, mime_type, size, local_path FROM attachments WHERE message_id = ?1 AND inline = 0",
                )?;
                attachments = stmt
                    .query_map([t.id], |r| {
                        let id: i64 = r.get(0)?;
                        Ok(DraftAttachment {
                            id: format!("src-{id}"),
                            filename: r.get(1)?,
                            mime_type: r.get(2)?,
                            size: r.get(3)?,
                            path: r.get(4)?,
                            source: Some(id),
                        })
                    })?
                    .collect::<Result<_, _>>()?;
            }
            DraftKind::New => {}
        }
    }
    let defaults = Defaults {
        from: Some(from.clone()),
        to: to.clone(),
        cc: cc.clone(),
        bcc: vec![],
        subject: subject.clone(),
        body_html: String::new(),
        attachments: attachments.iter().map(|a| a.id.clone()).collect(),
    };
    ctx.tx.execute(
        "INSERT INTO drafts (id, account_id, kind, reply_all, thread_id, target_message_id, started_on_latest,
            started_fresh, from_json, to_json, cc_json, subject, quote_html, attachments_json, defaults_json,
            in_reply_to, references_hdr, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 1, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)",
        params![
            draft,
            account,
            kind.as_str(),
            reply_all,
            thread,
            target_id,
            started_on_latest,
            to_json(&Some(from)),
            to_json(&to),
            to_json(&cc),
            subject,
            quote,
            to_json(&attachments),
            to_json(&defaults),
            in_reply_to,
            references,
            ctx.now,
        ],
    )?;
    ctx.touch_draft(account, draft)?;
    if let Some(t) = thread {
        ctx.touch_thread(account, t)?;
    }
    Ok(())
}

pub fn account_of(ctx: &Ctx, draft: &str) -> anyhow::Result<Option<(AccountId, Option<ThreadId>)>> {
    Ok(ctx
        .tx
        .query_row(
            "SELECT account_id, thread_id FROM drafts WHERE id = ?1",
            [draft],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?)
}

/// Queues one upsert per draft; the runner reads the draft's latest state.
pub fn queue_upsert(ctx: &mut Ctx, account: AccountId, draft: &str) -> anyhow::Result<()> {
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

pub fn save(ctx: &mut Ctx, draft: &str, save: DraftSave) -> anyhow::Result<()> {
    // A save that arrives after its draft was sent or discarded is dropped.
    let in_outbox: bool = ctx.tx.query_row(
        "SELECT EXISTS (SELECT 1 FROM outbox WHERE draft_id = ?1)",
        [draft],
        |r| r.get(0),
    )?;
    if in_outbox {
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
            if account_of(ctx, draft)?.is_some() {
                return Ok(());
            }
            create(
                ctx,
                draft,
                account,
                kind,
                reply_all,
                thread,
                target,
                started_on_latest,
            )
        }
        DraftSave::Update { fields } => {
            let Some((account, thread)) = account_of(ctx, draft)? else {
                return Ok(());
            };
            ctx.tx.execute(
                "UPDATE drafts SET from_json = ?2, to_json = ?3, cc_json = ?4, bcc_json = ?5, subject = ?6,
                    body_html = ?7, attachments_json = ?8, updated_at = ?9 WHERE id = ?1",
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
            if !is_untouched(ctx, draft)? {
                queue_upsert(ctx, account, draft)?;
            }
            Ok(())
        }
    }
}

/// Untouched: subject, body, and attachments as the composer started them,
/// and recipients and From the defaults.
pub fn is_untouched(ctx: &Ctx, draft: &str) -> anyhow::Result<bool> {
    let row: Option<(String, String, String, String, String, String, String, Option<String>)> = ctx
        .tx
        .query_row(
            "SELECT from_json, to_json, cc_json, bcc_json, subject, body_html, attachments_json, defaults_json
             FROM drafts WHERE id = ?1",
            [draft],
            |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                    r.get(6)?,
                    r.get(7)?,
                ))
            },
        )
        .optional()?;
    let Some((from, to, cc, bcc, subject, body, atts, defaults)) = row else {
        return Ok(false);
    };
    let Some(d) = defaults.and_then(|d| serde_json::from_str::<Defaults>(&d).ok()) else {
        return Ok(false);
    };
    let atts: Vec<DraftAttachment> = from_json(&atts);
    let empty_body = ub_mime::html_to_text(&body).trim().is_empty() && !body.contains("<img");
    Ok(
        serde_json::from_str::<Option<Addr>>(&from).ok().flatten() == d.from
            && from_json::<Vec<Addr>>(&to) == d.to
            && from_json::<Vec<Addr>>(&cc) == d.cc
            && from_json::<Vec<Addr>>(&bcc) == d.bcc
            && subject == d.subject
            && (empty_body || body == d.body_html)
            && atts.iter().map(|a| a.id.clone()).collect::<Vec<_>>() == d.attachments,
    )
}

/// Closing a composer (second Esc, switching, quitting). A draft started fresh
/// and untouched is discarded, locally and on Gmail.
pub fn close(ctx: &mut Ctx, draft: &str) -> anyhow::Result<()> {
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
    let in_outbox: bool = ctx.tx.query_row(
        "SELECT EXISTS (SELECT 1 FROM outbox WHERE draft_id = ?1)",
        [draft],
        |r| r.get(0),
    )?;
    if in_outbox {
        return Ok(());
    }
    let untouched = is_untouched(ctx, draft)?;
    if fresh && untouched {
        return discard(ctx, draft);
    }
    ctx.tx
        .execute("UPDATE drafts SET started_fresh = 0 WHERE id = ?1", [draft])?;
    ctx.touch_draft(account, draft)?;
    // A Gmail change skipped while this composer was open is read now, if no edits were saved.
    if pending_read && untouched {
        ctx.effect(Effect::ReadDraft {
            account,
            draft: draft.to_string(),
        });
    }
    Ok(())
}

/// Deletes the local draft and the Gmail draft.
pub fn discard(ctx: &mut Ctx, draft: &str) -> anyhow::Result<()> {
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
    ctx.touched.drafts.insert(draft.to_string());
    ctx.touched.draft_lists.insert(account);
    ctx.touch_summary(account);
    if let Some(t) = thread
        && ctx
            .tx
            .query_row("SELECT 1 FROM threads WHERE id = ?1", [t], |_| Ok(()))
            .is_ok()
    {
        ctx.touch_thread(account, t)?;
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
    let draft_msgs: Vec<i64> = {
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
pub fn draft_dir(draft: &str) -> std::path::PathBuf {
    let base = DRAFT_FILES
        .get()
        .cloned()
        .unwrap_or_else(std::env::temp_dir);
    base.join(draft.replace(['/', '.'], "_"))
}

pub static DRAFT_FILES: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();

/// On launch: apply the untouched rule to drafts whose started-fresh flag is
/// still set (e.g. after a crash).
pub fn on_launch(ctx: &mut Ctx) -> anyhow::Result<()> {
    let fresh: Vec<String> = {
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
    ctx: &mut Ctx,
    account: AccountId,
    kind: &str,
    payload: &str,
) -> anyhow::Result<()> {
    let v: serde_json::Value = serde_json::from_str(payload).unwrap_or_default();
    if kind == "draft_upsert"
        && let Some(d) = v["draft"].as_str()
    {
        ctx.touch_draft(account, d)?;
    }
    Ok(())
}
