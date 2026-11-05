//! The parsed form of a query, and what callers ask of it.

use std::fmt;

use chrono::{DateTime, Days, Months, NaiveTime, TimeZone};

use super::dates::{Age, DateArg};
use super::{RuleError, parser};
use crate::snooze::resolve_local;
use crate::{MessageFacts, Millis, SystemLabel};

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
    /// `label:`, and in search `in:`, `is:`, and `category:` for a label:
    /// a system or category id on the message, or a user label name on the
    /// thread.
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
    /// `-term`: the child does not match. Search only.
    Not(Box<Expr>),
    /// `has:attachment`: the message has an attachment that is not inline.
    /// Search only.
    HasAttachment,
    /// `in:snoozed` or `is:snoozed`: the thread is snoozed in this app.
    /// Search only.
    Snoozed,
    /// `in:anywhere`: every message, Trash and Spam included. Search only.
    Anywhere,
    /// `after:`, `before:`, `newer_than:`, `older_than:` (and `newer:`,
    /// `older:`), placed in time: the message's date is at or after `at`
    /// (`after`), or before it. Search only.
    Date {
        /// At or after `at`, rather than before it.
        after: bool,
        /// The bound.
        at: Millis,
    },
    /// Syntax this language does not evaluate, kept verbatim. Matches
    /// nothing, and a query holding one is left to Gmail.
    Unsupported(String),
}

impl Expr {
    /// Whether any node satisfies `pred`.
    fn any(&self, pred: &impl Fn(&Self) -> bool) -> bool {
        match self {
            Self::And(xs) | Self::Or(xs) => xs.iter().any(|x| x.any(pred)),
            Self::Not(x) => x.any(pred),
            leaf => pred(leaf),
        }
    }

    /// Whether the FTS5 index alone decides this, for live messages: only
    /// AND, OR, and terms on its columns.
    fn index_decides(&self) -> bool {
        match self {
            Self::And(xs) | Self::Or(xs) => xs.iter().all(Self::index_decides),
            Self::Term(t) => t.in_index(),
            _ => false,
        }
    }
}

impl Term {
    /// A term the FTS5 index can look up: a bare word or an address or
    /// subject operator, except `me`, which names this account's addresses.
    pub(super) fn in_index(&self) -> bool {
        match self.field {
            Field::Any | Field::Subject => true,
            Field::From | Field::To | Field::Cc => !self.is_me(),
            Field::Bcc | Field::Label => false,
        }
    }

    /// `from:me` and the like, which search reads as this account's addresses.
    pub(super) fn is_me(&self) -> bool {
        matches!(self.field, Field::From | Field::To | Field::Cc | Field::Bcc)
            && self.value.trim().eq_ignore_ascii_case("me")
    }
}

/// Which messages a search looks at. Like Gmail, it skips drafts and mail
/// in Trash and Spam unless the query names them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Scope {
    /// `in:trash`, `label:trash`, or `in:anywhere`.
    pub trash: bool,
    /// `in:spam`, `label:spam`, or `in:anywhere`.
    pub spam: bool,
    /// `in:drafts` or `label:draft`.
    pub drafts: bool,
}

impl Scope {
    /// Only live messages: not drafts, not in Trash or Spam.
    pub fn live_only(self) -> bool {
        self == Self::default()
    }

