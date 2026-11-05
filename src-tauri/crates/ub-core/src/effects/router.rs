//! The effect router: one task that hands each committed effect to the
//! worker that does it.

use std::sync::Arc;

use tokio::sync::mpsc;

use super::Effect;
use crate::activity::{Kick, Tracked, parked};
use crate::workers::{AccountWorkers, Notifier, Searches};

/// Where the writer sends committed effects.
pub(crate) type EffectSender = mpsc::UnboundedSender<Tracked<Effect>>;
/// The router's end.
pub(crate) type EffectReceiver = mpsc::UnboundedReceiver<Tracked<Effect>>;

/// What the router hands effects to.
pub(crate) struct Router {
    pub(crate) workers: Arc<AccountWorkers>,
    pub(crate) searches: Searches,
    pub(crate) notifier: Arc<Notifier>,
    /// Wakes the snooze timer.
    pub(crate) snoozes: Arc<Kick>,
    /// Wakes the badge count.
    pub(crate) badge: Arc<Kick>,
}

impl Router {
    /// Routes effects until every sender is gone. Each effect is busy until
    /// it is handled; tasks it starts count on their own.
    pub(crate) async fn run(self, mut effects: EffectReceiver) {
        while let Some(tracked) = parked(effects.recv()).await {
            let (effect, _handling) = tracked.into_parts();
            self.route(effect);
        }
    }

    fn route(&self, effect: Effect) {
        match effect {
            Effect::Badge => self.badge.kick(),
            Effect::FetchThread { account, thread } => {
                if let Some(worker) = self.workers.get(account) {
                    worker.fetch_opened(thread);
                }
            }
            Effect::ThreadUnneeded(thread) => self.workers.release_opened(thread),
            Effect::GmailList {
                sub,
                account,
                query,
                page,
            } => self
                .searches
                .start(&self.workers, sub, account, query, page),
            Effect::CancelSub(sub) => self.searches.cancel(&sub),
            Effect::Ops(account) => {
                if let Some(worker) = self.workers.get(account) {
                    worker.kick_ops();
                }
            }
            Effect::RefreshLabels(account) => {
                if let Some(worker) = self.workers.get(account) {
                    worker.refresh_labels();
                }
            }
            Effect::AccountRemoved { account, email } => self.workers.forget(account, email),
            Effect::Outbox(account) => {
                if let Some(worker) = self.workers.get(account) {
                    worker.kick_outbox();
                }
            }
            Effect::DraftParts { account, draft } => {
                if let Some(worker) = self.workers.get(account) {
                    worker.download_parts(draft);
                }
            }
            Effect::ReadDraft { account, draft } => {
                if let Some(worker) = self.workers.get(account) {
                    worker.read_draft(draft);
                }
            }
            Effect::DraftFilesGone(draft) => self.workers.delete_draft_files(&draft),
            Effect::Notify(notification) => self.notifier.show(notification),
            Effect::Snoozes => self.snoozes.kick(),
        }
    }
}
