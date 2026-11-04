//! The parsed form of a query, and what callers ask of it.

use std::fmt;

use super::{RuleError, parser};

/// Which part of a message a [`Term`] looks at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    /// A bare word or phrase: the sender or the subject, and in search the body.
    Any,
    /// `from:`: the sender's name or address.
    From,
    /// `to:`: a `To` name or address.
    To,
    /// `cc:`: a `Cc` name or address.
    Cc,
    /// `bcc:`: a `Bcc` name or address.
    Bcc,
    /// `subject:`.
    Subject,
    /// `label:`: a system or category id on the message, or a user label
    /// name on the thread.
    Label,
}

impl Field {
    /// The operator as written, `""` for [`Field::Any`].
    fn operator(self) -> &'static str {
        match self {
            Self::Any => "",
            Self::From => "from:",
            Self::To => "to:",
            Self::Cc => "cc:",
            Self::Bcc => "bcc:",
            Self::Subject => "subject:",
            Self::Label => "label:",
        }
    }
}

/// One `field:value` test, or a bare word or phrase when `field` is [`Field::Any`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Term {
    /// What it looks at.
    pub field: Field,
    /// The value as typed, without surrounding quotes.
    pub value: String,
    /// The last term while the user may still be typing it (no trailing
    /// space and no closing quote). [`Query::to_fts5`] makes it a prefix match.
    pub prefix: bool,
}

/// A parsed query expression.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Expr {
    /// Every child must match. `And(vec![])` is the empty query and matches everything.
    And(Vec<Expr>),
    /// At least one child must match.
    Or(Vec<Expr>),
    /// One test.
    Term(Term),
    /// `is:list`: the message has a `List-Id` or `List-Unsubscribe` header.
    IsList,
    /// Syntax this language does not evaluate, kept verbatim. Always matches.
    Unsupported(String),
}

impl Expr {
    /// Whether any node satisfies `pred`.
    fn any(&self, pred: &impl Fn(&Self) -> bool) -> bool {
        match self {
            Self::And(xs) | Self::Or(xs) => xs.iter().any(|x| x.any(pred)),
            leaf => pred(leaf),
        }
    }
}

/// A parsed query.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Query {
    expr: Expr,
}

impl Query {
    pub(super) fn from_expr(expr: Expr) -> Self {
        Self { expr }
    }

    /// Parses leniently, for search as you type. Never fails.
    pub fn parse(input: &str) -> Self {
        parser::parse(input).0
    }

    /// Parses a split rule, refusing anything [`Query::parse`] would have to
    /// guess at: an empty rule, unbalanced parentheses, an unclosed quote, a
    /// dangling `AND` or `OR`, an operator with no value, or an unsupported
    /// term. With several problems, the first by position is reported.
    pub fn parse_rule(input: &str) -> Result<Self, RuleError> {
        let (query, errors) = parser::parse(input);
        if let Some(first) = errors.into_iter().min_by_key(RuleError::offset) {
            return Err(first);
        }
        if query.is_empty() {
            return Err(RuleError::Empty);
        }
        Ok(query)
    }

    /// The expression tree.
    pub fn expr(&self) -> &Expr {
        &self.expr
    }

    /// The empty query, which matches everything.
    pub fn is_empty(&self) -> bool {
        matches!(&self.expr, Expr::And(xs) if xs.is_empty())
    }

    /// True when the query has `bcc:`, `label:`, or `is:list` terms, which
    /// [`Query::to_fts5`] can't express, so its candidates need [`Query::matches`].
    pub fn needs_post_filter(&self) -> bool {
        self.expr.any(&|e| match e {
            Expr::Term(t) => matches!(t.field, Field::Bcc | Field::Label),
            Expr::IsList => true,
            _ => false,
        })
    }

    /// True when a bare word or phrase is present: only those read the body
    /// in search, so a caller can skip loading it otherwise.
    pub fn reads_body(&self) -> bool {
        self.expr
            .any(&|e| matches!(e, Expr::Term(t) if t.field == Field::Any))
    }

    /// False when the query is empty or holds only unsupported terms, so
    /// local search has nothing to go on and leaves results to Gmail.
    pub fn has_local_terms(&self) -> bool {
        self.expr
            .any(&|e| matches!(e, Expr::Term(_) | Expr::IsList))
    }
}

impl fmt::Display for Query {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.expr.fmt(f)
    }
}

/// A compact s-expression, for tests and debugging:
/// `(or (and from:"alice" subject:"q3 plan") "bob"*)`.
impl fmt::Display for Expr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let list = |f: &mut fmt::Formatter<'_>, op: &str, xs: &[Self]| {
            write!(f, "({op}")?;
            for x in xs {
                write!(f, " {x}")?;
            }
            f.write_str(")")
        };
        match self {
            Self::And(xs) => list(f, "and", xs),
            Self::Or(xs) => list(f, "or", xs),
            Self::Term(t) => {
                let star = if t.prefix { "*" } else { "" };
                write!(f, "{}\"{}\"{star}", t.field.operator(), t.value)
            }
            Self::IsList => f.write_str("is:list"),
            Self::Unsupported(raw) => write!(f, "?{raw}"),
        }
    }
}
