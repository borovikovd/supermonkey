use std::fmt;
use std::sync::Arc;

use chrono::{DateTime, Local};
use sm_domain::contacts::{MatchField, contact_matches};
use sm_domain::snooze::snooze_picker;
use sm_domain::{AccountId, GmailThreadId, MessageId, ThreadId};
use sm_ipc::{ContactSuggestion, ListRow, MessageBody, SnoozeChoice};
use sm_store::{ReadPool, ReadTx, StoreError};

use crate::clock::Clock;
use crate::{Error, views};

const SUGGESTIONS: usize = 6;

pub struct Queries {
    pool: Arc<ReadPool>,
    clock: Arc<dyn Clock>,
}

impl fmt::Debug for Queries {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Queries").finish_non_exhaustive()
    }
}

impl Queries {
    pub(crate) fn new(pool: Arc<ReadPool>, clock: Arc<dyn Clock>) -> Self {
        Self { pool, clock }
    }

    async fn read<T: Send + 'static>(
        &self,
        read: impl FnOnce(&ReadTx<'_>) -> Result<T, StoreError> + Send + 'static,
    ) -> Result<T, Error> {
        crate::read::read(&self.pool, read).await
    }

    pub async fn fetch_rows(&self, drafts: bool, keys: Vec<String>) -> Result<Vec<ListRow>, Error> {
        self.read(move |r| views::rows_for_keys(**r, drafts, &keys))
            .await
    }

    pub async fn get_bodies(&self, ids: Vec<MessageId>) -> Result<Vec<MessageBody>, Error> {
        self.read(move |r| {
            Ok(r.messages()
                .bodies(&ids)?
                .into_iter()
                .map(views::message_body)
                .collect())
        })
        .await
    }

    pub async fn autocomplete(
        &self,
        account: AccountId,
        query: String,
    ) -> Result<Vec<ContactSuggestion>, Error> {
        if query.trim().is_empty() {
            return Ok(Vec::new());
        }
        let now = self.clock.now();
        self.read(move |r| {
            let Some(identity) = r.accounts().identity(account)? else {
                return Ok(Vec::new());
            };
            let mut ranked = Vec::new();
            for contact in r.contacts().candidates(account, &query)? {
                if !contact
                    .stats
                    .is_suggestible(&contact.email, identity.is_mine(&contact.email))
                {
                    continue;
                }
                let Some(span) = contact_matches(&query, &contact.name, &contact.email) else {
                    continue;
                };
                let rank = contact.stats.rank(now);
                ranked.push((
                    rank,
                    ContactSuggestion {
                        match_field: match span.field {
                            MatchField::Name => "name",
                            MatchField::Email => "email",
                        }
                        .to_owned(),
                        match_start: u32::try_from(span.start).unwrap_or(u32::MAX),
                        match_end: u32::try_from(span.end).unwrap_or(u32::MAX),
                        name: contact.name,
                        email: contact.email,
                    },
                ));
            }
            ranked.sort_by(|(a, _), (b, _)| {
                a.tier
                    .cmp(&b.tier)
                    .then_with(|| b.score.total_cmp(&a.score))
            });
            Ok(ranked
                .into_iter()
                .take(SUGGESTIONS)
                .map(|(_, suggestion)| suggestion)
                .collect())
        })
        .await
    }

    pub fn snooze_options(&self, text: &str) -> Vec<SnoozeChoice> {
        let Some(now) = DateTime::from_timestamp_millis(self.clock.now().0) else {
            tracing::warn!("app time is out of the calendar's range");
            return Vec::new();
        };
        snooze_picker(text, &now.with_timezone(&Local))
            .into_iter()
            .map(|pick| SnoozeChoice {
                label: pick.label,
                at: pick.at,
            })
            .collect()
    }

    pub async fn sender_threads(
        &self,
        account: AccountId,
        thread: ThreadId,
    ) -> Result<Vec<ThreadId>, Error> {
        self.read(move |r| r.threads().sender_inbox_threads(account, thread))
            .await
    }

    pub async fn gmail_thread(&self, thread: ThreadId) -> Result<Option<GmailThreadId>, Error> {
        self.read(move |r| Ok(r.threads().row(thread)?.map(|row| row.gmail_id)))
            .await
    }
}
