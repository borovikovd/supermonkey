//! The core's state on the writer thread: the `S` of `ub_store::Writer<S>`.

use std::sync::Arc;

use ub_domain::ActionNo;
use ub_ipc::ChannelMessage;

use crate::activity::{Activity, Busy, Tracked};
use crate::effects::{Effect, EffectSender};
use crate::subs::Registry;

/// Where channel messages go: the webview's channel, or the web bridge's
/// event stream. Called on the writer thread after each commit, so it must
/// not block.
pub type ChannelSink = Box<dyn FnMut(ChannelMessage) + Send>;

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
    activity: Arc<Activity>,
    router: EffectSender,
    sink: Option<ChannelSink>,
    /// Whether Gmail is reachable, for the app subscription.
    online: bool,
    /// The `ack` of the last channel message sent.
    pub(crate) sent_ack: ActionNo,
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
    pub(crate) fn new(activity: Arc<Activity>, router: EffectSender) -> Self {
        Self {
            subs: Registry::default(),
            effects: Vec::new(),
            held: Vec::new(),
            activity,
            router,
            sink: None,
            online: true,
            sent_ack: ActionNo(0),
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

    /// Sends channel messages to `sink` from now on.
    pub(crate) fn set_sink(&mut self, sink: ChannelSink) {
        self.sink = Some(sink);
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
        self.held.clear();
    }

    /// Drops what a rolled-back job recorded, but the work of the
    /// subscription changes it made, and lets it go idle.
    pub(crate) fn discard(&mut self) {
        let effects = std::mem::take(&mut self.effects);
        self.send_effects(
            effects
                .into_iter()
                .filter(|recorded| recorded.survives_rollback)
                .map(|recorded| recorded.effect),
        );
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
