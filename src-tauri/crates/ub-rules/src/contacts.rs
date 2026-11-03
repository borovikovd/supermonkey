//! Recipient autocomplete: matching, filtering, and ranking contacts.

/// Recency half-life for ranking: a contact last seen this many days ago
/// counts half as much as one seen today.
pub const RECENCY_HALF_LIFE_DAYS: f64 = 90.0;

const MS_PER_DAY: f64 = 86_400_000.0;

/// Which string a [`MatchSpan`] points into.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatchField {
    Name,
    Email,
}

/// The matched part of a suggestion, to show in bold: `start..end` is a byte
/// range in the name or the email address as passed in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MatchSpan {
    pub field: MatchField,
    pub start: usize,
    pub end: usize,
}

/// Whether `query` matches a contact: case-insensitively, at the start of any
/// word in the name, the start of the address, or the start of the domain.
/// The query may run across words (`john sm` matches `John Smith`). Checks in
/// that order and returns the first hit; a blank query matches nothing.
pub fn contact_matches(query: &str, name: &str, email: &str) -> Option<MatchSpan> {
    let query = query.trim();
    if query.is_empty() {
        return None;
    }
    let span = |field, start, len| MatchSpan {
        field,
        start,
        end: start + len,
    };
    for start in word_starts(name) {
        if let Some(len) = prefix_len_ci(&name[start..], query) {
            return Some(span(MatchField::Name, start, len));
        }
    }
    if let Some(len) = prefix_len_ci(email, query) {
        return Some(span(MatchField::Email, 0, len));
    }
    let domain = email.rfind('@')? + 1;
    prefix_len_ci(&email[domain..], query).map(|len| span(MatchField::Email, domain, len))
}

/// Byte offsets where a word starts: an alphanumeric character at the start
/// or after a non-alphanumeric one (`Mary-Jane O'Brien` has four words).
fn word_starts(s: &str) -> impl Iterator<Item = usize> + '_ {
    let mut prev_alnum = false;
    s.char_indices().filter_map(move |(i, c)| {
        let starts = c.is_alphanumeric() && !prev_alnum;
        prev_alnum = c.is_alphanumeric();
        starts.then_some(i)
    })
}

/// If `haystack` starts with `needle` ignoring case, the byte length of that
/// prefix in `haystack`.
fn prefix_len_ci(haystack: &str, needle: &str) -> Option<usize> {
    let mut hay = haystack.char_indices();
    let mut len = 0;
    for n in needle.chars() {
        let (i, h) = hay.next()?;
        if h != n && !h.to_lowercase().eq(n.to_lowercase()) {
            return None;
        }
        len = i + h.len_utf8();
    }
    Some(len)
}

/// Whether the local part of `email` contains `noreply` or `no-reply`, ignoring case.
pub fn is_noreply(email: &str) -> bool {
    let local = email.rsplit_once('@').map_or(email, |(local, _)| local);
    let local = local.to_ascii_lowercase();
    local.contains("noreply") || local.contains("no-reply")
}

/// Whether a contact may be suggested at all. Never: the account's own
/// addresses, `noreply` addresses, and addresses never sent to that appeared
/// only on list mail.
pub fn is_suggestible(sent_count: u32, list_only: bool, is_noreply: bool, is_own: bool) -> bool {
    !is_own && !is_noreply && (sent_count != 0 || !list_only)
}

/// `0.5 ^ (age / 90 days)`: 1.0 for contact now, 0.5 at 90 days, 0.25 at
/// 180. A last contact in the future counts as now.
pub fn recency_factor(last_contact_ms: i64, now_ms: i64) -> f64 {
    let age_ms = now_ms.saturating_sub(last_contact_ms).max(0);
    // Whole seconds fit in f64 exactly for any span this ranking sees.
    let age_days =
        f64::from(u32::try_from(age_ms / 1000).unwrap_or(u32::MAX)) * 1000.0 / MS_PER_DAY;
    0.5f64.powf(age_days / RECENCY_HALF_LIFE_DAYS)
}

/// Ranking key for a suggestion: sort by `tier` ascending, then `score`
/// descending.
///
/// - tier 0, sent to: `score = sent_count * recency`
/// - tier 1, received from only: `score = received_count * recency`
/// - tier 2, seen only as a co-recipient: `score = recency`
///
/// `recency` is [`recency_factor`] (90-day half-life), so ten messages sent
/// last week outrank fifty sent a year ago.
pub fn contact_score(
    sent_count: u32,
    received_count: u32,
    last_contact_ms: i64,
    now_ms: i64,
) -> (u8, f64) {
    let recency = recency_factor(last_contact_ms, now_ms);
    if sent_count > 0 {
        (0, f64::from(sent_count) * recency)
    } else if received_count > 0 {
        (1, f64::from(received_count) * recency)
    } else {
        (2, recency)
    }
}
