use std::collections::HashSet;
use std::sync::Arc;

use parking_lot::Mutex;
use sm_domain::DraftId;

#[derive(Debug, Default)]
pub(crate) struct DraftsWriting(Mutex<HashSet<DraftId>>);

impl DraftsWriting {
    pub(crate) fn begin(self: &Arc<Self>, draft: DraftId) -> Option<WritingDraft> {
        self.0.lock().insert(draft.clone()).then(|| WritingDraft {
            writing: Arc::clone(self),
            draft,
        })
    }

    pub(crate) fn is_busy(&self, draft: &DraftId) -> bool {
        self.0.lock().contains(draft)
    }
}

#[derive(Debug)]
pub(crate) struct WritingDraft {
    writing: Arc<DraftsWriting>,
    draft: DraftId,
}

impl WritingDraft {
    pub(crate) const fn draft(&self) -> &DraftId {
        &self.draft
    }
}

impl Drop for WritingDraft {
    fn drop(&mut self) {
        self.writing.0.lock().remove(&self.draft);
    }
}
