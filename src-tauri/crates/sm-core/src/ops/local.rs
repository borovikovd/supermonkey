use serde::de::DeserializeOwned;
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
        ops.extend(queued::<ModifyOp>(reads, account, OpKind::Modify)?);
        ops.extend(
            queued::<GmailMessageId>(reads, account, OpKind::TrashMessage)?
                .into_iter()
                .map(ModifyOp::trash),
        );
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

fn queued<P: DeserializeOwned>(
    reads: Reads<'_>,
    account: AccountId,
    kind: OpKind,
) -> Result<Vec<P>, StoreError> {
    Ok(reads
        .ops()
        .of_kind(account, kind)?
        .into_iter()
        .filter_map(|op| {
            op.payload::<P>()
                .inspect_err(|e| {
                    tracing::warn!(op = %op.id, error = %e, "skipping a queued op that does not parse");
                })
                .ok()
        })
        .collect())
}
