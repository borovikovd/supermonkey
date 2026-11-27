use std::collections::{HashMap, HashSet};
use std::path::Path;

use crate::domain::{
    AccountId, Addr, GmailMessageId, GmailThreadId, HistoryId, Identity, LabelId, LabelSet, Millis,
    OpKind, Rev, SystemLabel, Thread, ThreadId, gmail_draft_aim, is_forward_subject,
};
use crate::gmail::{
    Format, Gmail, GmailError, Outcome, Priority as GmailPriority, Thread as WireThread,
};
use crate::mime::{CidImages, parse_message, snippet_text};
use crate::store::{
    ContactSighting, MessageBodyWrite, NewAttachment, NewMessage, Priority as StorePriority,
    ReadPool, StoreError, WriteTx, Writer,
};

use super::super::actions::schedule_draft_upsert;
use super::super::attachments::apply_quarantine;
use super::super::drafts::{complete_waiting_drafts_for_message, sanitize_filename};
use super::super::events::{Effect, NewMailAlert};
use super::super::runtime::Clock;
use super::super::writer_state::{ModifyOpPayload, TrashSentOpPayload, WriterState};

#[derive(Debug)]
pub(crate) struct IngestResult {
    pub(crate) thread_id: Option<ThreadId>,
    pub(crate) unknown_labels: bool,
    pub(crate) added_messages: bool,
    pub(crate) alerts: Vec<NewMailAlert>,
}

struct PreparedMessage {
    new_msg: NewMessage,
    body: Option<PreparedBody>,
    sighting: Option<ContactSighting>,
}

struct PreparedBody {
    html: String,
    quote_html: Option<String>,
    body_text: String,
    attachments: Vec<NewAttachment>,
}

#[expect(
    clippy::redundant_closure_for_method_calls,
    reason = "Db carries a lifetime so a closure is needed for ReadPool::with HRTB"
)]
pub(super) fn current_rev(pool: &ReadPool) -> Rev {
    pool.with(|db| db.rev()).unwrap_or(Rev(0))
}

#[expect(
    clippy::too_many_arguments,
    clippy::too_many_lines,
    reason = "thread fetch needs account, gmail client, storage, clock, attachment dir, and fetch options"
)]
pub(crate) async fn fetch_and_ingest_thread(
    account: AccountId,
    gmail: &Gmail,
    pool: &ReadPool,
    writer: &Writer<WriterState>,
    clock: &Clock,
    attachments_dir: Option<&Path>,
    gmail_tid: &GmailThreadId,
    format: Format,
    priority: GmailPriority,
    notify_new_mail: bool,
) -> Result<IngestResult, GmailError> {
    let start_rev = current_rev(pool);
    let wire = match gmail.get_thread(gmail_tid, format, priority).await {
        Ok(t) => t,
        Err(err) if err.outcome == Outcome::NotFound => {
            let gid = gmail_tid.clone();
            super::super::log_err(
                "delete 404 thread",
                writer
                    .call(StorePriority::Sync, move |tx, state| {
                        if let Some(tid) = tx.read().thread_by_gmail(account, &gid)? {
                            tx.delete_thread(tid)?;
                            state.emit(Effect::RefreshBadge);
                        }
                        Ok::<_, StoreError>(())
                    })
                    .await,
            );
            return Ok(IngestResult {
                thread_id: None,
                unknown_labels: false,
                added_messages: false,
                alerts: Vec::new(),
            });
        }
        Err(err) => return Err(err),
    };

    let identity = pool
        .with(|db| Ok::<_, StoreError>(db.account(account)?.map(|a| a.identity())))
        .ok()
        .flatten();
    let prepared = prepare_wire_thread(
        &wire,
        identity.as_ref(),
        gmail,
        clock,
        attachments_dir,
        format,
        priority,
    )
    .await?;
    let offset = clock.local_offset();
    let wire_history = wire.history_id;
    let gid = wire.id;

    let outcome = writer
        .call(StorePriority::Sync, move |tx, state| {
            commit_ingested_thread(
                tx,
                state,
                account,
                &gid,
                wire_history,
                start_rev,
                prepared,
                format == Format::Full,
                notify_new_mail,
                offset,
            )
        })
        .await;

    match outcome {
        Ok(res) => Ok(res),
        Err(err) => {
            tracing::warn!(error = %err, "failed to commit ingested thread");
            Ok(IngestResult {
                thread_id: None,
                unknown_labels: false,
                added_messages: false,
                alerts: Vec::new(),
            })
        }
    }
}

