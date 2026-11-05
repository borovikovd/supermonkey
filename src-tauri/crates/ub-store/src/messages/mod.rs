//! Messages: headers, labels, bodies, attachments, and their search index.
//! Every write re-derives the threads it touches (see `routing`).

mod model;
mod reads;
mod writes;

pub use model::{
    AttachmentRecord, AttachmentSource, BodyRecord, MessageRecord, NewAttachment, NewBody,
    NewMessage,
};
pub use reads::MessageReads;
pub use writes::MessageWrites;
pub(crate) use writes::{delete_row, derive_all, upsert_row};
