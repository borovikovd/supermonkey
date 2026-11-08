//! The core's state on the writer thread: the `S` of `sm_store::Writer<S>`.

use std::collections::HashSet;
use std::sync::Arc;

use sm_domain::{AccountId, ActionNo, DraftId, Millis};
use sm_ipc::{ChannelMessage, Rejection};

use crate::actions::NOT_SAVED;
use crate::activity::{Activity, Busy, Tracked};
use crate::clock::Clock;
use crate::effects::{Effect, EffectSender};
use crate::subs::Registry;

/// What the writer thread owns besides the database. Jobs and the commit
/// hooks reach it without a lock.
pub(crate) struct WriterState {
    /// The open subscriptions.
    pub(crate) subs: Registry,
    /// Effects the running job recorded; released after its commit. On
    /// rollback only those of subscription changes go out.
    effects: Vec<Recorded>,
    /// The running job's activity, held until its channel messages and
    /// effects are out.
    held: Vec<Busy>,
    /// Undoes what the running job changed outside the database, if it
    /// rolls back.
    undo: Vec<Box<dyn FnOnce() + Send>>,
    activity: Arc<Activity>,
    router: EffectSender,
    /// Where channel messages go: the webview's channel, or the web
    /// bridge's event stream. Called on the writer thread after each
    /// commit, so it must not block.
    sink: Option<Box<dyn FnMut(ChannelMessage) + Send>>,
    /// Whether Gmail is reachable, for the app subscription.
    online: bool,
    /// App time.
    clock: Arc<dyn Clock>,
    /// The `ack` of the last channel message sent.
    pub(crate) sent_ack: ActionNo,
    /// The running action batch's numbers, applied or refused: if the batch
    /// rolls back, each one not refused yet is refused then.
    batch: Vec<ActionNo>,
    /// Refusals the page has not heard of: they go with the next channel
    /// message, which is never later than the first whose `ack` covers them.
    rejected: Vec<Rejection>,
    /// Local drafts discarded or sent this run, by account: a Gmail copy
    /// that names one is a stray, never a draft to bring back. Kept on the
    /// writer thread, so a reconcile job after the discard's commit sees it.
    /// An account removed and added again gets a new id, so its own drafts
    /// are still read back.
    gone_drafts: HashSet<(AccountId, DraftId)>,
    /// Tests: the next action batch's commit fails.
    #[cfg(test)]
    pub(crate) fail_next_commit: bool,
}

/// An effect a job recorded.
struct Recorded {
    effect: Tracked<Effect>,
    /// The work a subscription change asks for: the change stays in the
    /// registry when the job rolls back, so this goes out either way.
    survives_rollback: bool,
}

