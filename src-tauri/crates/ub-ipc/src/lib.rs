//! The IPC contract: every type exported to TypeScript, and nothing else
//! (ARCHITECTURE.md, sections 2 and 3).
//!
//! `cargo test` writes the bindings into `src/bindings/`. They are frozen: the
//! gate fails unless the output is byte-identical to the committed files. ts-rs
//! copies Rust doc comments on types and fields into the TypeScript, so types
//! and fields whose binding has no comment are described with plain `//`
//! comments. Variant docs are not copied, so variants use `///` freely.
//!
//! Ids and times are `ub_domain` newtypes; they print as `number` or `string`.
//! The small enums rules depend on (`View`, the kinds and states) live in
//! `ub_domain` and are re-exported here.

mod account;
mod actions;
mod addr;
mod commands;
mod compose;
mod conversation;
mod list;
mod subscriptions;

pub use account::{
    AccountInfo, AccountSummary, AppState, GlobalSettings, LabelInfo, Notice, PinInfo, SendAs,
    SplitInfo, UndoEntryInfo,
};
pub use actions::{Action, ModifyAction, NumberedAction, Setting, SnoozeChange};
pub use addr::Addr;
pub use commands::{
    AppEvent, BlockResult, BlockedSender, BlockedSenders, Command, CommandReply, ContactSuggestion,
    LaunchInfo, Replier, Reply, SnoozeChoice, Unsubscribed,
};
pub use compose::{DraftAttachment, DraftFields, DraftSave, DraftState};
pub use conversation::{
    AttachmentInfo, ConversationData, DraftSummary, MessageBody, MessageSummary, OutboxRow,
};
pub use list::{ListEntry, ListMove, ListRow};
pub use subscriptions::{ChannelMessage, SubKey, SubUpdate};
pub use ub_domain::{
    DraftKind, ModifyKind, NoticeKind, OutboxState, PinKind, SplitKind, SyncStatus, UndoKind, View,
};
