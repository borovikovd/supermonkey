//! HTML sanitizing, HTML ⇄ text conversion, and `data:` ⇄ `cid:` image rewriting.

use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::LazyLock;

use regex::{Captures, Regex};

use crate::InlinePart;
use crate::encoding::{decode_base64, encode_base64, percent_decode};

static SANITIZER: LazyLock<ammonia::Builder<'static>> = LazyLock::new(|| {
    let mut b = ammonia::Builder::default();
    b.add_tags([
        "style", "font", "tfoot", "section", "article", "aside", "main", "address", "big", "label",
        "fieldset", "legend",
    ])
    // Removed together with everything inside them.
    .clean_content_tags(
        [
            "script", "title", "noscript", "iframe", "object", "embed", "applet", "frameset",
            "svg", "math", "template", "textarea", "select",
        ]
        .into(),
    )
    .add_generic_attributes([
        "style",
        "class",
        "id",
        "align",
        "valign",
        "width",
        "height",
        "bgcolor",
        "background",
        "border",
        "cellpadding",
        "cellspacing",
        "color",
        "face",
        "size",
        "dir",
        "nowrap",
        "role",
        "type",
        "colspan",
        "rowspan",
        "span",
        "start",
        "frame",
        "rules",
        "summary",
        "hspace",
        "vspace",
        "alt",
        "abbr",
        "scope",
        "headers",
        "cite",
    ])
    .add_tag_attributes("a", ["target", "name"])
    .add_url_schemes(["data"])
    .attribute_filter(|element, attribute, value| {
        let lower = value.trim_start().to_ascii_lowercase();
        let image_ok = (element == "img" && attribute == "src") || attribute == "background";
        if lower.starts_with("data:") && !(image_ok && lower.starts_with("data:image/")) {
            return None;
        }
        if attribute == "background"
            && !(lower.starts_with("http://")
                || lower.starts_with("https://")
                || lower.starts_with("data:image/"))
        {
            return None;
        }
        Some(Cow::Borrowed(value))
    });
    b
});

static CID_REF: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?i)\bcid:([^"'\s)>]+)"#).unwrap());

/// Sanitizes received HTML for storage and display.
///
/// First rewrites `cid:` references whose Content-ID (without `<>`) is a key of `cid_data`
/// to the mapped `data:` URI. Then cleans with ammonia: `style`, `class`, `id` and table/layout
/// attributes and `<style>` blocks survive, as do `data:image/*` URLs on `<img src>` and
/// `background`. Scripts, event handlers, forms and their controls, iframes, objects, embeds,
/// `<meta>`, SVG/MathML and `javascript:` URLs are removed. Links get `rel="noopener noreferrer"`.
pub fn sanitize_html(html: &str, cid_data: &HashMap<String, String>) -> String {
    let rewritten = if cid_data.is_empty() {
        Cow::Borrowed(html)
    } else {
        CID_REF.replace_all(html, |c: &Captures| {
            let raw = &c[1];
            let decoded = String::from_utf8_lossy(&percent_decode(raw)).into_owned();
            let key = decoded.trim_start_matches('<').trim_end_matches('>');
            match cid_data.get(raw).or_else(|| cid_data.get(key)) {
                Some(uri) => uri.clone(),
                None => c[0].to_string(),
            }
        })
    };
    SANITIZER.clean(&rewritten).to_string()
}

/// Comments, `<!DOCTYPE>`/`<![CDATA[` declarations, and `<?xml ?>` processing instructions.
static COMMENT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?s)<!--.*?-->|<![^>]*>|<\?[^>]*>").unwrap());
static HIDDEN_BLOCKS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    ["head", "style", "script", "title", "noscript", "template"]
        .iter()
        .map(|t| Regex::new(&format!(r"(?is)<{t}\b[^>]*>.*?</{t}\s*>")).unwrap())
        .collect()
});
static TAG: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"<(/?)([a-zA-Z][a-zA-Z0-9:-]*)((?:[^>"']|"[^"]*"|'[^']*')*)>"#).unwrap()
});
static HREF: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?i)\bhref\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s>]+))"#).unwrap());

