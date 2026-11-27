use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;

use crate::domain::contacts::{contact_matches, is_excluded, rank};
use crate::domain::snooze::snooze_picker;
use crate::domain::{AccountId, AttachmentId, DraftId, MessageId, ThreadId};
use crate::ipc::{
    BlockResult, BlockedSenders, ContactSuggestion, DraftAttachment, LaunchInfo, ListRow,
    MessageBody, SnoozeChoice, Submitted, Unsubscribed,
};
use crate::store::{Priority as StorePriority, StoreError};

use super::actions::apply_batch;
use super::attachments::open_attachment;
use super::drafts::MAX_ATTACHMENT_BYTES;
use super::subs::fetch_rows;
use super::unwanted::{
    block_sender, list_blocked_senders, sender_threads, unblock_sender, unsubscribe_thread,
};
use super::{Core, CoreError};

const MAX_AUTOCOMPLETE: usize = 6;

impl Core {
    pub fn connect(&self) -> Result<LaunchInfo, CoreError> {
        #[expect(
            clippy::redundant_closure_for_method_calls,
            reason = "Db carries a lifetime so a closure is needed for ReadPool::with HRTB"
        )]
        let last_action = self
            .pool
            .with(|db| db.last_action())
            .map_err(|e| CoreError::internal(e.to_string()))?;
        let max_attachment_bytes = u32::try_from(MAX_ATTACHMENT_BYTES).unwrap_or(u32::MAX);
        Ok(LaunchInfo {
            last_action,
            debug: cfg!(debug_assertions),
            max_attachment_bytes,
        })
    }

    pub async fn submit(&self, actions: Vec<Submitted>) -> Result<(), CoreError> {
        self.writer
            .call(StorePriority::Ui, move |tx, state| {
                apply_batch(tx, state, actions)
            })
            .await
            .map_err(|e| CoreError::internal(e.to_string()))
    }

    pub fn fetch_rows(&self, drafts: bool, keys: Vec<String>) -> Result<Vec<ListRow>, CoreError> {
        self.pool
            .with(|db| fetch_rows(db, drafts, keys))
            .map_err(|e| CoreError::internal(e.to_string()))
    }

    pub fn get_bodies(&self, ids: &[MessageId]) -> Result<Vec<MessageBody>, CoreError> {
        self.pool
            .with(|db| {
                let rows = db.message_bodies(ids)?;
                Ok::<_, StoreError>(
                    rows.into_iter()
                        .map(|r| MessageBody {
                            id: r.id,
                            html: r.html,
                            quote_html: r.quote_html,
                        })
                        .collect(),
                )
            })
            .map_err(|e| CoreError::internal(e.to_string()))
    }

    pub fn autocomplete(
        &self,
        account: AccountId,
        query: &str,
    ) -> Result<Vec<ContactSuggestion>, CoreError> {
        let trimmed = query.trim();
        if trimmed.is_empty() {
            return Ok(Vec::new());
        }
        let now = self.clock.now();
        self.pool
            .with(|db| {
                let Some(acct) = db.account(account)? else {
                    return Ok::<_, StoreError>(Vec::new());
                };
                let identity = acct.into_identity();
                let candidates = db.contact_candidates(account, trimmed)?;
                let mut scored = Vec::new();
                for stats in candidates {
                    if is_excluded(&stats, &identity) {
                        continue;
                    }
                    let Some(span) = contact_matches(&stats.name, &stats.email, trimmed) else {
                        continue;
                    };
                    let r = rank(&stats, now);
                    scored.push((r, stats, span));
                }
                scored.sort_by(|a, b| a.0.cmp_desc(b.0).then_with(|| a.1.email.cmp(&b.1.email)));
                scored.truncate(MAX_AUTOCOMPLETE);
                Ok(scored
                    .into_iter()
                    .map(|(_, stats, span)| ContactSuggestion {
                        name: stats.name,
                        email: stats.email,
                        match_field: span.field.as_str().to_owned(),
                        match_start: span.start,
                        match_end: span.end,
                    })
                    .collect())
            })
            .map_err(|e| CoreError::internal(e.to_string()))
    }

    pub async fn add_account(&self) -> Result<AccountId, CoreError> {
        self.accounts.add_account(self.platform.as_ref()).await
    }

    pub async fn sign_in_again(&self, account: AccountId) -> Result<(), CoreError> {
        self.accounts
            .sign_in_again(account, self.platform.as_ref())
            .await
    }

    pub async fn open_attachment(&self, attachment: AttachmentId) -> Result<(), CoreError> {
        open_attachment(
            attachment,
            &self.data_dir,
            &self.accounts,
            &self.pool,
            &self.writer,
            &self.clock,
            self.platform.as_ref(),
        )
        .await
    }

    pub fn open_url(&self, url: &str) -> Result<(), CoreError> {
        let parsed =
            url::Url::parse(url).map_err(|err| CoreError::other(format!("Invalid URL: {err}")))?;
        if !matches!(parsed.scheme(), "http" | "https" | "mailto") {
            return Err(CoreError::other("Unsupported link scheme"));
        }
        self.platform.open_url(parsed.as_str())
    }

    pub async fn unsubscribe(
        &self,
        account: AccountId,
        thread: ThreadId,
    ) -> Result<Unsubscribed, CoreError> {
        unsubscribe_thread(
            account,
            thread,
            &self.accounts,
            &self.pool,
            &self.http,
            &self.clock,
            self.platform.as_ref(),
        )
        .await
    }

    pub async fn block(
        &self,
        account: AccountId,
        thread: ThreadId,
        domain: bool,
    ) -> Result<BlockResult, CoreError> {
        block_sender(account, thread, domain, &self.accounts, &self.pool).await
    }

    pub async fn blocked_senders(&self, account: AccountId) -> Result<BlockedSenders, CoreError> {
        list_blocked_senders(account, &self.accounts).await
    }

    pub async fn unblock(&self, account: AccountId, id: &str) -> Result<(), CoreError> {
        unblock_sender(account, id, &self.accounts).await
    }

    pub fn sender_threads(
        &self,
        account: AccountId,
        thread: ThreadId,
    ) -> Result<Vec<ThreadId>, CoreError> {
        sender_threads(account, thread, &self.pool)
    }

    pub async fn add_attachment(
        &self,
        draft: DraftId,
        filename: String,
        mime_type: String,
        data: &str,
    ) -> Result<DraftAttachment, CoreError> {
        let bytes = STANDARD
            .decode(data.trim())
            .map_err(|err| CoreError::other(format!("Couldn't read attachment data: {err}")))?;
        if bytes.len() > MAX_ATTACHMENT_BYTES {
            return Err(CoreError::other("Attachment exceeds the 25 MB limit"));
        }
        let existing = self
            .pool
            .with(|db| db.draft(&draft))
            .map_err(|e| CoreError::internal(e.to_string()))?;
        let Some(rec) = existing else {
            return Err(CoreError::draft_gone());
        };
        if rec.in_outbox {
            return Err(CoreError::draft_gone());
        }

        let (att_id, path) = self
            .files
            .store_attachment(&draft, &filename, &bytes)
            .map_err(|e| CoreError::internal(e.to_string()))?;
        let path_str = path.to_string_lossy().into_owned();
        let size = i64::try_from(bytes.len()).unwrap_or(i64::MAX);

        let still_exists = self
            .writer
            .call(StorePriority::Ui, move |tx, _state| {
                Ok::<_, StoreError>(tx.read().draft(&draft)?.is_some_and(|d| !d.in_outbox))
            })
            .await
            .map_err(|e| CoreError::internal(e.to_string()))?;
        if !still_exists {
            super::log_err("remove unattached file", std::fs::remove_file(&path));
            return Err(CoreError::draft_gone());
        }

        Ok(DraftAttachment {
            id: att_id,
            filename,
            mime_type,
            size,
            path: Some(path_str),
            source: None,
        })
    }

    pub fn snooze_options(&self, text: &str) -> Vec<SnoozeChoice> {
        snooze_picker(text, &self.clock.local_now())
            .into_iter()
            .map(|p| SnoozeChoice {
                label: p.label,
                at: p.at,
            })
            .collect()
    }
}
