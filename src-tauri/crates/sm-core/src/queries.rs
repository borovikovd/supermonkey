//! The `Queries` handle: plain reads for the UI's lookups, on the read pool
//! (SPEC UI state, Ordering). Each returns rows with their `rev` and never
//! changes pending actions.

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

/// How many suggestions autocomplete gives.
const SUGGESTIONS: usize = 6;

/// Reads for the UI's lookups: rows outside the pushed window, bodies,
/// autocomplete, snooze times, and a sender's threads.
pub struct Queries {
    pool: Arc<ReadPool>,
    clock: Arc<dyn Clock>,
}

/// Its name only: it holds the read pool and the clock.
impl fmt::Debug for Queries {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Queries").finish_non_exhaustive()
    }
}

impl Queries {
    pub(crate) fn new(pool: Arc<ReadPool>, clock: Arc<dyn Clock>) -> Self {
        Self { pool, clock }
    }

    /// Runs `read` on a pool connection, off the async threads.
    async fn read<T: Send + 'static>(
        &self,
        read: impl FnOnce(&ReadTx<'_>) -> Result<T, StoreError> + Send + 'static,
    ) -> Result<T, Error> {
        crate::read::read(&self.pool, read).await
    }

    /// Rows for list keys, fetched ahead of scrolling: draft ids for the
    /// Drafts list, thread ids for every other. Unknown keys are left out.
    pub async fn fetch_rows(&self, drafts: bool, keys: Vec<String>) -> Result<Vec<ListRow>, Error> {
        self.read(move |r| views::rows_for_keys(**r, drafts, &keys))
            .await
    }

    /// Sanitized bodies, in the order asked. A message with no stored body
    /// is left out; plain-text mail comes as HTML.
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

    /// Recipient suggestions for what the user typed (SPEC Recipients and
    /// autocomplete): matching contacts the account may suggest, best first.
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

    /// The snooze picker's rows for what the user typed, in the local time
    /// zone at app time: the time the text names, then the fixed choices.
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

    /// Inbox threads with a message from the sender of `thread`'s latest
    /// message, `thread` included: what Unsubscribe's "all" choices act on.
    pub async fn sender_threads(
        &self,
        account: AccountId,
        thread: ThreadId,
    ) -> Result<Vec<ThreadId>, Error> {
        self.read(move |r| r.threads().sender_inbox_threads(account, thread))
            .await
    }

    /// Gmail's id for a thread, if the app has the thread.
    pub async fn gmail_thread(&self, thread: ThreadId) -> Result<Option<GmailThreadId>, Error> {
        self.read(move |r| Ok(r.threads().row(thread)?.map(|row| row.gmail_id)))
            .await
    }
}
