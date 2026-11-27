use std::fs;
use std::path::{Path, PathBuf};

use rusqlite::{Connection, OpenFlags, params};
use serde_json::{Value, json};

use crate::domain::{IMPORTANT_POSITION, OTHER_POSITION};

use super::sql::to_json;
use super::{BUSY_TIMEOUT, Db, SCHEMA_VERSION, StoreError, move_aside};

pub const LEGACY_DB_FILE: &str = "supermonkey.db";

const NEW_INDEXES: &str = "
CREATE INDEX IF NOT EXISTS split_overrides_split ON split_overrides (split_id);
CREATE INDEX IF NOT EXISTS threads_split ON threads (split_id) WHERE split_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS drafts_target ON drafts (target_message_id) WHERE target_message_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS ops_draft ON ops (kind, draft_id) WHERE draft_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS undo_outbox ON undo_entries (outbox_id) WHERE outbox_id IS NOT NULL;
";

pub fn import_legacy(legacy: &Path, target: &Path) -> Result<(), StoreError> {
    let staging = staging_path(target);
    remove_staging(&staging);
    if let Err(err) = copy_legacy(legacy, &staging).and_then(|()| convert_copy(&staging)) {
        remove_staging(&staging);
        return Err(err);
    }
    fs::rename(&staging, target).map_err(|source| StoreError::Io {
        action: "put the imported database in place at",
        path: target.to_path_buf(),
        source,
    })?;
    move_aside(legacy)
}

fn staging_path(target: &Path) -> PathBuf {
    let mut name = target.as_os_str().to_owned();
    name.push(".importing");
    PathBuf::from(name)
}

fn remove_staging(staging: &Path) {
    for suffix in ["", "-journal", "-wal", "-shm"] {
        let mut name = staging.as_os_str().to_owned();
        name.push(suffix);
        let path = PathBuf::from(name);
        if path.exists()
            && let Err(err) = fs::remove_file(&path)
        {
            tracing::warn!(error = %err, "failed to remove a half-imported database file");
        }
    }
}

fn copy_legacy(legacy: &Path, staging: &Path) -> Result<(), StoreError> {
    let flags = OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    let source = Connection::open_with_flags(legacy, flags)
        .map_err(StoreError::sql("open the earlier database"))?;
    source
        .busy_timeout(BUSY_TIMEOUT)
        .map_err(StoreError::sql("set earlier database busy timeout"))?;
    let version: i32 = source
        .pragma_query_value(None, "user_version", |r| r.get(0))
        .map_err(StoreError::sql("read the earlier database version"))?;
    if version != SCHEMA_VERSION {
        return Err(StoreError::Invalid(
            "the earlier database has a schema this version cannot import",
        ));
    }
    let staging_text = staging.to_string_lossy();
    source
        .execute("VACUUM INTO ?1", [staging_text.as_ref()])
        .map_err(StoreError::sql("copy the earlier database"))?;
    Ok(())
}

fn convert_copy(staging: &Path) -> Result<(), StoreError> {
    let flags = OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    let mut conn = Connection::open_with_flags(staging, flags)
        .map_err(StoreError::sql("open the copied database"))?;
    conn.execute_batch("PRAGMA journal_mode = DELETE; PRAGMA foreign_keys = ON;")
        .map_err(StoreError::sql("configure the copied database"))?;
    let tx = conn
        .transaction()
        .map_err(StoreError::sql("begin the import"))?;
    let db = Db(&tx);
    convert_accounts(db)?;
    convert_message_headers(db)?;
    convert_drafts(db)?;
    drop_draft_messages(db)?;
    convert_ops(db)?;
    convert_undo_entries(db)?;
    convert_splits(db)?;
    convert_label_pins(db)?;
    rebuild_search_index(db)?;
    tx.execute_batch(NEW_INDEXES)
        .map_err(StoreError::sql("add the indexes this version uses"))?;
    tx.commit().map_err(StoreError::sql("commit the import"))
}