impl WriterState {
    pub(crate) fn new(
        activity: Arc<Activity>,
        router: EffectSender,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            clock,
            subs: Registry::default(),
            effects: Vec::new(),
            held: Vec::new(),
            undo: Vec::new(),
            activity,
            router,
            sink: None,
            online: true,
            sent_ack: ActionNo(0),
            batch: Vec::new(),
            rejected: Vec::new(),
            gone_drafts: HashSet::new(),
            #[cfg(test)]
            fail_next_commit: false,
        }
    }

    /// Records an effect, released once this job commits. An effect
    /// recorded twice in one job is released once, in the place of its last
    /// recording (see [`WriterState::release`]).
    pub(crate) fn effect(&mut self, effect: Effect) {
        self.record(effect, false);
    }

    /// Records the work a subscription opened, closed, or paged asks for: a
    /// listing, a thread's bodies, a draft's parts, or stopping them. The
    /// registry keeps the change even if the job rolls back, so this goes
    /// out then too; dropped, the subscription would wait for it forever.
    pub(crate) fn sub_effect(&mut self, effect: Effect) {
        self.record(effect, true);
    }

    fn record(&mut self, effect: Effect, survives_rollback: bool) {
        self.effects.push(Recorded {
            effect: self.activity.track("effect", effect),
            survives_rollback,
        });
    }

    /// The local draft was discarded or sent. Kept even if the job rolls
    /// back: it is read only for a draft no longer stored, so a stale entry
    /// changes nothing.
    pub(crate) fn draft_gone(&mut self, account: AccountId, draft: DraftId) {
        self.gone_drafts.insert((account, draft));
    }

    /// Whether the local draft was discarded or sent this run.
    pub(crate) fn is_draft_gone(&self, account: AccountId, draft: &DraftId) -> bool {
        self.gone_drafts.contains(&(account, draft.clone()))
    }

    /// Where the effects recorded so far end, for [`WriterState::drop_effects_from`].
    pub(crate) fn effects_mark(&self) -> usize {
        self.effects.len()
    }

    /// Drops the effects recorded since `mark`: the step that recorded them
    /// was rejected. A subscription action is rejected only before it
    /// changes the registry, so none of its work is lost here.
    pub(crate) fn drop_effects_from(&mut self, mark: usize) {
        self.effects.truncate(mark);
    }

    /// App time now, for searches that count back from it.
    pub(crate) fn now(&self) -> Millis {
        self.clock.now()
    }

    /// Whether Gmail is reachable.
    pub(crate) fn online(&self) -> bool {
        self.online
    }

    /// Records whether Gmail is reachable; the app subscription shows it.
    pub(crate) fn set_online(&mut self, online: bool) {
        if self.online != online {
            self.online = online;
            self.subs.force_app();
        }
    }

    /// Sends channel messages to `sink` from now on. The new page never sent
    /// the actions refused for the one before.
    pub(crate) fn set_sink(&mut self, sink: Box<dyn FnMut(ChannelMessage) + Send>) {
        self.sink = Some(sink);
        self.rejected.clear();
    }

    /// The running batch applies or refuses action `n`.
    pub(crate) fn note_action(&mut self, n: ActionNo) {
        self.batch.push(n);
    }

    /// The running batch refused an action; the page hears why with the
    /// next channel message.
    pub(crate) fn refuse(&mut self, rejection: Rejection) {
        self.rejected.push(rejection);
    }

    /// The refusals to send with this commit's message, taken.
    pub(crate) fn take_rejected(&mut self) -> Vec<Rejection> {
        self.batch.clear();
        std::mem::take(&mut self.rejected)
    }

    /// The running job rolled back. If it was an action batch, none of its
    /// actions was applied: each one not refused already is refused now, and
    /// the next message says so, before or with the one whose `ack` moves
    /// past them.
    fn refuse_batch(&mut self) {
        for n in std::mem::take(&mut self.batch) {
            if !self.rejected.iter().any(|r| r.n == n) {
                self.rejected.push(Rejection {
                    n,
                    message: NOT_SAVED.to_owned(),
                    split: None,
                });
            }
        }
    }

    /// Runs `undo` if the running job rolls back: it changed something
    /// outside the database that must go back with it.
    pub(crate) fn undo_on_rollback(&mut self, undo: impl FnOnce() + Send + 'static) {
        self.undo.push(Box::new(undo));
    }

    /// Keeps the running job busy until its commit's messages and effects are out.
    pub(crate) fn hold(&mut self, busy: Busy) {
        self.held.push(busy);
    }

    /// Sends a channel message, if a page is connected.
    pub(crate) fn send(&mut self, message: ChannelMessage) {
        if let Some(sink) = &mut self.sink {
            sink(message);
        }
    }

    /// Hands the job's effects to the router, and lets the job go idle.
    ///
    /// An effect recorded more than once goes out once, where it was last
    /// recorded: a later effect may undo an earlier one, and the last word
    /// must stay last. A conversation opened, closed, and opened again in
    /// one batch asks for its bodies, then says they are unneeded, then asks
    /// again; keeping the first ask would end with the read stopped.
    pub(crate) fn release(&mut self) {
        let effects = std::mem::take(&mut self.effects);
        self.send_effects(effects.into_iter().map(|recorded| recorded.effect));
        self.undo.clear();
        self.held.clear();
    }

    /// Drops what a rolled-back job recorded, but the work of the
    /// subscription changes it made and the refusal of its actions, and lets
    /// it go idle.
    pub(crate) fn discard(&mut self) {
        self.refuse_batch();
        let effects = std::mem::take(&mut self.effects);
        self.send_effects(
            effects
                .into_iter()
                .filter(|recorded| recorded.survives_rollback)
                .map(|recorded| recorded.effect),
        );
        for undo in self.undo.drain(..).rev() {
            undo();
        }
        self.held.clear();
    }

    /// Sends effects to the router in order, each once, where it last was.
    fn send_effects(&self, effects: impl DoubleEndedIterator<Item = Tracked<Effect>>) {
        let mut last: Vec<Tracked<Effect>> = Vec::new();
        for effect in effects.rev() {
            if !last.iter().any(|kept| kept.get() == effect.get()) {
                last.push(effect);
            }
        }
        for effect in last.into_iter().rev() {
            // The router is gone only while the core shuts down; the effect
            // has nowhere to go then.
            let _ = self.router.send(effect);
        }
    }
}
