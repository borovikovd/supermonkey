//! Derives a thread's row from its messages (SPEC Views): the latest message,
//! the views it is in, and its split. Call `refresh` after any change to a
//! thread's messages or labels, inside the same transaction.

use std::collections::{BTreeSet, HashSet};

use rusqlite::{OptionalExtension, params};

use crate::db::{from_json, queries};
use crate::model::*;
use crate::writer::Ctx;

#[derive(Debug, Clone)]
pub struct Msg {
    pub id: MessageId,
    pub gmail_id: String,
    pub date: i64,
    pub from_name: String,
    pub from_email: String,
    pub to: Vec<Addr>,
    pub cc: Vec<Addr>,
    pub bcc: Vec<Addr>,
    pub subject: String,
    pub snippet: String,
    pub is_draft: bool,
    pub is_list: bool,
    pub labels: Vec<String>,
    pub has_attachments: bool,
}

impl Msg {
    pub fn has(&self, label: &str) -> bool {
        self.labels.iter().any(|l| l == label)
    }
    pub fn trashed_or_spam(&self) -> bool {
        self.has("TRASH") || self.has("SPAM")
    }
    /// Not a draft and not in trash or spam.
    pub fn live(&self) -> bool {
        !self.is_draft && !self.trashed_or_spam()
    }
}

pub fn messages(conn: &rusqlite::Connection, thread: ThreadId) -> anyhow::Result<Vec<Msg>> {
    let mut stmt = conn.prepare_cached(
        "SELECT id, gmail_id, internal_date, from_name, from_email, to_json, cc_json, bcc_json, subject, snippet,
                is_draft, is_list, label_ids, has_attachments
         FROM messages WHERE thread_id = ?1 ORDER BY internal_date, id",
    )?;
    let rows = stmt.query_map([thread], |r| {
        Ok(Msg {
            id: r.get(0)?,
            gmail_id: r.get(1)?,
            date: r.get(2)?,
            from_name: r.get(3)?,
            from_email: r.get(4)?,
            to: from_json(&r.get::<_, String>(5)?),
            cc: from_json(&r.get::<_, String>(6)?),
            bcc: from_json(&r.get::<_, String>(7)?),
            subject: r.get(8)?,
            snippet: r.get(9)?,
            is_draft: r.get(10)?,
            is_list: r.get(11)?,
            labels: from_json(&r.get::<_, String>(12)?),
            has_attachments: r.get(13)?,
        })
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// The thread's latest message: newest live message, else newest non-draft,
/// else the newest draft.
pub fn latest(msgs: &[Msg]) -> Option<&Msg> {
    msgs.iter()
        .rev()
        .find(|m| m.live())
        .or_else(|| msgs.iter().rev().find(|m| !m.is_draft))
        .or_else(|| msgs.last())
}

/// The message rules and pins read: the newest live message not sent from this
/// account, or the latest message when every message is from this account.
pub fn latest_from_someone_else<'a>(msgs: &'a [Msg], mine: &HashSet<String>) -> Option<&'a Msg> {
    msgs.iter()
        .rev()
        .find(|m| m.live() && !mine.contains(&m.from_email.to_lowercase()))
        .or_else(|| latest(msgs))
}

/// Labels the thread is in: labels on live messages, plus TRASH or SPAM when
/// any message has them.
pub fn thread_labels(msgs: &[Msg]) -> BTreeSet<String> {
    let mut set = BTreeSet::new();
    for m in msgs {
        if m.live() {
            set.extend(m.labels.iter().filter(|l| *l != "DRAFT").cloned());
        }
        if m.has("TRASH") {
            set.insert("TRASH".into());
        }
        if m.has("SPAM") {
            set.insert("SPAM".into());
        }
    }
    set
}

/// Recomputes a thread's row. Deletes it when it has no messages left.
pub fn refresh(ctx: &mut Ctx, thread: ThreadId) -> anyhow::Result<()> {
    let Some(account) = ctx
        .tx
        .query_row(
            "SELECT account_id FROM threads WHERE id = ?1",
            [thread],
            |r| r.get::<_, i64>(0),
        )
        .optional()?
    else {
        return Ok(());
    };
    let msgs = messages(ctx.tx, thread)?;
    if msgs.is_empty() {
        // Drafts keep their row; their thread link is cleared by the foreign key.
        ctx.tx
            .execute("DELETE FROM threads WHERE id = ?1", [thread])?;
        ctx.thread_gone(account, thread);
        return Ok(());
    }
    let latest = latest(&msgs).expect("non-empty");
    let labels = thread_labels(&msgs);
    let subject = msgs
        .iter()
        .find(|m| !m.is_draft && !m.subject.is_empty())
        .map(|m| m.subject.clone())
        .unwrap_or_else(|| latest.subject.clone());
    let in_inbox = labels.contains("INBOX");
    let live_count = msgs.iter().filter(|m| m.live()).count();
    let non_draft = msgs.iter().filter(|m| !m.is_draft).count();
    let shown = if live_count > 0 {
        live_count
    } else {
        non_draft.max(1)
    };

    let mine = queries::my_addresses(ctx.tx, account)?;
    let split = if in_inbox {
        let subject_msg = latest_from_someone_else(&msgs, &mine).expect("non-empty");
        crate::splits::route(ctx.tx, account, &labels, subject_msg)?
    } else {
        None
    };

    ctx.tx.execute(
        "UPDATE threads SET subject = ?2, snippet = ?3, sender_name = ?4, sender_email = ?5, sort_time = ?6,
            message_count = ?7, unread = ?8, starred = ?9, has_attachments = ?10, in_inbox = ?11,
            in_trash = ?12, in_spam = ?13, has_live = ?14,
            seen_in_inbox = seen_in_inbox OR ?11, split_id = ?15
         WHERE id = ?1",
        params![
            thread,
            subject,
            latest.snippet,
            if latest.from_name.is_empty() { &latest.from_email } else { &latest.from_name },
            latest.from_email,
            latest.date,
            shown as i64,
            msgs.iter().any(|m| m.live() && m.has("UNREAD")) || (live_count == 0 && msgs.iter().any(|m| m.has("UNREAD") && !m.is_draft)),
            labels.contains("STARRED"),
            msgs.iter().any(|m| !m.is_draft && m.has_attachments),
            in_inbox,
            labels.contains("TRASH"),
            labels.contains("SPAM"),
            live_count > 0,
            split,
        ],
    )?;
    ctx.tx
        .execute("DELETE FROM thread_labels WHERE thread_id = ?1", [thread])?;
    {
        let mut ins = ctx
            .tx
            .prepare_cached("INSERT INTO thread_labels (thread_id, label_id) VALUES (?1, ?2)")?;
        for l in &labels {
            ins.execute(params![thread, l])?;
        }
    }
    ctx.touch_thread(account, thread)?;
    Ok(())
}

/// Sets a message's label list and stamps it.
pub fn set_message_labels(
    ctx: &mut Ctx,
    message: MessageId,
    labels: &[String],
) -> anyhow::Result<()> {
    ctx.tx.execute(
        "UPDATE messages SET label_ids = ?2 WHERE id = ?1",
        params![message, crate::db::to_json(&labels)],
    )?;
    ctx.touch_message(message)?;
    Ok(())
}

pub fn account_of(
    conn: &rusqlite::Connection,
    thread: ThreadId,
) -> anyhow::Result<Option<AccountId>> {
    Ok(conn
        .query_row(
            "SELECT account_id FROM threads WHERE id = ?1",
            [thread],
            |r| r.get(0),
        )
        .optional()?)
}
