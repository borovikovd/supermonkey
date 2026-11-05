//! Testing a query against one message.

use super::{Expr, Field, Query, Term};
use crate::labels::is_system_label_id;
use crate::{Addr, Identity, MessageFacts};

/// A message as a rule or a search sees it.
#[derive(Debug, Clone, Copy)]
pub struct MatchTarget<'a> {
    message: &'a MessageFacts,
    thread_label_names: &'a [String],
    /// The plain body, in search only.
    body: Option<&'a str>,
    /// The thread is snoozed, in search only.
    snoozed: bool,
    /// Whose `me` is, in search only.
    me: Option<&'a Identity>,
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
            snoozed: false,
            me: None,
        }
    }

    /// For local search: bare words also match the plain `body`. Only
    /// queries that [`Query::reads_body`] look at it. `from:me` and the like
    /// match `me`'s addresses, and `in:snoozed` matches when `snoozed`.
    pub fn search(
        message: &'a MessageFacts,
        thread_label_names: &'a [String],
        body: &'a str,
        snoozed: bool,
        me: &'a Identity,
    ) -> Self {
        Self {
            message,
            thread_label_names,
            body: Some(body),
            snoozed,
            me: Some(me),
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
        Expr::Not(x) => !expr_matches(x, t),
        Expr::Term(term) => term_matches(term, t),
        Expr::IsList => t.message.is_list,
        Expr::HasAttachment => t.message.has_attachments,
        Expr::Snoozed => t.snoozed,
        Expr::Anywhere => true,
        Expr::Date { after: true, at } => t.message.date >= *at,
        Expr::Date { after: false, at } => t.message.date < *at,
        Expr::Unsupported(_) => false,
    }
}

fn term_matches(term: &Term, t: &MatchTarget<'_>) -> bool {
    let m = t.message;
    if let Some(me) = t.me.filter(|_| term.is_me()) {
        let mine = |list: &[Addr]| list.iter().any(|a| me.is_mine(&a.email));
        return match term.field {
            Field::From => me.is_mine(&m.from.email),
            Field::To => mine(&m.to),
            Field::Cc => mine(&m.cc),
            Field::Bcc => mine(&m.bcc),
            // `is_me` is only ever true for the address fields.
            Field::Any | Field::Subject | Field::Label => false,
        };
    }
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
