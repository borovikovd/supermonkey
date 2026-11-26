use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use chrono::FixedOffset;
use uuid::Uuid;

use crate::domain::{
    AccountId, AttachmentId, DraftDefaults, DraftId, DraftKind, DraftLifecycle, DraftStage,
    DraftWait, GmailAttachmentId, Identity, MessageId, Millis, ReplyTarget, Rev, ThreadId,
    addressing,
};
use crate::ipc::{DraftAttachment, DraftFields};
use crate::mime::{
    OutAttachment, Outgoing, QuoteSource, build_mime, data_images_to_cid, forward_quote_html,
    forward_subject, html_to_text, is_valid_email, reply_quote_html, reply_subject, snippet_text,
    text_to_html,
};
use crate::store::{
    Db, DraftAttachmentItem, DraftRecord, MessageRecord, NewDraft, StoreError, WriteTx,
};

pub(crate) const MAX_ATTACHMENT_BYTES: usize = 25 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct DraftFiles {
    root: PathBuf,
}

impl DraftFiles {
    #[must_use]
    pub fn new(data_dir: &Path) -> Self {
        Self {
            root: data_dir.join("drafts"),
        }
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    #[must_use]
    pub fn attachments_dir(&self) -> PathBuf {
        self.root.parent().unwrap_or(&self.root).join("attachments")
    }

    pub fn store_attachment(
        &self,
        draft: &DraftId,
        filename: &str,
        bytes: &[u8],
    ) -> Result<(String, PathBuf), std::io::Error> {
        if !is_safe_segment(draft.as_str()) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "invalid draft id",
            ));
        }
        let dir = self.root.join(draft.as_str());
        fs::create_dir_all(&dir)?;
        let att_id = Uuid::new_v4().to_string();
        let clean_name = sanitize_filename(filename);
        let path = dir.join(format!("{att_id}-{clean_name}"));
        fs::write(&path, bytes)?;
        Ok((att_id, path))
    }

    pub fn store_carried(
        &self,
        draft: &DraftId,
        att_id: &str,
        filename: &str,
        bytes: &[u8],
    ) -> Result<PathBuf, std::io::Error> {
        if !is_safe_segment(draft.as_str()) || !is_safe_segment(att_id) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "invalid draft or attachment id",
            ));
        }
        let dir = self.root.join(draft.as_str());
        fs::create_dir_all(&dir)?;
        let clean_name = sanitize_filename(filename);
        let path = dir.join(format!("{att_id}-{clean_name}"));
        fs::write(&path, bytes)?;
        Ok(path)
    }

    pub fn remove_draft_dir(&self, draft: &DraftId) {
        if !is_safe_segment(draft.as_str()) {
            return;
        }
        let dir = self.root.join(draft.as_str());
        if dir.exists()
            && let Err(err) = fs::remove_dir_all(&dir)
        {
            tracing::warn!(error = %err, "failed to remove draft folder");
        }
    }

    pub fn remove_unused_files(&self, draft: &DraftId, keep_paths: &HashSet<String>) {
        if !is_safe_segment(draft.as_str()) {
            return;
        }
        let dir = self.root.join(draft.as_str());
        let Ok(entries) = fs::read_dir(&dir) else {
            return;
        };
        let mut remaining = 0usize;
        for entry in entries.flatten() {
            let path = entry.path();
            let key = path.to_string_lossy();
            if keep_paths.contains(key.as_ref()) {
                remaining += 1;
            } else {
                super::log_err("remove unused draft file", fs::remove_file(&path));
            }
        }
        if remaining == 0 {
            let _removed = fs::remove_dir(&dir).is_ok();
        }
    }

    pub fn sweep_orphans(&self, live: &HashSet<DraftId>) {
        let Ok(entries) = fs::read_dir(&self.root) else {
            return;
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(s) = name.to_str() else {
                continue;
            };
            if live.contains(s) {
                continue;
            }
            remove_orphan_entry(&entry.path());
        }
    }

    #[must_use]
    pub fn folder_names(&self) -> Vec<String> {
        let Ok(entries) = fs::read_dir(&self.root) else {
            return Vec::new();
        };
        let mut out: Vec<String> = entries
            .flatten()
            .filter(|e| e.path().is_dir())
            .filter_map(|e| e.file_name().to_str().map(str::to_owned))
            .collect();
        out.sort();
        out
    }
}