#[expect(
    clippy::too_many_arguments,
    clippy::too_many_lines,
    reason = "parses MIME headers, inline CID images, sanitized HTML body, attachments, and contact sighting per message"
)]
async fn prepare_wire_thread(
    wire: &WireThread,
    identity: Option<&Identity>,
    gmail: &Gmail,
    clock: &Clock,
    attachments_dir: Option<&Path>,
    format: Format,
    priority: GmailPriority,
) -> Result<Vec<PreparedMessage>, GmailError> {
    let mut out = Vec::with_capacity(wire.messages.len());
    for msg in &wire.messages {
        let Some(payload) = &msg.payload else {
            continue;
        };
        let parsed = parse_message(payload);
        let date = msg
            .internal_date
            .or(parsed.headers.date)
            .unwrap_or(Millis(0));
        let labels: LabelSet = msg.label_ids.iter().cloned().collect();
        let is_draft = labels.has(SystemLabel::Draft);

        let body = if format == Format::Full && !is_draft {
            let mut cids = CidImages::new();
            for inline in parsed.inline_fetches() {
                if let Some(bytes) = inline.inline_data {
                    cids.insert(
                        inline.content_id,
                        inline.mime_type.to_owned(),
                        bytes.to_vec(),
                    );
                } else if let Some(att_id) = inline.attachment_id {
                    match gmail.get_attachment(&msg.id, att_id, priority).await {
                        Ok(bytes) => {
                            cids.insert(inline.content_id, inline.mime_type.to_owned(), bytes);
                        }
                        Err(err)
                            if err.is_transient()
                                || matches!(
                                    err.outcome,
                                    Outcome::InvalidGrant | Outcome::Unauthorized
                                ) =>
                        {
                            return Err(err);
                        }
                        Err(_) => {}
                    }
                }
            }
            let rendered = parsed
                .raw_body
                .render(&cids, is_forward_subject(&parsed.headers.subject));
            let now = clock.now();
            let attachments: Vec<NewAttachment> = parsed
                .parts
                .iter()
                .map(|p| {
                    let local_path = if !p.is_inline
                        && p.attachment_id.is_none()
                        && let Some(bytes) = &p.inline_data
                        && let Some(dir) = attachments_dir
                    {
                        save_inline_attachment(dir, &msg.id, &p.id, &p.filename, bytes, now)
                    } else {
                        None
                    };
                    NewAttachment {
                        part_id: p.id.clone(),
                        filename: p.filename.clone(),
                        mime_type: p.mime_type.clone(),
                        size: p.size,
                        gmail_att_id: p.attachment_id.clone(),
                        content_id: p.content_id.clone(),
                        is_inline: p.is_inline,
                        local_path,
                    }
                })
                .collect();
            Some(PreparedBody {
                html: rendered.html,
                quote_html: rendered.quote_html,
                body_text: rendered.text,
                attachments,
            })
        } else {
            None
        };

        let has_attachments =
            (format == Format::Full).then(|| parsed.parts.iter().any(|p| !p.is_inline));
        let snippet = if msg.snippet.is_empty() {
            body.as_ref()
                .map(|b| snippet_text(&b.html))
                .unwrap_or_default()
        } else {
            html_escape::decode_html_entities(&msg.snippet).into_owned()
        };

        let from = parsed
            .headers
            .from
            .into_iter()
            .next()
            .unwrap_or_else(|| Addr::new(""));
        let sent_by_me =
            labels.has(SystemLabel::Sent) || identity.is_some_and(|id| id.is_mine(&from.email));
        let sighting = (!is_draft).then(|| ContactSighting {
            gmail_msg_id: msg.id.clone(),
            date,
            sent_by_me,
            is_list: parsed.headers.is_list,
            from: from.clone(),
            to: parsed.headers.to.clone(),
            cc: parsed.headers.cc.clone(),
        });

        out.push(PreparedMessage {
            new_msg: NewMessage {
                gmail_id: msg.id.clone(),
                date,
                from,
                to: parsed.headers.to,
                cc: parsed.headers.cc,
                bcc: parsed.headers.bcc,
                reply_to: parsed.headers.reply_to,
                subject: parsed.headers.subject,
                snippet,
                message_id_hdr: parsed.headers.message_id,
                in_reply_to: parsed.headers.in_reply_to,
                references: parsed.headers.references,
                is_list: parsed.headers.is_list,
                is_draft,
                sm_draft_id: parsed.headers.draft_id,
                labels,
                has_attachments,
            },
            body,
            sighting,
        });
    }
    Ok(out)
}

