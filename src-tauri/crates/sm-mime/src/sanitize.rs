//! The HTML sanitizer for received mail (SPEC Reading, Security).

use std::borrow::Cow;
use std::sync::LazyLock;

use regex::{Captures, Regex};

use crate::cid::{CidImages, inline_cid_images};

/// The policy: ammonia's defaults, plus what mail layout needs.
static POLICY: LazyLock<ammonia::Builder<'static>> = LazyLock::new(|| {
    let mut policy = ammonia::Builder::default();
    policy
        .add_tags([
            "style", "font", "tfoot", "section", "article", "aside", "main", "address", "big",
            "label", "fieldset", "legend",
        ])
        // Removed together with everything inside them. This replaces the
        // default set, which holds `style`.
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
            is_allowed_url(element, attribute, value).then_some(Cow::Borrowed(value))
        });
    policy
});

/// `data:` URLs only for images, and only where an image goes (`<img src>`,
/// `background`); `background` also takes http(s). ammonia checks the schemes of
/// every other URL attribute.
fn is_allowed_url(element: &str, attribute: &str, value: &str) -> bool {
    let url = value.trim_start().to_ascii_lowercase();
    let is_image_slot = (element == "img" && attribute == "src") || attribute == "background";
    if url.starts_with("data:") && !(is_image_slot && url.starts_with("data:image/")) {
        return false;
    }
    attribute != "background"
        || url.starts_with("http://")
        || url.starts_with("https://")
        || url.starts_with("data:image/")
}

/// Sanitizes received HTML for storage and display.
///
/// First rewrites `cid:` references to the matching [`CidImages`]. Then cleans
/// with ammonia: `style`, `class`, `id`, table and layout attributes, and
/// `<style>` blocks stay, as do `data:image/*` URLs on `<img src>` and
/// `background`. Scripts, event handlers, forms and their controls, iframes,
/// objects, embeds, `<meta>`, `<base>`, `<link>`, SVG, `MathML`, and `javascript:`
/// URLs go, including in CSS `url(…)`. Links get `rel="noopener noreferrer"`.
pub fn sanitize_html(html: &str, images: &CidImages) -> String {
    let with_images = inline_cid_images(html, images);
    let clean = POLICY.clean(&with_images).to_string();
    match defuse_css_urls(&clean) {
        Cow::Borrowed(_) => clean,
        Cow::Owned(defused) => defused,
    }
}

/// A CSS `url(…)`, quoted or not.
static CSS_URL: LazyLock<Regex> =
    LazyLock::new(|| crate::regex(r#"(?i)url\(\s*('[^']*'|"[^"]*"|[^)'"]*)\s*\)"#));

/// Empties CSS `url(…)`s with a script scheme, which ammonia leaves in `style`
/// attributes and `<style>` blocks.
fn defuse_css_urls(html: &str) -> Cow<'_, str> {
    CSS_URL.replace_all(html, |c: &Captures<'_>| {
        let whole = c.get(0).map_or("", |m| m.as_str());
        let target = c.get(1).map_or("", |m| m.as_str());
        if is_script_url(target) {
            "url()".to_owned()
        } else {
            whole.to_owned()
        }
    })
}

/// Whether a CSS `url(…)` argument runs script. HTML entities (as attributes are
/// serialized), CSS escapes, quotes, whitespace, and controls are removed before
/// the scheme is read, as a browser would ignore or decode them.
fn is_script_url(target: &str) -> bool {
    let decoded = html_escape::decode_html_entities(target);
    let scheme: String = decode_css_escapes(&decoded)
        .chars()
        .filter(|c| !c.is_whitespace() && !c.is_control() && *c != '"' && *c != '\'')
        .take("javascript:".len())
        .collect::<String>()
        .to_ascii_lowercase();
    scheme.starts_with("javascript:") || scheme.starts_with("vbscript:")
}

/// Decodes CSS escapes: `\6a` and `\00006a ` (up to six hex digits and one
/// optional space) are the code point, and `\x` is `x`.
fn decode_css_escapes(s: &str) -> Cow<'_, str> {
    if !s.contains('\\') {
        return Cow::Borrowed(s);
    }
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        let mut code = 0u32;
        let mut digits = 0;
        while digits < 6
            && let Some(d) = chars.peek().and_then(|d| d.to_digit(16))
        {
            code = code * 16 + d;
            digits += 1;
            chars.next();
        }
        if digits == 0 {
            out.extend(chars.next());
            continue;
        }
        if chars.peek().is_some_and(|c| c.is_whitespace()) {
            chars.next();
        }
        out.push(char::from_u32(code).unwrap_or(char::REPLACEMENT_CHARACTER));
    }
    Cow::Owned(out)
}