fn remove_orphan_entry(path: &Path) {
    let res = if path.is_dir() {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    };
    if let Err(err) = res {
        tracing::warn!(error = %err, "failed to sweep orphan draft entry");
    }
}

fn is_safe_segment(s: &str) -> bool {
    !s.is_empty()
        && !s.contains('/')
        && !s.contains('\\')
        && !s.contains('\0')
        && s != "."
        && s != ".."
}

#[must_use]
pub(crate) fn sanitize_filename(name: &str) -> Cow<'_, str> {
    let clean = !name.is_empty()
        && !name.starts_with('.')
        && name.trim() == name
        && name.chars().take(121).count() <= 120
        && !name
            .chars()
            .any(|c| c.is_control() || matches!(c, '/' | '\\' | ':' | '\0'));
    if clean {
        return Cow::Borrowed(name);
    }
    let cleaned: String = name
        .chars()
        .map(|c| {
            if c.is_control() || matches!(c, '/' | '\\' | ':' | '\0') {
                '_'
            } else {
                c
            }
        })
        .collect();
    let trimmed = cleaned.trim().trim_start_matches('.');
    if trimmed.is_empty() {
        Cow::Borrowed("attachment")
    } else {
        Cow::Owned(trimmed.chars().take(120).collect())
    }
}

#[must_use]
pub(crate) fn is_blank_html(html: &str) -> bool {
    let lower = html.to_ascii_lowercase();
    if lower.contains("<img") {
        return false;
    }
    html_to_text(html).trim().is_empty()
}

#[derive(Debug, Clone)]
pub(crate) struct CreateDraftArgs {
    pub(crate) id: DraftId,
    pub(crate) account: AccountId,
    pub(crate) kind: DraftKind,
    pub(crate) reply_all: bool,
    pub(crate) thread: Option<ThreadId>,
    pub(crate) target: Option<MessageId>,
    pub(crate) started_on_latest: bool,
    pub(crate) now: Millis,
    pub(crate) offset: FixedOffset,
}

