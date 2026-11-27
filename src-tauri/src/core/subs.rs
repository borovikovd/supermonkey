use std::collections::{HashMap, HashSet};

use crate::domain::query::Query;
use crate::domain::{
    AccountId, DraftId, Identity, Rev, SettingKey, SplitId, SubId, SystemLabel, ThreadId, View,
};
use crate::ipc::{
    AccountInfo, AccountSummary, AppState, AttachmentInfo, ConversationData, DraftAttachment,
    DraftState, DraftSummary, GlobalSettings, LabelInfo, ListEntry, ListMove, ListRow, MessageAddr,
    MessageSummary, Notice, OutboxRow, SendAs, SplitInfo, SubKey, SubUpdate, UndoEntryInfo,
    avatar_hue,
};
use crate::store::{
    AccountRecord, Changes, Db, DraftRecord, MessageRecord, OutboxRecord, StoreError, Table,
    ThreadRow,
};

use super::runtime::Clock;

const INITIAL_ROW_WINDOW: usize = 60;

#[derive(Debug, Default, Clone)]
pub(crate) struct ListingState {
    pub(crate) remote_hits: Vec<ThreadId>,
    pub(crate) searching: bool,
    pub(crate) more: bool,
}

#[derive(Debug, Default)]
struct SubState {
    delivered: bool,
    last_rev: Rev,
    last_ids: Vec<ListEntry>,
    listing: ListingState,
}

#[derive(Debug)]
struct SubEntry {
    key: SubKey,
    state: SubState,
}

#[derive(Debug, Default)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "mirrors independent boolean fields in SubUpdate::List"
)]
struct ListMeta {
    searching: bool,
    needs_gmail: bool,
    looks_in_trash: bool,
    terms: Vec<String>,
    more: bool,
    oldest_hit: Option<String>,
}

#[derive(Debug, Default)]
pub(crate) struct Registry {
    subs: HashMap<SubId, SubEntry>,
}

impl Registry {
    #[must_use]
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn clear(&mut self) {
        self.subs.clear();
    }

    pub(crate) fn subscribe(&mut self, sub: SubId, key: SubKey) {
        self.subs.insert(
            sub,
            SubEntry {
                key,
                state: SubState::default(),
            },
        );
    }

    pub(crate) fn unsubscribe(&mut self, sub: &SubId) -> Option<SubKey> {
        self.subs.remove(sub).map(|e| e.key)
    }

    #[must_use]
    pub(crate) fn is_draft_open(&self, draft: &DraftId) -> bool {
        self.subs
            .values()
            .any(|e| matches!(&e.key, SubKey::Composer { draft: d } if d == draft))
    }

    #[must_use]
    pub(crate) fn open_threads(&self) -> HashSet<ThreadId> {
        self.subs
            .values()
            .filter_map(|e| match e.key {
                SubKey::Conversation {
                    thread, open: true, ..
                } => Some(thread),
                SubKey::App
                | SubKey::Account { .. }
                | SubKey::List { .. }
                | SubKey::Search { .. }
                | SubKey::Conversation { open: false, .. }
                | SubKey::Composer { .. } => None,
            })
            .collect()
    }

    #[must_use]
    pub(crate) fn subscribed_threads(&self) -> HashSet<ThreadId> {
        self.subs
            .values()
            .filter_map(|e| match e.key {
                SubKey::Conversation { thread, .. } => Some(thread),
                SubKey::App
                | SubKey::Account { .. }
                | SubKey::List { .. }
                | SubKey::Search { .. }
                | SubKey::Composer { .. } => None,
            })
            .collect()
    }

    pub(crate) fn all_composer_subs(&self, draft: &DraftId) -> Vec<SubId> {
        self.subs
            .iter()
            .filter(|(_, entry)| matches!(&entry.key, SubKey::Composer { draft: d } if d == draft))
            .map(|(id, _)| id.clone())
            .collect()
    }

    pub(crate) fn app_and_search_subs(&self) -> Vec<SubId> {
        self.subs
            .iter()
            .filter(|(_, entry)| matches!(entry.key, SubKey::App | SubKey::Search { .. }))
            .map(|(id, _)| id.clone())
            .collect()
    }

