use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use html5ever::tendril::StrTendril;
use html5ever::tokenizer::{
    BufferQueue, Token as HtmlToken, TokenSink, TokenSinkResult, Tokenizer, TokenizerOpts,
};
use regex::Regex;

use super::collapse_whitespace;
use super::outgoing::InlinePart;
use super::parse::{decode_base64_lenient, normalize_cid};

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct CidImages {
    by_cid: HashMap<String, (String, Vec<u8>)>,
}

impl CidImages {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, cid: &str, mime_type: String, bytes: Vec<u8>) {
        if let Some(norm) = normalize_cid(cid) {
            self.by_cid.insert(norm, (mime_type, bytes));
        }
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.by_cid.is_empty()
    }

    #[must_use]
    pub fn data_uri(&self, cid_ref: &str) -> Option<String> {
        let raw = cid_ref
            .strip_prefix("cid:")
            .or_else(|| cid_ref.strip_prefix("CID:"))?;
        let norm = normalize_cid(raw)?;
        let (mime, bytes) = self.by_cid.get(&norm)?;
        if !mime.starts_with("image/") {
            return None;
        }
        Some(format!("data:{mime};base64,{}", STANDARD.encode(bytes)))
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct RawBody {
    pub html: Option<String>,
    pub text: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Body {
    pub html: String,
    pub quote_html: Option<String>,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DraftBody {
    pub html: String,
    pub quote_html: Option<String>,
}

impl RawBody {
    #[must_use]
    pub fn render(self, cids: &CidImages, is_forward: bool) -> Body {
        let source_html = match (&self.html, &self.text) {
            (Some(html), _) if !html.trim().is_empty() => Cow::Borrowed(html.as_str()),
            (_, Some(text)) => Cow::Owned(text_to_html(text)),
            _ => Cow::Borrowed(""),
        };

        let (visible_raw, quote_raw) = if is_forward {
            (source_html.as_ref(), None)
        } else {
            split_quoted_history(&source_html)
        };

        let html = sanitize_html(visible_raw, cids);
        let quote_html = quote_raw
            .map(|q| sanitize_html(q, cids))
            .filter(|q| !q.trim().is_empty());

        let text = match self.text {
            Some(t) if !t.trim().is_empty() => t,
            _ => html_to_text(&source_html),
        };

        Body {
            html,
            quote_html,
            text,
        }
    }

    #[must_use]
    pub fn to_draft(&self, cids: &CidImages) -> DraftBody {
        let source_html = match (&self.html, &self.text) {
            (Some(html), _) if !html.trim().is_empty() => Cow::Borrowed(html.as_str()),
            (_, Some(text)) => Cow::Owned(text_to_html(text)),
            _ => Cow::Borrowed(""),
        };
        let (body_raw, quote_raw) = split_gmail_quote(&source_html);
        let html = sanitize_html(body_raw, cids);
        let quote_html = quote_raw
            .map(|q| sanitize_html(q, cids))
            .filter(|q| !q.trim().is_empty());
        DraftBody { html, quote_html }
    }
}

pub(crate) fn collect_cid_refs(html: &str) -> HashSet<String> {
    static CID_ATTR: LazyLock<Option<Regex>> =
        LazyLock::new(|| Regex::new(r#"(?i)\bcid:([^\s"'<>)\]]+)"#).ok());
    let mut out = HashSet::new();
    let Some(re) = CID_ATTR.as_ref() else {
        return out;
    };
    for cap in re.captures_iter(html) {
        if let Some(m) = cap.get(1)
            && let Some(norm) = normalize_cid(m.as_str())
        {
            out.insert(norm);
        }
    }
    out
}

#[must_use]
pub fn sanitize_html(html: &str, cids: &CidImages) -> String {
    static POLICY: LazyLock<ammonia::Builder<'static>> = LazyLock::new(ammonia_builder);
    let cleaned = POLICY.clean(html).to_string();
    let styled = sanitize_style_blocks(&cleaned);
    if cids.is_empty() {
        match styled {
            Cow::Borrowed(_) => cleaned,
            Cow::Owned(s) => s,
        }
    } else {
        match replace_cid_attrs(&styled, cids) {
            Cow::Borrowed(_) => match styled {
                Cow::Borrowed(_) => cleaned,
                Cow::Owned(s) => s,
            },
            Cow::Owned(s) => s,
        }
    }
}

fn replace_cid_attrs<'a>(html: &'a str, cids: &CidImages) -> Cow<'a, str> {
    static CID_ATTR: LazyLock<Option<Regex>> =
        LazyLock::new(|| Regex::new(r#"(?i)(\b(?:src|background)=")(cid:[^"]+)(")"#).ok());
    let Some(re) = CID_ATTR.as_ref() else {
        return Cow::Borrowed(html);
    };
    re.replace_all(html, |caps: &regex::Captures<'_>| {
        let prefix = caps.get(1).map_or("", |m| m.as_str());
        let cid_val = caps.get(2).map_or("", |m| m.as_str());
        let suffix = caps.get(3).map_or("", |m| m.as_str());
        cids.data_uri(cid_val).map_or_else(
            || format!("{prefix}{cid_val}{suffix}"),
            |uri| format!("{prefix}{uri}{suffix}"),
        )
    })
}

fn ammonia_builder() -> ammonia::Builder<'static> {
    let mut b = ammonia::Builder::default();
    b.add_tags(["style", "font", "center", "u", "s", "big", "small"]);
    b.rm_clean_content_tags(["style"]);
    b.add_clean_content_tags([
        "title", "iframe", "object", "svg", "math", "noscript", "textarea", "select", "option",
    ]);
    b.add_generic_attributes([
        "id",
        "class",
        "style",
        "align",
        "valign",
        "bgcolor",
        "background",
        "width",
        "height",
        "title",
        "dir",
    ]);
    b.add_tag_attributes("a", ["href", "target", "name"]);
    b.add_tag_attributes("img", ["src", "alt", "border"]);
    b.add_tag_attributes("table", ["cellpadding", "cellspacing", "border"]);
    b.add_tag_attributes("td", ["colspan", "rowspan", "nowrap"]);
    b.add_tag_attributes("th", ["colspan", "rowspan", "nowrap"]);
    b.add_tag_attributes("font", ["face", "color", "size"]);
    b.add_url_schemes(["http", "https", "mailto", "ftp", "cid", "data"]);
    b.link_rel(Some("noopener noreferrer"));
    b.attribute_filter(filter_attribute);
    b
}

fn filter_attribute<'a>(tag: &str, attr: &str, value: &'a str) -> Option<Cow<'a, str>> {
    match attr {
        "style" => Some(Cow::Owned(sanitize_css(value))),
        "src" | "background" => {
            let trimmed = value.trim();
            if trimmed
                .get(..4)
                .is_some_and(|p| p.eq_ignore_ascii_case("cid:"))
            {
                return Some(Cow::Borrowed(trimmed));
            }
            if trimmed
                .get(..5)
                .is_some_and(|p| p.eq_ignore_ascii_case("data:"))
            {
                return is_safe_image_data_uri(trimmed).then_some(Cow::Borrowed(trimmed));
            }
            is_safe_url(trimmed).then_some(Cow::Borrowed(trimmed))
        }
        "href" => {
            let trimmed = value.trim();
            if trimmed
                .get(..5)
                .is_some_and(|p| p.eq_ignore_ascii_case("data:"))
            {
                return None;
            }
            (tag == "a" && is_safe_link_url(trimmed)).then_some(Cow::Borrowed(trimmed))
        }
        _ => Some(Cow::Borrowed(value)),
    }
}

fn is_safe_image_data_uri(uri: &str) -> bool {
    let lower = uri.trim_start().to_ascii_lowercase();
    lower.starts_with("data:image/png;")
        || lower.starts_with("data:image/jpeg;")
        || lower.starts_with("data:image/jpg;")
        || lower.starts_with("data:image/gif;")
        || lower.starts_with("data:image/webp;")
}

fn is_safe_url(url: &str) -> bool {
    let compact: String = url
        .chars()
        .filter(|c| !c.is_ascii_control() && !c.is_whitespace())
        .collect();
    let lower = compact.to_ascii_lowercase();
    if lower.starts_with('#') || !lower.contains(':') {
        return true;
    }
    lower.starts_with("http://") || lower.starts_with("https://") || lower.starts_with("mailto:")
}

fn is_safe_link_url(url: &str) -> bool {
    let compact: String = url
        .chars()
        .filter(|c| !c.is_ascii_control() && !c.is_whitespace())
        .collect();
    let lower = compact.to_ascii_lowercase();
    if lower.starts_with('#') || !lower.contains(':') {
        return true;
    }
    lower.starts_with("http://")
        || lower.starts_with("https://")
        || lower.starts_with("mailto:")
        || lower.starts_with("ftp://")
}

fn sanitize_style_blocks(html: &str) -> Cow<'_, str> {
    static STYLE_RE: LazyLock<Option<Regex>> =
        LazyLock::new(|| Regex::new(r"(?is)(<style\b[^>]*>)(.*?)(</style>)").ok());
    let Some(re) = STYLE_RE.as_ref() else {
        return Cow::Borrowed(html);
    };
    re.replace_all(html, |caps: &regex::Captures<'_>| {
        let open = caps.get(1).map_or("", |m| m.as_str());
        let css = caps.get(2).map_or("", |m| m.as_str());
        let close = caps.get(3).map_or("", |m| m.as_str());
        format!("{open}{}{close}", sanitize_css(css))
    })
}

