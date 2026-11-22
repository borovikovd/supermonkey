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
