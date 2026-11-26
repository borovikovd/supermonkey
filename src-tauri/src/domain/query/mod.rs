mod eval;
mod parse;

use chrono::{DateTime, TimeZone};
use thiserror::Error;

use super::Millis;

pub use eval::{MatchTarget, gmail_query};

#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("{message}")]
pub struct RuleError {
    pub offset: usize,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Field {
    Any,
    From,
    To,
    Cc,
    Subject,
    Label,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    Normal,
    Trash,
    Spam,
    Anywhere,
    Drafts,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Term {
    Text {
        field: Field,
        value: String,
        prefix: bool,
    },
    Me(Field),
    Label(String),
    IsList,
    IsUnread,
    IsRead,
    IsStarred,
    IsSnoozed,
    InInbox,
    InSent,
    InTrash,
    InSpam,
    InDrafts,
    InAnywhere,
    HasAttachment,
    Category(String),
    After(Millis),
    Before(Millis),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Expr {
    Term(Term),
    Not(Box<Self>),
    And(Vec<Self>),
    Or(Vec<Self>),
    Unsupported,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Query {
    pub(crate) expr: Expr,
}

impl Query {
    #[must_use]
    pub fn parse(input: &str) -> Self {
        parse::parse_lenient::<chrono::Utc>(input, None)
    }

    #[must_use]
    pub fn parse_search<Tz: TimeZone>(input: &str, now: &DateTime<Tz>) -> Self {
        parse::parse_lenient(input, Some(now))
    }

    pub fn parse_rule(input: &str) -> Result<Self, RuleError> {
        parse::parse_rule(input)
    }

    #[must_use]
    pub const fn is_empty(&self) -> bool {
        matches!(&self.expr, Expr::And(items) if items.is_empty())
    }

    #[must_use]
    pub const fn expr(&self) -> &Expr {
        &self.expr
    }

    #[must_use]
    pub fn searches_locally(&self) -> bool {
        !contains_unsupported(&self.expr)
    }

    #[must_use]
    pub fn needs_gmail(&self) -> bool {
        contains_unsupported(&self.expr)
    }

    #[must_use]
    pub fn scope(&self) -> Scope {
        eval::scope(&self.expr)
    }

    #[must_use]
    pub fn looks_in_trash(&self) -> bool {
        matches!(self.scope(), Scope::Trash | Scope::Spam | Scope::Anywhere)
    }

    #[must_use]
    pub fn reads_body(&self) -> bool {
        eval::reads_body(&self.expr)
    }

    #[must_use]
    pub fn needs_post_filter(&self) -> bool {
        eval::needs_post_filter(&self.expr)
    }

    #[must_use]
    pub fn highlight_terms(&self) -> Vec<String> {
        eval::highlight_terms(&self.expr)
    }

    #[must_use]
    pub fn to_fts5(&self) -> Option<String> {
        eval::to_fts5(&self.expr)
    }

    #[must_use]
    pub fn matches(&self, target: &MatchTarget<'_>) -> bool {
        eval::matches(&self.expr, target)
    }
}

fn contains_unsupported(expr: &Expr) -> bool {
    match expr {
        Expr::Unsupported => true,
        Expr::Term(_) => false,
        Expr::Not(inner) => contains_unsupported(inner),
        Expr::And(items) | Expr::Or(items) => items.iter().any(contains_unsupported),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use chrono::Utc;
    use rusqlite::Connection;

    use super::*;
    use crate::domain::{
        Addr, GmailMessageId, Identity, LabelId, LabelSet, MessageFacts, MessageId, SystemLabel,
    };

    fn sample_msg() -> MessageFacts {
        MessageFacts {
            id: MessageId(1),
            gmail_id: GmailMessageId("m1".to_owned()),
            date: Millis(1_793_620_800_000),
            from: Addr::named("Carol Chen", "carol@corp.test"),
            to: vec![Addr::named("Ana Ames", "ana@acme.test")],
            cc: vec![Addr::named("Dave", "dave@corp.test")],
            bcc: vec![],
            reply_to: vec![],
            subject: "Offsite venue".to_owned(),
            snippet: "Let us meet".to_owned(),
            message_id_hdr: None,
            in_reply_to: None,
            references: vec![],
            is_list: false,
            is_draft: false,
            has_attachments: true,
            labels: [
                SystemLabel::Inbox.label_id(),
                SystemLabel::Unread.label_id(),
                LabelId("L_rec".to_owned()),
            ]
            .into_iter()
            .collect::<LabelSet>(),
        }
    }

    #[test]
    fn rule_parser_rejects_negation_and_search_only_operators() {
        let err = Query::parse_rule("from:carol -venue").unwrap_err();
        assert_eq!(err.message, "-venue isn't supported in split rules");
        assert!(
            Query::parse_rule("in:inbox").is_err(),
            "in: is rejected in split rules"
        );
        assert!(
            Query::parse_rule("is:unread").is_err(),
            "is:unread is rejected in split rules"
        );
        assert!(
            Query::parse_rule("is:list").is_ok(),
            "is:list is allowed in split rules"
        );
        assert!(
            Query::parse_rule("from:carol OR subject:venue").is_ok(),
            "OR is allowed in split rules"
        );
        assert!(
            Query::parse_rule("label:CATEGORY_PROMOTIONS").is_ok(),
            "label: is allowed in split rules"
        );
    }

    #[test]
    fn search_operators_scope_and_unsupported_detection() {
        let now = Utc.with_ymd_and_hms(2026, 11, 2, 15, 0, 0).unwrap();
        assert_eq!(
            Query::parse_search("in:trash wombat", &now).scope(),
            Scope::Trash
        );
        assert_eq!(
            Query::parse_search("in:spam foo", &now).scope(),
            Scope::Spam
        );
        assert_eq!(
            Query::parse_search("in:anywhere foo", &now).scope(),
            Scope::Anywhere
        );
        assert!(
            Query::parse_search("venue -filename:pdf", &now).needs_gmail(),
            "filename: needs Gmail"
        );
        assert!(
            !Query::parse_search("venue -filename:pdf", &now).searches_locally(),
            "unsupported operator disables local search"
        );
        assert_eq!(
            Query::parse_search("from:dave plan", &now).highlight_terms(),
            vec!["plan".to_owned()]
        );
    }

    #[test]
    fn local_matching_and_fts5_escaping_work_with_sqlite() {
        let now = Utc.with_ymd_and_hms(2026, 11, 2, 15, 0, 0).unwrap();
        let msg = sample_msg();
        let id = Identity::new("ana@acme.test", "Ana", vec![]);
        let labels = HashMap::from([(LabelId("L_rec".to_owned()), "Receipts".to_owned())]);
        let target = MatchTarget::search(&msg, "The quokka arrived", &labels, &id, false);

        assert!(Query::parse_search("from:carol quokka", &now).matches(&target));
        assert!(Query::parse_search("label:receipts has:attachment", &now).matches(&target));
        assert!(!Query::parse_search("from:carol -venue", &now).matches(&target));
        assert!(Query::parse_search("to:me", &now).matches(&target));

        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE VIRTUAL TABLE t USING fts5(from_text, to_text, cc_text, subject, body, tokenize='unicode61 remove_diacritics 2');",
        )
        .unwrap();
        for raw in [
            "venue",
            "from:carol",
            "\"offsite venue\"",
            "a OR b",
            "c++ #1",
        ] {
            let q = Query::parse_search(raw, &now);
            if let Some(fts) = q.to_fts5() {
                let mut stmt = conn
                    .prepare("SELECT rowid FROM t WHERE t MATCH ?1")
                    .unwrap();
                let rows: Result<Vec<i64>, _> =
                    stmt.query_map([&fts], |r| r.get(0)).unwrap().collect();
                assert!(rows.is_ok(), "FTS5 accepts {fts:?} for {raw:?}");
            }
        }
    }

    #[test]
    fn gmail_query_strips_is_list_and_dangling_operators() {
        assert_eq!(gmail_query("is:list digest"), "digest");
        assert_eq!(gmail_query("is:list"), "");
        assert_eq!(gmail_query("-is:list"), "");
        assert_eq!(gmail_query("--is:list"), "");
        assert_eq!(gmail_query("(is:list)"), "");
        assert_eq!(gmail_query("is:list OR venue"), "venue");
        assert_eq!(gmail_query("venue AND is:list"), "venue");
    }
}
