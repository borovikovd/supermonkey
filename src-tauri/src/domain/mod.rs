pub mod contacts;
mod draft;
mod ids;
mod modify;
pub mod query;
pub mod snooze;
mod splits;
mod thread;
mod types;

pub use draft::{
    DraftAim, DraftContentRef, DraftDefaults, DraftLifecycle, DraftStage, GmailWrite,
    gmail_draft_aim, gmail_draft_kind,
};
pub use ids::{
    AccountId, ActionNo, AttachmentId, DraftId, FilterId, GmailAttachmentId, GmailDraftId,
    GmailMessageId, GmailThreadId, HistoryId, LabelId, MessageId, Millis, NoticeId, OpId, OutboxId,
    PageToken, Rev, SplitId, SubId, ThreadId, UndoEntryId,
};
pub use modify::{
    LabelDelta, MessageChange, ModifyError, SnoozeEdit, ThreadChange, ThreadModify, plan_modify,
    snooze_after,
};
pub use splits::{
    IMPORTANT_POSITION, OTHER_POSITION, Pin, Router, STARTER_SPLITS, Split, StarterSplit,
    check_split_rule, email_domain, is_person_to_person, split_position,
};
pub use thread::{
    Addressing, GmailListing, MessageFacts, ReplyTarget, Thread, ThreadDigest, addressing,
    is_forward_subject,
};
pub use types::{
    Addr, DraftKind, DraftWait, Identity, LabelSet, ModifyKind, NoticeKind, OpKind, OpQueue,
    OutboxState, PinKind, SendAsAddress, SettingKey, SplitKind, SyncStatus, SystemLabel,
    UnknownKind, View,
};
