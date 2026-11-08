//! This app's label changes, a sent message's trash included, still on their way to Gmail, applied on top of
//! what Gmail says about the same messages (SPEC Offline: a queued action
//! wins on the messages it recorded, without hiding newer mail).

use ub_domain::{AccountId, GmailMessageId, LabelSet};
use ub_store::{OpKind, Reads, StoreError};

use super::ModifyOp;

/// The account's queued modify ops, after those that landed while the
/// answer being written was in flight, in the order they were made.
#[derive(Debug, Default)]
pub(crate) struct LocalChanges(Vec<ModifyOp>);

impl LocalChanges {
    /// Reads the queue. `landed` comes from the answer's
    /// [`ReadTicket`](super::ReadTicket).
    pub(crate) fn load(
        reads: Reads<'_>,
        account: AccountId,
        landed: Vec<ModifyOp>,
    ) -> Result<Self, StoreError> {
        let mut ops = landed;
        for op in reads.ops().of_kind(account, OpKind::Modify)? {
            match op.payload::<ModifyOp>() {
                Ok(payload) => ops.push(payload),
                Err(e) => {
                    tracing::warn!(op = %op.id, error = %e, "skipping a queued op that does not parse");
                }
            }
        }
        // An undone send's trash is a label change too.
        for op in reads.ops().of_kind(account, OpKind::TrashMessage)? {
            match op.payload::<GmailMessageId>() {
                Ok(message) => ops.push(ModifyOp::trash(message)),
                Err(e) => {
                    tracing::warn!(op = %op.id, error = %e, "skipping a queued op that does not parse");
                }
            }
        }
        Ok(Self(ops))
    }

    /// Puts this app's pending changes to `message` on top of `labels`.
    pub(crate) fn apply(&self, message: &GmailMessageId, labels: &mut LabelSet) {
        for op in self.0.iter().filter(|op| op.names(message)) {
            op.apply_to(labels);
        }
    }

    /// Whether this app has a change to `message` still on its way.
    pub(crate) fn touches(&self, message: &GmailMessageId) -> bool {
        self.0.iter().any(|op| op.names(message))
    }
}
