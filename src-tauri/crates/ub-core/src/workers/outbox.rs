//! The outbox runner, filled by the compose slice: waits out each send's
//! undo window, then `drafts.send` (SPEC Send and undo).
