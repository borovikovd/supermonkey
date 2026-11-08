//! SQLite storage: the writer actor, the read pool, and typed repositories
//! (ARCHITECTURE.md, section 6).
//!
//! - [`Writer`] owns the only write connection, on its own thread. Jobs run
//!   one at a time by [`Priority`], each in its own transaction, through a
//!   [`WriteTx`]: the only way to write. Every write method stamps the rows
//!   it changes with the transaction's `rev` and records them, and the
//!   account's [`Table`]s it wrote, in its [`Changes`]; after each commit the
//!   writer hands a [`Snapshot`] and the [`Commit`] to the caller's
//!   `on_commit`, which decides what that makes stale.
//! - [`ReadPool`] gives read-only connections; [`ReadTx`] reads in one
//!   snapshot.
//! - [`Reads`] holds every read, the same on a [`Snapshot`], a [`ReadTx`],
//!   and [`WriteTx::read`].
//! - Writing a message re-derives its thread's row, labels, and split by the
//!   domain rules (`sm_domain::Thread::digest`, `sm_domain::Router`), and a
//!   split, pin, label, or address change re-routes the Inbox, so derived
//!   state can't drift from the messages.
//! - The schema is created fresh; a file with another [`SCHEMA_VERSION`] is
//!   moved aside to `<name>.old-<n>`.

mod accounts;
mod changes;
mod contacts;
mod db;
mod drafts;
mod error;
mod labels;
mod messages;
mod meta;
mod notices;
mod open;
mod ops;
mod outbox;
mod pool;
mod reply;
mod routing;
mod search;
mod settings;
mod splits;
mod threads;
mod tx;
mod undo;
mod writer;

pub use accounts::{AccountReads, AccountRecord, AccountWrites};
pub use changes::{Changes, Commit, Table};
pub use contacts::{ContactReads, ContactRecord, ContactWrites, Sighting};
pub use drafts::{
    Draft, DraftAttachment, DraftLifecycle, DraftReads, DraftRecord, DraftStage, DraftWait,
    DraftWrites,
};
pub use error::StoreError;
pub use labels::{GmailLabel, LabelReads, LabelRecord, LabelWrites};
pub use messages::{
    AttachmentRecord, AttachmentSource, BodyRecord, MessageReads, MessageRecord, MessageWrites,
    NewAttachment, NewBody, NewMessage,
};
pub use meta::{MetaReads, MetaWrites};
pub use notices::{NewNotice, NoticeReads, NoticeRecord, NoticeWrites};
pub use open::SCHEMA_VERSION;
pub use ops::{NewOp, OpId, OpKind, OpReads, OpRecord, OpWrites};
pub use outbox::{NewOutbox, Outbox, OutboxReads, OutboxRecord, OutboxWrites};
pub use pool::ReadPool;
pub use reply::Pending;
pub use search::{SEARCH_LIMIT, SearchReads};
pub use settings::{SettingReads, SettingWrites};
pub use splits::{SplitEdit, SplitReads, SplitRecord, SplitWrites};
pub use threads::{
    ConversationMessage, ConversationRows, FetchedThread, ListedThread, StoredThread, ThreadReads,
    ThreadRow, ThreadSyncState, ThreadWrites,
};
pub use tx::{ReadTx, Reads, Snapshot, WriteTx};
pub use undo::{UNDO_KEPT, UndoAction, UndoReads, UndoRecord, UndoWrites};
pub use writer::{Priority, Writer};
