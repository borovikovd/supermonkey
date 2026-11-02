//! RFC 5322 address lists, as they show up in real mail.

use serde::{Deserialize, Serialize};

use crate::collapse_whitespace;
use crate::encoding::decode_encoded_words;

/// One mailbox: an optional display name and an address.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Addr {
    pub name: Option<String>,
    pub email: String,
}

impl Addr {
    /// An address without a display name.
    pub fn new(email: impl Into<String>) -> Self {
        Addr {
            name: None,
            email: email.into(),
        }
    }

    /// An address with a display name.
    pub fn named(name: impl Into<String>, email: impl Into<String>) -> Self {
        Addr {
            name: Some(name.into()),
            email: email.into(),
        }
    }
}

/// Parses an address-list header (`From`, `To`, `Cc`, `Reply-To`, ...).
///
/// Handles quoted names with escapes, `addr (Comment Name)`, RFC 2047 encoded-words, groups
/// (`team: a@x, b@y;`, `undisclosed-recipients:;`), unquoted names with commas
/// (`Doe, John <j@x.com>`), and `mailto:` inside angle brackets. Entries without an `@`
/// are dropped. Never panics.
pub fn parse_address_list(header: &str) -> Vec<Addr> {
    let mut out = Vec::new();
    // Segments without an `@` are usually half of a name that held a comma; they are
    // carried into the next segment.
    let mut carry = String::new();
    for segment in split_segments(header) {
        let segment = segment.trim();
        if segment.is_empty() {
            continue;
        }
        if !segment.contains('@') {
            carry.push_str(segment);
            carry.push_str(", ");
            continue;
        }
        let full = std::mem::take(&mut carry) + segment;
        if let Some(addr) = parse_mailbox(&full) {
            out.push(addr);
        }
    }
    out
}

/// Splits on top-level `,` and `;`, dropping group names (`name:`).
fn split_segments(header: &str) -> Vec<String> {
    let mut segments = Vec::new();
    let mut current = String::new();
    let mut in_quote = false;
    let mut escaped = false;
    let mut in_angle = false;
    let mut comment_depth = 0u32;

    for c in header.chars() {
        if escaped {
            escaped = false;
            current.push(c);
            continue;
        }
        match c {
            '\\' if in_quote || comment_depth > 0 => escaped = true,
            '"' if comment_depth == 0 => in_quote = !in_quote,
            '(' if !in_quote => comment_depth += 1,
            ')' if !in_quote && comment_depth > 0 => comment_depth -= 1,
            '<' if !in_quote && comment_depth == 0 => in_angle = true,
            '>' if !in_quote && comment_depth == 0 => in_angle = false,
            ',' | ';' if !in_quote && comment_depth == 0 && !in_angle => {
                segments.push(std::mem::take(&mut current));
                continue;
            }
            ':' if !in_quote && comment_depth == 0 && !in_angle && !current.contains('@') => {
                // Group start: what came before is the group's name.
                current.clear();
                continue;
            }
            _ => {}
        }
        current.push(c);
    }
    segments.push(current);
    segments
}

/// Parses one mailbox: `Name <addr>`, `"Name" <addr>`, `addr (Name)`, or a bare address.
fn parse_mailbox(s: &str) -> Option<Addr> {
    const PHRASE: usize = 0;
    const ANGLE: usize = 1;
    const COMMENT: usize = 2;
    let mut bufs = [String::new(), String::new(), String::new()];
    let mut has_angle = false;
    let mut in_quote = false;
    let mut escaped = false;
    let mut in_angle = false;
    let mut comment_depth = 0u32;

    for c in s.chars() {
        let dest = if comment_depth > 0 {
            COMMENT
        } else if in_angle {
            ANGLE
        } else {
            PHRASE
        };
        if escaped {
            escaped = false;
            bufs[dest].push(c);
            continue;
        }
        match c {
            '\\' if in_quote || comment_depth > 0 => escaped = true,
            '"' if comment_depth == 0 && !in_angle => in_quote = !in_quote,
            '(' if !in_quote => {
                if comment_depth > 0 {
                    bufs[COMMENT].push(c);
                }
                comment_depth += 1;
            }
            ')' if !in_quote && comment_depth > 0 => {
                comment_depth -= 1;
                bufs[COMMENT].push(if comment_depth > 0 { c } else { ' ' });
            }
            '<' if !in_quote && comment_depth == 0 => {
                in_angle = true;
                has_angle = true;
                bufs[ANGLE].clear();
            }
            '>' if !in_quote && comment_depth == 0 => in_angle = false,
            _ => bufs[dest].push(c),
        }
    }

    let [phrase, angle, comment] = bufs;
    let (email, name) = if has_angle {
        (angle, phrase)
    } else {
        (phrase, comment)
    };
    let email = clean_email(&email);
    if email.is_empty() {
        return None;
    }
    let name = clean_name(&name).filter(|n| !n.eq_ignore_ascii_case(&email));
    Some(Addr { name, email })
}

