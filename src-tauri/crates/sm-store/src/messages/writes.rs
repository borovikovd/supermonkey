//! Writes of messages: each re-derives the threads it touches (see
//! `routing`) and keeps the search index in step.

use std::collections::BTreeSet;

use rusqlite::named_params;
use sm_domain::{
    AccountId, AttachmentId, DraftId, LabelSet, MessageChange, MessageId, SystemLabel, ThreadId,
};

use super::model::{NewBody, NewMessage};
use crate::StoreError;
use crate::db::{Db, id_list, json_column, to_json};
use crate::routing;
use crate::tx::{WriteTx, require};

/// Writes to messages. Each re-derives, stamps, and records the threads it
/// touches, once per call.
#[derive(Debug)]
pub struct MessageWrites<'t, 'c> {
    tx: &'t mut WriteTx<'c>,
}

impl<'t, 'c> MessageWrites<'t, 'c> {
    pub(crate) const fn new(tx: &'t mut WriteTx<'c>) -> Self {
        Self { tx }
    }

    /// Stores a message's headers and labels in `thread`, adding it or
    /// replacing what was stored (a metadata fetch replaces labels). Its body,
    /// if stored, stays. Returns its id.
    pub fn upsert(
        &mut self,
        thread: ThreadId,
        message: &NewMessage,
    ) -> Result<MessageId, StoreError> {
        let account = require(
            self.tx.read().threads().account_of(thread)?,
            "thread",
            &thread,
        )?;
        let mut touched = BTreeSet::from([thread]);
        let (id, _) = upsert_row(self.tx, account, thread, message, &mut touched)?;
        derive_all(self.tx, touched)?;
        Ok(id)
    }

    /// Sets messages' labels, as a sync or a merge decides them.
    pub fn set_labels(&mut self, labels: &[(MessageId, LabelSet)]) -> Result<(), StoreError> {
        let mut touched = BTreeSet::new();
        for (id, set) in labels {
            touched.insert(write_labels(self.tx, *id, set)?);
        }
        derive_all(self.tx, touched)
    }

    /// Applies an action's label changes, each to the message's current
    /// labels. Messages that are gone are skipped.
    pub fn apply(&mut self, changes: &[MessageChange]) -> Result<(), StoreError> {
        let mut touched = BTreeSet::new();
        for change in changes {
            let Some(current) = self.tx.db().optional(
                "read a message's labels",
                "SELECT label_ids FROM messages WHERE id = ?1",
                [change.message],
                |r| json_column::<LabelSet>(r, "label_ids"),
            )?
            else {
                continue;
            };
            touched.insert(write_labels(
                self.tx,
                change.message,
                &change.apply(&current),
            )?);
        }
        derive_all(self.tx, touched)
    }

    /// Stores a message's body and attachments. Attachments keep their id and
    /// downloaded file by part id; parts the body no longer has go.
    pub fn store_body(&mut self, id: MessageId, body: &NewBody) -> Result<(), StoreError> {
        let rev = self.tx.stamp();
        let db = self.tx.db();
        let thread: ThreadId = require(
            db.optional(
                "find a message's thread",
                "SELECT thread_id FROM messages WHERE id = ?1",
                [id],
                |r| r.get("thread_id"),
            )?,
            "message",
            &id,
        )?;
        db.execute(
            "store a message's body",
            "UPDATE messages SET has_body = 1, body_html = :html, quote_html = :quote,
                 body_text = :text, is_plain = :plain, rev = :rev
             WHERE id = :id",
            named_params! {
                ":id": id, ":html": body.html, ":quote": body.quote_html,
                ":text": body.text, ":plain": body.is_plain, ":rev": rev,
            },
        )?;
        for part in &body.attachments {
            db.execute(
                "store an attachment",
                "INSERT INTO attachments (message_id, part_id, filename, mime_type, size,
                     gmail_att_id, content_id, is_inline)
                 VALUES (:message, :part, :filename, :mime, :size, :gmail, :cid, :inline)
                 ON CONFLICT (message_id, part_id) DO UPDATE SET
                     filename = excluded.filename, mime_type = excluded.mime_type,
                     size = excluded.size, gmail_att_id = excluded.gmail_att_id,
                     content_id = excluded.content_id, is_inline = excluded.is_inline",
                named_params! {
                    ":message": id, ":part": part.part_id, ":filename": part.filename,
                    ":mime": part.mime_type, ":size": part.size,
                    ":gmail": part.gmail_attachment_id, ":cid": part.content_id,
                    ":inline": part.is_inline,
                },
            )?;
        }
        let parts: Vec<&str> = body
            .attachments
            .iter()
            .map(|a| a.part_id.as_str())
            .collect();
        db.execute(
            "drop attachments a body no longer has",
            "DELETE FROM attachments WHERE message_id = ?1
               AND part_id NOT IN (SELECT value FROM json_each(?2))",
            (id, id_list(&parts)?),
        )?;
        reindex(db, id)?;
        self.tx.record().message(id);
        derive_all(self.tx, BTreeSet::from([thread]))
    }

    /// Deletes messages, from Gmail's history or a refetch. Drafts aimed at
    /// one lose their target. Unknown ids are skipped.
    pub fn delete(&mut self, ids: &[MessageId]) -> Result<(), StoreError> {
        let mut touched = BTreeSet::new();
        for &id in ids {
            if let Some(thread) = delete_row(self.tx, id)? {
                touched.insert(thread);
            }
        }
        derive_all(self.tx, touched)
    }

    /// Records where an attachment was downloaded.
    pub fn set_attachment_path(&mut self, id: AttachmentId, path: &str) -> Result<(), StoreError> {
        let rev = self.tx.stamp();
        let db = self.tx.db();
        let message: MessageId = require(
            db.optional(
                "find an attachment's message",
                "SELECT message_id FROM attachments WHERE id = ?1",
                [id],
                |r| r.get("message_id"),
            )?,
            "attachment",
            &id,
        )?;
        db.execute(
            "store an attachment's file",
            "UPDATE attachments SET local_path = ?2 WHERE id = ?1",
            (id, path),
        )?;
        db.execute(
            "stamp a message",
            "UPDATE messages SET rev = ?2 WHERE id = ?1",
            (message, rev),
        )?;
        self.tx.record().message(message);
        let thread = self.tx.read().messages().get(message)?.map(|m| m.thread);
        derive_all(self.tx, thread.into_iter().collect())
    }
}

