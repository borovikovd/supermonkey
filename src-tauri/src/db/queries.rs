//! Read queries shared by the writer (for first loads and patches) and the read
//! pool (for window fetches and bodies). They take `&Connection`, so a write
//! transaction can call them on its own snapshot.

use std::collections::{HashMap, HashSet};

use rusqlite::{Connection, OptionalExtension, ToSql, params};
use serde::Deserialize;

use super::from_json;
use crate::model::{
    AccountId, AccountInfo, AccountSummary, Addr, AppState, AttachmentInfo, ConversationData,
    DraftAttachment, DraftId, DraftKind, DraftState, DraftSummary, GlobalSettings, LabelInfo,
    ListEntry, ListRow, MessageBody, MessageId, MessageSummary, Millis, Notice, NoticeKind,
    OutboxId, OutboxRow, OutboxState, PinInfo, PinKind, SendAs, SplitInfo, SplitKind, SyncStatus,
    ThreadId, UndoEntryInfo, UndoKind, View,
};

pub const HUES: [&str; 6] = [
    "#714CB6", "#D6478A", "#DB7A2B", "#2E9C6A", "#2A8CC4", "#B08A2E",
];

/// The view list's ids in order, each with its split.
pub fn list_ids(
    conn: &Connection,
    account: AccountId,
    view: &View,
) -> anyhow::Result<Vec<ListEntry>> {
    let (sql, label): (&str, Option<&str>) = match view {
        View::Drafts => return draft_list_ids(conn, account),
        View::Inbox => (
            "SELECT t.id, t.split_id FROM threads t LEFT JOIN splits s ON s.id = t.split_id
             WHERE t.account_id = ?1 AND t.in_inbox = 1 AND t.snooze_until IS NULL
             ORDER BY s.position, t.sort_time DESC, t.id DESC",
            None,
        ),
        View::Done => (
            "SELECT t.id, NULL FROM threads t
             WHERE t.account_id = ?1 AND t.in_inbox = 0
               AND t.snooze_until IS NULL AND t.has_live = 1
             ORDER BY t.sort_time DESC, t.id DESC",
            None,
        ),
        View::Snoozed => (
            "SELECT t.id, NULL FROM threads t
             WHERE t.account_id = ?1 AND t.snooze_until IS NOT NULL
             ORDER BY t.snooze_until, t.id",
            None,
        ),
        View::Trash => (
            "SELECT t.id, NULL FROM threads t WHERE t.account_id = ?1 AND t.in_trash = 1
             ORDER BY t.sort_time DESC, t.id DESC",
            None,
        ),
        View::Spam => (
            "SELECT t.id, NULL FROM threads t WHERE t.account_id = ?1 AND t.in_spam = 1
             ORDER BY t.sort_time DESC, t.id DESC",
            None,
        ),
        View::Starred => (LABEL_SQL, Some("STARRED")),
        View::Sent => (LABEL_SQL, Some("SENT")),
        View::Label { label } => (LABEL_SQL, Some(label.as_str())),
    };
    let mut stmt = conn.prepare_cached(sql)?;
    let map = |r: &rusqlite::Row<'_>| {
        Ok(ListEntry {
            key: r.get::<_, ThreadId>(0)?.to_string(),
            split: r.get(1)?,
        })
    };
    let rows = match label {
        Some(l) => stmt.query_map(params![account, l], map)?,
        None => stmt.query_map([account], map)?,
    };
    Ok(rows.collect::<Result<_, _>>()?)
}

