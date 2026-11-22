use chrono::DateTime;
use sm_domain::{DraftId, Millis};

use crate::collapse_whitespace;

pub(crate) const DRAFT_HEADER: &str = "X-Supermonkey-Draft";

pub(crate) fn bare_id(id: &str) -> &str {
    id.trim().trim_start_matches('<').trim_end_matches('>')
}

pub(crate) fn is_message_id(id: &str) -> bool {
    !id.is_empty()
        && !id
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || c == '<' || c == '>')
}

pub(crate) fn message_ids(value: &str) -> Vec<&str> {
    if value.contains('<') {
        value
            .split('<')
            .skip(1)
            .filter_map(|s| s.split_once('>').map(|(id, _)| id.trim()))
            .filter(|id| is_message_id(id))
            .collect()
    } else {
        value
            .split_whitespace()
            .filter(|id| is_message_id(id))
            .collect()
    }
}

pub(crate) fn parse_date(value: &str) -> Option<Millis> {
    let without_comment = value.split_once('(').map_or(value, |(date, _)| date);
    DateTime::parse_from_rfc2822(&collapse_whitespace(without_comment))
        .ok()
        .map(|date| Millis(date.timestamp_millis()))
}

pub(crate) fn is_draft_token(id: &str) -> bool {
    (1..=128).contains(&id.len())
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
}

pub(crate) fn parse_draft_id(value: &str) -> Option<DraftId> {
    let id = value.trim();
    is_draft_token(id).then(|| DraftId::from(id))
}
