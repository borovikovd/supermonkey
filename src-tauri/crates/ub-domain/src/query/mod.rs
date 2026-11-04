//! The custom-rule and local-search query language (SPEC Custom rules, Search).
//!
//! Supported: `from:`, `to:`, `cc:`, `bcc:`, `subject:`, `label:`, `is:list`,
//! bare words, `"quoted phrases"`, `AND`, `OR`, and parentheses. Terms next to
//! each other are AND, and AND binds tighter than OR. Operator names, `AND`,
//! `OR`, and all matching are case-insensitive.
//!
//! Other Gmail syntax (`has:attachment`, `after:2024/01/01`, `-foo`, `in:inbox`,
//! `from:(a b)`, ...) parses to [`Expr::Unsupported`], which always matches, so
//! local search ignores it while the raw string still goes to Gmail.
//!
//! - [`Query::parse`] never fails: unbalanced parentheses, an unclosed quote,
//!   a dangling `OR`, or an operator with no value yet degrade to a
//!   best-effort query, for search as you type.
//! - [`Query::parse_rule`] reports those as a [`RuleError`] for the split
//!   settings sheet, and also rejects unsupported terms, since a rule never
//!   goes to Gmail.
//!
//! Local search runs [`Query::to_fts5`] against an FTS5 table with the
//! [`FTS_COLUMNS`]. That selects a superset of the matches: when it is `None`
//! or [`Query::needs_post_filter`] is true, filter the candidates with
//! [`Query::matches`] on a [`MatchTarget::search`]. When
//! [`Query::has_local_terms`] is false there is nothing to search locally.

mod ast;
mod error;
mod fts;
mod gmail;
mod lexer;
mod matching;
mod parser;

#[cfg(test)]
mod tests;

pub use ast::{Expr, Field, Query, Term};
pub use error::RuleError;
pub use fts::FTS_COLUMNS;
pub use gmail::gmail_query;
pub use matching::MatchTarget;
