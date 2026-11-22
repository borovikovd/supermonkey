use sm_domain::{ModifyError, RuleError, SplitId};
use sm_store::StoreError;

pub(crate) const NOT_SAVED: &str = "Couldn't save that change. Try again.";

#[derive(Debug, thiserror::Error)]
pub(crate) enum ActionError {
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error("{0}")]
    Invalid(&'static str),
    #[error(transparent)]
    Modify(#[from] ModifyError),
    #[error("a split rule doesn't parse")]
    Rule {
        split: SplitId,
        #[source]
        error: RuleError,
    },
    #[error(transparent)]
    Unsendable(#[from] Unsendable),
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum Unsendable {
    #[error("the draft has no recipient")]
    NoRecipient,
    #[error("a recipient is not a valid address")]
    BadRecipient(String),
}

impl ActionError {
    pub(crate) fn message(&self) -> String {
        match self {
            Self::Store(_) => NOT_SAVED.to_owned(),
            Self::Invalid(sentence) => (*sentence).to_owned(),
            Self::Modify(ModifyError::UnreadWithOtherLabels) => {
                "Mark unread can't change labels at the same time".to_owned()
            }
            Self::Rule { error, .. } => error.to_string(),
            Self::Unsendable(Unsendable::NoRecipient) => "Not sent: add a recipient".to_owned(),
            Self::Unsendable(Unsendable::BadRecipient(email)) => {
                format!("Not sent: {email} isn't a valid email address")
            }
        }
    }

    pub(crate) const fn split(&self) -> Option<SplitId> {
        match self {
            Self::Rule { split, .. } => Some(*split),
            Self::Store(_) | Self::Invalid(_) | Self::Modify(_) | Self::Unsendable(_) => None,
        }
    }
}
