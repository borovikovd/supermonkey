//! Recipient autocomplete (SPEC Recipients and autocomplete): matching,
//! filtering, and ranking contacts.

use crate::Millis;

/// A contact last seen this many days ago counts half as much as one seen today.
pub const RECENCY_HALF_LIFE_DAYS: f64 = 90.0;

const MS_PER_DAY: f64 = 86_400_000.0;

/// Which string a [`MatchSpan`] points into.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatchField {
    /// The display name.
    Name,
    /// The address.
    Email,
}

/// The matched part of a suggestion, to show in bold: `start..end` is a byte
/// range in the name or the address as passed in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MatchSpan {
    /// Which string.
    pub field: MatchField,
    /// First byte.
    pub start: usize,
    /// Byte past the end.
    pub end: usize,
}

/// Whether `query` matches a contact, ignoring case, at the start of any word
/// of the name, the start of the address, or the start of its domain, checked
/// in that order. The query may run across words (`john sm` matches `John
/// Smith`). A blank query matches nothing.
#[must_use]
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
        if let Some(len) = prefix_len(&name[start..], query) {
            return Some(span(MatchField::Name, start, len));
        }
    }
    if let Some(len) = prefix_len(email, query) {
        return Some(span(MatchField::Email, 0, len));
    }
    let domain = email.rfind('@')? + 1;
    prefix_len(&email[domain..], query).map(|len| span(MatchField::Email, domain, len))
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
fn prefix_len(haystack: &str, needle: &str) -> Option<usize> {
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
#[must_use]
pub fn is_noreply(email: &str) -> bool {
    let local = email
        .rsplit_once('@')
        .map_or(email, |(local, _)| local)
        .to_ascii_lowercase();
    local.contains("noreply") || local.contains("no-reply")
}

/// What autocomplete knows about one address.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContactStats {
    /// Messages this account sent to it.
    pub sent: u32,
    /// Messages this account received from it.
    pub received: u32,
    /// Seen only on list mail.
    pub list_only: bool,
    /// When it was last in a message.
    pub last_contact: Millis,
}

/// How a suggestion ranks: by `tier` ascending, then `score` descending.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ContactRank {
    /// 0: sent to; 1: received from only; 2: seen only as a co-recipient.
    pub tier: u8,
    /// Sent count (tier 0) or received count (tier 1) times the recency
    /// factor, or the factor alone (tier 2).
    pub score: f64,
}

impl ContactStats {
    /// Whether the address may be suggested at all. Never: the account's own
    /// addresses (`is_own`), `noreply` addresses, and addresses never sent to
    /// that appeared only on list mail.
    #[must_use]
    pub fn is_suggestible(self, email: &str, is_own: bool) -> bool {
        !is_own && !is_noreply(email) && (self.sent != 0 || !self.list_only)
    }

    /// The suggestion's rank at `now`. Ten messages sent last week outrank
    /// fifty sent a year ago.
    #[must_use]
    pub fn rank(self, now: Millis) -> ContactRank {
        let recency = recency_factor(self.last_contact, now);
        if self.sent > 0 {
            ContactRank {
                tier: 0,
                score: f64::from(self.sent) * recency,
            }
        } else if self.received > 0 {
            ContactRank {
                tier: 1,
                score: f64::from(self.received) * recency,
            }
        } else {
            ContactRank {
                tier: 2,
                score: recency,
            }
        }
    }
}

/// `0.5 ^ (age / 90 days)`: 1.0 for contact now, 0.5 at 90 days, 0.25 at
/// 180. A last contact in the future counts as now.
#[must_use]
pub fn recency_factor(last_contact: Millis, now: Millis) -> f64 {
    let age_ms = now.since(last_contact).max(0);
    // Whole seconds fit in f64 exactly for any span this ranking sees.
    let age_days =
        f64::from(u32::try_from(age_ms / 1000).unwrap_or(u32::MAX)) * 1000.0 / MS_PER_DAY;
    0.5f64.powf(age_days / RECENCY_HALF_LIFE_DAYS)
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY: i64 = 86_400_000;

    #[test]
    fn matches_word_starts_then_address_then_domain() {
        let hit = |q: &str| contact_matches(q, "Mary-Jane O'Brien", "mj@example.com");
        assert_eq!(
            hit("jan"),
            Some(MatchSpan {
                field: MatchField::Name,
                start: 5,
                end: 8
            })
        );
        assert_eq!(
            hit("BRI"),
            Some(MatchSpan {
                field: MatchField::Name,
                start: 12,
                end: 15
            })
        );
        assert_eq!(
            hit("mj@"),
            Some(MatchSpan {
                field: MatchField::Email,
                start: 0,
                end: 3
            })
        );
        assert_eq!(
            hit("exa"),
            Some(MatchSpan {
                field: MatchField::Email,
                start: 3,
                end: 6
            })
        );
        assert_eq!(hit("ane"), None, "not a word start");
        assert_eq!(hit("  "), None);
        assert!(contact_matches("john sm", "John Smith", "j@x.io").is_some());
    }

    #[test]
    fn noreply_and_suggestible() {
        assert!(is_noreply("NoReply@x.io"));
        assert!(is_noreply("billing-no-reply@x.io"));
        assert!(
            !is_noreply("alice@noreply.io"),
            "only the local part counts"
        );
        let stats = ContactStats {
            sent: 0,
            received: 3,
            list_only: false,
            last_contact: Millis(0),
        };
        assert!(stats.is_suggestible("a@x.io", false));
        assert!(!stats.is_suggestible("a@x.io", true));
        assert!(!stats.is_suggestible("noreply@x.io", false));
        let list = ContactStats {
            list_only: true,
            ..stats
        };
        assert!(!list.is_suggestible("a@x.io", false));
        assert!(ContactStats { sent: 1, ..list }.is_suggestible("a@x.io", false));
    }

    #[test]
    fn ranking_tiers_and_recency() {
        let now = Millis(1000 * DAY);
        assert!((recency_factor(now, now) - 1.0).abs() < 1e-9);
        assert!((recency_factor(Millis(910 * DAY), now) - 0.5).abs() < 1e-9);
        assert!((recency_factor(Millis(2000 * DAY), now) - 1.0).abs() < 1e-9);
        let base = ContactStats {
            sent: 0,
            received: 0,
            list_only: false,
            last_contact: now,
        };
        assert_eq!(ContactStats { sent: 2, ..base }.rank(now).tier, 0);
        assert_eq!(
            ContactStats {
                received: 2,
                ..base
            }
            .rank(now)
            .tier,
            1
        );
        assert_eq!(base.rank(now).tier, 2);
        let recent = ContactStats {
            sent: 10,
            last_contact: Millis(993 * DAY),
            ..base
        };
        let old = ContactStats {
            sent: 50,
            last_contact: Millis(635 * DAY),
            ..base
        };
        assert!(recent.rank(now).score > old.rank(now).score);
    }
}