    /// Whether search looks at `message` at all.
    pub fn admits(self, message: &MessageFacts) -> bool {
        (self.drafts || !message.is_draft)
            && (self.trash || !message.has(SystemLabel::Trash))
            && (self.spam || !message.has(SystemLabel::Spam))
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

    /// Parses leniently. Never fails. With no clock to place them in time,
    /// date operators are [`Expr::Unsupported`]: search uses
    /// [`Query::parse_search`].
    pub fn parse(input: &str) -> Self {
        parser::parse(input, &|_| None).0
    }

    /// Parses leniently for search as you type, at `now` in the user's time
    /// zone. As in Gmail, `after:2026/01/31` is local midnight at the start
    /// of that day, a bare number is seconds since the epoch, and
    /// `newer_than:2d`, `3m`, `1y` count back calendar days, months, and
    /// years from `now`.
    pub fn parse_search<Tz: TimeZone>(input: &str, now: &DateTime<Tz>) -> Self {
        let place = |arg: &DateArg| -> Option<Millis> {
            let at = match *arg {
                DateArg::Epoch(at) => return Some(at),
                DateArg::Day(day) => resolve_local(&now.timezone(), day.and_time(NaiveTime::MIN)),
                DateArg::Ago(n, Age::Days) => now.clone().checked_sub_days(Days::new(n.into()))?,
                DateArg::Ago(n, Age::Months) => now.clone().checked_sub_months(Months::new(n))?,
                DateArg::Ago(n, Age::Years) => now
                    .clone()
                    .checked_sub_months(Months::new(n.checked_mul(12)?))?,
            };
            Some(Millis(at.timestamp_millis()))
        };
        parser::parse(input, &place).0
    }

    /// Parses a split rule, refusing anything [`Query::parse`] would have to
    /// guess at: an empty rule, unbalanced parentheses, an unclosed quote, a
    /// dangling `AND` or `OR`, an operator with no value, or an unsupported
    /// term. With several problems, the first by position is reported.
    pub fn parse_rule(input: &str) -> Result<Self, RuleError> {
        let (query, errors) = parser::parse(input, &|_| None);
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

    /// True when [`Query::to_fts5`] can't express the whole query (`bcc:`,
    /// `label:`, `is:list`, `-term`, dates, ...) or it looks past live
    /// messages, so its candidates need [`Query::matches`].
    pub fn needs_post_filter(&self) -> bool {
        !self.expr.index_decides() || !self.scope().live_only()
    }

    /// True when a bare word or phrase is present: only those read the body
    /// in search, so a caller can skip loading it otherwise.
    pub fn reads_body(&self) -> bool {
        self.expr
            .any(&|e| matches!(e, Expr::Term(t) if t.field == Field::Any))
    }

    /// True when a term is [`Expr::Unsupported`], so only Gmail can answer
    /// the query: local search lists nothing for it.
    pub fn needs_gmail(&self) -> bool {
        self.expr.any(&|e| matches!(e, Expr::Unsupported(_)))
    }

    /// False when the query is empty or [`Query::needs_gmail`], so local
    /// search has nothing to go on and leaves results to Gmail.
    pub fn searches_locally(&self) -> bool {
        !self.is_empty() && !self.needs_gmail()
    }

    /// The messages a search looks at: Trash, Spam, and drafts only when a
    /// term names them, as in Gmail, even negated (`-in:trash` looks in
    /// Trash and finds nothing there).
    pub fn scope(&self) -> Scope {
        let names = |label: SystemLabel| {
            self.expr.any(&|e| {
                matches!(e, Expr::Term(t)
                    if t.field == Field::Label && t.value.trim().eq_ignore_ascii_case(label.id()))
            })
        };
        let anywhere = self.expr.any(&|e| matches!(e, Expr::Anywhere));
        Scope {
            trash: anywhere || names(SystemLabel::Trash),
            spam: anywhere || names(SystemLabel::Spam),
            drafts: names(SystemLabel::Draft),
        }
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
            Self::Not(x) => write!(f, "(not {x})"),
            Self::HasAttachment => f.write_str("has:attachment"),
            Self::Snoozed => f.write_str("in:snoozed"),
            Self::Anywhere => f.write_str("in:anywhere"),
            Self::Date { after: true, at } => write!(f, "after:{at}"),
            Self::Date { after: false, at } => write!(f, "before:{at}"),
            Self::Unsupported(raw) => write!(f, "?{raw}"),
        }
    }
}
