//! What happens after each writer job (SPEC UI state, Ordering): one channel
//! message for the commit, the first loads as their own message, then the
//! effects. A job that rolled back sends nothing and its effects go.

use ub_ipc::ChannelMessage;
use ub_store::{Commit, Snapshot};

use super::WriterState;
use crate::effects::Effect;

/// `on_commit`: builds the commit's updates from its snapshot and sends
/// them, then releases the job's effects.
pub(crate) fn on_commit(state: &mut WriterState, snapshot: &Snapshot<'_>, commit: &Commit) {
    let ack = match snapshot.meta().last_action() {
        Ok(ack) => ack,
        Err(e) => {
            tracing::error!(error = %e, "couldn't read the last action number after a commit");
            state.sent_ack
        }
    };
    let online = state.online();
    let updates = state.subs.updates(**snapshot, &commit.changes, online);
    let ack_moved = ack != state.sent_ack;
    // An action batch moves `ack` even when it changed nothing a
    // subscription shows, so the mirror drops those pending actions.
    if !updates.patches.is_empty() || (ack_moved && updates.first_loads.is_empty()) {
        state.send(ChannelMessage {
            rev: commit.rev,
            ack,
            updates: updates.patches,
        });
    }
    if !updates.first_loads.is_empty() {
        state.send(ChannelMessage {
            rev: commit.rev,
            ack,
            updates: updates.first_loads,
        });
    }
    state.sent_ack = ack;
    // Unread counts, accounts, and mutes are what the badge counts.
    if commit.changes.app || !commit.changes.settings.is_empty() {
        state.effect(Effect::Badge);
    }
    state.release();
}

/// `on_rollback`: the job's effects and activity go with its transaction.
/// Subscriptions it opened or closed stay so: a first load it would have
/// sent goes out with the next commit.
pub(crate) fn on_rollback(state: &mut WriterState) {
    state.discard();
}