#[expect(
    clippy::too_many_lines,
    reason = "constructs fresh new, reply, or forward draft with addressing, threading, quote, and carried attachments"
)]
pub(crate) fn build_new_draft(db: Db<'_>, args: CreateDraftArgs) -> Result<NewDraft, StoreError> {
    let acct = db
        .account(args.account)?
        .ok_or(StoreError::NotFound("account"))?;
    let identity = acct.identity();

    if args.kind == DraftKind::New {
        let default_from = Some(identity.default_from());
        let defaults = DraftDefaults {
            from: default_from.clone(),
            to: Vec::new(),
            cc: Vec::new(),
            bcc: Vec::new(),
            subject: String::new(),
            body_html: String::new(),
            attachments: Vec::new(),
        };
        return Ok(NewDraft {
            id: args.id,
            account: args.account,
            kind: DraftKind::New,
            reply_all: false,
            thread: None,
            target: None,
            started_on_latest: false,
            from: default_from,
            to: Vec::new(),
            cc: Vec::new(),
            bcc: Vec::new(),
            subject: String::new(),
            body_html: String::new(),
            snippet: String::new(),
            quote_html: None,
            attachments: Vec::new(),
            in_reply_to: None,
            references: Vec::new(),
            updated_at: args.now,
            lifecycle: DraftLifecycle::fresh(defaults, None),
        });
    }

    let thread_id = args
        .thread
        .ok_or(StoreError::Invalid("reply has no thread"))?;
    let thread_row = db
        .thread_row(thread_id)?
        .ok_or(StoreError::NotFound("thread"))?;
    let target_msg = resolve_target_message(db, thread_id, args.target)?;

    let (from, to, cc) = match (args.kind, target_msg.as_ref()) {
        (DraftKind::Reply, Some(t)) => {
            let aimed = addressing(
                ReplyTarget {
                    from: &t.from,
                    to: &t.to,
                    cc: &t.cc,
                    reply_to: &t.reply_to,
                },
                &identity,
                args.reply_all,
            );
            (Some(aimed.from), aimed.to, aimed.cc)
        }
        _ => (Some(identity.default_from()), Vec::new(), Vec::new()),
    };

    let base_subject = target_msg
        .as_ref()
        .map_or(thread_row.subject.as_str(), |m| m.subject.as_str());
    let subject = match args.kind {
        DraftKind::Forward => forward_subject(base_subject),
        DraftKind::Reply => reply_subject(base_subject),
        DraftKind::New => String::new(),
    };

    let (in_reply_to, references) = target_msg
        .as_ref()
        .map(build_threading_headers)
        .unwrap_or_default();

    let mut quote_html = None;
    let mut attachments = Vec::new();
    let mut waits_for = None;

    if let Some(target) = &target_msg {
        if target.has_body {
            quote_html = build_target_quote(db, target, args.kind, args.offset)?;
            if args.kind == DraftKind::Forward {
                attachments = carried_attachments_from_message(db, target.id, None)?;
            }
        } else {
            waits_for = Some(DraftWait::TargetBody);
            quote_html = build_snippet_quote(target, args.kind, args.offset);
        }
    }

    let defaults = DraftDefaults {
        from: from.clone(),
        to: to.clone(),
        cc: cc.clone(),
        bcc: Vec::new(),
        subject: subject.clone(),
        body_html: String::new(),
        attachments: attachments.iter().map(|a| a.id.clone()).collect(),
    };
    let mut lifecycle = DraftLifecycle::fresh(defaults, waits_for);
    lifecycle.gmail_thread_id = Some(thread_row.gmail_id);

    Ok(NewDraft {
        id: args.id,
        account: args.account,
        kind: args.kind,
        reply_all: args.reply_all,
        thread: Some(thread_id),
        target: target_msg.map(|m| m.id),
        started_on_latest: args.started_on_latest,
        from,
        to,
        cc,
        bcc: Vec::new(),
        subject,
        body_html: String::new(),
        snippet: String::new(),
        quote_html,
        attachments,
        in_reply_to,
        references,
        updated_at: args.now,
        lifecycle,
    })
}

fn resolve_target_message(
    db: Db<'_>,
    thread: ThreadId,
    target: Option<MessageId>,
) -> Result<Option<MessageRecord>, StoreError> {
    if let Some(mid) = target
        && let Some(msg) = db.message(mid)?
        && msg.thread == thread
    {
        return Ok(Some(msg));
    }
    let mut msgs = db.thread_messages(thread)?;
    let pos = msgs
        .iter()
        .rposition(|m| m.is_live)
        .or_else(|| msgs.iter().rposition(|m| !m.is_draft));
    Ok(pos.map(|i| msgs.swap_remove(i)))
}

fn build_threading_headers(target: &MessageRecord) -> (Option<String>, Vec<String>) {
    let mut refs = target.references.clone();
    if let Some(mid) = &target.message_id_hdr {
        if !refs.iter().any(|r| r == mid) {
            refs.push(mid.clone());
        }
        (Some(mid.clone()), refs)
    } else {
        (None, refs)
    }
}

