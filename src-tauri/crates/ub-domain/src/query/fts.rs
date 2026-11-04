//! The SQLite FTS5 `MATCH` string for a query.

use super::{Expr, Field, Query};

/// The columns of the FTS5 table that [`Query::to_fts5`] searches, in order.
pub const FTS_COLUMNS: [&str; 5] = ["from_text", "to_text", "cc_text", "subject", "body"];

/// Column filter for bare words.
const ANY_COLUMNS: &str = "{from_text subject body}";

impl Query {
    /// An FTS5 `MATCH` expression over the [`FTS_COLUMNS`], or `None` when
    /// nothing in the query can narrow the index (only `label:` or
    /// `is:list`, say).
    ///
    /// It selects a superset of the matches. Terms FTS can't express
    /// (`bcc:`, `label:`, `is:list`, unsupported operators) drop out of an
    /// AND; an OR holding one can't narrow anything and drops out whole.
    /// This is the one SQL string built at runtime; bind it as a parameter.
    pub fn to_fts5(&self) -> Option<String> {
        fts(self.expr()).map(|(s, _)| s)
    }
}

/// The expression, and whether it is compound (needs parentheses when nested).
fn fts(e: &Expr) -> Option<(String, bool)> {
    match e {
        Expr::Term(t) => {
            let column = match t.field {
                Field::Any => ANY_COLUMNS,
                Field::From => "from_text",
                Field::To => "to_text",
                Field::Cc => "cc_text",
                Field::Subject => "subject",
                Field::Bcc | Field::Label => return None,
            };
            let literal = fts_string(&t.value)?;
            let star = if t.prefix { "*" } else { "" };
            Some((format!("{column} : {literal}{star}"), false))
        }
        Expr::IsList | Expr::Unsupported(_) => None,
        Expr::And(xs) => join(xs.iter().filter_map(fts).collect(), " AND "),
        Expr::Or(xs) => join(xs.iter().map(fts).collect::<Option<_>>()?, " OR "),
    }
}

fn join(parts: Vec<(String, bool)>, op: &str) -> Option<(String, bool)> {
    if parts.len() <= 1 {
        return parts.into_iter().next();
    }
    let joined = parts
        .into_iter()
        .map(|(s, compound)| if compound { format!("({s})") } else { s })
        .collect::<Vec<_>>()
        .join(op);
    Some((joined, true))
}

/// An FTS5 string literal, or `None` when the value has no token characters
/// (an empty phrase is no use as a term). Control characters become spaces
/// and `"` is doubled, so any input gives a valid literal.
fn fts_string(value: &str) -> Option<String> {
    if !value.chars().any(char::is_alphanumeric) {
        return None;
    }
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for c in value.chars() {
        match c {
            '"' => out.push_str("\"\""),
            c if c.is_control() => out.push(' '),
            c => out.push(c),
        }
    }
    out.push('"');
    Some(out)
}
