//! Turns Gmail messages into stored rows (SPEC Sync, Reading, Recipients).
//!
//! Parsing and sanitizing happen before a job reaches the writer thread:
//! `parse` runs on the async side, `store_thread` runs inside a writer job.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::sync::Arc;

use parking_lot::Mutex;
use rusqlite::{OptionalExtension, params};

use crate::db::{from_json, queries, to_json};
use crate::model::{AccountId, Addr, MessageId, Millis, ThreadId};
use crate::writer::{Ctx, Effect};

/// One message, parsed and (when the body was fetched) sanitized.
#[derive(Debug, Clone)]
pub struct Incoming {
    pub gmail_id: String,
    pub gmail_thread_id: String,
    pub internal_date: Millis,
    /// Gmail label ids.
    pub labels: Vec<String>,
    pub snippet: String,
    pub from: Option<Addr>,
    pub to: Vec<Addr>,
    pub cc: Vec<Addr>,
    pub bcc: Vec<Addr>,
    pub reply_to: Vec<Addr>,
    pub subject: String,
    pub message_id: Option<String>,
    pub in_reply_to: Option<String>,
    pub references: Vec<String>,
    pub is_list: bool,
    pub ub_draft_id: Option<String>,
    pub body: Option<Body>,
    pub parts: Vec<ub_mime::PartInfo>,
}

#[derive(Debug, Clone)]
pub struct Body {
    pub html: String,
    pub quote: Option<String>,
    pub text: String,
    pub plain: bool,
}

fn addr(a: ub_mime::Addr) -> Addr {
    Addr {
        name: a.name,
        email: a.email,
    }
}

fn addrs(v: Vec<ub_mime::Addr>) -> Vec<Addr> {
    v.into_iter().map(addr).collect()
}

/// Content-IDs of inline parts whose bytes must be fetched with `attachments.get`.
pub fn inline_parts_to_fetch(m: &ub_gmail::Message) -> Vec<(String, String, String)> {
    let Some(payload) = &m.payload else {
        return vec![];
    };
    let parsed = ub_mime::parse_gmail_payload(payload);
    parsed
        .parts
        .into_iter()
        .filter(|p| p.content_id.is_some() && p.data.is_none() && p.mime_type.starts_with("image/"))
        .filter_map(|p| Some((p.content_id?, p.attachment_id?, p.mime_type)))
        .collect()
}

/// Parses a Gmail message. `with_body` is true for `format=full`. `cid_data`
/// maps Content-IDs of fetched inline images to `data:` URIs.
pub fn parse(
    m: &ub_gmail::Message,
    with_body: bool,
    mut cid_data: HashMap<String, String>,
) -> Incoming {
    let mut parsed = m
        .payload
        .as_ref()
        .map(ub_mime::parse_gmail_payload)
        .unwrap_or_default();
    let body = with_body.then(|| {
        for p in &parsed.parts {
            if let (Some(cid), Some(data)) = (&p.content_id, &p.data) {
                cid_data
                    .entry(cid.clone())
                    .or_insert_with(|| ub_mime::data_uri(&p.mime_type, data));
            }
        }
        let text = parsed.text.take();
        let (raw, plain) = match (parsed.html.take(), &text) {
            (Some(h), _) => (h, false),
            (None, Some(t)) => (ub_mime::text_to_html(t), true),
            (None, None) => (String::new(), true),
        };
        let (visible, quote) = ub_mime::split_quoted_history(&raw);
        let text = text.unwrap_or_else(|| ub_mime::html_to_text(&raw));
        Body {
            html: ub_mime::sanitize_html(visible, &cid_data),
            quote: quote.map(|q| ub_mime::sanitize_html(q, &cid_data)),
            text,
            plain,
        }
    });
    Incoming {
        gmail_id: m.id.clone(),
        gmail_thread_id: m.thread_id.clone(),
        internal_date: m
            .internal_date
            .and_then(|d| i64::try_from(d).ok())
            .or(parsed.date_ms)
            .unwrap_or(0),
        labels: m.label_ids.clone(),
        snippet: html_escape::decode_html_entities(&m.snippet).into_owned(),
        from: parsed.from.map(addr),
        to: addrs(parsed.to),
        cc: addrs(parsed.cc),
        bcc: addrs(parsed.bcc),
        reply_to: addrs(parsed.reply_to),
        subject: parsed.subject,
        message_id: parsed.message_id,
        in_reply_to: parsed.in_reply_to,
        references: parsed.references,
        is_list: parsed.is_list,
        ub_draft_id: parsed.ub_draft_id,
        body,
        parts: parsed.parts,
    }
}