    pub(crate) fn set_listing(&mut self, sub: &SubId, listing: ListingState) -> bool {
        let Some(entry) = self.subs.get_mut(sub) else {
            return false;
        };
        entry.state.listing = listing;
        true
    }

    pub(crate) fn stop_searching_for_account(&mut self, account: AccountId) -> Vec<SubId> {
        let mut changed = Vec::new();
        for (id, entry) in &mut self.subs {
            let matches = matches!(
                entry.key,
                SubKey::List { account: a, .. } | SubKey::Search { account: a, .. } if a == account
            );
            if matches && (entry.state.listing.searching || entry.state.listing.more) {
                entry.state.listing.searching = false;
                entry.state.listing.more = false;
                changed.push(id.clone());
            }
        }
        changed
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "commit evaluation projects all subscriptions with commit context, network state, and clock"
    )]
    pub(crate) fn evaluate_commit(
        &mut self,
        db: Db<'_>,
        changes: &Changes,
        rev: Rev,
        online: bool,
        clock: &Clock,
        force_subs: &HashSet<SubId>,
    ) -> Result<Vec<SubUpdate>, StoreError> {
        let mut ids: Vec<SubId> = self.subs.keys().cloned().collect();
        ids.sort_by(|a, b| a.0.cmp(&b.0));
        let mut updates = Vec::new();
        for sub_id in ids {
            let forced_flag = force_subs.contains(&sub_id);
            let Some(entry) = self.subs.get_mut(&sub_id) else {
                continue;
            };
            let forced = !entry.state.delivered || forced_flag;
            if !forced && !sub_affected(&entry.key, changes) {
                continue;
            }
            if let Some(update) = build_sub_update(db, sub_id, entry, changes, rev, online, clock)?
            {
                entry.state.delivered = true;
                entry.state.last_rev = rev;
                updates.push(update);
            }
        }
        Ok(updates)
    }
}

#[expect(
    clippy::too_many_lines,
    reason = "maps every SubKey variant to its exact set of invalidated SQLite tables"
)]
fn sub_affected(key: &SubKey, changes: &Changes) -> bool {
    match key {
        SubKey::App => {
            changes.has_any_account_table(Table::Accounts)
                || changes.has_any_account_table(Table::Settings)
                || changes.has_any_account_table(Table::Threads)
        }
        SubKey::Account { account } => changes.touches_any(
            Some(*account),
            &[
                Table::Accounts,
                Table::Labels,
                Table::Splits,
                Table::Threads,
                Table::Outbox,
                Table::Ops,
                Table::Undo,
                Table::Notices,
                Table::Settings,
            ],
        ),
        SubKey::List { account, view } => {
            if matches!(view, View::Drafts) {
                changes.touches_any(
                    Some(*account),
                    &[Table::Drafts, Table::Outbox, Table::Accounts],
                )
            } else {
                changes.touches_any(
                    Some(*account),
                    &[
                        Table::Threads,
                        Table::Messages,
                        Table::Labels,
                        Table::Splits,
                        Table::Pins,
                        Table::Accounts,
                    ],
                )
            }
        }
        SubKey::Search { account, .. } => changes.touches_any(
            Some(*account),
            &[
                Table::Threads,
                Table::Messages,
                Table::Labels,
                Table::Accounts,
            ],
        ),
        SubKey::Conversation { thread, .. } => {
            changes.threads.contains(thread)
                || changes.has_any_account_table(Table::Accounts)
                || changes.has_any_account_table(Table::Messages)
                || changes.has_any_account_table(Table::Attachments)
                || changes.has_any_account_table(Table::Drafts)
                || changes.has_any_account_table(Table::Outbox)
        }
        SubKey::Composer { draft } => {
            changes.drafts.contains(draft)
                || changes.has_any_account_table(Table::Accounts)
                || changes.has_any_account_table(Table::Outbox)
        }
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "single subscription projection takes commit context, network state, and clock"
)]
fn build_sub_update(
    db: Db<'_>,
    sub_id: SubId,
    entry: &mut SubEntry,
    changes: &Changes,
    rev: Rev,
    online: bool,
    clock: &Clock,
) -> Result<Option<SubUpdate>, StoreError> {
    let SubEntry { key, state } = entry;
    let first = !state.delivered;
    match key {
        SubKey::App => Ok(Some(SubUpdate::App {
            sub: sub_id,
            first,
            state: project_app_state(db, online)?,
        })),
        SubKey::Account { account } => {
            let Some(summary) = project_account_summary(db, *account)? else {
                return Ok(None);
            };
            Ok(Some(SubUpdate::Account {
                sub: sub_id,
                first,
                summary,
            }))
        }
        SubKey::List { account, view } => {
            let update = build_list_update(db, sub_id, *account, view, state, changes, first)?;
            Ok(Some(update))
        }
        SubKey::Search { account, query } => {
            let update =
                build_search_update(db, sub_id, *account, query, state, changes, clock, first)?;
            Ok(Some(update))
        }
        SubKey::Conversation {
            thread, trash_mode, ..
        } => {
            let data = project_conversation(db, *thread, *trash_mode, rev)?;
            Ok(Some(SubUpdate::Conversation {
                sub: sub_id,
                first,
                data,
            }))
        }
        SubKey::Composer { draft } => {
            let data = project_draft(db, draft, rev)?;
            Ok(Some(SubUpdate::Composer {
                sub: sub_id,
                first,
                draft: data,
            }))
        }
    }
}

