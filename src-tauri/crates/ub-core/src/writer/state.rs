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
    /// Effects the running job recorded; released after its commit, dropped
    /// on rollback.
    effects: Vec<Tracked<Effect>>,
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

    /// Records an effect, released once this job commits. Recording the
    /// same effect twice in one job releases it once.
    pub(crate) fn effect(&mut self, effect: Effect) {
        if !self.effects.iter().any(|e| *e.get() == effect) {
            self.effects.push(self.activity.track("effect", effect));
        }
    }

    /// Where the effects recorded so far end, for [`WriterState::drop_effects_from`].
    pub(crate) fn effects_mark(&self) -> usize {
        self.effects.len()
    }

    /// Drops the effects recorded since `mark`: the step that recorded them
    /// rolled back.
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
    pub(crate) fn release(&mut self) {
        for effect in self.effects.drain(..) {
            // The router is gone only while the core shuts down; the effect
            // has nowhere to go then.
            let _ = self.router.send(effect);
        }
        self.held.clear();
    }

    /// Drops what a rolled-back job recorded, and lets it go idle.
    pub(crate) fn discard(&mut self) {
        self.effects.clear();
        self.held.clear();
    }
}
