use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use sm_domain::{AccountId, ActionNo, DraftId, Millis};
use sm_ipc::{ChannelMessage, RejectReason, Rejection};

use crate::actions::NOT_SAVED;
use crate::activity::{Activity, Busy, Tracked};
use crate::clock::Clock;
use crate::effects::{Effect, EffectSender};
use crate::subs::Registry;

pub(crate) struct WriterState {
    pub(crate) subs: Registry,
    effects: Vec<Recorded>,
    held: Vec<Busy>,
    undo: Vec<Box<dyn FnOnce() + Send>>,
    activity: Arc<Activity>,
    router: EffectSender,
    sink: Option<Box<dyn FnMut(ChannelMessage) + Send>>,
    online: bool,
    clock: Arc<dyn Clock>,
    pub(crate) sent_ack: ActionNo,
    batch: Vec<ActionNo>,
    rejected: Vec<Rejection>,
    gone_drafts: HashMap<AccountId, HashSet<DraftId>>,
    #[cfg(test)]
    pub(crate) fail_next_commit: bool,
}

struct Recorded {
    effect: Tracked<Effect>,
    survives_rollback: bool,
}

impl WriterState {
    pub(crate) fn new(
        activity: Arc<Activity>,
        router: EffectSender,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            subs: Registry::default(),
            effects: Vec::new(),
            held: Vec::new(),
            undo: Vec::new(),
            activity,
            router,
            sink: None,
            online: true,
            clock,
            sent_ack: ActionNo(0),
            batch: Vec::new(),
            rejected: Vec::new(),
            gone_drafts: HashMap::new(),
            #[cfg(test)]
            fail_next_commit: false,
        }
    }

    pub(crate) fn effect(&mut self, effect: Effect) {
        self.record(effect, false);
    }

    pub(crate) fn sub_effect(&mut self, effect: Effect) {
        self.record(effect, true);
    }

    fn record(&mut self, effect: Effect, survives_rollback: bool) {
        self.effects.push(Recorded {
            effect: self.activity.track("effect", effect),
            survives_rollback,
        });
    }

    pub(crate) fn draft_gone(&mut self, account: AccountId, draft: DraftId) {
        self.gone_drafts.entry(account).or_default().insert(draft);
    }

    pub(crate) fn is_draft_gone(&self, account: AccountId, draft: &DraftId) -> bool {
        self.gone_drafts
            .get(&account)
            .is_some_and(|gone| gone.contains(draft))
    }

    pub(crate) const fn effects_mark(&self) -> usize {
        self.effects.len()
    }

    pub(crate) fn drop_effects_from(&mut self, mark: usize) {
        self.effects.truncate(mark);
    }

    pub(crate) fn now(&self) -> Millis {
        self.clock.now()
    }

    pub(crate) const fn online(&self) -> bool {
        self.online
    }

    pub(crate) const fn set_online(&mut self, online: bool) {
        if self.online != online {
            self.online = online;
            self.subs.force_app();
        }
    }

    pub(crate) fn set_sink(&mut self, sink: Box<dyn FnMut(ChannelMessage) + Send>) {
        self.sink = Some(sink);
        self.rejected.clear();
    }

    pub(crate) fn note_action(&mut self, n: ActionNo) {
        self.batch.push(n);
    }

    pub(crate) fn refuse(&mut self, rejection: Rejection) {
        self.rejected.push(rejection);
    }

    pub(crate) fn take_rejected(&mut self) -> Vec<Rejection> {
        self.batch.clear();
        std::mem::take(&mut self.rejected)
    }

    fn refuse_batch(&mut self) {
        for n in std::mem::take(&mut self.batch) {
            if !self.rejected.iter().any(|r| r.n == n) {
                self.rejected.push(Rejection {
                    n,
                    message: NOT_SAVED.to_owned(),
                    reason: RejectReason::Other,
                });
            }
        }
    }

    pub(crate) fn undo_on_rollback(&mut self, undo: impl FnOnce() + Send + 'static) {
        self.undo.push(Box::new(undo));
    }

    pub(crate) fn hold(&mut self, busy: Busy) {
        self.held.push(busy);
    }

    pub(crate) fn send(&mut self, message: ChannelMessage) {
        if let Some(sink) = &mut self.sink {
            sink(message);
        }
    }

    pub(crate) fn release(&mut self) {
        let effects = std::mem::take(&mut self.effects);
        self.send_effects(effects.into_iter().map(|recorded| recorded.effect));
        self.undo.clear();
        self.held.clear();
    }

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

    fn send_effects(&self, effects: impl DoubleEndedIterator<Item = Tracked<Effect>>) {
        let mut last: Vec<Tracked<Effect>> = Vec::new();
        for effect in effects.rev() {
            if !last.iter().any(|kept| kept.get() == effect.get()) {
                last.push(effect);
            }
        }
        for effect in last.into_iter().rev() {
            _ = self.router.send(effect);
        }
    }
}