fn project_app_state(db: Db<'_>, online: bool) -> Result<AppState, StoreError> {
    let records = db.accounts()?;
    let mut accounts = Vec::with_capacity(records.len());
    for rec in records {
        accounts.push(project_account_info(db, rec)?);
    }
    let remote_images = db.setting_is_on(None, SettingKey::RemoteImages)?;
    Ok(AppState {
        accounts,
        settings: GlobalSettings { remote_images },
        online,
    })
}

fn project_account_info(db: Db<'_>, rec: AccountRecord) -> Result<AccountInfo, StoreError> {
    let inbox_unread = db.inbox_unread(rec.id)?;
    let muted = db.setting_is_on(Some(rec.id), SettingKey::Muted)?;
    let send_as = rec
        .send_as
        .into_iter()
        .map(|s| SendAs {
            email: s.email,
            name: s.display_name,
        })
        .collect();
    Ok(AccountInfo {
        id: rec.id,
        email: rec.email,
        name: rec.display_name,
        hue: avatar_hue(rec.hue).to_owned(),
        inbox_unread,
        needs_signin: rec.needs_signin,
        muted,
        sync_status: rec.sync_status,
        send_as,
    })
}

fn project_account_summary(
    db: Db<'_>,
    account: AccountId,
) -> Result<Option<AccountSummary>, StoreError> {
    let Some(rec) = db.account(account)? else {
        return Ok(None);
    };
    let info = project_account_info(db, rec)?;
    let labels = db
        .labels(account)?
        .into_iter()
        .map(|l| LabelInfo {
            id: l.id,
            name: l.name,
            system: l.system,
        })
        .collect();
    let splits = db
        .splits(account)?
        .into_iter()
        .map(|s| SplitInfo {
            id: s.id,
            name: s.name,
            kind: s.kind,
            query: s.query,
            enabled: s.enabled,
            hide_when_empty: s.hide_when_empty,
        })
        .collect();
    let outbox = db
        .outbox_for_account(account)?
        .into_iter()
        .map(project_outbox_row)
        .collect();
    let undo = db
        .undo_entries(account)?
        .into_iter()
        .map(|u| UndoEntryInfo {
            id: u.id,
            outbox: u.outbox,
        })
        .collect();
    let notices = db
        .notices(account)?
        .into_iter()
        .map(|n| Notice {
            id: n.id,
            kind: n.kind,
            message: n.message,
            draft: n.draft,
            thread: n.thread,
        })
        .collect();
    let queued_ops = db.ops_count(account)?;
    Ok(Some(AccountSummary {
        account: info,
        labels,
        splits,
        outbox,
        undo,
        notices,
        queued_ops,
    }))
}