fn convert_accounts(db: Db<'_>) -> Result<(), StoreError> {
    let rows = db.all(
        "SELECT id, send_as FROM accounts",
        [],
        "read earlier accounts",
        |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)),
    )?;
    let empty = Value::String(String::new());
    for (id, raw) in rows {
        let old: Vec<Value> = serde_json::from_str(&raw)?;
        let new: Vec<Value> = old
            .iter()
            .map(|addr| {
                json!({
                    "email": addr.get("email").unwrap_or(&empty),
                    "displayName": addr
                        .get("displayName")
                        .or_else(|| addr.get("name"))
                        .unwrap_or(&empty),
                    "primary": addr
                        .get("primary")
                        .or_else(|| addr.get("is_primary"))
                        .and_then(Value::as_bool)
                        .unwrap_or(false),
                })
            })
            .collect();
        db.execute(
            "UPDATE accounts SET send_as = ?2 WHERE id = ?1",
            params![id, to_json(&new)?],
            "write account addresses",
        )?;
    }
    db.execute(
        "UPDATE accounts SET sync_status = 'needs_signin' WHERE needs_signin = 1",
        [],
        "write account sign-in status",
    )?;
    Ok(())
}

fn bracketed(id: &str) -> String {
    let inner = id.trim().trim_start_matches('<').trim_end_matches('>');
    format!("<{inner}>")
}

fn references_json(plain: &str) -> Result<String, StoreError> {
    let ids: Vec<String> = plain.split_whitespace().map(bracketed).collect();
    to_json(&ids)
}

fn convert_reference_column(db: Db<'_>, table: &'static str) -> Result<(), StoreError> {
    let (select, update) = match table {
        "messages" => (
            "SELECT id, references_hdr FROM messages WHERE references_hdr NOT LIKE '[%'",
            "UPDATE messages SET references_hdr = ?2 WHERE id = ?1",
        ),
        _ => (
            "SELECT rowid, references_hdr FROM drafts WHERE references_hdr NOT LIKE '[%'",
            "UPDATE drafts SET references_hdr = ?2 WHERE rowid = ?1",
        ),
    };
    let rows = db.all(select, [], "read earlier references", |r| {
        Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
    })?;
    for (id, plain) in rows {
        db.execute(
            update,
            params![id, references_json(&plain)?],
            "write references",
        )?;
    }
    Ok(())
}

fn bracket_id_column(
    db: Db<'_>,
    table: &'static str,
    column: &'static str,
) -> Result<(), StoreError> {
    let blank = format!(
        "UPDATE {table} SET {column} = NULL WHERE {column} IS NOT NULL AND trim({column}) = ''"
    );
    db.execute(&blank, [], "clear blank message ids")?;
    let wrap = format!(
        "UPDATE {table} SET {column} = '<' || trim({column}) || '>' WHERE {column} IS NOT NULL AND trim({column}) NOT LIKE '<%'"
    );
    db.execute(&wrap, [], "bracket message ids")?;
    Ok(())
}

fn convert_message_headers(db: Db<'_>) -> Result<(), StoreError> {
    convert_reference_column(db, "messages")?;
    bracket_id_column(db, "messages", "message_id_hdr")?;
    bracket_id_column(db, "messages", "in_reply_to")
}

fn convert_draft_attachment(db: Db<'_>, item: Value) -> Result<Value, StoreError> {
    let Value::Object(mut map) = item else {
        return Ok(item);
    };
    if let Some(mime) = map.remove("mime_type") {
        map.insert("mimeType".to_owned(), mime);
    }
    if let Some(source) = map.get("source").and_then(Value::as_i64) {
        let of_draft_copy = db.optional(
            "SELECT a.gmail_att_id FROM attachments a JOIN messages m ON m.id = a.message_id WHERE a.id = ?1 AND m.is_draft = 1",
            [source],
            "read a draft copy's attachment",
            |r| r.get::<_, Option<String>>(0),
        )?;
        if let Some(gmail_att_id) = of_draft_copy {
            map.insert("source".to_owned(), Value::Null);
            map.insert("gmailAttId".to_owned(), json!(gmail_att_id));
        }
    }
    Ok(Value::Object(map))
}