/// Inserts or updates one message row and its index entry. Adds to `touched`
/// the thread it is in and, when it moved, the one it left. Returns its id
/// and whether it is new.
pub(crate) fn upsert_row(
    tx: &mut WriteTx<'_>,
    account: AccountId,
    thread: ThreadId,
    m: &NewMessage,
    touched: &mut BTreeSet<ThreadId>,
) -> Result<(MessageId, bool), StoreError> {
    let rev = tx.stamp();
    let db = tx.db();
    let is_live =
        !m.is_draft && !m.labels.has(SystemLabel::Trash) && !m.labels.has(SystemLabel::Spam);
    let existing: Option<(MessageId, ThreadId)> = db.optional(
        "find a stored message",
        "SELECT id, thread_id FROM messages WHERE account_id = ?1 AND gmail_id = ?2",
        (account, &m.gmail_id),
        |r| Ok((r.get("id")?, r.get("thread_id")?)),
    )?;
    let to = to_json("to_addrs", &m.to)?;
    let cc = to_json("cc_addrs", &m.cc)?;
    let bcc = to_json("bcc_addrs", &m.bcc)?;
    let reply_to = to_json("reply_to", &m.reply_to)?;
    let labels = to_json("label_ids", &m.labels)?;
    let params = named_params! {
        ":account": account, ":thread": thread, ":gmail": m.gmail_id, ":date": m.date,
        ":from_name": m.from.name, ":from_email": m.from.email, ":to": to, ":cc": cc,
        ":bcc": bcc, ":reply_to": reply_to, ":subject": m.subject, ":snippet": m.snippet,
        ":msgid": m.message_id_header, ":in_reply_to": m.in_reply_to,
        ":references": m.references, ":is_list": m.is_list, ":is_draft": m.is_draft,
        ":is_live": is_live, ":sm_draft": m.sm_draft, ":labels": labels,
        ":attachments": m.has_attachments, ":rev": rev,
    };
    let (id, is_new) = if let Some((id, old_thread)) = existing {
        db.execute(
            "update a stored message",
            "UPDATE messages SET thread_id = :thread, internal_date = :date,
                 from_name = :from_name, from_email = :from_email, to_addrs = :to,
                 cc_addrs = :cc, bcc_addrs = :bcc, reply_to = :reply_to, subject = :subject,
                 snippet = :snippet, message_id_hdr = :msgid, in_reply_to = :in_reply_to,
                 references_hdr = :references, is_list = :is_list, is_draft = :is_draft,
                 is_live = :is_live, sm_draft_id = :sm_draft, label_ids = :labels,
                 has_attachments = :attachments, rev = :rev
             WHERE account_id = :account AND gmail_id = :gmail",
            params,
        )?;
        touched.insert(old_thread);
        (id, false)
    } else {
        let id = db.insert(
            "add a message",
            "INSERT INTO messages (account_id, thread_id, gmail_id, internal_date, from_name,
                 from_email, to_addrs, cc_addrs, bcc_addrs, reply_to, subject, snippet,
                 message_id_hdr, in_reply_to, references_hdr, is_list, is_draft, is_live,
                 sm_draft_id, label_ids, has_attachments, rev)
             VALUES (:account, :thread, :gmail, :date, :from_name, :from_email, :to, :cc,
                 :bcc, :reply_to, :subject, :snippet, :msgid, :in_reply_to, :references,
                 :is_list, :is_draft, :is_live, :sm_draft, :labels, :attachments, :rev)",
            params,
        )?;
        (MessageId(id), true)
    };
    reindex(db, id)?;
    touched.insert(thread);
    tx.record().message(id);
    Ok((id, is_new))
}

