//! `List-Unsubscribe` (RFC 2369) and `List-Unsubscribe-Post` (RFC 8058).

use crate::is_valid_email;

/// The ways a list offers to unsubscribe.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ListUnsubscribe {
    /// The first `mailto:` URI with a valid address.
    pub mailto: Option<Mailto>,
    /// The first `https:` or `http:` URI.
    pub web: Option<String>,
    /// `List-Unsubscribe-Post: List-Unsubscribe=One-Click` with a web URI: a
    /// POST to [`ListUnsubscribe::web`] unsubscribes without a page. Which URLs
    /// may be posted to (https) is the caller's decision.
    pub one_click: bool,
}

/// A `mailto:` unsubscribe address with its optional subject and body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mailto {
    pub to: String,
    pub subject: Option<String>,
    pub body: Option<String>,
}

/// Reads the `<uri>, <uri>` list of a `List-Unsubscribe` header. URIs outside
/// angle brackets and schemes other than mailto and http(s) are ignored.
pub fn list_unsubscribe(header: &str, post: Option<&str>) -> ListUnsubscribe {
    let mut out = ListUnsubscribe::default();
    for uri in header
        .split('<')
        .skip(1)
        .filter_map(|part| part.split_once('>').map(|(uri, _)| uri.trim()))
    {
        let Ok(url) = url::Url::parse(uri) else {
            continue;
        };
        match url.scheme() {
            "mailto" if out.mailto.is_none() => out.mailto = mailto(&url),
            "https" | "http" if out.web.is_none() => out.web = Some(url.to_string()),
            _ => {}
        }
    }
    out.one_click = post
        .is_some_and(|p| p.trim().eq_ignore_ascii_case("List-Unsubscribe=One-Click"))
        && out.web.is_some();
    out
}

fn mailto(url: &url::Url) -> Option<Mailto> {
    let to = percent_decode(url.path());
    if !is_valid_email(&to) {
        return None;
    }
    let field = |name: &str| {
        url.query_pairs()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.into_owned())
            .filter(|v| !v.is_empty())
    };
    Some(Mailto {
        to,
        subject: field("subject"),
        body: field("body"),
    })
}

fn percent_decode(s: &str) -> String {
    percent_encoding::percent_decode_str(s)
        .decode_utf8_lossy()
        .into_owned()
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
                    to: "unsub@list.example".into(),
                    subject: Some("stop please".into()),
                    body: None,
                }),
                web: Some("https://list.example/u/42".into()),
                one_click: true,
            }
        );
    }

    #[test]
    fn one_click_needs_the_post_header_and_a_web_uri() {
        assert!(!list_unsubscribe("<https://list.example/u>", None).one_click);
        assert!(
            !list_unsubscribe(
                "<mailto:u@list.example>",
                Some("List-Unsubscribe=One-Click")
            )
            .one_click
        );
        assert!(
            list_unsubscribe(
                "<https://list.example/u>",
                Some(" list-unsubscribe=one-click ")
            )
            .one_click
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
                to: "list+unsub@d.example".into(),
                subject: None,
                body: Some("remove me".into()),
            })
        );
    }
}