fn convert_drafts(db: Db<'_>) -> Result<(), StoreError> {
    convert_reference_column(db, "drafts")?;
    bracket_id_column(db, "drafts", "in_reply_to")?;
    let rows = db.all(
        "SELECT rowid, attachments FROM drafts WHERE attachments != '[]'",
        [],
        "read earlier draft attachments",
        |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)),
    )?;
    for (rowid, raw) in rows {
        let old: Vec<Value> = serde_json::from_str(&raw)?;
        let mut new = Vec::with_capacity(old.len());
        for item in old {
            new.push(convert_draft_attachment(db, item)?);
        }
        db.execute(
            "UPDATE drafts SET attachments = ?2 WHERE rowid = ?1",
            params![rowid, to_json(&new)?],
            "write draft attachments",
        )?;
    }
    db.execute(
        "UPDATE drafts SET waits_for = NULL WHERE waits_for = 'parts'",
        [],
        "clear the wait for attachment parts",
    )?;
    db.execute(
        "UPDATE drafts SET waits_for = NULL WHERE waits_for = 'target_body' AND (target_message_id IS NULL OR EXISTS (SELECT 1 FROM messages m WHERE m.id = drafts.target_message_id AND m.has_body = 1))",
        [],
        "clear the wait for a body already here",
    )?;
    db.execute(
        "UPDATE drafts SET gmail_thread_id = (SELECT t.gmail_id FROM threads t WHERE t.id = drafts.thread_id) WHERE gmail_thread_id IS NULL AND thread_id IS NOT NULL",
        [],
        "write draft Gmail thread ids",
    )?;
    Ok(())
}

fn drop_draft_messages(db: Db<'_>) -> Result<(), StoreError> {
    db.execute(
        "DELETE FROM messages WHERE is_draft = 1",
        [],
        "drop stored draft copies",
    )?;
    db.execute(
        "DELETE FROM threads WHERE NOT EXISTS (SELECT 1 FROM messages m WHERE m.thread_id = threads.id)",
        [],
        "drop threads that held only draft copies",
    )?;
    Ok(())
}

fn convert_thread_change(
    db: Db<'_>,
    old: &Value,
) -> Result<Option<(Value, Vec<String>)>, StoreError> {
    let Some(thread) = old.get("thread").and_then(Value::as_i64) else {
        return Ok(None);
    };
    let empty_arr = Value::Array(Vec::new());
    let changes = old
        .get("messages")
        .or_else(|| old.get("changes"))
        .and_then(Value::as_array)
        .map_or(&[][..], Vec::as_slice);
    let mut messages = Vec::with_capacity(changes.len());
    let mut gmail_ids = Vec::with_capacity(changes.len());
    for change in changes {
        let Some(message) = change.get("message").and_then(Value::as_i64) else {
            continue;
        };
        let known = change
            .get("gmail_id")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let gmail_id = match known {
            Some(id) => Some(id),
            None => db.optional(
                "SELECT gmail_id FROM messages WHERE id = ?1",
                [message],
                "read a changed message's Gmail id",
                |r| r.get::<_, String>(0),
            )?,
        };
        let Some(gmail_id) = gmail_id else {
            continue;
        };
        messages.push(json!({
            "message": message,
            "gmail_id": gmail_id,
            "added": change.get("added").unwrap_or(&empty_arr),
            "removed": change.get("removed").unwrap_or(&empty_arr),
        }));
        gmail_ids.push(gmail_id);
    }
    let change = json!({
        "thread": thread,
        "messages": messages,
        "snooze_before": old.get("snooze_before"),
        "snooze_after": old.get("snooze_after"),
    });
    Ok(Some((change, gmail_ids)))
}

struct LegacyOp {
    account: i64,
    action: Option<i64>,
    kind: String,
    draft_id: Option<String>,
    gmail_draft_id: Option<String>,
    payload: String,
}

