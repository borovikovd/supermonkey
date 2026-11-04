//! The effect router: one task that hands each committed effect to the
//! worker that does it.

use std::sync::Arc;

use tokio::sync::mpsc;
use ub_store::ReadPool;

use super::Effect;
use crate::Platform;
use crate::activity::{Tracked, parked};
use crate::views;

/// Where the writer sends committed effects.
pub(crate) type EffectSender = mpsc::UnboundedSender<Tracked<Effect>>;
/// The router's end.
pub(crate) type EffectReceiver = mpsc::UnboundedReceiver<Tracked<Effect>>;

/// What the router needs to do the effects it handles itself.
pub(crate) struct Router {
    pub(crate) pool: Arc<ReadPool>,
    pub(crate) platform: Arc<dyn Platform>,
}

impl Router {
    /// Routes effects until every sender is gone. Each effect is busy until
    /// it is handled; tasks it starts count on their own.
    pub(crate) async fn run(self, mut effects: EffectReceiver) {
        while let Some(tracked) = parked(effects.recv()).await {
            let (effect, _handling) = tracked.into_parts();
            self.route(effect).await;
        }
    }

    async fn route(&self, effect: Effect) {
        match effect {
            Effect::Badge => self.badge().await,
            // Extension points: the sync slice fetches threads and orders
            // requests by the account on screen, the search slice runs and
            // cancels Gmail listings, and the compose slice downloads draft
            // parts and deletes draft files.
            Effect::FetchThread { .. }
            | Effect::GmailList { .. }
            | Effect::CancelSub(_)
            | Effect::DraftParts { .. }
            | Effect::DraftFilesGone(_)
            | Effect::OnScreen(_) => {
                tracing::debug!(effect = effect.name(), "no worker for this effect yet");
            }
        }
    }

    /// The Dock badge: unread Inbox threads across accounts not muted.
    async fn badge(&self) {
        let pool = Arc::clone(&self.pool);
        let count =
            tokio::task::spawn_blocking(move || pool.with(|r| views::badge_count(**r))).await;
        match count {
            Ok(Ok(count)) => self.platform.set_badge((count > 0).then_some(count)),
            Ok(Err(e)) => tracing::warn!(error = %e, "couldn't count unread mail for the badge"),
            Err(e) => tracing::warn!(error = %e, "the badge count task failed"),
        }
    }
}