fn sanitize_css(css: &str) -> String {
    static COMMENT_RE: LazyLock<Option<Regex>> =
        LazyLock::new(|| Regex::new(r"(?s)/\*.*?\*/").ok());
    static URL_RE: LazyLock<Option<Regex>> =
        LazyLock::new(|| Regex::new(r"(?is)url\s*\(([^)]*)\)").ok());
    static EXPR_RE: LazyLock<Option<Regex>> =
        LazyLock::new(|| Regex::new(r"(?is)expression\s*\([^)]*\)").ok());

    let unescaped = html_escape::decode_html_entities(css);
    let decoded = decode_css_escapes(&unescaped);
    let no_comments = COMMENT_RE.as_ref().map_or_else(
        || Cow::Borrowed(decoded.as_ref()),
        |re| re.replace_all(&decoded, ""),
    );

    let safe_urls = URL_RE.as_ref().map_or_else(
        || Cow::Borrowed(no_comments.as_ref()),
        |re| {
            re.replace_all(&no_comments, |caps: &regex::Captures<'_>| {
                let inner = caps
                    .get(1)
                    .map_or("", |m| m.as_str())
                    .trim()
                    .trim_matches(['\'', '"']);
                if is_safe_url(inner) || is_safe_image_data_uri(inner) {
                    caps.get(0).map_or("", |m| m.as_str()).to_owned()
                } else {
                    "none".to_owned()
                }
            })
        },
    );

    match EXPR_RE.as_ref() {
        Some(re) => re.replace_all(&safe_urls, "none").into_owned(),
        None => safe_urls.into_owned(),
    }
}

