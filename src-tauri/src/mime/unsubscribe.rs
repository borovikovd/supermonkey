use url::Url;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mailto {
    pub to: String,
    pub subject: String,
    pub body: String,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ListUnsubscribe {
    pub one_click: Option<String>,
    pub mailto: Option<Mailto>,
    pub url: Option<String>,
}

#[must_use]
pub fn list_unsubscribe(header: Option<&str>, post_header: Option<&str>) -> ListUnsubscribe {
    let Some(header) = header else {
        return ListUnsubscribe::default();
    };
    let allows_one_click = post_header.is_some_and(|p| {
        p.to_ascii_lowercase()
            .contains("list-unsubscribe=one-click")
    });

    let mut insecure_url: Option<String> = None;
    let mut secure_url: Option<String> = None;
    let mut mailto: Option<Mailto> = None;

    let mut rest = header;
    while let Some(open) = rest.find('<') {
        let tail = &rest[open + 1..];
        let Some(close) = tail.find('>') else {
            break;
        };
        let uri = tail[..close].trim();
        if let Ok(parsed) = Url::parse(uri) {
            match parsed.scheme() {
                "https" if secure_url.is_none() => secure_url = Some(uri.to_owned()),
                "http" if insecure_url.is_none() => insecure_url = Some(uri.to_owned()),
                "mailto" if mailto.is_none() => mailto = parse_mailto(&parsed),
                _ => {}
            }
        }
        rest = &tail[close + 1..];
    }

    let chosen_url = secure_url.clone().or(insecure_url);
    let one_click = if allows_one_click {
        if cfg!(debug_assertions) {
            secure_url.or_else(|| chosen_url.clone())
        } else {
            secure_url
        }
    } else {
        None
    };

    ListUnsubscribe {
        one_click,
        mailto,
        url: chosen_url,
    }
}

fn parse_mailto(parsed: &Url) -> Option<Mailto> {
    let to = percent_encoding::percent_decode_str(parsed.path())
        .decode_utf8_lossy()
        .trim()
        .to_owned();
    if to.is_empty() || !to.contains('@') {
        return None;
    }
    let mut subject = String::new();
    let mut body = String::new();
    for (k, v) in parsed.query_pairs() {
        match k.to_ascii_lowercase().as_str() {
            "subject" => subject = v.into_owned(),
            "body" => body = v.into_owned(),
            _ => {}
        }
    }
    Some(Mailto { to, subject, body })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_one_click_mailto_and_web_urls() {
        let parsed = list_unsubscribe(
            Some("<mailto:unsub@x.test?subject=Bye&body=Now>, <https://x.test/unsub>"),
            Some("List-Unsubscribe=One-Click"),
        );
        assert_eq!(parsed.one_click.as_deref(), Some("https://x.test/unsub"));
        assert_eq!(
            parsed.mailto,
            Some(Mailto {
                to: "unsub@x.test".to_owned(),
                subject: "Bye".to_owned(),
                body: "Now".to_owned(),
            })
        );
        assert_eq!(parsed.url.as_deref(), Some("https://x.test/unsub"));
    }

    #[test]
    fn one_click_requires_post_header() {
        let parsed = list_unsubscribe(Some("<https://x.test/unsub>"), None);
        assert_eq!(parsed.one_click, None);
        assert_eq!(parsed.url.as_deref(), Some("https://x.test/unsub"));
    }
}
