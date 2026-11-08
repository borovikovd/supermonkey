//! Values of single headers: message ids, dates, and the draft header.

use chrono::DateTime;
use sm_domain::{DraftId, Millis};

use crate::collapse_whitespace;

/// The header with the local id of a draft this app wrote (SPEC Drafts).
pub(crate) const DRAFT_HEADER: &str = "X-Supermonkey-Draft";

/// A message id or Content-ID without surrounding whitespace and angle brackets.
pub(crate) fn bare_id(id: &str) -> &str {
    id.trim().trim_start_matches('<').trim_end_matches('>')
}

/// Whether `id` can be written back into a header between `<` and `>`: not
/// empty, and no whitespace, controls, or brackets that would end it early.
pub(crate) fn is_message_id(id: &str) -> bool {
    !id.is_empty()
        && !id
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || c == '<' || c == '>')
}

/// The ids in a `Message-ID`, `In-Reply-To` or `References` value, without angle
/// brackets. A value with no brackets at all is read as whitespace-separated ids.
/// Ids that could not be written back safely are dropped.
pub(crate) fn message_ids(value: &str) -> Vec<String> {
    let ids: Vec<&str> = if value.contains('<') {
        value
            .split('<')
            .skip(1)
            .filter_map(|s| s.split_once('>').map(|(id, _)| id.trim()))
            .collect()
    } else {
        value.split_whitespace().collect()
    };
    ids.into_iter()
        .filter(|id| is_message_id(id))
        .map(str::to_owned)
        .collect()
}

/// An RFC 2822 `Date` value as a point in time. Tolerates a trailing comment
/// such as `(UTC)` and runs of whitespace.
pub(crate) fn parse_date(value: &str) -> Option<Millis> {
    let without_comment = value.split_once('(').map_or(value, |(date, _)| date);
    DateTime::parse_from_rfc2822(&collapse_whitespace(without_comment))
        .ok()
        .map(|date| Millis(date.timestamp_millis()))
}

/// Whether a local draft id may go into the draft header and the `Message-ID`
/// as it is: 1 to 128 ASCII letters, digits, `-`, `_` or `.`. The UI makes UUIDs;
/// anything else could inject header text, so it is never written or trusted.
pub(crate) fn is_draft_token(id: &str) -> bool {
    (1..=128).contains(&id.len())
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
}

/// The local draft id in a draft header value, when it is a valid token.
pub(crate) fn parse_draft_id(value: &str) -> Option<DraftId> {
    let id = value.trim();
    is_draft_token(id).then(|| DraftId::from(id))
}