fn decode_css_escapes(input: &str) -> Cow<'_, str> {
    if !input.contains('\\') {
        return Cow::Borrowed(input);
    }
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            out.push(ch);
            continue;
        }
        let mut hex = String::new();
        while hex.len() < 6 && chars.peek().is_some_and(char::is_ascii_hexdigit) {
            if let Some(d) = chars.next() {
                hex.push(d);
            }
        }
        if !hex.is_empty() {
            if chars.peek().is_some_and(char::is_ascii_whitespace) {
                chars.next();
            }
            if let Ok(code) = u32::from_str_radix(&hex, 16)
                && let Some(decoded) = char::from_u32(code)
            {
                out.push(decoded);
            }
        } else if let Some(next) = chars.next() {
            out.push(next);
        }
    }
    Cow::Owned(out)
}

#[must_use]
pub fn html_to_text(html: &str) -> String {
    collapse_whitespace(&extract_plain_text(html, false))
}

#[must_use]
pub fn html_to_plain(html: &str) -> String {
    extract_plain_text(html, true)
}

#[must_use]
pub fn snippet_text(html: &str) -> String {
    let text = html_to_text(html);
    let mut out = String::new();
    for ch in text.chars().take(200) {
        out.push(ch);
    }
    out
}

struct ExtractState {
    out: String,
    skip_depth: usize,
    quote_depth: usize,
    at_line_start: bool,
}