fn build_target_quote(
    db: Db<'_>,
    target: &MessageRecord,
    kind: DraftKind,
    offset: FixedOffset,
) -> Result<Option<String>, StoreError> {
    let Some(body) = db.message_body(target.id)? else {
        return Ok(None);
    };
    let full = match body.quote_html.as_deref() {
        Some(q) if !q.is_empty() => format!("{}{q}", body.html),
        _ => body.html,
    };
    let src = QuoteSource {
        from: &target.from,
        to: &target.to,
        cc: &target.cc,
        subject: &target.subject,
        date: target.date,
        html: &full,
    };
    Ok(Some(match kind {
        DraftKind::Forward => forward_quote_html(src, offset),
        DraftKind::Reply => reply_quote_html(src, offset),
        DraftKind::New => String::new(),
    }))
}

fn build_snippet_quote(
    target: &MessageRecord,
    kind: DraftKind,
    offset: FixedOffset,
) -> Option<String> {
    if target.snippet.trim().is_empty() {
        return None;
    }
    let snippet_html = text_to_html(&target.snippet);
    let src = QuoteSource {
        from: &target.from,
        to: &target.to,
        cc: &target.cc,
        subject: &target.subject,
        date: target.date,
        html: &snippet_html,
    };
    Some(match kind {
        DraftKind::Forward => forward_quote_html(src, offset),
        DraftKind::Reply => reply_quote_html(src, offset),
        DraftKind::New => String::new(),
    })
}

fn carried_attachments_from_message(
    db: Db<'_>,
    msg_id: MessageId,
    added_at: Option<Rev>,
) -> Result<Vec<DraftAttachmentItem>, StoreError> {
    let atts = db.attachments_for_message(msg_id)?;
    let mut out = Vec::new();
    for att in atts {
        if att.is_inline {
            continue;
        }
        out.push(DraftAttachmentItem {
            id: Uuid::new_v4().to_string(),
            filename: att.filename,
            mime_type: att.mime_type,
            size: att.size,
            path: att.local_path,
            source: Some(att.id),
            gmail_att_id: None,
            added: added_at,
        });
    }
    Ok(out)
}

pub(crate) fn complete_waiting_drafts_for_message(
    tx: &mut WriteTx<'_>,
    msg_id: MessageId,
    offset: FixedOffset,
) -> Result<Vec<DraftRecord>, StoreError> {
    let Some(target) = tx.read().message(msg_id)? else {
        return Ok(Vec::new());
    };
    let waiting = tx.read().drafts_awaiting_body(msg_id)?;
    let mut ready_to_sync = Vec::new();
    for mut draft in waiting {
        if !draft.lifecycle.finish_wait(DraftWait::TargetBody) {
            continue;
        }
        draft.quote_html = build_target_quote(tx.read(), &target, draft.kind, offset)?;
        if draft.kind == DraftKind::Forward {
            let rev = tx.stamp();
            let carried = carried_attachments_from_message(tx.read(), msg_id, Some(rev))?;
            let added_ids: Vec<String> = carried.iter().map(|a| a.id.clone()).collect();
            draft.attachments.extend(carried);
            if let DraftStage::Fresh { defaults } = &mut draft.lifecycle.stage {
                defaults.attachments.extend(added_ids);
            }
        }
        tx.put_draft(&draft.as_write())?;
        let untouched = draft_is_untouched(&draft);
        if (draft.in_outbox || !untouched || draft.lifecycle.is_kept())
            && draft.lifecycle.waits_for.is_none()
        {
            ready_to_sync.push(draft);
        }
    }
    Ok(ready_to_sync)
}

