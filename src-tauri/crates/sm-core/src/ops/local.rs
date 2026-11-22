use sm_domain::{AccountId, GmailMessageId, LabelSet};
use sm_store::{OpKind, Reads, StoreError};

use super::ModifyOp;

#[derive(Debug, Default)]
pub(crate) struct LocalChanges(Vec<ModifyOp>);

impl LocalChanges {
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

    pub(crate) fn apply(&self, message: &GmailMessageId, labels: &mut LabelSet) {
        for op in self.0.iter().filter(|op| op.names(message)) {
            op.apply_to(labels);
        }
    }

    pub(crate) fn touches(&self, message: &GmailMessageId) -> bool {
        self.0.iter().any(|op| op.names(message))
    }
}
