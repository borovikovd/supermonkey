use sm_domain::Addr;

use crate::collapse_whitespace;
use crate::encoded_word::decode_encoded_words;

#[must_use]
pub fn parse_address_list(header: &str) -> Vec<Addr> {
    let mut out = Vec::new();
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
        let mailbox = if carry.is_empty() {
            parse_mailbox(segment)
        } else {
            carry.push_str(segment);
            parse_mailbox(&std::mem::take(&mut carry))
        };
        out.extend(mailbox);
    }
    out
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Place {
    Phrase,
    Quoted,
    Comment,
    Angle,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Token {
    Char(Place, char),
    NestedParen(char),
    CommentEnd,
    AngleStart,
    Syntax,
}

#[derive(Default)]
struct Lexer {
    in_quote: bool,
    in_angle: bool,
    comment_depth: u32,
    escaped: bool,
}

impl Lexer {
    const fn place(&self) -> Place {
        if self.comment_depth > 0 {
            Place::Comment
        } else if self.in_angle {
            Place::Angle
        } else if self.in_quote {
            Place::Quoted
        } else {
            Place::Phrase
        }
    }

    const fn open_comment(&mut self, in_comment: bool) -> Token {
        self.comment_depth += 1;
        if in_comment {
            Token::NestedParen('(')
        } else {
            Token::Syntax
        }
    }

    const fn close_comment(&mut self) -> Token {
        self.comment_depth -= 1;
        if self.comment_depth > 0 {
            Token::NestedParen(')')
        } else {
            Token::CommentEnd
        }
    }

    const fn next(&mut self, c: char) -> Token {
        if self.escaped {
            self.escaped = false;
            return Token::Char(self.place(), c);
        }
        let in_comment = self.comment_depth > 0;
        match c {
            '\\' if self.in_quote || in_comment => {
                self.escaped = true;
                Token::Syntax
            }
            '"' if !in_comment && !self.in_angle => {
                self.in_quote = !self.in_quote;
                Token::Syntax
            }
            '(' if !self.in_quote => self.open_comment(in_comment),
            ')' if !self.in_quote && in_comment => self.close_comment(),
            '<' if !self.in_quote && !in_comment => {
                self.in_angle = true;
                Token::AngleStart
            }
            '>' if !self.in_quote && !in_comment => {
                self.in_angle = false;
                Token::Syntax
            }
            _ => Token::Char(self.place(), c),
        }
    }
}

fn split_segments(header: &str) -> Vec<&str> {
    let mut segments = Vec::new();
    let mut start = 0;
    let mut lexer = Lexer::default();
    for (i, c) in header.char_indices() {
        match lexer.next(c) {
            Token::Char(Place::Phrase, ',' | ';') => {
                segments.push(header.get(start..i).unwrap_or_default());
                start = i + 1;
            }
            Token::Char(Place::Phrase, ':')
                if !header.get(start..i).unwrap_or_default().contains('@') =>
            {
                start = i + 1;
            }
            Token::Char(..)
            | Token::NestedParen(_)
            | Token::CommentEnd
            | Token::AngleStart
            | Token::Syntax => {}
        }
    }
    segments.push(header.get(start..).unwrap_or_default());
    segments
}

fn parse_mailbox(s: &str) -> Option<Addr> {
    let mut phrase = String::new();
    let mut angle = String::new();
    let mut comment = String::new();
    let mut has_angle = false;
    let mut lexer = Lexer::default();
    for c in s.chars() {
        match lexer.next(c) {
            Token::Char(Place::Phrase | Place::Quoted, c) => phrase.push(c),
            Token::Char(Place::Angle, c) => angle.push(c),
            Token::Char(Place::Comment, c) | Token::NestedParen(c) => comment.push(c),
            Token::CommentEnd => comment.push(' '),
            Token::AngleStart => {
                has_angle = true;
                angle.clear();
            }
            Token::Syntax => {}
        }
    }
    let (email, name) = if has_angle {
        (angle, phrase)
    } else {
        (phrase, comment)
    };
    let email = clean_email(email);
    if !email.contains('@') {
        return None;
    }
    let name = clean_name(&name).filter(|n| !n.eq_ignore_ascii_case(&email));
    Some(Addr { name, email })
}

fn clean_email(mut email: String) -> String {
    email.retain(|c| !c.is_whitespace());
    let unquoted = email.trim_matches(|c| c == '"' || c == '\'');
    let bare = match unquoted.get(..7) {
        Some(scheme) if scheme.eq_ignore_ascii_case("mailto:") => {
            unquoted.get(7..).unwrap_or_default()
        }
        _ => unquoted,
    };
    if bare.len() == email.len() {
        email
    } else {
        bare.to_owned()
    }
}

fn clean_name(s: &str) -> Option<String> {
    let collapsed = collapse_whitespace(&decode_encoded_words(s.trim()));
    let name = collapsed.trim_matches(|c| c == '"' || c == '\'').trim();
    if name.is_empty() {
        None
    } else if name.len() == collapsed.len() {
        Some(collapsed)
    } else {
        Some(name.to_owned())
    }
}

#[must_use]
pub fn is_valid_email(s: &str) -> bool {
    let Some((local, domain)) = s.rsplit_once('@') else {
        return false;
    };
    let is_bad_local =
        |c: char| c.is_whitespace() || c.is_control() || "<>()[],;:\\\"@".contains(c);
    if s.len() > 254
        || local.is_empty()
        || local.len() > 64
        || local.chars().any(is_bad_local)
        || local.starts_with('.')
        || local.ends_with('.')
        || local.contains("..")
    {
        return false;
    }
    let labels: Vec<&str> = domain.split('.').collect();
    let tld = labels.last().copied().unwrap_or_default();
    labels.len() >= 2
        && labels.iter().all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label.chars().all(|c| c.is_alphanumeric() || c == '-')
        })
        && tld.chars().count() >= 2
        && !tld.chars().all(|c| c.is_ascii_digit())
}
