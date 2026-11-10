//! Which drafts are being written to Gmail right now.

use std::collections::HashSet;
use std::sync::Arc;

use parking_lot::Mutex;
use sm_domain::DraftId;

/// The drafts of one account being written to Gmail now, by an upsert or by
/// the outbox. Two writes of one draft never overlap: an upsert still in
/// flight when the send's time comes would otherwise land after the
/// outbox's write, as an extra Gmail draft or as an older version under the
/// one being sent. A draft reconcile also leaves a draft being written
/// alone, since Gmail's answer may predate the write.
#[derive(Debug, Default)]
pub(crate) struct DraftsWriting(Mutex<HashSet<DraftId>>);

impl DraftsWriting {
    /// Starts writing `draft`, or `None` while another write of it runs.
    pub(crate) fn begin(self: &Arc<Self>, draft: DraftId) -> Option<WritingDraft> {
        self.0.lock().insert(draft.clone()).then(|| WritingDraft {
            writing: Arc::clone(self),
            draft,
        })
    }

    /// Whether a write of `draft` is running.
    pub(crate) fn is_busy(&self, draft: &DraftId) -> bool {
        self.0.lock().contains(draft)
    }
}

/// A write of one draft in progress; it ends when dropped.
#[derive(Debug)]
pub(crate) struct WritingDraft {
    writing: Arc<DraftsWriting>,
    draft: DraftId,
}

impl WritingDraft {
    /// The draft being written.
    pub(crate) const fn draft(&self) -> &DraftId {
        &self.draft
    }
}

impl Drop for WritingDraft {
    fn drop(&mut self) {
        self.writing.0.lock().remove(&self.draft);
    }
}