#[expect(
    clippy::too_many_arguments,
    reason = "list projection compares previous sub state against db and commit changes"
)]
fn build_list_update(
    db: Db<'_>,
    sub_id: SubId,
    account: AccountId,
    view: &View,
    state: &mut SubState,
    changes: &Changes,
    first: bool,
) -> Result<SubUpdate, StoreError> {
    if matches!(view, View::Drafts) {
        let drafts = db.drafts_list(account)?;
        let new_ids: Vec<ListEntry> = drafts
            .iter()
            .map(|d| ListEntry {
                key: d.id.0.clone(),
                split: None,
            })
            .collect();
        let meta = ListMeta::default();
        return Ok(finish_draft_list_update(
            sub_id, state, drafts, new_ids, meta, changes, first,
        ));
    }

    let ids = db.view_threads(account, view)?;
    let remote_set: HashSet<ThreadId> = state.listing.remote_hits.iter().copied().collect();
    let oldest_hit = oldest_hit_key(&ids, &remote_set);
    let new_ids: Vec<ListEntry> = if matches!(view, View::Inbox) {
        ids.iter()
            .map(|(tid, split)| ListEntry {
                key: tid.0.to_string(),
                split: *split,
            })
            .collect()
    } else {
        ids.iter()
            .map(|(tid, _)| ListEntry {
                key: tid.0.to_string(),
                split: None,
            })
            .collect()
    };
    let meta = ListMeta {
        searching: state.listing.searching,
        needs_gmail: false,
        looks_in_trash: matches!(view, View::Trash | View::Spam),
        terms: Vec::new(),
        more: state.listing.more,
        oldest_hit,
    };
    finish_thread_list_update(db, sub_id, state, new_ids, meta, changes, first)
}

#[expect(
    clippy::too_many_arguments,
    reason = "search projection merges local query hits, remote hits, and diff state"
)]
fn build_search_update(
    db: Db<'_>,
    sub_id: SubId,
    account: AccountId,
    query: &str,
    state: &mut SubState,
    changes: &Changes,
    clock: &Clock,
    first: bool,
) -> Result<SubUpdate, StoreError> {
    let parsed = Query::parse_search(query, &clock.local_now());
    let ordered = db.search_threads(account, &parsed, &state.listing.remote_hits)?;
    let remote_set: HashSet<ThreadId> = state.listing.remote_hits.iter().copied().collect();
    let pairs: Vec<(ThreadId, Option<SplitId>)> = ordered.iter().map(|id| (*id, None)).collect();
    let oldest_hit = oldest_hit_key(&pairs, &remote_set);
    let new_ids: Vec<ListEntry> = ordered
        .into_iter()
        .map(|id| ListEntry {
            key: id.0.to_string(),
            split: None,
        })
        .collect();
    let meta = ListMeta {
        searching: state.listing.searching,
        needs_gmail: parsed.needs_gmail(),
        looks_in_trash: parsed.looks_in_trash(),
        terms: parsed.highlight_terms(),
        more: state.listing.more,
        oldest_hit,
    };
    finish_thread_list_update(db, sub_id, state, new_ids, meta, changes, first)
}