#[must_use]
pub(crate) fn merge_attachments(
    existing: Vec<DraftAttachmentItem>,
    incoming: Vec<DraftAttachment>,
    incoming_rev: Rev,
) -> Vec<DraftAttachmentItem> {
    let mut slots: Vec<Option<DraftAttachmentItem>> = existing.into_iter().map(Some).collect();
    let mut index_by_id: HashMap<String, usize> = slots
        .iter()
        .enumerate()
        .filter_map(|(i, s)| s.as_ref().map(|a| (a.id.clone(), i)))
        .collect();
    let mut out = Vec::with_capacity(incoming.len());
    let mut seen: HashSet<String> = HashSet::new();

    for item in incoming {
        if !seen.insert(item.id.clone()) {
            continue;
        }
        if let Some(idx) = index_by_id.remove(&item.id)
            && let Some(prev) = slots[idx].take()
        {
            out.push(prev);
        } else {
            out.push(DraftAttachmentItem {
                id: item.id,
                filename: item.filename,
                mime_type: item.mime_type,
                size: item.size,
                path: item.path,
                source: item.source,
                gmail_att_id: None,
                added: None,
            });
        }
    }
    for prev in slots.into_iter().flatten() {
        if !seen.contains(&prev.id) && prev.added.is_some_and(|a| a > incoming_rev) {
            out.push(prev);
        }
    }
    out
}

pub(crate) fn apply_update_fields(
    draft: &mut DraftRecord,
    fields: DraftFields,
    incoming_rev: Rev,
    now: Millis,
) {
    if fields.from.is_some() {
        draft.from = fields.from;
    }
    draft.to = fields.to;
    draft.cc = fields.cc;
    draft.bcc = fields.bcc;
    draft.subject = fields.subject;
    draft.body_html = fields.body_html;
    let existing = std::mem::take(&mut draft.attachments);
    draft.attachments = merge_attachments(existing, fields.attachments, incoming_rev);
    draft.snippet = snippet_text(&draft.body_html);
    draft.updated_at = now;
    draft.lifecycle.gmail_change_skipped = false;
}

#[must_use]
pub(crate) fn draft_is_untouched(draft: &DraftRecord) -> bool {
    draft.lifecycle.is_untouched(
        draft.content_ref(),
        draft.attachments.iter().map(|a| a.id.as_str()),
        is_blank_html(&draft.body_html),
    )
}

pub(crate) fn validate_sendable(draft: &DraftRecord) -> Result<(), String> {
    let total = draft.to.len() + draft.cc.len() + draft.bcc.len();
    if total == 0 {
        return Err("Add a recipient".to_owned());
    }
    let mut bad = Vec::new();
    for addr in draft.to.iter().chain(&draft.cc).chain(&draft.bcc) {
        if !is_valid_email(&addr.email) {
            bad.push(addr.email.as_str());
        }
    }
    if bad.len() == 1 {
        return Err(format!("Not sent: {} isn't a valid email address", bad[0]));
    }
    if bad.len() > 1 {
        return Err(format!("Fix these addresses: {}", bad.join(", ")));
    }
    if let Some(from) = &draft.from
        && !is_valid_email(&from.email)
    {
        return Err(format!(
            "Not sent: {} isn't a valid email address",
            from.email
        ));
    }
    Ok(())
}

pub(crate) fn build_draft_mime(
    draft: &DraftRecord,
    identity: &Identity,
    now: Millis,
) -> Result<Vec<u8>, String> {
    let from = draft
        .from
        .clone()
        .unwrap_or_else(|| identity.default_from());
    let mut out_atts = Vec::with_capacity(draft.attachments.len());
    for att in &draft.attachments {
        let Some(path) = &att.path else {
            return Err(format!("attachment {} is not downloaded yet", att.filename));
        };
        let data = fs::read(path)
            .map_err(|err| format!("attachment {} is missing ({err})", att.filename))?;
        out_atts.push(OutAttachment {
            filename: att.filename.clone(),
            mime_type: att.mime_type.clone(),
            data,
        });
    }

    let (body_html, mut inline) = data_images_to_cid(&draft.body_html, &[]);
    let quote_html = draft.quote_html.as_deref().map(|q| {
        let (rewritten, more) = data_images_to_cid(q, &inline);
        inline.extend(more);
        rewritten
    });

    build_mime(&Outgoing {
        from: &from,
        to: &draft.to,
        cc: &draft.cc,
        bcc: &draft.bcc,
        subject: &draft.subject,
        body_html: &body_html,
        quote_html: quote_html.as_deref(),
        in_reply_to: draft.in_reply_to.as_deref(),
        references: &draft.references,
        draft_id: Some(&draft.id),
        attachments: &out_atts,
        inline: &inline,
        date: now,
    })
    .map_err(|e| e.to_string())
}

