use crate::Millis;

const RECENCY_HALF_LIFE_DAYS: f64 = 90.0;

const MS_PER_DAY: f64 = 86_400_000.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatchField {
    Name,
    Email,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MatchSpan {
    pub field: MatchField,
    pub start: usize,
    pub end: usize,
}

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

fn word_starts(s: &str) -> impl Iterator<Item = usize> + '_ {
    let mut prev_alnum = false;
    s.char_indices().filter_map(move |(i, c)| {
        let starts = c.is_alphanumeric() && !prev_alnum;
        prev_alnum = c.is_alphanumeric();
        starts.then_some(i)
    })
}

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

#[must_use]
fn is_noreply(email: &str) -> bool {
    let local = email
        .rsplit_once('@')
        .map_or(email, |(local, _)| local)
        .to_ascii_lowercase();
    local.contains("noreply") || local.contains("no-reply")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContactStats {
    pub sent: u32,
    pub received: u32,
    pub list_only: bool,
    pub last_contact: Millis,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ContactRank {
    pub tier: u8,
    pub score: f64,
}

impl ContactStats {
    #[must_use]
    pub fn is_suggestible(self, email: &str, is_own: bool) -> bool {
        !is_own && !is_noreply(email) && (self.sent != 0 || !self.list_only)
    }

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

#[must_use]
fn recency_factor(last_contact: Millis, now: Millis) -> f64 {
    let age_ms = now.since(last_contact).max(0);
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