/// Maps Gmail label ids to local ids. Unknown ids are kept as they are.
pub fn local_labels(
    ctx: &Ctx<'_>,
    account: AccountId,
    gmail: &[String],
) -> anyhow::Result<(Vec<String>, bool)> {
    let mut stmt = ctx
        .tx
        .prepare_cached("SELECT id FROM labels WHERE account_id = ?1 AND gmail_id = ?2")?;
    let mut out = Vec::with_capacity(gmail.len());
    let mut unknown = false;
    for g in gmail {
        if let Some(id) = stmt
            .query_row(params![account, g], |r| r.get::<_, String>(0))
            .optional()?
        {
            out.push(id);
        } else {
            unknown = true;
            out.push(g.clone());
        }
    }
    Ok((out, unknown))
}

/// Pending modify ops, in order, so a queued action wins over a remote change
/// to the messages it recorded (SPEC Offline).
pub struct PendingOps(Vec<crate::mail::ModifyOp>);

impl PendingOps {
    /// The queued ops, after `landed`: those that reached Gmail while the
    /// answer being written was in flight.
    pub fn load(
        conn: &rusqlite::Connection,
        account: AccountId,
        landed: Vec<crate::mail::ModifyOp>,
    ) -> anyhow::Result<PendingOps> {
        let mut stmt = conn.prepare_cached(
            "SELECT payload FROM ops WHERE account_id = ?1 AND kind = 'modify' ORDER BY id",
        )?;
        let mut ops = landed;
        ops.extend(
            stmt.query_map([account], |r| r.get::<_, String>(0))?
                .filter_map(|p| p.ok().and_then(|p| serde_json::from_str(&p).ok())),
        );
        Ok(PendingOps(ops))
    }

    pub fn touches(&self, gmail_msg: &str) -> bool {
        self.0
            .iter()
            .any(|o| o.messages.iter().any(|m| m == gmail_msg))
    }

    pub fn apply(&self, gmail_msg: &str, labels: &mut Vec<String>) {
        for op in self
            .0
            .iter()
            .filter(|o| o.messages.iter().any(|m| m == gmail_msg))
        {
            labels.retain(|l| !op.remove.contains(l));
            for a in &op.add {
                if !labels.contains(a) {
                    labels.push(a.clone());
                }
            }
        }
    }
}

/// Modify ops that left the queue while a Gmail read was in flight. Its
/// answer may predate them, so they are re-applied on top of it like queued
/// ops; otherwise it would undo them locally (a woken snooze losing INBOX).
#[derive(Default)]
pub struct Landed(Mutex<LandedLog>);

#[derive(Default)]
struct LandedLog {
    next: u64,
    ops: VecDeque<(u64, crate::mail::ModifyOp)>,
    /// Reads in flight: how many started at each point of the log.
    reads: BTreeMap<u64, usize>,
}

impl Landed {
    /// Starts a read whose answer will be written. Take it before the request.
    pub fn read(self: &Arc<Self>) -> Read {
        let mut log = self.0.lock();
        let at = log.next;
        *log.reads.entry(at).or_default() += 1;
        Read {
            landed: self.clone(),
            at,
        }
    }