#[expect(
    clippy::too_many_arguments,
    reason = "draft list diffing compares previous sub state against current drafts and commit changes"
)]
fn finish_draft_list_update(
    sub_id: SubId,
    state: &mut SubState,
    drafts: Vec<DraftRecord>,
    new_ids: Vec<ListEntry>,
    meta: ListMeta,
    changes: &Changes,
    first: bool,
) -> SubUpdate {
    if first {
        let rows = drafts
            .into_iter()
            .take(INITIAL_ROW_WINDOW)
            .map(draft_to_row)
            .collect();
        state.last_ids.clone_from(&new_ids);
        return SubUpdate::List {
            sub: sub_id,
            first: true,
            ids: Some(new_ids),
            removed: Vec::new(),
            moved: Vec::new(),
            rows,
            searching: meta.searching,
            needs_gmail: meta.needs_gmail,
            looks_in_trash: meta.looks_in_trash,
            terms: meta.terms,
            more: meta.more,
            oldest_hit: meta.oldest_hit,
        };
    }
    let (removed, moved) = diff_list(&state.last_ids, &new_ids);
    let rows = {
        let moved_keys: HashSet<&str> = moved.iter().map(|m| m.key.as_str()).collect();
        drafts
            .into_iter()
            .filter(|d| {
                d.rev > state.last_rev
                    || changes.drafts.contains(&d.id)
                    || moved_keys.contains(d.id.0.as_str())
            })
            .map(draft_to_row)
            .collect()
    };
    state.last_ids = new_ids;
    SubUpdate::List {
        sub: sub_id,
        first: false,
        ids: None,
        removed,
        moved,
        rows,
        searching: meta.searching,
        needs_gmail: meta.needs_gmail,
        looks_in_trash: meta.looks_in_trash,
        terms: meta.terms,
        more: meta.more,
        oldest_hit: meta.oldest_hit,
    }
}

#[expect(
    clippy::too_many_arguments,
    clippy::too_many_lines,
    reason = "thread list diffing builds either first load or minimal move patch with updated rows"
)]
fn finish_thread_list_update(
    db: Db<'_>,
    sub_id: SubId,
    state: &mut SubState,
    new_ids: Vec<ListEntry>,
    meta: ListMeta,
    changes: &Changes,
    first: bool,
) -> Result<SubUpdate, StoreError> {
    if first {
        let initial_tids: Vec<ThreadId> = new_ids
            .iter()
            .take(INITIAL_ROW_WINDOW)
            .filter_map(|e| e.key.parse::<i64>().ok().map(ThreadId))
            .collect();
        let rows = db
            .thread_rows(&initial_tids)?
            .into_iter()
            .map(thread_to_row)
            .collect();
        state.last_ids.clone_from(&new_ids);
        return Ok(SubUpdate::List {
            sub: sub_id,
            first: true,
            ids: Some(new_ids),
            removed: Vec::new(),
            moved: Vec::new(),
            rows,
            searching: meta.searching,
            needs_gmail: meta.needs_gmail,
            looks_in_trash: meta.looks_in_trash,
            terms: meta.terms,
            more: meta.more,
            oldest_hit: meta.oldest_hit,
        });
    }

    let (removed, moved) = diff_list(&state.last_ids, &new_ids);
    let mut fetch_tids: HashSet<ThreadId> = moved
        .iter()
        .filter_map(|m| m.key.parse::<i64>().ok().map(ThreadId))
        .collect();
    let new_tid_set: HashSet<ThreadId> = new_ids
        .iter()
        .filter_map(|e| e.key.parse::<i64>().ok().map(ThreadId))
        .collect();
    for tid in &changes.threads {
        if new_tid_set.contains(tid) {
            fetch_tids.insert(*tid);
        }
    }
    let fetch_vec: Vec<ThreadId> = fetch_tids.into_iter().collect();
    let rows = db
        .thread_rows(&fetch_vec)?
        .into_iter()
        .map(thread_to_row)
        .collect();
    state.last_ids = new_ids;
    Ok(SubUpdate::List {
        sub: sub_id,
        first: false,
        ids: None,
        removed,
        moved,
        rows,
        searching: meta.searching,
        needs_gmail: meta.needs_gmail,
        looks_in_trash: meta.looks_in_trash,
        terms: meta.terms,
        more: meta.more,
        oldest_hit: meta.oldest_hit,
    })
}

fn oldest_hit_key(
    ordered: &[(ThreadId, Option<SplitId>)],
    remote_set: &HashSet<ThreadId>,
) -> Option<String> {
    if remote_set.is_empty() {
        return None;
    }
    ordered
        .iter()
        .rev()
        .find(|(tid, _)| remote_set.contains(tid))
        .map(|(tid, _)| tid.0.to_string())
}