/// The Drafts list: drafts not yet handed to the outbox, newest first.
fn draft_list_ids(conn: &Connection, account: AccountId) -> anyhow::Result<Vec<ListEntry>> {
    let mut stmt = conn.prepare_cached(
        "SELECT d.id FROM drafts d
         WHERE d.account_id = ?1 AND NOT EXISTS (SELECT 1 FROM outbox o WHERE o.draft_id = d.id)
         ORDER BY d.updated_at DESC, d.id",
    )?;
    let rows = stmt.query_map([account], |r| {
        Ok(ListEntry {
            key: r.get(0)?,
            split: None,
        })
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}

const LABEL_SQL: &str =
    "SELECT t.id, NULL FROM thread_labels tl JOIN threads t ON t.id = tl.thread_id
     WHERE tl.label_id = ?2 AND t.account_id = ?1
     ORDER BY t.sort_time DESC, t.id DESC";

const THREAD_ROW_COLS: &str =
    "t.id, t.sender_name, t.sender_email, t.subject, t.snippet, t.sort_time,
     t.unread, t.starred, t.has_attachments, t.message_count, t.snooze_until, t.split_id, t.rev,
     (SELECT group_concat(label_id, char(31)) FROM thread_labels WHERE thread_id = t.id)";

fn thread_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<ListRow> {
    let id: ThreadId = r.get(0)?;
    let labels: Option<String> = r.get(13)?;
    Ok(ListRow {
        key: id.to_string(),
        thread: Some(id),
        draft: None,
        sender: r.get(1)?,
        sender_email: r.get(2)?,
        subject: r.get(3)?,
        snippet: r.get(4)?,
        time: r.get(5)?,
        unread: r.get(6)?,
        starred: r.get(7)?,
        has_attachments: r.get(8)?,
        message_count: r.get(9)?,
        labels: labels
            .map(|s| s.split('\u{1f}').map(str::to_string).collect())
            .unwrap_or_default(),
        snooze_until: r.get(10)?,
        split: r.get(11)?,
        rev: r.get(12)?,
    })
}

/// Rows for thread ids, in no particular order. Unknown ids are skipped.
pub fn thread_rows(conn: &Connection, ids: &[ThreadId]) -> anyhow::Result<Vec<ListRow>> {
    let mut out = Vec::with_capacity(ids.len());
    for chunk in ids.chunks(400) {
        let marks = vec!["?"; chunk.len()].join(",");
        let sql = format!("SELECT {THREAD_ROW_COLS} FROM threads t WHERE t.id IN ({marks})");
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(chunk.iter()), thread_row)?;
        for row in rows {
            out.push(row?);
        }
    }
    Ok(out)
}

/// Rows for draft ids (the Drafts list).
pub fn draft_rows(conn: &Connection, ids: &[impl AsRef<str>]) -> anyhow::Result<Vec<ListRow>> {
    let mut out = Vec::with_capacity(ids.len());
    let mut stmt = conn.prepare_cached(
        "SELECT d.id, d.thread_id, d.to_json, d.subject, d.body_html, d.updated_at, d.rev, d.attachments_json
         FROM drafts d WHERE d.id = ?1",
    )?;
    for id in ids {
        let row = stmt
            .query_row([id.as_ref()], |r| {
                let to: Vec<Addr> = from_json(&r.get::<_, String>(2)?);
                let body: String = r.get(4)?;
                let atts: Vec<DraftAttachment> = from_json(&r.get::<_, String>(7)?);
                let sender = if to.is_empty() {
                    "(no recipients)".to_string()
                } else {
                    to.iter()
                        .map(|a| {
                            a.name
                                .as_deref()
                                .filter(|n| !n.is_empty())
                                .unwrap_or(&a.email)
                        })
                        .collect::<Vec<_>>()
                        .join(", ")
                };
                Ok(ListRow {
                    key: r.get(0)?,
                    thread: r.get(1)?,
                    draft: Some(r.get(0)?),
                    sender,
                    sender_email: to.first().map(|a| a.email.clone()).unwrap_or_default(),
                    subject: r.get(3)?,
                    snippet: snippet_of_html(&body),
                    time: r.get(5)?,
                    unread: false,
                    starred: false,
                    has_attachments: !atts.is_empty(),
                    message_count: 1,
                    labels: vec!["DRAFT".into()],
                    snooze_until: None,
                    split: None,
                    rev: r.get(6)?,
                })
            })
            .optional()?;
        if let Some(row) = row {
            out.push(row);
        }
    }
    Ok(out)
}

pub fn snippet_of_html(html: &str) -> String {
    let text = ub_mime::html_to_text(html);
    let mut s: String = text.chars().take(200).collect();
    if s.len() < text.len() {
        s.push('…');
    }
    s
}

/// Rows for list keys of any view.
pub fn rows_for_keys(
    conn: &Connection,
    view_is_drafts: bool,
    keys: &[impl AsRef<str>],
) -> anyhow::Result<Vec<ListRow>> {
    if view_is_drafts {
        draft_rows(conn, keys)
    } else {
        let ids: Vec<ThreadId> = keys
            .iter()
            .filter_map(|k| k.as_ref().parse().ok())
            .collect();
        thread_rows(conn, &ids)
    }
}

/// Every address this account sends as, lowercased.
pub fn my_addresses(conn: &Connection, account: AccountId) -> anyhow::Result<HashSet<String>> {
    let (email, send_as): (String, String) = conn.query_row(
        "SELECT email, send_as FROM accounts WHERE id = ?1",
        [account],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    let mut set: HashSet<String> = from_json::<Vec<SendAs>>(&send_as)
        .into_iter()
        .map(|s| s.email.to_lowercase())
        .collect();
    set.insert(email.to_lowercase());
    Ok(set)
}

fn attachments_of(conn: &Connection, message: MessageId) -> anyhow::Result<Vec<AttachmentInfo>> {
    let mut stmt = conn.prepare_cached(
        "SELECT id, filename, mime_type, size, inline, local_path IS NOT NULL FROM attachments
         WHERE message_id = ?1 ORDER BY id",
    )?;
    let rows = stmt.query_map([message], |r| {
        Ok(AttachmentInfo {
            id: r.get(0)?,
            filename: r.get(1)?,
            mime_type: r.get(2)?,
            size: r.get(3)?,
            inline: r.get(4)?,
            downloaded: r.get(5)?,
        })
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// A thread's non-draft messages, oldest first. Outside trash mode, messages
/// in trash or spam are left out (SPEC Reading).
fn conversation_messages(
    conn: &Connection,
    thread: ThreadId,
    trash_mode: bool,
    mine: &HashSet<String>,
) -> anyhow::Result<Vec<MessageSummary>> {
    let mut stmt = conn.prepare_cached(
        "SELECT id, from_name, from_email, to_json, cc_json, bcc_json, subject, snippet, internal_date,
                label_ids, has_body, rev, is_plain
         FROM messages WHERE thread_id = ?1 AND is_draft = 0 ORDER BY internal_date, id",
    )?;
    let rows = stmt.query_map([thread], |r| {
        let from_name: String = r.get(1)?;
        let from_email: String = r.get(2)?;
        let labels: Vec<String> = from_json(&r.get::<_, String>(9)?);
        let has = |l: &str| labels.iter().any(|x| x == l);
        Ok(MessageSummary {
            id: r.get(0)?,
            from_me: mine.contains(&from_email.to_lowercase()),
            from: Addr {
                name: (!from_name.is_empty()).then_some(from_name),
                email: from_email,
            },
            to: from_json(&r.get::<_, String>(3)?),
            cc: from_json(&r.get::<_, String>(4)?),
            bcc: from_json(&r.get::<_, String>(5)?),
            subject: r.get(6)?,
            snippet: r.get(7)?,
            date: r.get(8)?,
            unread: has("UNREAD"),
            starred: has("STARRED"),
            in_trash: has("TRASH"),
            in_spam: has("SPAM"),
            has_body: r.get(10)?,
            plain: r.get(12)?,
            attachments: Vec::new(),
            rev: r.get(11)?,
        })
    })?;
    let mut out = Vec::new();
    for row in rows {
        let mut m = row?;
        if !trash_mode && (m.in_trash || m.in_spam) {
            continue;
        }
        m.attachments = attachments_of(conn, m.id)?;
        out.push(m);
    }
    Ok(out)
}

pub fn conversation(
    conn: &Connection,
    thread: ThreadId,
    trash_mode: bool,
) -> anyhow::Result<ConversationData> {
    let head = conn
        .query_row(
            "SELECT account_id, subject, snooze_until, rev FROM threads WHERE id = ?1",
            [thread],
            |r| {
                Ok((
                    r.get::<_, AccountId>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, Option<Millis>>(2)?,
                    r.get::<_, i64>(3)?,
                ))
            },
        )
        .optional()?;
    let Some((account, subject, snooze_until, rev)) = head else {
        return Ok(ConversationData {
            thread,
            account: AccountId::NONE,
            subject: String::new(),
            labels: vec![],
            snooze_until: None,
            messages: vec![],
            drafts: vec![],
            outbox: vec![],
            gone: true,
            rev: 0,
        });
    };
    let mine = my_addresses(conn, account)?;
    let labels: Vec<String> = {
        let mut stmt =
            conn.prepare_cached("SELECT label_id FROM thread_labels WHERE thread_id = ?1")?;
        stmt.query_map([thread], |r| r.get(0))?
            .collect::<Result<_, _>>()?
    };
    let messages = conversation_messages(conn, thread, trash_mode, &mine)?;
    let drafts = thread_drafts(conn, thread)?;
    let outbox = outbox_rows(conn, OutboxFilter::Thread(thread))?;
    Ok(ConversationData {
        thread,
        account,
        subject,
        labels,
        snooze_until,
        messages,
        drafts,
        outbox,
        gone: false,
        rev,
    })
}

fn thread_drafts(conn: &Connection, thread: ThreadId) -> anyhow::Result<Vec<DraftSummary>> {
    let mut stmt = conn.prepare_cached(
        "SELECT d.id, d.kind, d.target_message_id, d.started_on_latest, d.to_json, d.body_html, d.updated_at
         FROM drafts d WHERE d.thread_id = ?1 AND d.kind != 'new'
           AND NOT EXISTS (SELECT 1 FROM outbox o WHERE o.draft_id = d.id)
         ORDER BY d.updated_at",
    )?;
    let rows = stmt.query_map([thread], |r| {
        let body: String = r.get(5)?;
        Ok(DraftSummary {
            id: r.get(0)?,
            kind: DraftKind::parse(&r.get::<_, String>(1)?),
            target: r.get(2)?,
            started_on_latest: r.get(3)?,
            to: from_json(&r.get::<_, String>(4)?),
            snippet: snippet_of_html(&body),
            updated_at: r.get(6)?,
        })
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}

#[derive(Debug, Clone, Copy)]
pub enum OutboxFilter {
    Thread(ThreadId),
    Account(AccountId),
}

pub fn outbox_rows(conn: &Connection, filter: OutboxFilter) -> anyhow::Result<Vec<OutboxRow>> {
    let (filter_sql, key): (&str, &dyn ToSql) = match &filter {
        // A sent row shows until its message is stored on the thread.
        OutboxFilter::Thread(t) => (
            "o.thread_id = ?1 AND NOT (o.state = 'sent' AND EXISTS
               (SELECT 1 FROM messages m WHERE m.thread_id = o.thread_id AND m.gmail_id = o.sent_message_id))",
            t,
        ),
        OutboxFilter::Account(a) => ("o.account_id = ?1", a),
    };
    let sql = format!(
        "SELECT o.id, o.account_id, o.draft_id, o.thread_id, o.send_after, o.state, o.sent_message_id,
                o.undo_requested, o.toast_closed, o.delayed, d.to_json, d.body_html, d.subject,
                (SELECT u.id FROM undo_entries u WHERE u.kind = 'send' AND json_extract(u.payload, '$.outbox') = o.id)
         FROM outbox o LEFT JOIN drafts d ON d.id = o.draft_id WHERE {filter_sql} ORDER BY o.id"
    );
    let mut stmt = conn.prepare_cached(&sql)?;
    let rows = stmt.query_map([key], |r| {
        Ok(OutboxRow {
            id: r.get(0)?,
            account: r.get(1)?,
            draft: r.get(2)?,
            thread: r.get(3)?,
            send_after: r.get(4)?,
            state: OutboxState::parse(&r.get::<_, String>(5)?),
            sent_message_gmail_id: r.get(6)?,
            undo_requested: r.get(7)?,
            toast_closed: r.get(8)?,
            delayed: r.get(9)?,
            to: from_json(&r.get::<_, Option<String>>(10)?.unwrap_or_default()),
            snippet: snippet_of_html(&r.get::<_, Option<String>>(11)?.unwrap_or_default()),
            subject: r.get::<_, Option<String>>(12)?.unwrap_or_default(),
            undo_entry: r.get(13)?,
        })
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}

pub fn draft_state(conn: &Connection, id: &DraftId) -> anyhow::Result<DraftState> {
    let row = conn
        .query_row(
            "SELECT account_id, kind, reply_all, thread_id, target_message_id, from_json, to_json, cc_json, bcc_json,
                    subject, body_html, quote_html, attachments_json, started_fresh, rev
             FROM drafts WHERE id = ?1",
            [id],
            |r| {
                Ok(DraftState {
                    id: id.clone(),
                    account: r.get(0)?,
                    kind: DraftKind::parse(&r.get::<_, String>(1)?),
                    reply_all: r.get(2)?,
                    thread: r.get(3)?,
                    target: r.get(4)?,
                    from: serde_json::from_str(&r.get::<_, String>(5)?).unwrap_or(None),
                    to: from_json(&r.get::<_, String>(6)?),
                    cc: from_json(&r.get::<_, String>(7)?),
                    bcc: from_json(&r.get::<_, String>(8)?),
                    subject: r.get(9)?,
                    body_html: r.get(10)?,
                    quote_html: r.get(11)?,
                    attachments: from_json(&r.get::<_, String>(12)?),
                    resumed: !r.get::<_, bool>(13)?,
                    gone: false,
                    rev: r.get(14)?,
                })
            },
        )
        .optional()?;
    Ok(row.unwrap_or_else(|| DraftState {
        id: id.clone(),
        account: AccountId::NONE,
        kind: DraftKind::New,
        reply_all: false,
        thread: None,
        target: None,
        from: None,
        to: vec![],
        cc: vec![],
        bcc: vec![],
        subject: String::new(),
        body_html: String::new(),
        quote_html: None,
        attachments: vec![],
        resumed: false,
        gone: true,
        rev: 0,
    }))
}

pub fn setting(conn: &Connection, account: AccountId, key: &str) -> anyhow::Result<Option<String>> {
    Ok(conn
        .query_row(
            "SELECT value FROM settings WHERE account_id = ?1 AND key = ?2",
            params![account, key],
            |r| r.get(0),
        )
        .optional()?)
}

pub fn global_settings(conn: &Connection) -> anyhow::Result<GlobalSettings> {
    Ok(GlobalSettings {
        remote_images: setting(conn, AccountId::NONE, "remote_images")?.as_deref() != Some("off"),
    })
}

fn sync_status(s: &str, needs_signin: bool) -> SyncStatus {
    if needs_signin {
        return SyncStatus::NeedsSignin;
    }
    match s {
        "syncing" => SyncStatus::Syncing,
        "offline" => SyncStatus::Offline,
        "retrying" => SyncStatus::Retrying,
        _ => SyncStatus::Idle,
    }
}

pub fn accounts(conn: &Connection) -> anyhow::Result<Vec<AccountInfo>> {
    let mut stmt = conn.prepare_cached(
        "SELECT a.id, a.email, a.display_name, a.hue, a.position, a.needs_signin, a.sync_status, a.send_as,
                (SELECT count(*) FROM threads t WHERE t.account_id = a.id AND t.in_inbox = 1
                   AND t.unread = 1 AND t.snooze_until IS NULL),
                EXISTS (SELECT 1 FROM settings s WHERE s.account_id = a.id AND s.key = 'muted' AND s.value = 'on')
         FROM accounts a ORDER BY a.position, a.id",
    )?;
    let rows = stmt.query_map([], |r| {
        let needs: bool = r.get(5)?;
        let hue: i64 = r.get(3)?;
        Ok(AccountInfo {
            id: r.get(0)?,
            email: r.get(1)?,
            name: r.get(2)?,
            hue: HUES[usize::try_from(hue).unwrap_or(0) % HUES.len()].to_string(),
            position: r.get(4)?,
            needs_signin: needs,
            sync_status: sync_status(&r.get::<_, String>(6)?, needs),
            send_as: from_json(&r.get::<_, String>(7)?),
            inbox_unread: r.get(8)?,
            muted: r.get(9)?,
        })
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}

pub fn app_state(conn: &Connection, online: bool) -> anyhow::Result<AppState> {
    Ok(AppState {
        accounts: accounts(conn)?,
        settings: global_settings(conn)?,
        online,
    })
}

pub fn splits(conn: &Connection, account: AccountId) -> anyhow::Result<Vec<SplitInfo>> {
    let mut stmt = conn.prepare_cached(
        "SELECT id, name, kind, query, enabled, hide_when_empty, position FROM splits
         WHERE account_id = ?1 ORDER BY position, id",
    )?;
    let rows = stmt.query_map([account], |r| {
        Ok(SplitInfo {
            id: r.get(0)?,
            name: r.get(1)?,
            kind: SplitKind::parse(&r.get::<_, String>(2)?),
            query: r.get(3)?,
            enabled: r.get(4)?,
            hide_when_empty: r.get(5)?,
            position: r.get(6)?,
        })
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}

pub fn pins(conn: &Connection, account: AccountId) -> anyhow::Result<Vec<PinInfo>> {
    let mut stmt =
        conn.prepare_cached("SELECT kind, value, split_id FROM split_overrides WHERE account_id = ?1 ORDER BY kind, value")?;
    let rows = stmt.query_map([account], |r| {
        Ok(PinInfo {
            kind: PinKind::parse(&r.get::<_, String>(0)?),
            value: r.get(1)?,
            split: r.get(2)?,
        })
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}

pub fn labels(conn: &Connection, account: AccountId) -> anyhow::Result<Vec<LabelInfo>> {
    let mut stmt =
        conn.prepare_cached("SELECT id, name, system FROM labels WHERE account_id = ?1 ORDER BY system DESC, name COLLATE NOCASE")?;
    let rows = stmt.query_map([account], |r| {
        Ok(LabelInfo {
            id: r.get(0)?,
            name: r.get(1)?,
            system: r.get(2)?,
        })
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}

pub fn account_summary(
    conn: &Connection,
    account: AccountId,
) -> anyhow::Result<Option<AccountSummary>> {
    let Some(info) = accounts(conn)?.into_iter().find(|a| a.id == account) else {
        return Ok(None);
    };
    let undo = {
        let mut stmt = conn.prepare_cached(
            "SELECT id, kind, label, payload FROM undo_entries WHERE account_id = ?1 ORDER BY id",
        )?;
        let rows = stmt.query_map([account], |r| {
            let payload: String = r.get(3)?;
            let v: serde_json::Value = serde_json::from_str(&payload).unwrap_or_default();
            Ok(UndoEntryInfo {
                id: r.get(0)?,
                kind: UndoKind::parse(&r.get::<_, String>(1)?),
                label: r.get(2)?,
                threads: v["threads"]
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .filter_map(|t| ThreadId::deserialize(&t["thread"]).ok())
                            .collect()
                    })
                    .unwrap_or_default(),
                outbox: OutboxId::deserialize(&v["outbox"]).ok(),
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>()?
    };
    let notices = {
        let mut stmt =
            conn.prepare_cached("SELECT id, kind, message, draft_id, thread_id FROM notices WHERE account_id = ?1 ORDER BY id")?;
        let rows = stmt.query_map([account], |r| {
            Ok(Notice {
                id: r.get(0)?,
                kind: NoticeKind::parse(&r.get::<_, String>(1)?),
                message: r.get(2)?,
                draft: r.get(3)?,
                thread: r.get(4)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>()?
    };
    let queued_ops: u32 = conn.query_row(
        "SELECT count(*) FROM ops WHERE account_id = ?1",
        [account],
        |r| r.get(0),
    )?;
    Ok(Some(AccountSummary {
        account: info,
        labels: labels(conn, account)?,
        splits: splits(conn, account)?,
        pins: pins(conn, account)?,
        outbox: outbox_rows(conn, OutboxFilter::Account(account))?,
        undo,
        notices,
        queued_ops,
    }))
}

/// Sanitized bodies for message ids. Messages with no stored body are skipped.
pub fn bodies(conn: &Connection, ids: &[MessageId]) -> anyhow::Result<Vec<MessageBody>> {
    let mut stmt = conn.prepare_cached(
        "SELECT body_html, quote_html, body_text FROM messages WHERE id = ?1 AND has_body = 1",
    )?;
    let mut out = Vec::new();
    for &id in ids {
        let row = stmt
            .query_row([id], |r| {
                Ok((
                    r.get::<_, Option<String>>(0)?,
                    r.get::<_, Option<String>>(1)?,
                    r.get::<_, Option<String>>(2)?,
                ))
            })
            .optional()?;
        if let Some((html, quote, text)) = row {
            let html = html.unwrap_or_else(|| ub_mime::text_to_html(&text.unwrap_or_default()));
            out.push(MessageBody {
                id,
                html,
                quote_html: quote,
            });
        }
    }
    Ok(out)
}

/// Thread id for a Gmail thread id.
pub fn thread_by_gmail(
    conn: &Connection,
    account: AccountId,
    gmail_id: &str,
) -> anyhow::Result<Option<ThreadId>> {
    Ok(conn
        .query_row(
            "SELECT id FROM threads WHERE account_id = ?1 AND gmail_id = ?2",
            params![account, gmail_id],
            |r| r.get(0),
        )
        .optional()?)
}

/// Map of local label id → Gmail label id for an account.
pub fn label_gmail_ids(
    conn: &Connection,
    account: AccountId,
) -> anyhow::Result<HashMap<String, String>> {
    let mut stmt = conn.prepare_cached(
        "SELECT id, gmail_id FROM labels WHERE account_id = ?1 AND gmail_id IS NOT NULL",
    )?;
    let rows = stmt.query_map([account], |r| Ok((r.get(0)?, r.get(1)?)))?;
    Ok(rows.collect::<Result<_, _>>()?)
}

pub fn last_action(conn: &Connection) -> anyhow::Result<i64> {
    Ok(conn.query_row(
        "SELECT value FROM meta WHERE key = 'last_action'",
        [],
        |r| r.get(0),
    )?)
}