struct TextExtractor {
    preserve_lines: bool,
    #[expect(
        clippy::disallowed_types,
        reason = "html5ever::TokenSink::process_token takes &self on a stack-local single-threaded tokenizer"
    )]
    state: std::cell::RefCell<ExtractState>,
}

impl TextExtractor {
    #[expect(
        clippy::disallowed_types,
        reason = "html5ever::TokenSink::process_token takes &self on a stack-local single-threaded tokenizer"
    )]
    const fn new(preserve_lines: bool) -> Self {
        Self {
            preserve_lines,
            state: std::cell::RefCell::new(ExtractState {
                out: String::new(),
                skip_depth: 0,
                quote_depth: 0,
                at_line_start: true,
            }),
        }
    }

    fn push_newline(&self) {
        let mut st = self.state.borrow_mut();
        if !st.out.is_empty() && !st.out.ends_with('\n') {
            st.out.push('\n');
        }
        st.at_line_start = true;
    }

    fn push_text(&self, s: &str) {
        let mut st = self.state.borrow_mut();
        if st.skip_depth > 0 {
            return;
        }
        let depth = st.quote_depth;
        for ch in s.chars() {
            if ch == '\r' {
                continue;
            }
            if ch == '\n' {
                st.out.push(' ');
                continue;
            }
            if st.at_line_start && self.preserve_lines && depth > 0 {
                for _ in 0..depth {
                    st.out.push_str("> ");
                }
            }
            st.out.push(ch);
            st.at_line_start = false;
        }
    }
}

impl TokenSink for TextExtractor {
    type Handle = ();

    fn process_token(&self, token: HtmlToken, _line: u64) -> TokenSinkResult<()> {
        match token {
            HtmlToken::TagToken(tag) => {
                let name = tag.name.as_ref();
                let is_end = matches!(tag.kind, html5ever::tokenizer::TagKind::EndTag);
                if matches!(
                    name,
                    "script" | "style" | "head" | "title" | "noscript" | "template"
                ) {
                    let mut st = self.state.borrow_mut();
                    if is_end {
                        st.skip_depth = st.skip_depth.saturating_sub(1);
                    } else {
                        st.skip_depth += 1;
                    }
                    return TokenSinkResult::Continue;
                }
                if self.state.borrow().skip_depth > 0 {
                    return TokenSinkResult::Continue;
                }
                if name == "blockquote" {
                    {
                        let mut st = self.state.borrow_mut();
                        if is_end {
                            st.quote_depth = st.quote_depth.saturating_sub(1);
                        } else {
                            st.quote_depth += 1;
                        }
                    }
                    self.push_newline();
                } else if matches!(
                    name,
                    "br" | "p"
                        | "div"
                        | "li"
                        | "tr"
                        | "h1"
                        | "h2"
                        | "h3"
                        | "h4"
                        | "h5"
                        | "h6"
                        | "hr"
                        | "pre"
                ) {
                    self.push_newline();
                    if name == "li" && !is_end && self.preserve_lines {
                        self.push_text("- ");
                    }
                } else if matches!(name, "td" | "th") && is_end {
                    self.push_text(" ");
                }
            }
            HtmlToken::CharacterTokens(s) => self.push_text(&s),
            HtmlToken::NullCharacterToken
            | HtmlToken::DoctypeToken(_)
            | HtmlToken::CommentToken(_)
            | HtmlToken::ParseError(_)
            | HtmlToken::EOFToken => {}
        }
        TokenSinkResult::Continue
    }
}

