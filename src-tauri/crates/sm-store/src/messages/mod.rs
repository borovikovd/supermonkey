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