#[expect(
    clippy::too_many_arguments,
    clippy::too_many_lines,
    clippy::cognitive_complexity,
    reason = "single transaction commits messages, local-wins overlay, bodies, waiting drafts, snooze, contacts, and alerts"
)]
fn commit_ingested_thread(
    tx: &mut WriteTx<'_>,
    state: &mut WriterState,
    account: AccountId,
    gmail_tid: &GmailThreadId,
    wire_history: Option<HistoryId>,
    start_rev: Rev,
    mut prepared: Vec<PreparedMessage>,
    full_fetch: bool,
    notify_new_mail: bool,
    offset: chrono::FixedOffset,
) -> Result<IngestResult, StoreError> {
    let Some(acct) = tx.read().account(account)? else {
        return Ok(IngestResult {
            thread_id: None,
            unknown_labels: false,
            added_messages: false,
            alerts: Vec::new(),
        });
    };
    let identity = acct.identity();
    let all_identities: Vec<Identity> = tx
        .read()
        .accounts()?
        .into_iter()
        .map(|a| a.identity())
        .collect();

    let existing_tid = tx.read().thread_by_gmail(account, gmail_tid)?;
    let before_row = match existing_tid {
        Some(tid) => tx.read().thread_row(tid)?,
        None => None,
    };
    let before_thread = match existing_tid {
        Some(tid) => Some(tx.read().thread_facts(tid)?),
        None => None,
    };
    let prior_msgs = match existing_tid {
        Some(tid) => tx.read().thread_messages(tid)?,
        None => Vec::new(),
    };

    let is_outdated = before_row
        .as_ref()
        .and_then(|r| r.history_id)
        .zip(wire_history)
        .is_some_and(|(cur, incoming)| incoming < cur);

    if is_outdated {
        if full_fetch && let Some(tid) = existing_tid {
            store_missing_bodies(tx, state, &prior_msgs, &prepared, offset)?;
            return Ok(IngestResult {
                thread_id: Some(tid),
                unknown_labels: false,
                added_messages: false,
                alerts: Vec::new(),
            });
        }
        return Ok(IngestResult {
            thread_id: existing_tid,
            unknown_labels: false,
            added_messages: false,
            alerts: Vec::new(),
        });
    }

    let (gmail_to_local, known_locals) = load_label_maps(tx, account)?;
    let mut unknown_labels = false;
    for item in &mut prepared {
        let mut mapped = LabelSet::new();
        for gid in &item.new_msg.labels {
            if let Some(local) = gmail_to_local.get(gid) {
                mapped.insert(local.clone());
            } else {
                if !SystemLabel::is_system_id(gid.as_str()) && !known_locals.contains(gid) {
                    unknown_labels = true;
                }
                mapped.insert(gid.clone());
            }
        }
        item.new_msg.labels = mapped;
    }

    reapply_local_ops(tx, state, account, start_rev, wire_history, &mut prepared)?;

    let non_draft_count = prepared.iter().filter(|p| !p.new_msg.is_draft).count();
    if non_draft_count == 0 {
        if let Some(tid) = existing_tid {
            tx.delete_thread(tid)?;
            state.emit(Effect::RefreshBadge);
        }
        return Ok(IngestResult {
            thread_id: None,
            unknown_labels,
            added_messages: false,
            alerts: Vec::new(),
        });
    }

    let tid = tx.place_thread(account, gmail_tid, wire_history)?;
    let prior_gids: HashSet<&GmailMessageId> = prior_msgs.iter().map(|m| &m.gmail_id).collect();
    let added_messages = prepared
        .iter()
        .any(|p| !p.new_msg.is_draft && !prior_gids.contains(&p.new_msg.gmail_id));
    let incoming_gids: HashSet<GmailMessageId> = prepared
        .iter()
        .map(|p| p.new_msg.gmail_id.clone())
        .collect();
    for old in &prior_msgs {
        if !incoming_gids.contains(&old.gmail_id) {
            tx.delete_message(old.id)?;
        }
    }

    let mut sightings = Vec::new();
    let mut stored_body_ids = Vec::new();
    for item in &prepared {
        if item.new_msg.is_draft {
            continue;
        }
        let msg_id = tx.upsert_message(account, tid, &item.new_msg)?;
        if let Some(body) = &item.body {
            tx.store_message_body(
                msg_id,
                &MessageBodyWrite {
                    html: &body.html,
                    quote_html: body.quote_html.as_deref(),
                    body_text: &body.body_text,
                    attachments: &body.attachments,
                },
            )?;
            stored_body_ids.push(msg_id);
        }
        if let Some(s) = &item.sighting {
            sightings.push(s.clone());
        }
    }

    tx.derive_thread(tid)?;
    if let Some(h) = wire_history {
        tx.set_thread_history_id(tid, h)?;
    }

    for ob in tx.read().outbox_for_thread(tid)? {
        if ob.item.state == crate::domain::OutboxState::Sent
            && ob.item.toast_closed
            && ob
                .item
                .sent_message_id
                .as_ref()
                .is_some_and(|gmid| incoming_gids.contains(gmid))
        {
            tx.delete_undo_for_outbox(account, ob.item.id)?;
            tx.delete_outbox(ob.item.id)?;
            tx.delete_draft(&ob.item.draft)?;
            state.emit(Effect::RemoveDraftFiles(ob.item.draft));
        }
    }

    for msg_id in stored_body_ids {
        let ready = complete_waiting_drafts_for_message(tx, msg_id, offset)?;
        for draft in ready {
            if draft.in_outbox {
                state.emit(Effect::KickOutbox(draft.account));
            } else {
                schedule_draft_upsert(tx, state, &draft)?;
            }
        }
    }

    let after_row = tx.read().thread_row(tid)?;
    let after_thread = tx.read().thread_facts(tid)?;

    if before_row.as_ref().and_then(|r| r.snooze_until).is_some()
        && after_row.as_ref().is_some_and(|r| r.in_inbox)
        && Thread::gained_inbox(before_thread.as_ref(), &after_thread, &identity)
    {
        tx.set_thread_snooze(tid, None)?;
        state.emit(Effect::KickSnooze);
    }

    if !sightings.is_empty() {
        tx.record_contacts(account, &sightings)?;
    }

    relink_unlinked_drafts_to_thread(tx, account, tid, gmail_tid, &after_thread)?;

    let mut alerts = Vec::new();
    if notify_new_mail {
        let old_ids: HashSet<&GmailMessageId> = before_thread
            .as_ref()
            .map(|t| t.messages().iter().map(|m| &m.gmail_id).collect())
            .unwrap_or_default();
        for msg in after_thread.messages() {
            if !msg.is_live()
                || !msg.has(SystemLabel::Inbox)
                || !msg.has(SystemLabel::Unread)
                || old_ids.contains(&msg.gmail_id)
                || all_identities.iter().any(|id| id.is_mine(&msg.from.email))
            {
                continue;
            }
            alerts.push(NewMailAlert {
                thread_id: tid,
                gmail_thread_id: gmail_tid.clone(),
                sender: msg.sender_display().to_owned(),
                subject: msg.subject.clone(),
                snippet: msg.snippet.clone(),
            });
        }
    }

    state.emit(Effect::RefreshBadge);
    Ok(IngestResult {
        thread_id: Some(tid),
        unknown_labels,
        added_messages,
        alerts,
    })
}

