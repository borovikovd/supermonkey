use std::collections::HashMap;

use super::parse::{TokenKind, lex};
use super::{Expr, Field, Scope, Term};
use crate::domain::{Addr, Identity, LabelId, LabelSet, MessageFacts, SystemLabel};

#[derive(Debug)]
pub struct MatchTarget<'a> {
    message: &'a MessageFacts,
    labels: &'a LabelSet,
    body_lower: Option<String>,
    subject_lower: String,
    from_lower: String,
    to_lower: String,
    cc_lower: String,
    user_labels: &'a HashMap<LabelId, String>,
    identity: Option<&'a Identity>,
    snoozed: bool,
}

impl<'a> MatchTarget<'a> {
    #[must_use]
    pub fn rule(message: &'a MessageFacts, user_labels: &'a HashMap<LabelId, String>) -> Self {
        Self::build(message, &message.labels, None, user_labels, None, false)
    }

    #[must_use]
    pub fn rule_with_labels(
        message: &'a MessageFacts,
        labels: &'a LabelSet,
        user_labels: &'a HashMap<LabelId, String>,
        identity: Option<&'a Identity>,
    ) -> Self {
        Self::build(message, labels, None, user_labels, identity, false)
    }

    #[must_use]
    pub fn search(
        message: &'a MessageFacts,
        body: &str,
        user_labels: &'a HashMap<LabelId, String>,
        identity: &'a Identity,
        snoozed: bool,
    ) -> Self {
        Self::build(
            message,
            &message.labels,
            Some(body.to_lowercase()),
            user_labels,
            Some(identity),
            snoozed,
        )
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "internal constructor populates pre-lowercased match fields from thread and message context"
    )]
    fn build(
        message: &'a MessageFacts,
        labels: &'a LabelSet,
        body_lower: Option<String>,
        user_labels: &'a HashMap<LabelId, String>,
        identity: Option<&'a Identity>,
        snoozed: bool,
    ) -> Self {
        Self {
            message,
            labels,
            body_lower,
            subject_lower: message.subject.to_lowercase(),
            from_lower: format_addr_lower(&message.from),
            to_lower: format_addrs_lower(&message.to),
            cc_lower: format_addrs_lower(&message.cc),
            user_labels,
            identity,
            snoozed,
        }
    }
}

fn format_addr_lower(addr: &Addr) -> String {
    match &addr.name {
        Some(name) if !name.is_empty() => {
            format!("{} {}", name.to_lowercase(), addr.email.to_lowercase())
        }
        _ => addr.email.to_lowercase(),
    }
}

fn format_addrs_lower(addrs: &[Addr]) -> String {
    addrs
        .iter()
        .map(format_addr_lower)
        .collect::<Vec<_>>()
        .join(" ")
}

pub(super) fn scope(expr: &Expr) -> Scope {
    let mut found = Scope::Normal;
    walk_scope(expr, false, &mut found);
    found
}

fn walk_scope(expr: &Expr, negated: bool, found: &mut Scope) {
    match expr {
        Expr::Term(term) if !negated => match term {
            Term::InAnywhere => *found = Scope::Anywhere,
            Term::InTrash if *found == Scope::Normal => *found = Scope::Trash,
            Term::InSpam if *found == Scope::Normal => *found = Scope::Spam,
            Term::InDrafts if *found == Scope::Normal => *found = Scope::Drafts,
            Term::Text { .. }
            | Term::Me(_)
            | Term::Label(_)
            | Term::IsList
            | Term::IsUnread
            | Term::IsRead
            | Term::IsStarred
            | Term::IsSnoozed
            | Term::InInbox
            | Term::InSent
            | Term::InTrash
            | Term::InSpam
            | Term::InDrafts
            | Term::HasAttachment
            | Term::Category(_)
            | Term::After(_)
            | Term::Before(_) => {}
        },
        Expr::Term(_) | Expr::Unsupported => {}
        Expr::Not(inner) => walk_scope(inner, !negated, found),
        Expr::And(items) | Expr::Or(items) => {
            for item in items {
                walk_scope(item, negated, found);
            }
        }
    }
}

