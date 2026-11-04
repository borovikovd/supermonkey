//! Why a split rule is refused.

/// Why [`Query::parse_rule`](super::Query::parse_rule) refused a rule. The
/// message is a short sentence to show under the rule field.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RuleError {
    /// Nothing but spaces.
    #[error("Enter a query")]
    Empty,
    /// A `(` that is never closed.
    #[error("Missing )")]
    MissingParen {
        /// Byte offset of the `(`.
        offset: usize,
    },
    /// A `)` with no `(`.
    #[error("Unmatched )")]
    UnmatchedParen {
        /// Byte offset of the `)`.
        offset: usize,
    },
    /// `()` with nothing to match inside.
    #[error("Empty parentheses")]
    EmptyParens {
        /// Byte offset of the `(`.
        offset: usize,
    },
    /// A `"` that is never closed.
    #[error("Missing closing quote")]
    MissingQuote {
        /// Byte offset of the term with the quote.
        offset: usize,
    },
    /// `OR` at the start of a group.
    #[error("OR needs a term before it")]
    OrWithoutLeft {
        /// Byte offset of the `OR`.
        offset: usize,
    },
    /// `OR` at the end of a group.
    #[error("OR needs a term after it")]
    OrWithoutRight {
        /// Byte offset of the `OR`.
        offset: usize,
    },
    /// `AND` at the start or end of a group.
    #[error("AND needs a term on both sides")]
    AndWithoutBoth {
        /// Byte offset of the `AND`.
        offset: usize,
    },
    /// `from:` and the like with nothing after the colon.
    #[error("{operator}: needs a value")]
    MissingValue {
        /// Byte offset of the term.
        offset: usize,
        /// The operator, lowercased.
        operator: String,
    },
    /// Gmail syntax a rule can't use, such as `has:attachment` or `-foo`.
    #[error("{term} isn't supported in split rules")]
    Unsupported {
        /// Byte offset of the term.
        offset: usize,
        /// The term as typed.
        term: String,
    },
}

impl RuleError {
    /// Byte offset in the rule where the problem is.
    pub fn offset(&self) -> usize {
        match self {
            Self::Empty => 0,
            Self::MissingParen { offset }
            | Self::UnmatchedParen { offset }
            | Self::EmptyParens { offset }
            | Self::MissingQuote { offset }
            | Self::OrWithoutLeft { offset }
            | Self::OrWithoutRight { offset }
            | Self::AndWithoutBoth { offset }
            | Self::MissingValue { offset, .. }
            | Self::Unsupported { offset, .. } => *offset,
        }
    }
}