    /// An op reached Gmail. Call before its queue row goes.
    pub fn push(&self, op: crate::mail::ModifyOp) {
        let mut log = self.0.lock();
        if log.reads.is_empty() {
            return;
        }
        let seq = log.next;
        log.next += 1;
        log.ops.push_back((seq, op));
    }
}

/// A read in flight; see [`Landed`].
pub struct Read {
    landed: Arc<Landed>,
    at: u64,
}

impl Read {
    /// The ops that landed since the read started, oldest first.
    pub fn landed(&self) -> Vec<crate::mail::ModifyOp> {
        let log = self.landed.0.lock();
        log.ops
            .iter()
            .filter(|(seq, _)| *seq >= self.at)
            .map(|(_, op)| op.clone())
            .collect()
    }
}

impl Drop for Read {
    fn drop(&mut self) {
        let mut log = self.landed.0.lock();
        if let Some(n) = log.reads.get_mut(&self.at) {
            *n -= 1;
            if *n == 0 {
                log.reads.remove(&self.at);
            }
        }
        // Keep only what a read still in flight can need.
        let oldest = log.reads.keys().next().copied().unwrap_or(u64::MAX);
        while log.ops.front().is_some_and(|(seq, _)| *seq < oldest) {
            log.ops.pop_front();
        }
    }
}

