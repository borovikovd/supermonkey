//! Ubermensch's mail model and the rules over it (ARCHITECTURE.md, section 3).
//!
//! Pure: nothing here does I/O, runs async, reads the clock, or knows SQL. The
//! `sqlite` and `ts` features only add conversions for the newtypes.

mod addr;
mod ids;
mod kinds;
mod time;

pub use addr::Addr;
pub use kinds::{
    DraftKind, ModifyKind, NoticeKind, OutboxState, PinKind, SplitKind, SyncStatus, UndoKind, View,
};

pub use ids::{
    AccountId, ActionNo, AttachmentId, DraftId, GmailDraftId, GmailMessageId, GmailThreadId,
    LabelId, MessageId, NoticeId, OutboxId, Rev, SplitId, SubId, ThreadId, UndoEntryId,
};
pub use time::Millis;