fn extract_plain_text(html: &str, preserve_lines: bool) -> String {
    let sink = TextExtractor::new(preserve_lines);
    let tokenizer = Tokenizer::new(sink, TokenizerOpts::default());
    let buffer = BufferQueue::default();
    buffer.push_back(StrTendril::from(html));
    drop(tokenizer.feed(&buffer));
    tokenizer.end();
    let raw = tokenizer.sink.state.into_inner().out;
    if preserve_lines {
        raw.lines()
            .map(str::trim_end)
            .collect::<Vec<_>>()
            .join("\n")
            .trim()
            .to_owned()
    } else {
        raw
    }
}

#[must_use]
pub fn text_to_html(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let quote_start = find_plain_quote_start(&lines);
    let (body_lines, quote_lines) = match quote_start {
        Some(idx) => (&lines[..idx], Some(&lines[idx..])),
        None => (lines.as_slice(), None),
    };
    let mut out = format_plain_lines(body_lines);
    if let Some(q) = quote_lines {
        out.push_str("<div class=\"gmail_quote\">");
        out.push_str(&format_plain_lines(q));
        out.push_str("</div>");
    }
    out
}

fn find_plain_quote_start(lines: &[&str]) -> Option<usize> {
    let last_non_empty = lines.iter().rposition(|l| !l.trim().is_empty())?;
    if !lines[last_non_empty].trim_start().starts_with('>') {
        return None;
    }
    let mut start = last_non_empty;
    while start > 0 {
        let prev = lines[start - 1].trim();
        if prev.starts_with('>') || prev.is_empty() {
            start -= 1;
        } else {
            break;
        }
    }
    while start < last_non_empty && lines[start].trim().is_empty() {
        start += 1;
    }
    if start > 0 {
        let mut attr = start - 1;
        while attr > 0 && lines[attr].trim().is_empty() {
            attr -= 1;
        }
        let line = lines[attr].trim();
        if line.ends_with("wrote:") || line.starts_with("On ") && line.ends_with(':') {
            start = attr;
        }
    }
    (start > 0).then_some(start)
}

fn format_plain_lines(lines: &[&str]) -> String {
    let mut out = String::new();
    for (idx, line) in lines.iter().enumerate() {
        if idx > 0 {
            out.push_str("<br>");
        }
        out.push_str(&linkify_escaped(line));
    }
    out
}