fn store_missing_bodies(
    tx: &mut WriteTx<'_>,
    state: &mut WriterState,
    existing: &[crate::store::MessageRecord],
    prepared: &[PreparedMessage],
    offset: chrono::FixedOffset,
) -> Result<(), StoreError> {
    let by_gid: HashMap<&GmailMessageId, &crate::store::MessageRecord> =
        existing.iter().map(|m| (&m.gmail_id, m)).collect();
    for item in prepared {
        if let Some(body) = &item.body
            && let Some(rec) = by_gid.get(&item.new_msg.gmail_id)
            && !rec.has_body
        {
            tx.store_message_body(
                rec.id,
                &MessageBodyWrite {
                    html: &body.html,
                    quote_html: body.quote_html.as_deref(),
                    body_text: &body.body_text,
                    attachments: &body.attachments,
                },
            )?;
            let ready = complete_waiting_drafts_for_message(tx, rec.id, offset)?;
            for draft in ready {
                if draft.in_outbox {
                    state.emit(Effect::KickOutbox(draft.account));
                } else {
                    schedule_draft_upsert(tx, state, &draft)?;
                }
            }
        }
    }
    Ok(())
}

#[expect(
    clippy::too_many_arguments,
    reason = "inline attachment writer takes destination directory, message/part identifiers, filename, bytes, and timestamp"
)]
fn save_inline_attachment(
    dir: &Path,
    msg_id: &GmailMessageId,
    part_id: &str,
    filename: &str,
    bytes: &[u8],
    now: Millis,
) -> Option<String> {
    if std::fs::create_dir_all(dir).is_err() {
        return None;
    }
    let clean_msg = sanitize_filename(&msg_id.0);
    let clean_part = sanitize_filename(part_id);
    let clean_name = sanitize_filename(filename);
    let path = dir.join(format!("inline-{clean_msg}-{clean_part}-{clean_name}"));
    if std::fs::write(&path, bytes).is_err() {
        return None;
    }
    super::super::log_err("quarantine inline attachment", apply_quarantine(&path, now));
    Some(path.to_string_lossy().into_owned())
}

