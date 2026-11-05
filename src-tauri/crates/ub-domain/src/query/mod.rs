//! The custom-rule and local-search query language (SPEC Custom rules, Search).
//!
//! Split rules: `from:`, `to:`, `cc:`, `bcc:`, `subject:`, `label:`,
//! `is:list`, bare words, `"quoted phrases"`, `AND`, `OR`, and parentheses.
//! Terms next to each other are AND, and AND binds tighter than OR. Operator
//! names, `AND`, `OR`, and all matching are case-insensitive.
//!
//! Search also evaluates, as Gmail does: `-term` and `-(group)`; `in:` a
//! system or user label, `in:anywhere`, `in:snoozed`, `in:drafts`; `is:`
//! `unread`, `read`, `starred`, `important`, `snoozed`; `has:attachment`;
//! `category:`; `after:`, `before:`, `newer:`, `older:`, `newer_than:`, and
//! `older_than:`; and `from:me` and the like. A split rule refuses these.
//!
//! Other Gmail syntax (`filename:pdf`, `larger:1M`, `from:(a b)`, a date
//! that is no date, ...) parses to [`Expr::Unsupported`]. A query holding
//! one [`Query::needs_gmail`]: local search lists nothing for it rather than
//! guess, while the raw string still goes to Gmail.
//!
//! - [`Query::parse`] never fails: unbalanced parentheses, an unclosed quote,
//!   a dangling `OR`, or an operator with no value yet degrade to a
//!   best-effort query, for search as you type. [`Query::parse_search`]
//!   does the same, and places dates in the user's time zone.
//! - [`Query::parse_rule`] reports those as a [`RuleError`] for the split
//!   settings sheet, and also rejects unsupported and search-only terms,
//!   since a rule never goes to Gmail.
//!
//! Local search runs [`Query::to_fts5`] against an FTS5 table with the
//! [`FTS_COLUMNS`]. That selects a superset of the matches: when it is `None`
//! or [`Query::needs_post_filter`] is true, filter the candidates with
//! [`Query::matches`] on a [`MatchTarget::search`], for the messages
//! [`Query::scope`] admits. When [`Query::searches_locally`] is false there
//! is nothing to search locally.

mod ast;
mod dates;
mod error;
mod fts;
mod gmail;
mod lexer;
mod matching;
mod parser;

#[cfg(test)]
mod tests;

pub use ast::{Expr, Field, Query, Scope, Term};
pub use error::RuleError;
pub use fts::FTS_COLUMNS;
pub use gmail::gmail_query;
pub use matching::MatchTarget;
