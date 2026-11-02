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
    let age_days = now_ms.saturating_sub(last_contact_ms).max(0) as f64 / MS_PER_DAY;
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

#[cfg(test)]
mod tests {
    use super::*;

    fn hit(query: &str, name: &str, email: &str) -> Option<(MatchField, String)> {
        contact_matches(query, name, email).map(|s| {
            let source = match s.field {
                MatchField::Name => name,
                MatchField::Email => email,
            };
            (s.field, source[s.start..s.end].to_owned())
        })
    }

    #[test]
    fn matches_word_starts_in_name() {
        let name = "Mary-Jane O'Brien Smith";
        let email = "mj@example.com";
        assert_eq!(
            hit("mar", name, email),
            Some((MatchField::Name, "Mar".into()))
        );
        assert_eq!(
            hit("JANE", name, email),
            Some((MatchField::Name, "Jane".into()))
        );
        assert_eq!(
            hit("bri", name, email),
            Some((MatchField::Name, "Bri".into()))
        );
        assert_eq!(
            hit("smi", name, email),
            Some((MatchField::Name, "Smi".into()))
        );
        assert_eq!(
            hit("o'b", name, email),
            Some((MatchField::Name, "O'B".into()))
        );
        // Not the middle of a word.
        assert_eq!(hit("ane", name, email), None);
        assert_eq!(hit("mith", name, email), None);
    }

    #[test]
    fn query_can_span_words() {
        assert_eq!(
            hit("john sm", "John Smith", "js@x.org"),
            Some((MatchField::Name, "John Sm".into()))
        );
        assert_eq!(
            hit("van der b", "Anna van der Berg", "a@x.org"),
            Some((MatchField::Name, "van der B".into()))
        );
        assert_eq!(hit("john x", "John Smith", "js@x.org"), None);
    }

    #[test]
    fn matches_address_and_domain_starts() {
        let email = "alice.smith@Example.co.uk";
        assert_eq!(
            hit("alice.s", "", email),
            Some((MatchField::Email, "alice.s".into()))
        );
        assert_eq!(
            hit("exa", "", email),
            Some((MatchField::Email, "Exa".into()))
        );
        let span = contact_matches("exa", "", email).unwrap();
        assert_eq!((span.start, span.end), (12, 15));
        // Not the start of the local part's second word, nor mid-domain.
        assert_eq!(hit("smith", "", email), None);
        assert_eq!(hit("co.uk", "", email), None);
        assert_eq!(hit("ample", "", email), None);
    }

    #[test]
    fn name_wins_over_address() {
        let span = contact_matches("al", "Alice", "alice@al.com").unwrap();
        assert_eq!(span.field, MatchField::Name);
        let span = contact_matches("al", "Bob", "bob@al.com").unwrap();
        assert_eq!(
            (span.field, span.start, span.end),
            (MatchField::Email, 4, 6)
        );
    }

    #[test]
    fn blank_and_overlong_queries() {
        assert_eq!(hit("", "Alice", "a@b.c"), None);
        assert_eq!(hit("   ", "Alice", "a@b.c"), None);
        assert_eq!(hit("alicex", "Alice", "a@b.c"), None);
        assert_eq!(
            hit(" ali ", "Alice", "a@b.c"),
            Some((MatchField::Name, "Ali".into()))
        );
        assert_eq!(hit("x", "", "no-at-sign"), None);
    }

    #[test]
    fn unicode_names_give_valid_byte_ranges() {
        let name = "Zoë Ångström";
        let s = contact_matches("ång", name, "z@x.se").unwrap();
        assert_eq!(&name[s.start..s.end], "Ång");
        let s = contact_matches("zoë", name, "z@x.se").unwrap();
        assert_eq!(&name[s.start..s.end], "Zoë");
        assert_eq!(
            hit("дми", "Дмитрий Иванов", "d@x.ru"),
            Some((MatchField::Name, "Дми".into()))
        );
        assert_eq!(
            hit("ив", "Дмитрий Иванов", "d@x.ru"),
            Some((MatchField::Name, "Ив".into()))
        );
    }

    #[test]
    fn noreply() {
        assert!(is_noreply("noreply@github.com"));
        assert!(is_noreply("No-Reply@x.com"));
        assert!(is_noreply("billing-noreply@x.com"));
        assert!(is_noreply("DoNotReply-NOREPLY@x.com"));
        assert!(
            !is_noreply("alice@noreply.com"),
            "only the local part counts"
        );
        assert!(!is_noreply("reply@x.com"));
        assert!(!is_noreply("donotreply@x.com"));
    }

    #[test]
    fn suggestible() {
        assert!(is_suggestible(0, false, false, false));
        assert!(
            is_suggestible(3, true, false, false),
            "sent to beats list-only"
        );
        assert!(!is_suggestible(0, true, false, false));
        assert!(!is_suggestible(5, false, true, false));
        assert!(!is_suggestible(5, false, false, true));
    }

    const DAY: i64 = 86_400_000;
    const NOW: i64 = 1_800_000_000_000;

    #[test]
    fn recency_halves_every_90_days() {
        assert_eq!(recency_factor(NOW, NOW), 1.0);
        assert_eq!(recency_factor(NOW + DAY, NOW), 1.0);
        assert!((recency_factor(NOW - 90 * DAY, NOW) - 0.5).abs() < 1e-12);
        assert!((recency_factor(NOW - 180 * DAY, NOW) - 0.25).abs() < 1e-12);
        assert!(recency_factor(i64::MIN, NOW) >= 0.0);
    }

    #[test]
    fn tiers_and_scores() {
        assert_eq!(contact_score(4, 9, NOW, NOW), (0, 4.0));
        assert_eq!(contact_score(0, 9, NOW, NOW), (1, 9.0));
        assert_eq!(contact_score(0, 0, NOW, NOW), (2, 1.0));

        // Recent beats frequent-but-stale within a tier.
        let recent = contact_score(10, 0, NOW - 7 * DAY, NOW);
        let stale = contact_score(50, 0, NOW - 365 * DAY, NOW);
        assert!(recent.1 > stale.1);
        // Any tier 0 contact sorts before any tier 1 contact.
        let mut ranked = [
            ("co", contact_score(0, 0, NOW, NOW)),
            ("received", contact_score(0, 500, NOW, NOW)),
            ("sent-stale", contact_score(1, 0, NOW - 1000 * DAY, NOW)),
            ("sent", contact_score(3, 0, NOW, NOW)),
        ];
        ranked.sort_by(|a, b| a.1.0.cmp(&b.1.0).then(b.1.1.total_cmp(&a.1.1)));
        let order: Vec<_> = ranked.iter().map(|r| r.0).collect();
        assert_eq!(order, ["sent", "sent-stale", "received", "co"]);
    }
}
