//! Effects: work a commit asks the async side to do, released only after
//! the commit lands (ARCHITECTURE.md, section 5.1), and the router that hands
//! each one to its worker.

mod effect;
mod router;

pub use effect::{Effect, GmailQuery};
pub(crate) use router::{EffectSender, Router};