fn convert_modify_op(
    db: Db<'_>,
    op: &LegacyOp,
    payload: &Value,
) -> Result<Vec<LegacyOp>, StoreError> {
    let empty_arr = Value::Array(Vec::new());
    let add = payload.get("add").unwrap_or(&empty_arr);
    let remove = payload.get("remove").unwrap_or(&empty_arr);
    let mut unplaced: Vec<String> = payload
        .get("messages")
        .and_then(Value::as_array)
        .map(|ids| {
            ids.iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    let threads = payload
        .get("threads")
        .and_then(Value::as_array)
        .map_or(&[][..], Vec::as_slice);
    let mut parts = Vec::with_capacity(threads.len());
    for old in threads {
        if let Some((change, gmail_ids)) = convert_thread_change(db, old)? {
            unplaced.retain(|id| !gmail_ids.contains(id));
            parts.push((change, gmail_ids));
        }
    }
    if let Some((_, first_ids)) = parts.first_mut() {
        first_ids.append(&mut unplaced);
    }
    let mut out = Vec::with_capacity(parts.len());
    for (change, gmail_ids) in parts {
        let new = json!({
            "thread": change.get("thread"),
            "gmailMsgIds": gmail_ids,
            "add": add,
            "remove": remove,
            "revert": change,
        });
        out.push(LegacyOp {
            account: op.account,
            action: op.action,
            kind: "modify".to_owned(),
            draft_id: None,
            gmail_draft_id: None,
            payload: to_json(&new)?,
        });
    }
    Ok(out)
}

fn convert_op(db: Db<'_>, op: LegacyOp) -> Result<Vec<LegacyOp>, StoreError> {
    let payload: Value = serde_json::from_str(&op.payload)?;
    let draft_payload = |op: &LegacyOp| {
        to_json(&json!({
            "draftId": op.draft_id.as_deref().unwrap_or(""),
            "gmailDraftId": op.gmail_draft_id,
        }))
    };
    match op.kind.as_str() {
        "modify" if payload.get("gmailMsgIds").is_some() => Ok(vec![op]),
        "modify" => convert_modify_op(db, &op, &payload),
        "label_create" | "create_label" => {
            let new = json!({
                "localId": payload.get("localId").or_else(|| payload.get("label")),
                "name": payload.get("name"),
            });
            Ok(vec![LegacyOp {
                kind: "create_label".to_owned(),
                payload: to_json(&new)?,
                ..op
            }])
        }
        "draft_upsert" if op.draft_id.is_none() => Ok(Vec::new()),
        "draft_upsert" | "draft_delete" => Ok(vec![LegacyOp {
            payload: draft_payload(&op)?,
            ..op
        }]),
        "trash_message" => {
            let gmail_msg_id = payload
                .as_str()
                .or_else(|| payload.get("gmailMsgId").and_then(Value::as_str));
            match gmail_msg_id {
                Some(id) => Ok(vec![LegacyOp {
                    payload: to_json(&json!({ "gmailMsgId": id }))?,
                    ..op
                }]),
                None => Ok(Vec::new()),
            }
        }
        _ => Ok(Vec::new()),
    }
}

fn convert_ops(db: Db<'_>) -> Result<(), StoreError> {
    let old = db.all(
        "SELECT account_id, action, kind, draft_id, gmail_draft_id, payload FROM ops ORDER BY id",
        [],
        "read earlier queued changes",
        |r| {
            Ok(LegacyOp {
                account: r.get(0)?,
                action: r.get(1)?,
                kind: r.get(2)?,
                draft_id: r.get(3)?,
                gmail_draft_id: r.get(4)?,
                payload: r.get(5)?,
            })
        },
    )?;
    db.execute("DELETE FROM ops", [], "clear earlier queued changes")?;
    for op in old {
        for new in convert_op(db, op)? {
            db.execute(
                "INSERT INTO ops (account_id, action, kind, draft_id, gmail_draft_id, payload) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    new.account,
                    new.action,
                    new.kind,
                    new.draft_id,
                    new.gmail_draft_id,
                    new.payload
                ],
                "write a queued change",
            )?;
        }
    }
    Ok(())
}

fn convert_undo_entry(db: Db<'_>, payload: Value) -> Result<Option<Value>, StoreError> {
    match payload.get("kind").and_then(Value::as_str) {
        Some("send") => Ok(Some(payload)),
        Some("mail") => {
            let old = payload
                .get("threads")
                .and_then(Value::as_array)
                .map_or(&[][..], Vec::as_slice);
            let mut threads = Vec::with_capacity(old.len());
            for change in old {
                if let Some((new, _)) = convert_thread_change(db, change)? {
                    threads.push(new);
                }
            }
            Ok(Some(json!({ "kind": "mail", "threads": threads })))
        }
        Some("pin") => Ok(Some(json!({
            "kind": "pin",
            "pin_kind": payload.get("pin_kind"),
            "value": payload.get("value"),
            "previous": payload.get("previous").or_else(|| payload.get("replaced")),
        }))),
        _ => Ok(None),
    }
}

fn convert_undo_entries(db: Db<'_>) -> Result<(), StoreError> {
    let rows = db.all(
        "SELECT id, payload FROM undo_entries",
        [],
        "read earlier undo entries",
        |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)),
    )?;
    for (id, raw) in rows {
        let converted = match serde_json::from_str::<Value>(&raw) {
            Ok(payload) => convert_undo_entry(db, payload)?,
            Err(_) => None,
        };
        match converted {
            Some(new) => db.execute(
                "UPDATE undo_entries SET payload = ?2 WHERE id = ?1",
                params![id, to_json(&new)?],
                "write an undo entry",
            )?,
            None => db.execute(
                "DELETE FROM undo_entries WHERE id = ?1",
                [id],
                "drop an undo entry this version cannot read",
            )?,
        };
    }
    Ok(())
}

