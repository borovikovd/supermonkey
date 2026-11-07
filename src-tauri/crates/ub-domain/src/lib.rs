//! Ubermensch's mail model and the rules over it (ARCHITECTURE.md, section 3).
//!
//! Pure: nothing here does I/O, runs async, reads the clock, or knows SQL. The
//! `sqlite` and `ts` features only add conversions for the newtypes and enums.
//!
//! - Model: ids and [`Millis`], [`Addr`], [`LabelSet`] and [`SystemLabel`],
//!   [`MessageFacts`], [`Thread`] and its [`ThreadDigest`], [`Identity`].
//! - Views: [`View::lists`], [`View::gmail_listing`].
//! - Actions: [`LabelDelta`], [`plan_modify`], [`MessageChange`],
//!   [`ThreadChange`], [`snooze_after`].
//! - Compose: [`addressing`], [`DraftContent`], [`gmail_draft_kind`],
//!   [`gmail_draft_aim`].
//! - Split Inbox: [`Router`], [`STARTER_SPLITS`].
//! - The query language: [`query`].
//! - Snooze times: [`snooze`].
//! - Autocomplete: [`contacts`].

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