pub(super) fn reads_body(expr: &Expr) -> bool {
    match expr {
        Expr::Term(Term::Text {
            field: Field::Any, ..
        }) => true,
        Expr::Term(_) | Expr::Unsupported => false,
        Expr::Not(inner) => reads_body(inner),
        Expr::And(items) | Expr::Or(items) => items.iter().any(reads_body),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListRequirement {
    Any,
    OnlyList,
    ExcludeList,
    Impossible,
}

impl ListRequirement {
    #[must_use]
    const fn and(self, other: Self) -> Self {
        match (self, other) {
            (Self::Impossible, _)
            | (_, Self::Impossible)
            | (Self::OnlyList, Self::ExcludeList)
            | (Self::ExcludeList, Self::OnlyList) => Self::Impossible,
            (Self::Any, x) | (x, Self::Any) => x,
            (Self::OnlyList, Self::OnlyList) => Self::OnlyList,
            (Self::ExcludeList, Self::ExcludeList) => Self::ExcludeList,
        }
    }
}

pub(super) fn list_requirement(expr: &Expr) -> ListRequirement {
    match expr {
        Expr::Term(Term::IsList) => ListRequirement::OnlyList,
        Expr::Not(inner) => match list_requirement(inner) {
            ListRequirement::OnlyList => ListRequirement::ExcludeList,
            ListRequirement::ExcludeList => ListRequirement::OnlyList,
            ListRequirement::Any | ListRequirement::Impossible => ListRequirement::Any,
        },
        Expr::And(items) => items.iter().fold(ListRequirement::Any, |acc, item| {
            acc.and(list_requirement(item))
        }),
        Expr::Term(_) | Expr::Or(_) | Expr::Unsupported => ListRequirement::Any,
    }
}

pub(super) fn needs_post_filter(expr: &Expr) -> bool {
    match expr {
        Expr::Term(Term::Text { field, .. }) => !is_fts_field(*field),
        Expr::Unsupported | Expr::Term(_) | Expr::Not(_) => true,
        Expr::And(items) | Expr::Or(items) => items.iter().any(needs_post_filter),
    }
}

const fn is_fts_field(field: Field) -> bool {
    matches!(
        field,
        Field::Any | Field::From | Field::To | Field::Cc | Field::Subject
    )
}

pub(super) fn highlight_terms(expr: &Expr) -> Vec<String> {
    let mut out = Vec::new();
    collect_highlights(expr, false, &mut out);
    out
}

fn collect_highlights(expr: &Expr, negated: bool, out: &mut Vec<String>) {
    match expr {
        Expr::Term(Term::Text {
            field: Field::Any | Field::Subject,
            value,
            ..
        }) if !negated => {
            if !value.is_empty() && !out.iter().any(|t| t.eq_ignore_ascii_case(value)) {
                out.push(value.clone());
            }
        }
        Expr::Term(_) | Expr::Unsupported => {}
        Expr::Not(inner) => collect_highlights(inner, !negated, out),
        Expr::And(items) | Expr::Or(items) => {
            for item in items {
                collect_highlights(item, negated, out);
            }
        }
    }
}

pub(super) fn to_fts5(expr: &Expr) -> Option<String> {
    match expr {
        Expr::Term(Term::Text {
            field,
            value,
            prefix,
        }) => fts_term(*field, value, *prefix),
        Expr::Term(_) | Expr::Not(_) | Expr::Unsupported => None,
        Expr::And(items) => {
            let parts: Vec<String> = items.iter().filter_map(to_fts5).collect();
            (!parts.is_empty()).then(|| format!("({})", parts.join(" AND ")))
        }
        Expr::Or(items) => {
            let mut parts = Vec::with_capacity(items.len());
            for item in items {
                parts.push(to_fts5(item)?);
            }
            (!parts.is_empty()).then(|| format!("({})", parts.join(" OR ")))
        }
    }
}

fn fts_term(field: Field, value: &str, prefix: bool) -> Option<String> {
    let tokens: Vec<&str> = value
        .split(|c: char| !c.is_alphanumeric())
        .filter(|s| !s.is_empty())
        .collect();
    if tokens.is_empty() {
        return None;
    }
    let col = match field {
        Field::Any => "{from_text to_text cc_text subject body}",
        Field::From => "from_text",
        Field::To => "to_text",
        Field::Cc => "cc_text",
        Field::Subject => "subject",
        Field::Label => return None,
    };
    let last_idx = tokens.len() - 1;
    let terms: Vec<String> = tokens
        .iter()
        .enumerate()
        .map(|(i, tok)| {
            let escaped = tok.replace('"', "\"\"");
            if prefix && i == last_idx {
                format!("\"{escaped}\"*")
            } else {
                format!("\"{escaped}\"")
            }
        })
        .collect();
    Some(format!("{col} : {}", terms.join(" + ")))
}

pub(super) fn matches(expr: &Expr, target: &MatchTarget<'_>) -> bool {
    match expr {
        Expr::Unsupported => false,
        Expr::Term(term) => match_term(term, target),
        Expr::Not(inner) => !matches(inner, target),
        Expr::And(items) => items.iter().all(|item| matches(item, target)),
        Expr::Or(items) => items.iter().any(|item| matches(item, target)),
    }
}

fn match_term(term: &Term, target: &MatchTarget<'_>) -> bool {
    match term {
        Term::Text { field, value, .. } => match_text(*field, &value.to_lowercase(), target),
        Term::Me(field) => match_me(*field, target),
        Term::Label(name) => match_label(name, target),
        Term::IsList => target.message.is_list,
        Term::IsUnread => target.message.has(SystemLabel::Unread),
        Term::IsRead => !target.message.has(SystemLabel::Unread),
        Term::IsStarred => target.message.has(SystemLabel::Starred),
        Term::IsSnoozed => target.snoozed,
        Term::InInbox => target.message.has(SystemLabel::Inbox),
        Term::InSent => target.message.has(SystemLabel::Sent),
        Term::InTrash => target.message.has(SystemLabel::Trash),
        Term::InSpam => target.message.has(SystemLabel::Spam),
        Term::InDrafts => target.message.is_draft,
        Term::InAnywhere => true,
        Term::HasAttachment => target.message.has_attachments,
        Term::Category(cat) => {
            let full = format!("CATEGORY_{cat}");
            target
                .labels
                .iter()
                .any(|l| l.as_str().eq_ignore_ascii_case(&full))
        }
        Term::After(ms) => target.message.date >= *ms,
        Term::Before(ms) => target.message.date < *ms,
    }
}

fn match_text(field: Field, needle: &str, target: &MatchTarget<'_>) -> bool {
    match field {
        Field::From => target.from_lower.contains(needle),
        Field::To => target.to_lower.contains(needle),
        Field::Cc => target.cc_lower.contains(needle),
        Field::Subject => target.subject_lower.contains(needle),
        Field::Any => {
            target.subject_lower.contains(needle)
                || target.from_lower.contains(needle)
                || target.to_lower.contains(needle)
                || target.cc_lower.contains(needle)
                || target
                    .body_lower
                    .as_deref()
                    .is_some_and(|b| b.contains(needle))
        }
        Field::Label => match_label(needle, target),
    }
}

fn match_me(field: Field, target: &MatchTarget<'_>) -> bool {
    let Some(identity) = target.identity else {
        return false;
    };
    match field {
        Field::From => identity.is_mine(&target.message.from.email),
        Field::To => target.message.to.iter().any(|a| identity.is_mine(&a.email)),
        Field::Cc => target.message.cc.iter().any(|a| identity.is_mine(&a.email)),
        Field::Any | Field::Subject | Field::Label => false,
    }
}

fn match_label(want: &str, target: &MatchTarget<'_>) -> bool {
    let normalized = normalize_label(want);
    target.labels.iter().any(|id| {
        if id.as_str().eq_ignore_ascii_case(want) {
            return true;
        }
        target
            .user_labels
            .get(id)
            .is_some_and(|name| normalize_label(name) == normalized)
    })
}

fn normalize_label(name: &str) -> String {
    name.trim()
        .to_lowercase()
        .chars()
        .map(|c| if c == ' ' || c == '/' { '-' } else { c })
        .collect()
}

#[must_use]
pub fn gmail_query(raw: &str) -> String {
    let mut kept: Vec<(TokenKind, &str)> = Vec::new();
    for tok in lex(raw) {
        if matches!(&tok.kind, TokenKind::Word(w) if w.eq_ignore_ascii_case("is:list")) {
            while matches!(kept.last(), Some((TokenKind::Minus, _))) {
                kept.pop();
            }
            continue;
        }
        kept.push((tok.kind, &raw[tok.start..tok.end]));
    }

    loop {
        let before = kept.len();
        kept = prune_pass(&kept);
        if kept.len() == before {
            break;
        }
    }

    let mut out = String::new();
    for (idx, (kind, slice)) in kept.iter().enumerate() {
        let prev = idx.checked_sub(1).and_then(|p| kept.get(p));
        let need_space = idx > 0
            && !matches!(kind, TokenKind::RParen)
            && !matches!(
                prev.map(|(k, _)| k),
                Some(TokenKind::LParen | TokenKind::Minus)
            );
        if need_space {
            out.push(' ');
        }
        out.push_str(slice);
    }
    out
}

fn prune_pass<'a>(items: &[(TokenKind, &'a str)]) -> Vec<(TokenKind, &'a str)> {
    let mut out: Vec<(TokenKind, &'a str)> = Vec::with_capacity(items.len());
    let mut i = 0;
    while i < items.len() {
        let (kind, slice) = &items[i];
        let next = items.get(i + 1).map(|(k, _)| k);
        if matches!(kind, TokenKind::LParen) && matches!(next, Some(TokenKind::RParen)) {
            while matches!(out.last(), Some((TokenKind::Minus, _))) {
                out.pop();
            }
            i += 2;
            continue;
        }
        if matches!(kind, TokenKind::Minus)
            && matches!(next, None | Some(TokenKind::RParen | TokenKind::Minus))
        {
            i += 1;
            continue;
        }
        if is_bool_op(kind, slice) {
            let prev = out.last().map(|(k, s)| (k, *s));
            let prev_missing = prev.is_none()
                || matches!(prev, Some((TokenKind::LParen | TokenKind::Minus, _)))
                || prev.is_some_and(|(k, s)| is_bool_op(k, s));
            let next_missing = next.is_none() || matches!(next, Some(TokenKind::RParen));
            if prev_missing || next_missing {
                i += 1;
                continue;
            }
        }
        out.push((kind.clone(), slice));
        i += 1;
    }
    out
}

fn is_bool_op(kind: &TokenKind, slice: &str) -> bool {
    matches!(kind, TokenKind::Word(_)) && matches!(slice, "OR" | "AND")
}