#[derive(Debug, Clone)]
pub(crate) enum CarriedSource {
    MessageAttachment(AttachmentId),
    GmailDraftAttachment(GmailAttachmentId),
}

#[must_use]
pub(crate) fn carried_sources_needing_download(
    draft: &DraftRecord,
) -> Vec<(String, CarriedSource, String)> {
    draft
        .attachments
        .iter()
        .filter_map(|a| {
            if a.path.is_some() {
                return None;
            }
            if let Some(src) = a.source {
                Some((
                    a.id.clone(),
                    CarriedSource::MessageAttachment(src),
                    a.filename.clone(),
                ))
            } else {
                a.gmail_att_id.as_ref().map(|gid| {
                    (
                        a.id.clone(),
                        CarriedSource::GmailDraftAttachment(gid.clone()),
                        a.filename.clone(),
                    )
                })
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::Addr;

    #[test]
    fn merge_attachments_preserves_server_added_after_client_rev() {
        let existing = vec![
            DraftAttachmentItem {
                id: "a1".to_owned(),
                filename: "old.txt".to_owned(),
                mime_type: "text/plain".to_owned(),
                size: 10,
                path: Some("/tmp/old.txt".to_owned()),
                source: None,
                gmail_att_id: None,
                added: Some(Rev(2)),
            },
            DraftAttachmentItem {
                id: "a2".to_owned(),
                filename: "late.pdf".to_owned(),
                mime_type: "application/pdf".to_owned(),
                size: 20,
                path: None,
                source: Some(AttachmentId(5)),
                gmail_att_id: None,
                added: Some(Rev(6)),
            },
        ];
        let incoming = vec![DraftAttachment {
            id: "a3".to_owned(),
            filename: "user.png".to_owned(),
            mime_type: "image/png".to_owned(),
            size: 30,
            path: Some("/tmp/user.png".to_owned()),
            source: None,
        }];
        let merged = merge_attachments(existing, incoming, Rev(4));
        let ids: Vec<&str> = merged.iter().map(|a| a.id.as_str()).collect();
        assert_eq!(ids, vec!["a3", "a2"]);
    }

    #[test]
    fn validate_sendable_reports_exact_recipient_messages() {
        let mut draft = DraftRecord {
            id: DraftId("d1".to_owned()),
            account: AccountId(1),
            kind: DraftKind::New,
            reply_all: false,
            thread: None,
            target: None,
            started_on_latest: false,
            from: None,
            to: Vec::new(),
            cc: Vec::new(),
            bcc: Vec::new(),
            subject: String::new(),
            body_html: String::new(),
            snippet: String::new(),
            quote_html: None,
            attachments: Vec::new(),
            in_reply_to: None,
            references: Vec::new(),
            updated_at: Millis(0),
            lifecycle: DraftLifecycle::kept(),
            rev: Rev(1),
            in_outbox: false,
        };
        assert_eq!(validate_sendable(&draft).unwrap_err(), "Add a recipient");
        draft.to = vec![Addr::new("not-an-email")];
        assert_eq!(
            validate_sendable(&draft).unwrap_err(),
            "Not sent: not-an-email isn't a valid email address"
        );
        draft.cc = vec![Addr::new("also-bad")];
        assert_eq!(
            validate_sendable(&draft).unwrap_err(),
            "Fix these addresses: not-an-email, also-bad"
        );
    }
}