fn linkify_escaped(line: &str) -> String {
    static URL_RE: LazyLock<Option<Regex>> =
        LazyLock::new(|| Regex::new(r#"https?://[^\s<>"]+"#).ok());
    let Some(re) = URL_RE.as_ref() else {
        return html_escape::encode_text(line).into_owned();
    };
    let mut out = String::with_capacity(line.len());
    let mut last = 0;
    for m in re.find_iter(line) {
        out.push_str(&html_escape::encode_text(&line[last..m.start()]));
        let url = m.as_str().trim_end_matches(['.', ',', ';', ')', '!', '?']);
        let escaped_url = html_escape::encode_double_quoted_attribute(url);
        let escaped_text = html_escape::encode_text(url);
        out.push_str("<a href=\"");
        out.push_str(&escaped_url);
        out.push_str("\">");
        out.push_str(&escaped_text);
        out.push_str("</a>");
        out.push_str(&html_escape::encode_text(&m.as_str()[url.len()..]));
        last = m.end();
    }
    out.push_str(&html_escape::encode_text(&line[last..]));
    out
}

#[must_use]
pub fn split_gmail_quote(html: &str) -> (&str, Option<&str>) {
    static GMAIL_QUOTE: LazyLock<Option<Regex>> = LazyLock::new(|| {
        Regex::new(r#"(?i)<div\b[^>]*class\s*=\s*["'][^"']*\bgmail_quote\b[^"']*["'][^>]*>"#).ok()
    });
    let Some(re) = GMAIL_QUOTE.as_ref() else {
        return (html, None);
    };
    let Some(m) = re.find(html) else {
        return (html, None);
    };
    (&html[..m.start()], Some(&html[m.start()..]))
}

#[must_use]
pub fn split_quoted_history(html: &str) -> (&str, Option<&str>) {
    static QUOTE_MARKERS: LazyLock<Option<Regex>> = LazyLock::new(|| {
        Regex::new(
            r#"(?i)<div\b[^>]*class\s*=\s*["'][^"']*\bgmail_quote\b[^"']*["'][^>]*>|<blockquote\b[^>]*type\s*=\s*["']cite["'][^>]*>|<div\b[^>]*id\s*=\s*["']appendonsend["'][^>]*>"#,
        )
        .ok()
    });
    let Some(re) = QUOTE_MARKERS.as_ref() else {
        return (html, None);
    };
    let Some(m) = re.find(html) else {
        return (html, None);
    };
    let before = &html[..m.start()];
    if html_to_text(before).is_empty() {
        return (html, None);
    }
    if let Some(end) = quoted_block_end(html, m.as_str(), m.end())
        && !html_to_text(&html[end..]).is_empty()
    {
        return (html, None);
    }
    (before, Some(&html[m.start()..]))
}

fn quoted_block_end(html: &str, open_tag: &str, after_open: usize) -> Option<usize> {
    static BLOCKQUOTE_TAGS: LazyLock<Option<Regex>> =
        LazyLock::new(|| Regex::new(r"(?i)</?blockquote\b[^>]*>").ok());
    static DIV_TAGS: LazyLock<Option<Regex>> =
        LazyLock::new(|| Regex::new(r"(?i)</?div\b[^>]*>").ok());
    let lower = open_tag.to_ascii_lowercase();
    let tags = if lower.starts_with("<blockquote") {
        BLOCKQUOTE_TAGS.as_ref()?
    } else if lower.contains("gmail_quote") {
        DIV_TAGS.as_ref()?
    } else {
        return None;
    };
    let rest = html.get(after_open..)?;
    let mut depth = 1usize;
    for tag in tags.find_iter(rest) {
        if tag.as_str().starts_with("</") {
            depth = depth.saturating_sub(1);
            if depth == 0 {
                return Some(after_open.saturating_add(tag.end()));
            }
        } else {
            depth = depth.saturating_add(1);
        }
    }
    None
}

#[must_use]
pub fn data_images_to_cid(html: &str, known: &[InlinePart]) -> (String, Vec<InlinePart>) {
    static DATA_IMG: LazyLock<Option<Regex>> = LazyLock::new(|| {
        Regex::new(r"(?i)data:(image/[a-z0-9.+-]+);base64,([A-Za-z0-9+/=\s]+)").ok()
    });
    let Some(re) = DATA_IMG.as_ref() else {
        return (html.to_owned(), Vec::new());
    };
    let mut added: Vec<InlinePart> = Vec::new();
    let rewritten = re
        .replace_all(html, |caps: &regex::Captures<'_>| {
            let mime = caps
                .get(1)
                .map_or("image/png", |m| m.as_str())
                .to_ascii_lowercase();
            let raw_b64 = caps.get(2).map_or("", |m| m.as_str());
            let bytes = decode_base64_lenient(raw_b64);
            if let Some(same) = known
                .iter()
                .chain(&added)
                .find(|p| p.data == bytes && p.mime_type == mime)
            {
                return format!("cid:{}", same.content_id);
            }
            let mut n = known.len().saturating_add(added.len()).saturating_add(1);
            let cid = loop {
                let candidate = format!("inline-{n}@supermonkey.local");
                if !known
                    .iter()
                    .chain(&added)
                    .any(|p| p.content_id == candidate)
                {
                    break candidate;
                }
                n = n.saturating_add(1);
            };
            added.push(InlinePart {
                content_id: cid.clone(),
                mime_type: mime,
                data: bytes,
            });
            format!("cid:{cid}")
        })
        .into_owned();
    (rewritten, added)
}