fn clean_email(s: &str) -> String {
    let s: String = s.chars().filter(|c| !c.is_whitespace()).collect();
    let s = s.trim_matches(|c| c == '"' || c == '\'');
    let s = match s.get(..7) {
        Some(p) if p.eq_ignore_ascii_case("mailto:") => &s[7..],
        _ => s,
    };
    s.to_string()
}

fn clean_name(s: &str) -> Option<String> {
    let collapsed = collapse_whitespace(&decode_encoded_words(s.trim()));
    let trimmed = collapsed.trim_matches(|c| c == '"' || c == '\'').trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

/// A practical address check for recipient chips: one `@`, a non-empty local part without
/// spaces or specials, and a dotted domain whose labels are letters, digits, or hyphens
/// (non-ASCII letters allowed) with a TLD of at least two characters.
pub fn is_valid_email(s: &str) -> bool {
    let Some((local, domain)) = s.rsplit_once('@') else {
        return false;
    };
    let bad_local = |c: char| c.is_whitespace() || c.is_control() || "<>()[],;:\\\"@".contains(c);
    if s.len() > 254
        || local.is_empty()
        || local.len() > 64
        || local.chars().any(bad_local)
        || local.starts_with('.')
        || local.ends_with('.')
        || local.contains("..")
    {
        return false;
    }
    let labels: Vec<&str> = domain.split('.').collect();
    let tld = labels.last().copied().unwrap_or("");
    labels.len() >= 2
        && labels.iter().all(|l| {
            !l.is_empty()
                && l.len() <= 63
                && !l.starts_with('-')
                && !l.ends_with('-')
                && l.chars().all(|c| c.is_alphanumeric() || c == '-')
        })
        && tld.chars().count() >= 2
        && !tld.chars().all(|c| c.is_ascii_digit())
}

/// Formats an address as `Name <email>`, quoting the name when it holds specials
/// (`"Doe, John" <j@x.com>`), or just `email` without a name. The result is Unicode;
/// `build_mime` does its own RFC 2047 encoding for real headers.
pub fn format_addr(a: &Addr) -> String {
    match a.name.as_deref().map(str::trim) {
        None | Some("") => a.email.clone(),
        Some(name) if name.contains(|c| "()<>[]:;@\\,.\"".contains(c)) => {
            let escaped = name.replace('\\', "\\\\").replace('"', "\\\"");
            format!("\"{escaped}\" <{}>", a.email)
        }
        Some(name) => format!("{name} <{}>", a.email),
    }
}

/// Formats a list with [`format_addr`], joined by `, `.
pub fn format_addr_list(list: &[Addr]) -> String {
    list.iter().map(format_addr).collect::<Vec<_>>().join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> Vec<Addr> {
        parse_address_list(s)
    }

    #[test]
    fn simple_forms() {
        assert_eq!(p("a@x.com"), vec![Addr::new("a@x.com")]);
        assert_eq!(p("<a@x.com>"), vec![Addr::new("a@x.com")]);
        assert_eq!(p("Alice <a@x.com>"), vec![Addr::named("Alice", "a@x.com")]);
        assert_eq!(
            p("Alice Smith <a@x.com>, bob@y.org"),
            vec![
                Addr::named("Alice Smith", "a@x.com"),
                Addr::new("bob@y.org")
            ]
        );
    }

    #[test]
    fn quoted_names_with_specials() {
        assert_eq!(
            p(r#""Doe, John" <j@x.com>, "O\"Neil; Pat" <p@x.com>"#),
            vec![
                Addr::named("Doe, John", "j@x.com"),
                Addr::named("O\"Neil; Pat", "p@x.com")
            ]
        );
        assert_eq!(p(r#""a@b.com" <a@b.com>"#), vec![Addr::new("a@b.com")]);
    }

    #[test]
    fn unquoted_name_with_comma_is_rejoined() {
        assert_eq!(
            p("Doe, John <j@x.com>"),
            vec![Addr::named("Doe, John", "j@x.com")]
        );
    }

    #[test]
    fn comment_names() {
        assert_eq!(
            p("j@x.com (John Doe)"),
            vec![Addr::named("John Doe", "j@x.com")]
        );
        assert_eq!(
            p("John (the (great)) <j@x.com>"),
            vec![Addr::named("John", "j@x.com")]
        );
    }

    #[test]
    fn encoded_word_names() {
        assert_eq!(
            p("=?UTF-8?B?w4lsb2RpZSBEdXBvbnQ=?= <e@x.fr>"),
            vec![Addr::named("Élodie Dupont", "e@x.fr")]
        );
        assert_eq!(
            p(
                "\"=?iso-8859-1?Q?Andr=E9?= Pirard\" <a@x.be>, =?UTF-8?Q?M=C3=BCller=2C_K?= <m@x.de>"
            ),
            vec![
                Addr::named("André Pirard", "a@x.be"),
                Addr::named("Müller, K", "m@x.de")
            ]
        );
    }

    #[test]
    fn groups() {
        assert_eq!(p("undisclosed-recipients:;"), vec![]);
        assert_eq!(
            p("Team: a@x.com, Bob <b@x.com>; c@y.com"),
            vec![
                Addr::new("a@x.com"),
                Addr::named("Bob", "b@x.com"),
                Addr::new("c@y.com")
            ]
        );
    }

    #[test]
    fn odd_input_never_panics() {
        for s in [
            "",
            ",,,",
            "<",
            ">",
            "\"",
            "\"unterminated <a@b.com>",
            "(((",
            "<<a@b.com>>",
            "a@",
            "@",
            "Name <>",
            "é <é@é.é>",
            "=?UTF-8?B?",
            "\\",
            "a@b.com (",
            ";;:;",
        ] {
            let _ = p(s);
        }
        assert_eq!(p("Name <>"), vec![]);
        assert_eq!(p("  <mailto:A@X.com>  "), vec![Addr::new("A@X.com")]);
        assert_eq!(p("'a@x.com'"), vec![Addr::new("a@x.com")]);
        assert_eq!(p("\"Bob\"<b@x.com>"), vec![Addr::named("Bob", "b@x.com")]);
        assert_eq!(
            p("Bob <b@x.com>\r\n\t, c@x.com"),
            vec![Addr::named("Bob", "b@x.com"), Addr::new("c@x.com")]
        );
    }

    #[test]
    fn validity() {
        for ok in [
            "a@x.co",
            "first.last+tag@sub.example.org",
            "ü@bücher.de",
            "a_b-c@x-y.io",
        ] {
            assert!(is_valid_email(ok), "{ok}");
        }
        for bad in [
            "",
            "a",
            "a@",
            "@x.com",
            "a@x",
            "a b@x.com",
            "a@x..com",
            "a@-x.com",
            "a@x.c",
            ".a@x.com",
            "a..b@x.com",
            "a@x.123",
            "<a@x.com>",
            "a,b@x.com",
        ] {
            assert!(!is_valid_email(bad), "{bad}");
        }
    }

    #[test]
    fn formatting() {
        assert_eq!(format_addr(&Addr::new("a@x.com")), "a@x.com");
        assert_eq!(
            format_addr(&Addr::named("Alice", "a@x.com")),
            "Alice <a@x.com>"
        );
        assert_eq!(
            format_addr(&Addr::named("Doe, J.", "j@x.com")),
            "\"Doe, J.\" <j@x.com>"
        );
        assert_eq!(
            format_addr(&Addr::named("a \"b\"", "j@x.com")),
            "\"a \\\"b\\\"\" <j@x.com>"
        );
        let list = vec![
            Addr::named("Doe, J.", "j@x.com"),
            Addr::named("Élodie", "e@x.fr"),
        ];
        assert_eq!(parse_address_list(&format_addr_list(&list)), list);
    }
}