/// Plain text of an HTML body for full-text search and snippets: no `<head>`, `<style>`,
/// `<script>` or comments, entities decoded, all whitespace collapsed to single spaces.
pub fn html_to_text(html: &str) -> String {
    render_text(html, false)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// The `text/plain` alternative of an outgoing HTML body: line breaks from blocks and `<br>`,
/// `> ` prefixes inside blockquotes, `- ` list items, and `text <url>` for http(s) links.
pub(crate) fn html_to_plain(html: &str) -> String {
    let text = render_text(html, true);
    let mut out = String::with_capacity(text.len());
    let mut blank_run = 0;
    for line in text.lines() {
        let line = line.trim_end();
        if line.is_empty() || line.chars().all(|c| c == '>') {
            blank_run += 1;
            if blank_run > 1 {
                continue;
            }
        } else {
            blank_run = 0;
        }
        out.push_str(line);
        out.push('\n');
    }
    out.trim_matches('\n').to_string() + "\n"
}

/// Walks the tags of `html` and writes its text. `plain` adds the formatting meant for a
/// readable text/plain part.
fn render_text(html: &str, plain: bool) -> String {
    let mut html = COMMENT.replace_all(html, "").into_owned();
    for re in HIDDEN_BLOCKS.iter() {
        html = re.replace_all(&html, " ").into_owned();
    }

    let mut w = TextWriter {
        out: String::new(),
        quote_depth: 0,
        plain,
    };
    let mut pre_depth = 0u32;
    let mut link: Option<(String, usize)> = None;
    let mut last = 0;
    for m in TAG.captures_iter(&html) {
        let whole = m.get(0).unwrap();
        w.text(&html[last..whole.start()], pre_depth > 0);
        last = whole.end();

        let closing = !m[1].is_empty();
        let name = m[2].to_ascii_lowercase();
        match (name.as_str(), closing) {
            ("br", _) => w.newline(),
            ("pre", false) => {
                pre_depth += 1;
                w.block();
            }
            ("pre", true) => {
                pre_depth = pre_depth.saturating_sub(1);
                w.block();
            }
            ("blockquote", false) => {
                w.block();
                w.quote_depth += 1;
            }
            ("blockquote", true) => {
                w.block();
                w.quote_depth = w.quote_depth.saturating_sub(1);
            }
            ("p" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "ul" | "ol" | "table", _) => {
                w.paragraph()
            }
            ("li", false) => {
                w.block();
                if plain {
                    w.text("- ", true);
                }
            }
            (
                "div" | "li" | "tr" | "hr" | "dt" | "dd" | "section" | "article" | "header"
                | "footer" | "address" | "center",
                _,
            ) => w.block(),
            ("td" | "th", true) => w.text(" ", true),
            ("a", false) if plain => {
                link = HREF.captures(&m[3]).map(|h| {
                    let href = h.get(1).or(h.get(2)).or(h.get(3)).unwrap().as_str();
                    (
                        html_escape::decode_html_entities(href).into_owned(),
                        w.out.len(),
                    )
                });
            }
            ("a", true) if plain => {
                if let Some((href, start)) = link.take() {
                    // `start` can be past the end after newline() trimmed spaces.
                    let shown = w.out.get(start..).unwrap_or("").trim();
                    let is_web = href.starts_with("http://") || href.starts_with("https://");
                    if is_web && shown != href && !shown.is_empty() {
                        w.text(&format!(" <{href}>"), true);
                    }
                }
            }
            _ => {}
        }
    }
    w.text(&html[last..], pre_depth > 0);
    w.out
}

/// Zero-width and filler characters newsletters use to pad their preview text.
const INVISIBLE: [char; 7] = [
    '\u{200b}', '\u{200c}', '\u{200d}', '\u{2060}', '\u{feff}', '\u{034f}', '\u{00ad}',
];

struct TextWriter {
    out: String,
    quote_depth: usize,
    plain: bool,
}

impl TextWriter {
    fn at_line_start(&self) -> bool {
        self.out.is_empty() || self.out.ends_with('\n')
    }

    fn prefix(&mut self) {
        if self.plain && self.quote_depth > 0 {
            self.out.push_str(&"> ".repeat(self.quote_depth));
        }
    }

    fn text(&mut self, raw: &str, preformatted: bool) {
        let decoded: String = html_escape::decode_html_entities(raw)
            .chars()
            .filter(|c| !INVISIBLE.contains(c))
            .map(|c| if c == '\u{a0}' { ' ' } else { c })
            .collect();
        if preformatted {
            for (i, line) in decoded.split('\n').enumerate() {
                if i > 0 {
                    self.newline();
                }
                if !line.is_empty() {
                    if self.at_line_start() {
                        self.prefix();
                    }
                    self.out.push_str(line);
                }
            }
            return;
        }
        let mut collapsed = decoded.split_whitespace().collect::<Vec<_>>().join(" ");
        if collapsed.is_empty() {
            if decoded.chars().any(char::is_whitespace)
                && !self.at_line_start()
                && !self.out.ends_with(' ')
            {
                self.out.push(' ');
            }
            return;
        }
        if decoded.starts_with(char::is_whitespace)
            && !self.at_line_start()
            && !self.out.ends_with(' ')
        {
            collapsed.insert(0, ' ');
        }
        if decoded.ends_with(char::is_whitespace) {
            collapsed.push(' ');
        }
        if self.at_line_start() {
            self.prefix();
            collapsed = collapsed.trim_start().to_string();
        }
        self.out.push_str(&collapsed);
    }

    fn newline(&mut self) {
        if self.at_line_start() && self.plain && self.quote_depth > 0 {
            self.out.push_str(&">".repeat(self.quote_depth));
        }
        let trimmed = self.out.trim_end_matches(' ').len();
        self.out.truncate(trimmed);
        self.out.push('\n');
    }

    fn block(&mut self) {
        if !self.at_line_start() {
            self.newline();
        }
    }

    fn paragraph(&mut self) {
        self.block();
        if !self.out.is_empty() && !self.out.ends_with("\n\n") {
            self.newline();
        }
    }
}

static URL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"https?://[^\s<>"]+"#).unwrap());