#[expect(
    clippy::too_many_lines,
    reason = "projects a thread conversation with messages, attachments, drafts, and pending outbox items"
)]
fn project_conversation(
    db: Db<'_>,
    thread: ThreadId,
    trash_mode: bool,
    rev: Rev,
) -> Result<ConversationData, StoreError> {
    let Some(view) = db.conversation(thread, trash_mode)? else {
        return Ok(ConversationData {
            thread,
            subject: String::new(),
            messages: Vec::new(),
            drafts: Vec::new(),
            outbox: Vec::new(),
            gone: true,
            rev,
        });
    };
    let identity = db.account(view.thread.account)?.map_or_else(
        || Identity::new(String::new(), String::new(), Vec::new()),
        |a| a.identity(),
    );
    let landed_gids: HashSet<crate::domain::GmailMessageId> = view
        .messages
        .iter()
        .map(|(m, _)| m.gmail_id.clone())
        .collect();
    let mut messages = Vec::with_capacity(view.messages.len());
    for (msg, att_recs) in view.messages {
        let atts = att_recs
            .into_iter()
            .map(|a| AttachmentInfo {
                id: a.id,
                filename: a.filename,
                mime_type: a.mime_type,
                size: a.size,
                inline: a.is_inline,
            })
            .collect();
        messages.push(project_message_summary(msg, &identity, atts));
    }
    let drafts = db
        .drafts_on_thread(thread)?
        .into_iter()
        .map(|d| DraftSummary {
            id: d.id,
            kind: d.kind,
            target: d.target,
            started_on_latest: d.started_on_latest,
            to: d.to,
            snippet: d.snippet,
            updated_at: d.updated_at,
        })
        .collect();
    let outbox = db
        .outbox_for_thread(thread)?
        .into_iter()
        .filter(|o| {
            !(o.item.state == crate::domain::OutboxState::Sent
                && o.item
                    .sent_message_id
                    .as_ref()
                    .is_some_and(|gmid| landed_gids.contains(gmid)))
        })
        .map(project_outbox_row)
        .collect();
    Ok(ConversationData {
        thread,
        subject: view.thread.subject,
        messages,
        drafts,
        outbox,
        gone: false,
        rev: view.thread.rev.max(rev),
    })
}

fn project_message_summary(
    msg: MessageRecord,
    identity: &Identity,
    attachments: Vec<AttachmentInfo>,
) -> MessageSummary {
    let to_addr = |a: crate::domain::Addr| {
        let me = identity.is_mine(&a.email);
        MessageAddr {
            name: a.name,
            email: a.email,
            me,
        }
    };
    MessageSummary {
        id: msg.id,
        from: to_addr(msg.from),
        to: msg.to.into_iter().map(to_addr).collect(),
        cc: msg.cc.into_iter().map(to_addr).collect(),
        bcc: msg.bcc.into_iter().map(to_addr).collect(),
        subject: msg.subject,
        snippet: msg.snippet,
        date: msg.date,
        unread: msg.labels.has(SystemLabel::Unread),
        starred: msg.labels.has(SystemLabel::Starred),
        has_body: msg.has_body,
        attachments,
        rev: msg.rev,
    }
}

