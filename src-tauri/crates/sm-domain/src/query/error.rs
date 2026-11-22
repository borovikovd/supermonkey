#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RuleError {
    #[error("Enter a query")]
    Empty,
    #[error("Missing )")]
    MissingParen { offset: usize },
    #[error("Unmatched )")]
    UnmatchedParen { offset: usize },
    #[error("Empty parentheses")]
    EmptyParens { offset: usize },
    #[error("Missing closing quote")]
    MissingQuote { offset: usize },
    #[error("OR needs a term before it")]
    OrWithoutLeft { offset: usize },
    #[error("OR needs a term after it")]
    OrWithoutRight { offset: usize },
    #[error("AND needs a term on both sides")]
    AndWithoutBoth { offset: usize },
    #[error("{operator}: needs a value")]
    MissingValue { offset: usize, operator: String },
    #[error("{term} isn't supported in split rules")]
    Unsupported { offset: usize, term: String },
}

impl RuleError {
    #[must_use]
    pub const fn offset(&self) -> usize {
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