fn convert_splits(db: Db<'_>) -> Result<(), StoreError> {
    db.execute(
        "UPDATE splits SET position = ?1 WHERE kind = 'important'",
        [IMPORTANT_POSITION],
        "place the Important split",
    )?;
    db.execute(
        "UPDATE splits SET position = ?1 WHERE kind = 'other'",
        [OTHER_POSITION],
        "place the Other split",
    )?;
    let custom = db.all(
        "SELECT id, account_id FROM splits WHERE kind = 'custom' ORDER BY account_id, position, id",
        [],
        "read earlier custom splits",
        |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)),
    )?;
    let mut account = None;
    let mut index = 0i64;
    for (id, owner) in custom {
        if account != Some(owner) {
            account = Some(owner);
            index = 0;
        }
        db.execute(
            "UPDATE splits SET position = ?2 WHERE id = ?1",
            params![id, index],
            "place a custom split",
        )?;
        index = index.saturating_add(1);
    }
    Ok(())
}

fn convert_label_pins(db: Db<'_>) -> Result<(), StoreError> {
    let pins = db.all(
        "SELECT account_id, value FROM split_overrides WHERE kind = 'label'",
        [],
        "read earlier label pins",
        |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)),
    )?;
    for (account, value) in pins {
        let names = db.all(
            "SELECT name FROM labels WHERE account_id = ?1 AND system = 0",
            [account],
            "read label names",
            |r| r.get::<_, String>(0),
        )?;
        let Some(name) = names
            .iter()
            .map(|n| n.trim())
            .find(|n| n.to_lowercase() == value && *n != value)
        else {
            continue;
        };
        db.execute(
            "UPDATE OR IGNORE split_overrides SET value = ?3 WHERE account_id = ?1 AND kind = 'label' AND value = ?2",
            params![account, value, name],
            "write a label pin",
        )?;
    }
    Ok(())
}

fn rebuild_search_index(db: Db<'_>) -> Result<(), StoreError> {
    db.execute(
        "INSERT INTO messages_fts (messages_fts) VALUES ('delete-all')",
        [],
        "clear the search index",
    )?;
    db.execute(
        "INSERT INTO messages_fts (rowid, from_text, to_text, cc_text, subject, body)
         SELECT m.id,
                CASE WHEN coalesce(m.from_name, '') = '' THEN m.from_email ELSE m.from_name || ' ' || m.from_email END,
                coalesce((SELECT group_concat(CASE WHEN coalesce(json_extract(a.value, '$.name'), '') = '' THEN json_extract(a.value, '$.email') ELSE json_extract(a.value, '$.name') || ' ' || json_extract(a.value, '$.email') END, ' ') FROM json_each(m.to_addrs) a), ''),
                coalesce((SELECT group_concat(CASE WHEN coalesce(json_extract(a.value, '$.name'), '') = '' THEN json_extract(a.value, '$.email') ELSE json_extract(a.value, '$.name') || ' ' || json_extract(a.value, '$.email') END, ' ') FROM json_each(m.cc_addrs) a), ''),
                m.subject,
                coalesce(m.body_text, m.snippet)
         FROM messages m",
        [],
        "rebuild the search index",
    )?;
    Ok(())
}
