use std::cmp::Ordering;

use super::{Identity, Millis};

const HALF_LIFE_DAYS: f64 = 90.0;
const MS_PER_DAY: f64 = 86_400_000.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatchField {
    Name,
    Email,
}

impl MatchField {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Name => "name",
            Self::Email => "email",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MatchSpan {
    pub field: MatchField,
    pub start: u32,
    pub end: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContactStats {
    pub email: String,
    pub name: String,
    pub sent_count: u32,
    pub recv_count: u32,
    pub list_only: bool,
    pub last_contact: Millis,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ContactRank {
    pub tier: u8,
    pub score: f64,
    pub last_contact: Millis,
}

impl ContactRank {
    #[must_use]
    pub fn cmp_desc(self, other: Self) -> Ordering {
        other
            .tier
            .cmp(&self.tier)
            .then_with(|| other.score.total_cmp(&self.score))
            .then_with(|| other.last_contact.cmp(&self.last_contact))
    }
}

#[must_use]
pub fn is_excluded(stats: &ContactStats, identity: &Identity) -> bool {
    if identity.is_mine(&stats.email) {
        return true;
    }
    if stats.list_only && stats.sent_count == 0 {
        return true;
    }
    is_noreply(&stats.email)
}

#[must_use]
pub fn is_noreply(email: &str) -> bool {
    let local = email
        .trim()
        .split_once('@')
        .map_or(email, |(l, _)| l)
        .to_ascii_lowercase();
    matches!(
        local.as_str(),
        "noreply"
            | "no-reply"
            | "no_reply"
            | "donotreply"
            | "do-not-reply"
            | "do_not_reply"
            | "mailer-daemon"
            | "postmaster"
            | "notifications"
            | "notification"
            | "bounce"
            | "bounces"
    ) || local.starts_with("noreply+")
        || local.starts_with("no-reply+")
        || local.starts_with("donotreply+")
        || local.starts_with("do-not-reply+")
}

#[must_use]
pub fn rank(stats: &ContactStats, now: Millis) -> ContactRank {
    let (tier, count) = if stats.sent_count > 0 {
        (2, stats.sent_count)
    } else if stats.recv_count > 0 {
        (1, stats.recv_count)
    } else {
        (0, 1)
    };
    let age_ms = now.since(stats.last_contact).max(0);
    let age_secs = u32::try_from(age_ms / 1000).unwrap_or(u32::MAX);
    let age_days = (f64::from(age_secs) * 1000.0) / MS_PER_DAY;
    let decay = 0.5_f64.powf(age_days / HALF_LIFE_DAYS);
    let score = f64::from(count) * decay;
    ContactRank {
        tier,
        score,
        last_contact: stats.last_contact,
    }
}

#[must_use]
pub fn contact_matches(name: &str, email: &str, query: &str) -> Option<MatchSpan> {
    let needle = query.trim().to_lowercase();
    if needle.is_empty() {
        return None;
    }
    if let Some((start, end)) = match_word_start(name, &needle) {
        return Some(MatchSpan {
            field: MatchField::Name,
            start,
            end,
        });
    }
    if let Some((start, end)) = match_email(email, &needle) {
        return Some(MatchSpan {
            field: MatchField::Email,
            start,
            end,
        });
    }
    None
}

fn match_word_start(haystack: &str, needle: &str) -> Option<(u32, u32)> {
    let mut prev_sep = true;
    for (idx, ch) in haystack.char_indices() {
        if prev_sep && !is_word_sep(ch) {
            let tail = &haystack[idx..];
            if let Some(len) = prefix_len_ignore_case(tail, needle) {
                let start = u32::try_from(haystack[..idx].encode_utf16().count()).ok()?;
                let span = u32::try_from(tail[..len].encode_utf16().count()).ok()?;
                return Some((start, start.saturating_add(span)));
            }
        }
        prev_sep = is_word_sep(ch);
    }
    None
}

fn match_email(email: &str, needle: &str) -> Option<(u32, u32)> {
    if let Some(len) = prefix_len_ignore_case(email, needle) {
        let span = u32::try_from(email[..len].encode_utf16().count()).ok()?;
        return Some((0, span));
    }
    let at = email.find('@')?;
    let domain = &email[at + 1..];
    let len = prefix_len_ignore_case(domain, needle)?;
    let start = u32::try_from(email[..=at].encode_utf16().count()).ok()?;
    let span = u32::try_from(domain[..len].encode_utf16().count()).ok()?;
    Some((start, start.saturating_add(span)))
}

const fn is_word_sep(ch: char) -> bool {
    ch.is_whitespace() || matches!(ch, '-' | '_' | '.' | ',' | '(' | ')' | '/' | '\'')
}

fn prefix_len_ignore_case(haystack: &str, lower_needle: &str) -> Option<usize> {
    let mut needle_chars = lower_needle.chars();
    let mut current_target = needle_chars.next()?;
    let mut consumed = 0;

    for ch in haystack.chars() {
        consumed += ch.len_utf8();
        for lc in ch.to_lowercase() {
            if lc != current_target {
                return None;
            }
            match needle_chars.next() {
                Some(next) => current_target = next,
                None => return Some(consumed),
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id() -> Identity {
        Identity::new("ana@acme.test", "Ana", vec![])
    }

    #[test]
    fn matches_name_word_starts_and_email_and_domain() {
        assert_eq!(
            contact_matches("Mark Miller", "mm@marks.test", "mar"),
            Some(MatchSpan {
                field: MatchField::Name,
                start: 0,
                end: 3,
            })
        );
        assert_eq!(
            contact_matches("Mark Miller", "mm@marks.test", "mil"),
            Some(MatchSpan {
                field: MatchField::Name,
                start: 5,
                end: 8,
            })
        );
        assert_eq!(
            contact_matches("Sue", "sue@recv.test", "recv"),
            Some(MatchSpan {
                field: MatchField::Email,
                start: 4,
                end: 8,
            })
        );
        assert_eq!(contact_matches("Sue", "sue@recv.test", "ue"), None);
    }

    #[test]
    fn excludes_own_noreply_and_unsent_list_only() {
        let base = ContactStats {
            email: "alice@x.test".to_owned(),
            name: "Alice".to_owned(),
            sent_count: 0,
            recv_count: 1,
            list_only: false,
            last_contact: Millis(1000),
        };
        assert!(!is_excluded(&base, &id()), "regular contact is kept");
        let mine = ContactStats {
            email: "ANA@ACME.TEST".to_owned(),
            ..base.clone()
        };
        assert!(is_excluded(&mine, &id()), "own email is excluded");
        let noreply = ContactStats {
            email: "no-reply@x.test".to_owned(),
            ..base.clone()
        };
        assert!(is_excluded(&noreply, &id()), "noreply is excluded");
        let list = ContactStats {
            list_only: true,
            ..base.clone()
        };
        assert!(is_excluded(&list, &id()), "unsent list contact is excluded");
        let sent_list = ContactStats {
            sent_count: 1,
            list_only: true,
            ..base
        };
        assert!(
            !is_excluded(&sent_list, &id()),
            "sent-to list contact is kept"
        );
    }

    #[test]
    fn ranks_sent_over_received_and_decays_by_half_every_90_days() {
        let now = Millis(180 * 86_400_000);
        let old_sent = ContactStats {
            email: "a@x.test".to_owned(),
            name: "A".to_owned(),
            sent_count: 2,
            recv_count: 0,
            list_only: false,
            last_contact: Millis(90 * 86_400_000),
        };
        let fresh_sent = ContactStats {
            email: "b@x.test".to_owned(),
            name: "B".to_owned(),
            sent_count: 1,
            recv_count: 0,
            list_only: false,
            last_contact: now,
        };
        let heavy_recv = ContactStats {
            email: "c@x.test".to_owned(),
            name: "C".to_owned(),
            sent_count: 0,
            recv_count: 100,
            list_only: false,
            last_contact: now,
        };
        let r_old = rank(&old_sent, now);
        let r_fresh = rank(&fresh_sent, now);
        let r_recv = rank(&heavy_recv, now);
        assert!(
            (r_old.score - r_fresh.score).abs() < 1e-6,
            "2 sends 90 days ago equal 1 send today"
        );
        assert_eq!(
            r_fresh.cmp_desc(r_recv),
            Ordering::Less,
            "sent tier beats received tier"
        );
    }
}
