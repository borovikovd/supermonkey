//! Why an action was rejected.

use ub_domain::{ModifyError, RuleError};
use ub_store::StoreError;

/// Why the core rejected a UI action. A rejected action's writes roll back
/// and `ack` still moves past it, so the mirror falls back to Rust's state;
/// a notice tells the user why (SPEC UI state, Actions).
///
/// `Display` is for logs and carries no user content; [`ActionError::message`]
/// is what the user reads.
#[derive(Debug, thiserror::Error)]
pub(crate) enum ActionError {
    /// The database refused a read or write.
    #[error(transparent)]
    Store(#[from] StoreError),
    /// The action names something that is gone, or asks for something the
    /// core does not accept. The text is a sentence for the user.
    #[error("{0}")]
    Invalid(&'static str),
    /// A mail change that can't be made as asked.
    #[error(transparent)]
    Modify(#[from] ModifyError),
    /// A split's rule doesn't parse.
    #[error("a split rule doesn't parse")]
    Rule(#[from] RuleError),
    /// A draft that can't be sent as it is: it stays a draft.
    #[error(transparent)]
    Unsendable(#[from] Unsendable),
}

/// Why a draft can't be sent.
#[derive(Debug, thiserror::Error)]
pub(crate) enum Unsendable {
    /// No To, Cc, or Bcc.
    #[error("the draft has no recipient")]
    NoRecipient,
    /// This recipient is not an address mail can go to.
    #[error("a recipient is not a valid address")]
    BadRecipient(String),
}

impl ActionError {
    /// What the user is told: a short sentence, with the user's own text
    /// (an address, a rule's term) where it says what to fix.
    pub(crate) fn message(&self) -> String {
        match self {
            Self::Store(_) => "Couldn't save that change. Try again.".to_owned(),
            Self::Invalid(sentence) => (*sentence).to_owned(),
            Self::Modify(ModifyError::UnreadWithOtherLabels) => {
                "Mark unread can't change labels at the same time".to_owned()
            }
            Self::Rule(e) => e.to_string(),
            Self::Unsendable(Unsendable::NoRecipient) => "Not sent: add a recipient".to_owned(),
            Self::Unsendable(Unsendable::BadRecipient(email)) => {
                format!("Not sent: {email} isn't a valid email address")
            }
        }
    }
}
