//! Request priorities.

/// A request's priority, 1 (first) to 6. SPEC Sync lists what runs at each.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Priority {
    /// UI actions, sends, and draft upserts.
    P1 = 1,
    /// An opened thread and its neighbors, search, label views, attachments,
    /// draft reconciliation.
    P2 = 2,
    /// History polls and the metadata fetches they report.
    P3 = 3,
    /// Poll bodies, inbox metadata and bodies, drafts and starred metadata.
    P4 = 4,
    /// Search-hit bodies, Sent, draft and starred bodies, other resync refetches.
    P5 = 5,
    /// The contacts backfill.
    P6 = 6,
}

impl Priority {
    /// The level, 1 to 6.
    pub fn level(self) -> u8 {
        match self {
            Self::P1 => 1,
            Self::P2 => 2,
            Self::P3 => 3,
            Self::P4 => 4,
            Self::P5 => 5,
            Self::P6 => 6,
        }
    }

    /// Priorities 4 to 6, which leave the bucket's reserve to 1 to 3.
    pub fn is_background(self) -> bool {
        self >= Self::P4
    }
}

/// A priority level outside 1 to 6.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("priority {0} is not between 1 and 6")]
pub struct InvalidPriority(pub u8);

impl TryFrom<u8> for Priority {
    type Error = InvalidPriority;

    fn try_from(level: u8) -> Result<Self, Self::Error> {
        match level {
            1 => Ok(Self::P1),
            2 => Ok(Self::P2),
            3 => Ok(Self::P3),
            4 => Ok(Self::P4),
            5 => Ok(Self::P5),
            6 => Ok(Self::P6),
            other => Err(InvalidPriority(other)),
        }
    }
}
