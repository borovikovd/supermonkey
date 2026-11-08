//! What happens after each writer job (SPEC UI state, Ordering): one channel
//! message for the commit, the first loads as their own message, then the
//! effects. A job that rolled back sends nothing and its effects go.

use sm_ipc::ChannelMessage;
use sm_store::{Commit, Snapshot};

use super::WriterState;
use crate::effects::Effect;
use crate::subs::affects_badge;

/// `on_commit`: builds the commit's updates from its snapshot and sends
/// them, then releases the job's effects. The commit has landed, so they go
/// out and the job goes idle even if building or sending the updates
/// panics; held, they would keep the core busy and `settle()` waiting.
pub(crate) fn on_commit(state: &mut WriterState, snapshot: &Snapshot<'_>, commit: &Commit) {
    let releasing = Releasing(state);
    let state = &mut *releasing.0;
    let ack = match snapshot.meta().last_action() {
        Ok(ack) => ack,
        Err(e) => {
            tracing::error!(error = %e, "couldn't read the last action number after a commit");
            state.sent_ack
        }
    };
    let online = state.online();
    let now = state.now();
    let updates = state.subs.updates(**snapshot, &commit.changes, online, now);
    let mut rejected = state.take_rejected();
    let ack_moved = ack != state.sent_ack;
    // An action batch moves `ack` even when it changed nothing a
    // subscription shows, so the mirror drops those pending actions; and a
    // refusal goes out with the first message after it.
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

/// Releases the committed job's effects and activity when dropped, however
/// `on_commit` ends.
struct Releasing<'s>(&'s mut WriterState);

impl Drop for Releasing<'_> {
    fn drop(&mut self) {
        self.0.release();
    }
}

/// `on_rollback`: the job's effects and activity go with its transaction,
/// and an action batch's actions are refused, for the next message to say.
/// Subscriptions it opened or closed stay so, and the work they asked for
/// goes out: a first load it would have sent goes out with the next commit.
pub(crate) fn on_rollback(state: &mut WriterState) {
    state.discard();
}