#[expect(
    clippy::too_many_lines,
    reason = "projects missing, in-outbox, and editable draft states into DraftState"
)]
fn project_draft(db: Db<'_>, draft: &DraftId, rev: Rev) -> Result<DraftState, StoreError> {
    let Some(rec) = db.draft(draft)? else {
        return Ok(DraftState {
            id: draft.clone(),
            account: AccountId(0),
            kind: crate::domain::DraftKind::New,
            reply_all: false,
            thread: None,
            target: None,
            from: None,
            to: Vec::new(),
            cc: Vec::new(),
            bcc: Vec::new(),
            subject: String::new(),
            body_html: String::new(),
            quote_html: None,
            attachments: Vec::new(),
            gone: true,
            rev,
        });
    };
    if rec.in_outbox {
        return Ok(DraftState {
            id: draft.clone(),
            account: rec.account,
            kind: rec.kind,
            reply_all: rec.reply_all,
            thread: rec.thread,
            target: rec.target,
            from: rec.from,
            to: rec.to,
            cc: rec.cc,
            bcc: rec.bcc,
            subject: rec.subject,
            body_html: rec.body_html,
            quote_html: rec.quote_html,
            attachments: Vec::new(),
            gone: true,
            rev: rec.rev,
        });
    }
    Ok(DraftState {
        id: rec.id,
        account: rec.account,
        kind: rec.kind,
        reply_all: rec.reply_all,
        thread: rec.thread,
        target: rec.target,
        from: rec.from,
        to: rec.to,
        cc: rec.cc,
        bcc: rec.bcc,
        subject: rec.subject,
        body_html: rec.body_html,
        quote_html: rec.quote_html,
        attachments: rec
            .attachments
            .into_iter()
            .map(|a| DraftAttachment {
                id: a.id,
                filename: a.filename,
                mime_type: a.mime_type,
                size: a.size,
                path: a.path,
                source: a.source,
            })
            .collect(),
        gone: false,
        rev: rec.rev,
    })
}

#[must_use]
fn project_outbox_row(rec: OutboxRecord) -> OutboxRow {
    OutboxRow {
        id: rec.item.id,
        account: rec.item.account,
        draft: rec.item.draft,
        thread: rec.item.thread,
        state: rec.item.state,
        undo_requested: rec.item.undo_requested,
        toast_closed: rec.item.toast_closed,
        delayed: rec.item.delayed,
        to: rec.to,
        snippet: rec.snippet,
        subject: rec.subject,
        undo_entry: rec.undo_entry,
    }
}

#[must_use]
fn thread_to_row(row: ThreadRow) -> ListRow {
    let starred = row
        .labels
        .iter()
        .any(|l| l.as_str() == SystemLabel::Starred.id());
    ListRow {
        key: row.id.0.to_string(),
        thread: Some(row.id),
        draft: None,
        sender: row.sender,
        sender_email: row.sender_email,
        subject: row.subject,
        snippet: row.snippet,
        time: row.sort_time,
        unread: row.unread,
        starred,
        has_attachments: row.has_attachments,
        message_count: row.message_count,
        labels: row.labels,
        snooze_until: row.snooze_until,
        split: row.split,
        rev: row.rev,
    }
}

#[must_use]
fn draft_to_row(d: DraftRecord) -> ListRow {
    let sender = if d.to.is_empty() {
        "Draft".to_owned()
    } else {
        d.to.iter()
            .map(|a| {
                a.name
                    .as_deref()
                    .filter(|n| !n.is_empty())
                    .unwrap_or(&a.email)
            })
            .collect::<Vec<_>>()
            .join(", ")
    };
    let sender_email =
        d.to.into_iter()
            .next()
            .map_or_else(String::new, |a| a.email);
    let has_attachments = !d.attachments.is_empty();
    ListRow {
        key: d.id.0.clone(),
        thread: d.thread,
        draft: Some(d.id),
        sender,
        sender_email,
        subject: d.subject,
        snippet: d.snippet,
        time: d.updated_at,
        unread: false,
        starred: false,
        has_attachments,
        message_count: 1,
        labels: Vec::new(),
        snooze_until: None,
        split: None,
        rev: d.rev,
    }
}

pub(crate) fn fetch_rows(
    db: Db<'_>,
    drafts: bool,
    keys: &[String],
) -> Result<Vec<ListRow>, StoreError> {
    if drafts {
        let mut out = Vec::with_capacity(keys.len());
        for k in keys {
            if let Some(d) = db.draft(&DraftId(k.clone()))? {
                out.push(draft_to_row(d));
            }
        }
        return Ok(out);
    }
    let tids: Vec<ThreadId> = keys
        .iter()
        .filter_map(|k| k.parse::<i64>().ok().map(ThreadId))
        .collect();
    Ok(db
        .thread_rows(&tids)?
        .into_iter()
        .map(thread_to_row)
        .collect())
}