/// Converts a plain-text body to HTML: escaped, newlines as `<br>`, spaces preserved with
/// `white-space:pre-wrap`, http(s) URLs linked. A trailing block of `>`-quoted lines (with the
/// `... wrote:` line before it) is wrapped in `<blockquote type="cite">` so
/// [`split_quoted_history`](crate::split_quoted_history) can fold it.
pub fn text_to_html(text: &str) -> String {
    let text = text.replace("\r\n", "\n");
    let lines: Vec<&str> = text.trim_end().split('\n').collect();
    let quote_start = trailing_quote_start(&lines);
    let render = |lines: &[&str]| {
        lines
            .iter()
            .map(|l| linkify(l))
            .collect::<Vec<_>>()
            .join("<br>")
    };

    let mut out = String::from(r#"<div dir="auto" style="white-space:pre-wrap">"#);
    match quote_start {
        Some(q) => {
            if q > 0 {
                // A <br> right before a block element does not show; keep the line break.
                out.push_str(&render(&lines[..q]));
                out.push_str("<br>");
            }
            out.push_str(r#"<blockquote type="cite" style="margin:0">"#);
            out.push_str(&render(&lines[q..]));
            out.push_str("</blockquote>");
        }
        None => out.push_str(&render(&lines)),
    }
    out.push_str("</div>");
    out
}

/// Index of the first line of the trailing `>`-quoted run, including a preceding
/// `... wrote:` attribution line.
fn trailing_quote_start(lines: &[&str]) -> Option<usize> {
    let mut start = None;
    for i in (0..lines.len()).rev() {
        let line = lines[i].trim();
        if line.starts_with('>') {
            start = Some(i);
        } else if !line.is_empty() {
            break;
        }
    }
    let q = start?;
    let attribution = (0..q).rev().find(|&i| !lines[i].trim().is_empty());
    match attribution {
        Some(i) if lines[i].trim_end().ends_with("wrote:") => Some(i),
        _ => Some(q),
    }
}

fn linkify(line: &str) -> String {
    let mut out = String::new();
    let mut last = 0;
    for m in URL.find_iter(line) {
        let mut url = m.as_str();
        loop {
            let trimmed = url.trim_end_matches(['.', ',', ';', ':', '!', '?', '\'']);
            let trimmed = if trimmed.ends_with(')')
                && trimmed.matches('(').count() < trimmed.matches(')').count()
            {
                &trimmed[..trimmed.len() - 1]
            } else {
                trimmed
            };
            if trimmed == url {
                break;
            }
            url = trimmed;
        }
        out.push_str(&html_escape::encode_text(&line[last..m.start()]));
        let attr = html_escape::encode_double_quoted_attribute(url);
        out.push_str(&format!(
            r#"<a href="{attr}">{}</a>"#,
            html_escape::encode_text(url)
        ));
        last = m.start() + url.len();
    }
    out.push_str(&html_escape::encode_text(&line[last..]));
    out
}

static DATA_URI: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)data:[a-z]+/[a-z0-9.+-]+;base64,([a-z0-9+/=]+)").unwrap());

/// Rewrites `data:` URIs in `html` whose decoded bytes equal a known inline part's data into
/// `cid:<content_id>`. Returns the rewritten HTML and the parts it referenced, in order of first
/// use and without duplicates. Unmatched `data:` URIs are left as they are.
pub fn data_images_to_cid(html: &str, known: &[InlinePart]) -> (String, Vec<InlinePart>) {
    let mut used: Vec<InlinePart> = Vec::new();
    let rewritten = DATA_URI.replace_all(html, |c: &Captures| {
        let found = decode_base64(&c[1]).and_then(|bytes| known.iter().find(|p| p.data == bytes));
        match found {
            Some(part) => {
                if !used.iter().any(|u| u.content_id == part.content_id) {
                    used.push(part.clone());
                }
                format!("cid:{}", part.content_id)
            }
            None => c[0].to_string(),
        }
    });
    (rewritten.into_owned(), used)
}

/// A `data:` URI for `bytes`, as used for the `cid_data` map of [`sanitize_html`].
pub fn data_uri(mime_type: &str, bytes: &[u8]) -> String {
    format!("data:{mime_type};base64,{}", encode_base64(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_collapses_and_skips_hidden() {
        let html = "<html><head><title>T</title><style>p{color:red}</style></head><body>\
            <p>Hello&nbsp;<b>world</b></p><!-- hidden --><script>x()</script>\
            <div>a &amp; b &zwnj;&#847; &#8203;</div></body></html>";
        assert_eq!(html_to_text(html), "Hello world a & b");
    }

    #[test]
    fn plain_keeps_structure() {
        let html = r#"<p>Hi Bob,</p><p>See <a href="https://x.com/a">the doc</a>.</p>
            <ul><li>one</li><li>two</li></ul><div>line<br>break</div>
            <blockquote>quoted<br>text</blockquote><div>end</div>"#;
        assert_eq!(
            html_to_plain(html),
            "Hi Bob,\n\nSee the doc <https://x.com/a>.\n\n- one\n- two\n\nline\nbreak\n> quoted\n> text\nend\n"
        );
    }

    #[test]
    fn plain_keeps_pre() {
        assert_eq!(html_to_plain("<pre>a  b\n  c</pre>"), "a  b\n  c\n");
        // A link right after trailing spaces and a break must not panic.
        assert_eq!(
            html_to_plain("<pre>é   <a href=\"https://x.io\"><br></a></pre>"),
            "é\n"
        );
    }

    #[test]
    fn text_to_html_escapes_and_links() {
        assert_eq!(
            text_to_html("a <b> & c\nsee https://x.com/p?a=1&b=2 (or http://y.org/x_(1)).\n"),
            "<div dir=\"auto\" style=\"white-space:pre-wrap\">a &lt;b&gt; &amp; c<br>see \
             <a href=\"https://x.com/p?a=1&amp;b=2\">https://x.com/p?a=1&amp;b=2</a> \
             (or <a href=\"http://y.org/x_(1)\">http://y.org/x_(1)</a>).</div>"
        );
    }

    #[test]
    fn text_to_html_wraps_trailing_quote() {
        let html = text_to_html("Sounds good.\n\nOn Mon, Bob wrote:\n> hi\n>\n> there\n");
        assert_eq!(
            html,
            "<div dir=\"auto\" style=\"white-space:pre-wrap\">Sounds good.<br><br>\
             <blockquote type=\"cite\" style=\"margin:0\">On Mon, Bob wrote:<br>&gt; hi<br>&gt;<br>&gt; there\
             </blockquote></div>"
        );
        assert!(!text_to_html("a\n> b\nc").contains("blockquote"));
    }
}
