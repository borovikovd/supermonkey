use sm_ipc::ChannelMessage;
use sm_store::{Commit, Snapshot};

use super::WriterState;
use crate::effects::Effect;
use crate::subs::affects_badge;

pub(crate) fn on_commit(state: &mut WriterState, snapshot: &Snapshot<'_>, commit: &Commit) {
    let releasing = Releasing(state);
    let state = &mut *releasing.0;
    let ack = snapshot.meta().last_action().unwrap_or_else(|e| {
        tracing::error!(error = %e, "couldn't read the last action number after a commit");
        state.sent_ack
    });
    let online = state.online();
    let now = state.now();
    let updates = state.subs.updates(**snapshot, &commit.changes, online, now);
    let mut rejected = state.take_rejected();
    let ack_moved = ack != state.sent_ack;
    let tells = ack_moved || !rejected.is_empty();
    if !updates.patches.is_empty() || (tells && updates.first_loads.is_empty()) {
        state.send(ChannelMessage {
            rev: commit.rev,
            ack,
            rejected: std::mem::take(&mut rejected),
            updates: updates.patches,
        });
    }
    if !updates.first_loads.is_empty() {
        state.send(ChannelMessage {
            rev: commit.rev,
            ack,
            rejected,
            updates: updates.first_loads,
        });
    }
    state.sent_ack = ack;
    if affects_badge(&commit.changes) {
        state.effect(Effect::Badge);
    }
}

struct Releasing<'s>(&'s mut WriterState);

impl Drop for Releasing<'_> {
    fn drop(&mut self) {
        self.0.release();
    }
}

pub(crate) fn on_rollback(state: &mut WriterState) {
    state.discard();
}