/// Stores a thread fetched from Gmail. `complete` means `msgs` is every
/// message Gmail has on the thread, so local messages missing from it are gone.
pub fn store_thread(
    ctx: &mut Ctx<'_>,
    account: AccountId,
    gmail_thread_id: &str,
    history_id: Option<u64>,
    msgs: &[Incoming],
    complete: bool,
    pending: &PendingOps,
) -> anyhow::Result<ThreadId> {
    let thread = if let Some(t) = queries::thread_by_gmail(ctx.tx, account, gmail_thread_id)? {
        t
    } else {
        ctx.tx.execute(
            "INSERT INTO threads (account_id, gmail_id) VALUES (?1, ?2)",
            params![account, gmail_thread_id],
        )?;
        ctx.tx.last_insert_rowid()
    };
    if let Some(h) = history_id {
        ctx.tx.execute(
            "UPDATE threads SET history_id = ?2 WHERE id = ?1",
            params![thread, h.to_string()],
        )?;
    }
    let mine = queries::my_addresses(ctx.tx, account)?;
    let mut unknown_labels = false;
    for m in msgs {
        let (mut labels, unknown) = local_labels(ctx, account, &m.labels)?;
        unknown_labels |= unknown;
        pending.apply(&m.gmail_id, &mut labels);
        store_message(ctx, account, thread, m, &labels, &mine)?;
    }
    if complete {
        let keep: HashSet<&str> = msgs.iter().map(|m| m.gmail_id.as_str()).collect();
        let local: Vec<(MessageId, String)> = {
            let mut stmt = ctx
                .tx
                .prepare_cached("SELECT id, gmail_id FROM messages WHERE thread_id = ?1")?;
            stmt.query_map([thread], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<Result<_, _>>()?
        };
        for (id, gid) in local {
            if !keep.contains(gid.as_str()) {
                delete_message(ctx, id)?;
            }
        }
    }
    if unknown_labels {
        ctx.effect(Effect::RefreshLabels(account));
    }
    crate::threads::refresh(ctx, thread)?;
    Ok(thread)
}

pub fn delete_message(ctx: &mut Ctx<'_>, id: MessageId) -> anyhow::Result<()> {
    ctx.tx
        .execute("DELETE FROM messages_fts WHERE rowid = ?1", [id])?;
    ctx.tx.execute("DELETE FROM messages WHERE id = ?1", [id])?;
    Ok(())
}

fn store_message(
    ctx: &mut Ctx<'_>,
    account: AccountId,
    thread: ThreadId,
    m: &Incoming,
    labels: &[String],
    mine: &HashSet<String>,
) -> anyhow::Result<MessageId> {
    let is_draft = m.labels.iter().any(|l| l == "DRAFT");
    let none = Addr {
        name: None,
        email: String::new(),
    };
    let from = m.from.as_ref().unwrap_or(&none);
    let existing: Option<(MessageId, bool)> = ctx
        .tx
        .query_row(
            "SELECT id, has_body FROM messages WHERE account_id = ?1 AND gmail_id = ?2",
            params![account, m.gmail_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    let has_attachments = m.parts.iter().any(|p| !p.inline);
    let id = if let Some((id, _)) = existing {
        ctx.tx.execute(
            "UPDATE messages SET thread_id = ?2, label_ids = ?3, snippet = ?4, is_draft = ?5 WHERE id = ?1",
            params![id, thread, to_json(&labels), m.snippet, is_draft],
        )?;
        id
    } else {
        ctx.tx.execute(
            "INSERT INTO messages (account_id, thread_id, gmail_id, internal_date, from_name, from_email, to_json,
                cc_json, bcc_json, reply_to_json, subject, snippet, message_id_hdr, in_reply_to, references_hdr,
                is_list, is_draft, ub_draft_id, label_ids, has_attachments)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20)",
            params![
                account,
                thread,
                m.gmail_id,
                m.internal_date,
                from.name.as_deref().unwrap_or(""),
                from.email,
                to_json(&m.to),
                to_json(&m.cc),
                to_json(&m.bcc),
                to_json(&m.reply_to),
                m.subject,
                m.snippet,
                m.message_id,
                m.in_reply_to,
                m.references.join(" "),
                m.is_list,
                is_draft,
                m.ub_draft_id,
                to_json(&labels),
                has_attachments,
            ],
        )?;
        let id = ctx.tx.last_insert_rowid();
        store_parts(ctx, id, &m.parts)?;
        id
    };
    let had_body = existing.is_some_and(|(_, b)| b);
    if let Some(body) = &m.body
        && !had_body
    {
        // Stored bodies never change.
        ctx.tx.execute(
            "UPDATE messages SET has_body = 1, body_html = ?2, quote_html = ?3, body_text = ?4, is_plain = ?5 WHERE id = ?1",
            params![id, body.html, body.quote, body.text, body.plain],
        )?;
        if existing.is_some() {
            // Metadata rows may not have listed every part yet.
            ctx.tx.execute(
                "DELETE FROM attachments WHERE message_id = ?1 AND local_path IS NULL",
                [id],
            )?;
            store_parts(ctx, id, &m.parts)?;
        }
    }
    if !is_draft {
        let to_text = m.to.iter().map(fmt_addr).collect::<Vec<_>>().join(" ");
        let cc_text = m.cc.iter().map(fmt_addr).collect::<Vec<_>>().join(" ");
        let body_text = m.body.as_ref().map(|b| b.text.as_str());
        ctx.tx
            .execute("DELETE FROM messages_fts WHERE rowid = ?1", [id])?;
        ctx.tx.execute(
            "INSERT INTO messages_fts (rowid, from_text, to_text, cc_text, subject, body)
             VALUES (?1, ?2, ?3, ?4, ?5, coalesce(?6, (SELECT body_text FROM messages WHERE id = ?1), ''))",
            params![id, fmt_addr(from), to_text, cc_text, m.subject, body_text],
        )?;
        let spam = m.labels.iter().any(|l| l == "SPAM");
        if !spam {
            count_contacts(ctx, account, m, from, mine)?;
        }
    }
    ctx.touch_message(id)?;
    Ok(id)
}

fn fmt_addr(a: &Addr) -> String {
    match &a.name {
        Some(n) if !n.is_empty() => format!("{n} {}", a.email),
        _ => a.email.clone(),
    }
}

fn store_parts(
    ctx: &mut Ctx<'_>,
    message: MessageId,
    parts: &[ub_mime::PartInfo],
) -> anyhow::Result<()> {
    let mut ins = ctx.tx.prepare_cached(
        "INSERT INTO attachments (message_id, part_id, filename, mime_type, size, gmail_att_id, content_id, inline)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
    )?;
    for p in parts {
        ins.execute(params![
            message,
            p.part_id,
            p.filename,
            p.mime_type,
            i64::try_from(p.size).unwrap_or(i64::MAX),
            p.attachment_id,
            p.content_id,
            p.inline
        ])?;
    }
    Ok(())
}

/// Recipient autocomplete upkeep (SPEC Recipients and autocomplete). Each
/// message id counts once.
pub fn count_contacts(
    ctx: &mut Ctx<'_>,
    account: AccountId,
    m: &Incoming,
    from: &Addr,
    mine: &HashSet<String>,
) -> anyhow::Result<()> {
    let fresh = ctx.tx.execute(
        "INSERT OR IGNORE INTO contact_sources (account_id, gmail_msg_id) VALUES (?1, ?2)",
        params![account, m.gmail_id],
    )?;
    if fresh == 0 {
        return Ok(());
    }
    let sent = mine.contains(&from.email.to_lowercase());
    let mut upsert = ctx.tx.prepare_cached(
        "INSERT INTO contacts (account_id, email, name, sent_count, recv_count, list_only, last_contact)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
         ON CONFLICT (account_id, email) DO UPDATE SET
           name = CASE WHEN excluded.name != '' AND excluded.last_contact >= contacts.last_contact
                       THEN excluded.name ELSE contacts.name END,
           sent_count = contacts.sent_count + excluded.sent_count,
           recv_count = contacts.recv_count + excluded.recv_count,
           list_only = min(contacts.list_only, excluded.list_only),
           last_contact = max(contacts.last_contact, excluded.last_contact)",
    )?;
    let mut put =
        |a: &Addr, sent_n: i64, recv_n: i64, list_only: bool| -> rusqlite::Result<usize> {
            let email = a.email.trim().to_lowercase();
            if email.is_empty() || !email.contains('@') || mine.contains(&email) {
                return Ok(0);
            }
            upsert.execute(params![
                account,
                email,
                a.name.as_deref().unwrap_or(""),
                sent_n,
                recv_n,
                list_only,
                m.internal_date
            ])
        };
    if sent {
        // On mail this account sent, each To and Cc address gains a sent count.
        for a in m.to.iter().chain(&m.cc).chain(&m.bcc) {
            put(a, 1, 0, false)?;
        }
    } else {
        // Only From gains a received count. Co-recipients are stored with no
        // count, unless this is list mail, which adds nothing for them.
        put(from, 0, 1, m.is_list)?;
        if !m.is_list {
            for a in m.to.iter().chain(&m.cc) {
                put(a, 0, 0, false)?;
            }
        }
    }
    Ok(())
}

/// Applies Gmail's labels for a known message, then re-applies pending ops.
pub fn set_labels_from_gmail(
    ctx: &mut Ctx<'_>,
    account: AccountId,
    gmail_msg: &str,
    gmail_labels: &[String],
    pending: &PendingOps,
) -> anyhow::Result<Option<ThreadId>> {
    let Some((id, thread, current)) = ctx
        .tx
        .query_row(
            "SELECT id, thread_id, label_ids FROM messages WHERE account_id = ?1 AND gmail_id = ?2",
            params![account, gmail_msg],
            |r| {
                Ok((
                    r.get::<_, MessageId>(0)?,
                    r.get::<_, ThreadId>(1)?,
                    r.get::<_, String>(2)?,
                ))
            },
        )
        .optional()?
    else {
        return Ok(None);
    };
    let (mut labels, unknown) = local_labels(ctx, account, gmail_labels)?;
    if unknown {
        ctx.effect(Effect::RefreshLabels(account));
    }
    pending.apply(gmail_msg, &mut labels);
    let current: Vec<String> = from_json(&current);
    if current != labels {
        crate::threads::set_message_labels(ctx, id, &labels)?;
    }
    Ok(Some(thread))
}