/// Writes one message's labels and returns its thread.
fn write_labels(
    tx: &mut WriteTx<'_>,
    id: MessageId,
    labels: &LabelSet,
) -> Result<ThreadId, StoreError> {
    let rev = tx.stamp();
    let db = tx.db();
    let thread: ThreadId = require(
        db.optional(
            "find a message's thread",
            "SELECT thread_id FROM messages WHERE id = ?1",
            [id],
            |r| r.get("thread_id"),
        )?,
        "message",
        &id,
    )?;
    db.execute(
        "store a message's labels",
        "UPDATE messages
         SET label_ids = :labels,
             is_live = (is_draft = 0 AND NOT EXISTS (SELECT 1 FROM json_each(:labels)
                                                     WHERE value IN ('TRASH', 'SPAM'))),
             rev = :rev
         WHERE id = :id",
        named_params! { ":id": id, ":labels": to_json("label_ids", labels)?, ":rev": rev },
    )?;
    tx.record().message(id);
    Ok(thread)
}

/// Deletes one message row and its index entry; returns its thread, or
/// `None` when it was not stored.
pub(crate) fn delete_row(
    tx: &mut WriteTx<'_>,
    id: MessageId,
) -> Result<Option<ThreadId>, StoreError> {
    let rev = tx.stamp();
    let db = tx.db();
    let Some((account, thread)): Option<(AccountId, ThreadId)> = db.optional(
        "find a message's thread",
        "SELECT account_id, thread_id FROM messages WHERE id = ?1",
        [id],
        |r| Ok((r.get("account_id")?, r.get("thread_id")?)),
    )?
    else {
        return Ok(None);
    };
    let aimed: Vec<DraftId> = db.all(
        "find drafts aimed at a message",
        "SELECT id FROM drafts WHERE target_message_id = ?1",
        [id],
        |r| r.get("id"),
    )?;
    db.execute(
        "unaim drafts from a message",
        "UPDATE drafts SET target_message_id = NULL, rev = ?2 WHERE target_message_id = ?1",
        (id, rev),
    )?;
    db.execute(
        "drop a message's index entry",
        "DELETE FROM messages_fts WHERE rowid = ?1",
        [id],
    )?;
    db.execute(
        "delete a message",
        "DELETE FROM messages WHERE id = ?1",
        [id],
    )?;
    let changes = tx.record();
    changes.message(id);
    for draft in aimed {
        changes.draft(account, draft);
    }
    Ok(Some(thread))
}

/// Rebuilds a message's search index entry from its row.
fn reindex(db: Db<'_>, id: MessageId) -> Result<(), StoreError> {
    db.execute(
        "drop a message's index entry",
        "DELETE FROM messages_fts WHERE rowid = ?1",
        [id],
    )?;
    db.execute(
        "index a message",
        "INSERT INTO messages_fts (rowid, from_text, to_text, cc_text, subject, body)
         SELECT m.id,
                coalesce(m.from_name || ' ', '') || m.from_email,
                (SELECT coalesce(group_concat(coalesce(json_extract(a.value, '$.name') || ' ', '')
                                              || json_extract(a.value, '$.email'), ' '), '')
                 FROM json_each(m.to_addrs) a),
                (SELECT coalesce(group_concat(coalesce(json_extract(a.value, '$.name') || ' ', '')
                                              || json_extract(a.value, '$.email'), ' '), '')
                 FROM json_each(m.cc_addrs) a),
                m.subject,
                coalesce(m.body_text, '')
         FROM messages m WHERE m.id = ?1",
        [id],
    )?;
    Ok(())
}

/// Re-derives each thread once.
pub(crate) fn derive_all(
    tx: &mut WriteTx<'_>,
    threads: BTreeSet<ThreadId>,
) -> Result<(), StoreError> {
    for thread in threads {
        routing::derive_thread(tx, thread)?;
    }
    Ok(())
}
