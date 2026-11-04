//! Testing a query against one message.

use super::{Expr, Field, Query, Term};
use crate::labels::is_system_label_id;
use crate::{Addr, MessageFacts};

/// A message as a rule or a search sees it.
#[derive(Debug, Clone, Copy)]
pub struct MatchTarget<'a> {
    message: &'a MessageFacts,
    thread_label_names: &'a [String],
    /// The plain body, in search only.
    body: Option<&'a str>,
}

impl<'a> MatchTarget<'a> {
    /// For a split rule: bare words match the sender's name or address, or
    /// the subject. `thread_label_names` are the names of the thread's user
    /// labels, since Gmail does not copy them onto later messages.
    pub fn rule(message: &'a MessageFacts, thread_label_names: &'a [String]) -> Self {
        Self {
            message,
            thread_label_names,
            body: None,
        }
    }

    /// For local search: bare words also match the plain `body`. Only
    /// queries that [`Query::reads_body`] look at it.
    pub fn search(
        message: &'a MessageFacts,
        thread_label_names: &'a [String],
        body: &'a str,
    ) -> Self {
        Self {
            message,
            thread_label_names,
            body: Some(body),
        }
    }
}

impl Query {
    /// Whether the message matches. Substrings, ignoring case.
    pub fn matches(&self, target: &MatchTarget<'_>) -> bool {
        expr_matches(self.expr(), target)
    }
}

fn expr_matches(e: &Expr, t: &MatchTarget<'_>) -> bool {
    match e {
        Expr::And(xs) => xs.iter().all(|x| expr_matches(x, t)),
        Expr::Or(xs) => xs.iter().any(|x| expr_matches(x, t)),
        Expr::Term(term) => term_matches(term, t),
        Expr::IsList => t.message.is_list,
        Expr::Unsupported(_) => true,
    }
}

fn term_matches(term: &Term, t: &MatchTarget<'_>) -> bool {
    let m = t.message;
    let needle = term.value.to_lowercase();
    let any_addr = |list: &[Addr]| list.iter().any(|a| addr_contains(a, &needle));
    match term.field {
        Field::Any => {
            addr_contains(&m.from, &needle)
                || contains(&m.subject, &needle)
                || t.body.is_some_and(|body| contains(body, &needle))
        }
        Field::From => addr_contains(&m.from, &needle),
        Field::To => any_addr(&m.to),
        Field::Cc => any_addr(&m.cc),
        Field::Bcc => any_addr(&m.bcc),
        Field::Subject => contains(&m.subject, &needle),
        Field::Label => {
            let value = term.value.trim();
            if is_system_label_id(value) {
                m.labels
                    .iter()
                    .any(|id| id.as_str().eq_ignore_ascii_case(value))
            } else {
                let want = label_key(value);
                t.thread_label_names
                    .iter()
                    .any(|name| label_key(name) == want)
            }
        }
    }
}

fn contains(haystack: &str, needle_lower: &str) -> bool {
    haystack.to_lowercase().contains(needle_lower)
}

fn addr_contains(addr: &Addr, needle_lower: &str) -> bool {
    addr.name
        .as_deref()
        .is_some_and(|name| contains(name, needle_lower))
        || contains(&addr.email, needle_lower)
}

/// Gmail writes `Work/Projects` and `Work Projects` as `label:work-projects`.
fn label_key(name: &str) -> String {
    name.trim()
        .chars()
        .flat_map(char::to_lowercase)
        .map(|c| if c == ' ' || c == '/' { '-' } else { c })
        .collect()
}
