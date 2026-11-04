//! `List-Unsubscribe` (RFC 2369) and `List-Unsubscribe-Post` (RFC 8058), for
//! SPEC Unwanted mail.

use percent_encoding::percent_decode_str;
use ub_domain::Addr;

use crate::addr::is_valid_email;

/// The ways a list offers to unsubscribe.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ListUnsubscribe {
    /// The first `mailto:` URI with a valid address.
    pub mailto: Option<Mailto>,
    /// The first `https:` or `http:` URI.
    pub web: Option<String>,
    /// `List-Unsubscribe-Post: List-Unsubscribe=One-Click` with a web URI: a
    /// POST to [`ListUnsubscribe::web`] unsubscribes without a page. Which URLs
    /// may be posted to (https only) is the caller's decision.
    pub is_one_click: bool,
}

/// A `mailto:` unsubscribe address with its optional subject and body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mailto {
    /// The address to write to, without a display name.
    pub to: Addr,
    /// The `subject` field, when it is not empty.
    pub subject: Option<String>,
    /// The `body` field, when it is not empty.
    pub body: Option<String>,
}

/// Reads the `<uri>, <uri>` list of a `List-Unsubscribe` header, and its
/// `List-Unsubscribe-Post` companion. URIs outside angle brackets and schemes
/// other than mailto and http(s) are ignored.
pub fn list_unsubscribe(header: &str, post: Option<&str>) -> ListUnsubscribe {
    let mut out = ListUnsubscribe::default();
    let uris = header
        .split('<')
        .skip(1)
        .filter_map(|part| part.split_once('>').map(|(uri, _)| uri.trim()));
    for uri in uris {
        let Ok(url) = url::Url::parse(uri) else {
            continue;
        };
        match url.scheme() {
            "mailto" if out.mailto.is_none() => out.mailto = mailto(&url),
            "https" | "http" if out.web.is_none() => out.web = Some(url.into()),
            _ => {}
        }
    }
    out.is_one_click = out.web.is_some()
        && post.is_some_and(|p| p.trim().eq_ignore_ascii_case("List-Unsubscribe=One-Click"));
    out
}

fn mailto(url: &url::Url) -> Option<Mailto> {
    let to = percent_decode_str(url.path()).decode_utf8_lossy();
    if !is_valid_email(&to) {
        return None;
    }
    let field = |name: &str| {
        url.query_pairs()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.into_owned())
            .filter(|value| !value.is_empty())
    };
    Some(Mailto {
        to: Addr::new(to),
        subject: field("subject"),
        body: field("body"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mailto_and_web_with_one_click() {
        let got = list_unsubscribe(
            "<mailto:unsub@list.example?subject=stop%20please>, <https://list.example/u/42>",
            Some("List-Unsubscribe=One-Click"),
        );
        assert_eq!(
            got,
            ListUnsubscribe {
                mailto: Some(Mailto {
                    to: Addr::new("unsub@list.example"),
                    subject: Some("stop please".into()),
                    body: None,
                }),
                web: Some("https://list.example/u/42".into()),
                is_one_click: true,
            }
        );
    }

    #[test]
    fn one_click_needs_the_post_header_and_a_web_uri() {
        assert!(!list_unsubscribe("<https://list.example/u>", None).is_one_click);
        assert!(
            !list_unsubscribe(
                "<mailto:u@list.example>",
                Some("List-Unsubscribe=One-Click")
            )
            .is_one_click
        );
        assert!(
            list_unsubscribe(
                "<https://list.example/u>",
                Some(" list-unsubscribe=one-click ")
            )
            .is_one_click
        );
    }

    #[test]
    fn junk_and_spacing() {
        let got = list_unsubscribe(
            "  < https://a.example/x > ,<ftp://b.example/y>, mailto:bare@c.example, <mailto:not-an-address>",
            None,
        );
        assert_eq!(got.web.as_deref(), Some("https://a.example/x"));
        assert_eq!(got.mailto, None);
        assert_eq!(list_unsubscribe("", None), ListUnsubscribe::default());
    }

    #[test]
    fn mailto_body_and_a_plus_in_the_address() {
        let got = list_unsubscribe(
            "<mailto:list+unsub@d.example?body=remove%20me&subject=>",
            None,
        );
        assert_eq!(
            got.mailto,
            Some(Mailto {
                to: Addr::new("list+unsub@d.example"),
                subject: None,
                body: Some("remove me".into()),
            })
        );
    }
}
