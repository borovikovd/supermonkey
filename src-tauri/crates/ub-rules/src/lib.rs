//! Pure rule logic for Ubermensch: split rules and local search, the
//! Important rule, snooze times, and contact autocomplete. No I/O.
//!
//! # Queries ([`query`])
//!
//! ```text
//! parse(input: &str) -> Query                               // lenient, never fails
//! parse_strict(input: &str) -> Result<Query, ParseError>    // split settings sheet
//! Query::matches<S: RuleSubject + ?Sized>(&self, &S, Mode) -> bool
//! Query::to_fts5(&self) -> Option<String>                   // FTS5 MATCH expression
//! Query::needs_post_filter(&self) -> bool                   // has bcc:/label:/is:list
//! Query::has_local_terms(&self) -> bool                     // false: empty or only unsupported
//! strip_is_list(raw: &str) -> String                        // the query to send Gmail
//! ```
//!
//! [`Query`] wraps an [`Expr`] tree (`And`, `Or`, `Term`, `IsList`,
//! `Unsupported`). [`RuleSubject`] is how the caller exposes a message;
//! [`MessageFields`] is a ready-made owned implementation. [`Mode::Rule`]
//! matches bare words against the sender and subject; [`Mode::Search`] adds
//! the body.
//!
//! Local search: run `to_fts5()` against an FTS5 table with columns
//! `from_text, to_text, cc_text, subject, body`. It selects a superset of
//! the matches. When it returns `None`, or `needs_post_filter()` is true,
//! filter the candidates with `matches(.., Mode::Search)`. When
//! `has_local_terms()` is false there is nothing to search locally.
//!
//! # Splits ([`splits`])
//!
//! ```text
//! is_person_to_person<S: AsRef<str>>(message_label_ids: &[S], is_list: bool) -> bool
//! STARTER_SPLITS: [StarterSplit { name, query }; 4]
//! ```
//!
//! # Snooze ([`snooze`])
//!
//! ```text
//! snooze_options<Tz: TimeZone>(now: &DateTime<Tz>) -> Vec<SnoozeOption<Tz>>
//! parse_snooze_text<Tz: TimeZone>(text: &str, now: &DateTime<Tz>) -> Option<DateTime<Tz>>
//! resolve_local<Tz: TimeZone>(tz: &Tz, naive: NaiveDateTime) -> DateTime<Tz>
//! ```
//!
//! Pass `chrono::Local::now()` in the app.
//!
//! # Contacts ([`contacts`])
//!
//! ```text
//! contact_matches(query: &str, name: &str, email: &str) -> Option<MatchSpan>
//! is_noreply(email: &str) -> bool
//! is_suggestible(sent_count: u32, list_only: bool, is_noreply: bool, is_own: bool) -> bool
//! contact_score(sent_count: u32, received_count: u32, last_contact_ms: i64, now_ms: i64) -> (u8, f64)
//! recency_factor(last_contact_ms: i64, now_ms: i64) -> f64   // 90-day half-life
//! ```

pub mod contacts;
pub mod query;
pub mod snooze;
pub mod splits;

pub use contacts::{
    MatchField, MatchSpan, contact_matches, contact_score, is_noreply, is_suggestible,
};
pub use query::{
    Address, Expr, Field, MessageFields, Mode, ParseError, Query, RuleSubject, Term, parse,
    parse_strict, strip_is_list,
};
pub use snooze::{SnoozeKind, SnoozeOption, parse_snooze_text, snooze_options};
pub use splits::{STARTER_SPLITS, StarterSplit, is_person_to_person};