fn load_label_maps(
    tx: &WriteTx<'_>,
    account: AccountId,
) -> Result<(HashMap<LabelId, LabelId>, HashSet<LabelId>), StoreError> {
    let labels = tx.read().labels(account)?;
    let mut gmail_to_local = HashMap::with_capacity(labels.len());
    let mut known_locals = HashSet::with_capacity(labels.len());
    for l in labels {
        known_locals.insert(l.id.clone());
        if let Some(gid) = l.gmail_id {
            gmail_to_local.insert(gid, l.id);
        }
    }
    Ok((gmail_to_local, known_locals))
}

#[expect(
    clippy::too_many_arguments,
    reason = "local-wins overlay applies both recently landed and still-queued ops against incoming thread"
)]
fn reapply_local_ops(
    tx: &WriteTx<'_>,
    state: &WriterState,
    account: AccountId,
    start_rev: Rev,
    wire_history: Option<HistoryId>,
    prepared: &mut [PreparedMessage],
) -> Result<(), StoreError> {
    let landed = state.freshness.landed_since(account, start_rev);
    let queued = tx.read().all_ops(account)?;

    for op in landed {
        let covered = wire_history
            .zip(op.min_history_id)
            .is_some_and(|(wh, mh)| wh >= mh);
        if covered {
            continue;
        }
        apply_label_overlay(prepared, &op.gmail_msg_ids, &op.add, &op.remove);
    }

    for op in queued {
        match op.kind {
            OpKind::Modify => {
                if let Ok(payload) = op.parse_payload::<ModifyOpPayload>() {
                    apply_label_overlay(
                        prepared,
                        &payload.gmail_msg_ids,
                        &payload.add,
                        &payload.remove,
                    );
                }
            }
            OpKind::TrashMessage => {
                if let Ok(payload) = op.parse_payload::<TrashSentOpPayload>() {
                    apply_label_overlay(
                        prepared,
                        &[payload.gmail_msg_id],
                        &[SystemLabel::Trash.label_id()],
                        &[SystemLabel::Inbox.label_id()],
                    );
                }
            }
            OpKind::CreateLabel | OpKind::DraftUpsert | OpKind::DraftDelete => {}
        }
    }
    Ok(())
}

fn apply_label_overlay(
    prepared: &mut [PreparedMessage],
    gmail_ids: &[GmailMessageId],
    add: &[LabelId],
    remove: &[LabelId],
) {
    let set: HashSet<&GmailMessageId> = gmail_ids.iter().collect();
    for item in prepared {
        if set.contains(&item.new_msg.gmail_id) {
            item.new_msg.labels = item.new_msg.labels.without(remove).and(add);
        }
    }
}

fn relink_unlinked_drafts_to_thread(
    tx: &mut WriteTx<'_>,
    account: AccountId,
    tid: ThreadId,
    gmail_tid: &GmailThreadId,
    thread: &Thread,
) -> Result<(), StoreError> {
    let drafts = tx.read().drafts_for_account(account)?;
    for mut d in drafts {
        if d.thread.is_none() && d.lifecycle.gmail_thread_id.as_ref() == Some(gmail_tid) {
            let aim = gmail_draft_aim(
                Some(thread),
                &d.subject,
                d.in_reply_to.as_deref(),
                &d.references,
            );
            d.thread = Some(tid);
            d.kind = aim.kind;
            d.target = aim.target;
            tx.put_draft(&d.as_write())?;
        }
    }
    Ok(())
}
