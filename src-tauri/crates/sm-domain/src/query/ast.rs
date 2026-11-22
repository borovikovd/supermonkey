use std::fmt;

use chrono::{DateTime, Days, Months, NaiveTime, TimeZone};

use super::dates::{Age, DateArg};
use super::{RuleError, parser};
use crate::snooze::resolve_local;
use crate::{MessageFacts, Millis, SystemLabel};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    Any,
    From,
    To,
    Cc,
    Bcc,
    Subject,
    Label,
}

impl Field {
    const fn operator(self) -> &'static str {
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Term {
    pub field: Field,
    pub value: String,
    pub prefix: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Expr {
    And(Vec<Self>),
    Or(Vec<Self>),
    Term(Term),
    IsList,
    Not(Box<Self>),
    HasAttachment,
    Snoozed,
    Anywhere,
    Date { after: bool, at: Millis },
    Unsupported(String),
}

impl Expr {
    fn any(&self, pred: &impl Fn(&Self) -> bool) -> bool {
        match self {
            Self::And(xs) | Self::Or(xs) => xs.iter().any(|x| x.any(pred)),
            Self::Not(x) => x.any(pred),
            leaf @ (Self::Term(_)
            | Self::IsList
            | Self::HasAttachment
            | Self::Snoozed
            | Self::Anywhere
            | Self::Date { .. }
            | Self::Unsupported(_)) => pred(leaf),
        }
    }

    fn index_decides(&self) -> bool {
        match self {
            Self::And(xs) | Self::Or(xs) => xs.iter().all(Self::index_decides),
            Self::Term(t) => t.in_index(),
            Self::IsList
            | Self::Not(_)
            | Self::HasAttachment
            | Self::Snoozed
            | Self::Anywhere
            | Self::Date { .. }
            | Self::Unsupported(_) => false,
        }
    }
}

impl Term {
    pub(super) fn in_index(&self) -> bool {
        match self.field {
            Field::Any | Field::Subject => true,
            Field::From | Field::To | Field::Cc => !self.is_me(),
            Field::Bcc | Field::Label => false,
        }
    }

    pub(super) fn is_me(&self) -> bool {
        matches!(self.field, Field::From | Field::To | Field::Cc | Field::Bcc)
            && self.value.trim().eq_ignore_ascii_case("me")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Scope {
    pub trash: bool,
    pub spam: bool,
    pub drafts: bool,
}

impl Scope {
    #[must_use]
    pub fn live_only(self) -> bool {
        self == Self::default()
    }

    #[must_use]
    pub fn admits(self, message: &MessageFacts) -> bool {
        (self.drafts || !message.is_draft)
            && (self.trash || !message.has(SystemLabel::Trash))
            && (self.spam || !message.has(SystemLabel::Spam))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Query {
    expr: Expr,
}

impl Query {
    pub(super) const fn from_expr(expr: Expr) -> Self {
        Self { expr }
    }

    #[must_use]
    pub fn parse(input: &str) -> Self {
        parser::parse(input, &|_| None).0
    }

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

    #[must_use]
    pub const fn expr(&self) -> &Expr {
        &self.expr
    }

    #[must_use]
    pub const fn is_empty(&self) -> bool {
        matches!(&self.expr, Expr::And(xs) if xs.is_empty())
    }

    #[must_use]
    pub fn needs_post_filter(&self) -> bool {
        !self.expr.index_decides() || !self.scope().live_only()
    }

    #[must_use]
    pub fn reads_body(&self) -> bool {
        self.expr
            .any(&|e| matches!(e, Expr::Term(t) if t.field == Field::Any))
    }

    #[must_use]
    pub fn needs_gmail(&self) -> bool {
        self.expr.any(&|e| matches!(e, Expr::Unsupported(_)))
    }

    #[must_use]
    pub fn searches_locally(&self) -> bool {
        !self.is_empty() && !self.needs_gmail()
    }

    #[must_use]
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
