//! Ordering the answers of a read that can run twice at once, such as a
//! focus refresh of the labels started while the last one is still on its
//! way: an answer is stored only if no read that started later was stored
//! first (AGENTS.md, stale answers).

use std::sync::atomic::{AtomicU64, Ordering};

/// One kind of read's starts and stores.
#[derive(Debug, Default)]
pub(crate) struct Freshness {
    started: AtomicU64,
    stored: AtomicU64,
}

/// A read's place in the order it started in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ReadNo(u64);

impl Freshness {
    /// Numbers a read. Call it before its request goes out.
    pub(crate) fn start(&self) -> ReadNo {
        ReadNo(
            self.started
                .fetch_add(1, Ordering::SeqCst)
                .saturating_add(1),
        )
    }

    /// Whether the answer of `read` is the newest yet, which it then is.
    /// Call it in the writer job that stores the answer, so stores are
    /// ordered with each other.
    pub(crate) fn store(&self, read: ReadNo) -> bool {
        self.stored.fetch_max(read.0, Ordering::SeqCst) < read.0
    }
}
