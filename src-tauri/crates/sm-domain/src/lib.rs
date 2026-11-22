mod addr;
mod addressing;
pub mod contacts;
mod drafts;
mod hue;
mod identity;
mod ids;
mod kinds;
mod kinds_text;
mod labels;
mod message;
mod modify;
pub mod query;
pub mod snooze;
mod splits;
mod thread;
mod time;
mod views;

#[cfg(test)]
mod test_support;
#[cfg(test)]
mod tests;

pub use addr::Addr;
pub use addressing::{Addressing, ReplyTarget, addressing};
pub use drafts::{DraftAim, DraftContent, DraftContentRef, gmail_draft_aim, gmail_draft_kind};
pub use hue::{AVATAR_HUES, avatar_hue};
pub use identity::{Identity, SendAsAddress};
pub use ids::{
    AccountId, ActionNo, AttachmentId, DraftId, GmailDraftId, GmailMessageId, GmailThreadId,
    HistoryId, LabelId, MessageId, NoticeId, OutboxId, Rev, SplitId, SubId, ThreadId, UndoEntryId,
};
pub use kinds::{
    DraftKind, ModifyKind, NoticeKind, OutboxState, PinKind, SettingKey, SplitKind, SyncStatus,
    UndoKind, View,
};
pub use kinds_text::UnknownKind;
pub use labels::{LabelSet, SystemLabel};
pub use message::MessageFacts;
pub use modify::{
    LabelDelta, MessageChange, ModifyError, SnoozeEdit, ThreadChange, ThreadModify, plan_modify,
    snooze_after,
};
pub use query::{Query, RuleError};
pub use splits::{
    IMPORTANT_POSITION, OTHER_POSITION, Pin, Router, STARTER_SPLITS, Split, StarterSplit,
    check_split_rule, email_domain, split_position,
};
pub use thread::{Thread, ThreadDigest};
pub use time::Millis;
pub use views::GmailListing;