#[must_use]
pub fn diff_list(old: &[ListEntry], new: &[ListEntry]) -> (Vec<String>, Vec<ListMove>) {
    let new_keys: HashSet<&str> = new.iter().map(|e| e.key.as_str()).collect();
    let removed: Vec<String> = old
        .iter()
        .filter(|e| !new_keys.contains(e.key.as_str()))
        .map(|e| e.key.clone())
        .collect();

    let old_pos: HashMap<&str, (usize, Option<SplitId>)> = old
        .iter()
        .enumerate()
        .map(|(i, e)| (e.key.as_str(), (i, e.split)))
        .collect();

    let mut pairs: Vec<(usize, usize)> = Vec::new();
    for (new_idx, e) in new.iter().enumerate() {
        if let Some(&(old_idx, old_split)) = old_pos.get(e.key.as_str())
            && old_split == e.split
        {
            pairs.push((old_idx, new_idx));
        }
    }
    let kept = lis_new_indices(&pairs);

    let mut moved = Vec::new();
    for (i, e) in new.iter().enumerate() {
        if !kept.contains(&i) {
            let after = if i == 0 {
                None
            } else {
                Some(new[i - 1].key.clone())
            };
            moved.push(ListMove {
                key: e.key.clone(),
                split: e.split,
                after,
            });
        }
    }
    (removed, moved)
}

fn lis_new_indices(pairs: &[(usize, usize)]) -> HashSet<usize> {
    if pairs.is_empty() {
        return HashSet::new();
    }
    let mut tails: Vec<usize> = Vec::new();
    let mut tail_pair_idx: Vec<usize> = Vec::new();
    let mut prev: Vec<Option<usize>> = vec![None; pairs.len()];

    for (i, &(old_idx, _)) in pairs.iter().enumerate() {
        let pos = tails.partition_point(|&v| v < old_idx);
        if pos > 0 {
            prev[i] = Some(tail_pair_idx[pos - 1]);
        }
        if pos == tails.len() {
            tails.push(old_idx);
            tail_pair_idx.push(i);
        } else {
            tails[pos] = old_idx;
            tail_pair_idx[pos] = i;
        }
    }

    let mut kept = HashSet::with_capacity(tails.len());
    let mut cur = tail_pair_idx.last().copied();
    while let Some(idx) = cur {
        kept.insert(pairs[idx].1);
        cur = prev[idx];
    }
    kept
}

#[cfg(test)]
pub(crate) fn apply_patch(
    old: &[ListEntry],
    removed: &[String],
    moved: &[ListMove],
) -> Vec<ListEntry> {
    let mut drop_set: HashSet<&str> = removed.iter().map(String::as_str).collect();
    for m in moved {
        drop_set.insert(m.key.as_str());
    }
    let mut out: Vec<ListEntry> = old
        .iter()
        .filter(|e| !drop_set.contains(e.key.as_str()))
        .cloned()
        .collect();
    for m in moved {
        let at = match &m.after {
            None => 0,
            Some(after) => out
                .iter()
                .position(|e| e.key == *after)
                .map_or(out.len(), |p| p + 1),
        };
        out.insert(
            at,
            ListEntry {
                key: m.key.clone(),
                split: m.split,
            },
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(k: &str, s: Option<i64>) -> ListEntry {
        ListEntry {
            key: k.to_owned(),
            split: s.map(SplitId),
        }
    }

    #[test]
    fn diff_list_minimizes_moves_and_updates_changed_splits() {
        let old = vec![
            entry("a", Some(1)),
            entry("b", Some(1)),
            entry("c", Some(1)),
            entry("d", Some(2)),
        ];
        let new = vec![
            entry("x", Some(1)),
            entry("a", Some(1)),
            entry("c", Some(2)),
            entry("d", Some(2)),
        ];
        let (removed, moved) = diff_list(&old, &new);
        assert_eq!(removed, vec!["b"]);
        assert_eq!(moved.len(), 2);
        assert_eq!(apply_patch(&old, &removed, &moved), new);
    }
}
