use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::writer::WriterState;

#[derive(Debug, Default)]
pub(crate) struct Freshness {
    started: AtomicU64,
    stored: Arc<AtomicU64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ReadNo(u64);

impl Freshness {
    pub(crate) fn start(&self) -> ReadNo {
        ReadNo(
            self.started
                .fetch_add(1, Ordering::SeqCst)
                .saturating_add(1),
        )
    }

    pub(crate) fn store(&self, read: ReadNo, state: &mut WriterState) -> bool {
        let before = self.stored.fetch_max(read.0, Ordering::SeqCst);
        if before >= read.0 {
            return false;
        }
        let stored = Arc::clone(&self.stored);
        state.undo_on_rollback(move || stored.store(before, Ordering::SeqCst));
        true
    }
}
