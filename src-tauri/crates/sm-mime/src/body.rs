//! From decoded bodies to what the app stores: sanitized HTML split at its
//! quoted history, and text for search.

use crate::cid::CidImages;
use crate::quote::{split_gmail_quote, split_quoted_history};
use crate::sanitize::sanitize_html;
use crate::text::html_to_text;
use crate::text_html::text_to_html;

/// The decoded bodies of a message, not sanitized.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RawBody {
    /// The `text/html` body.
    pub html: Option<String>,
    /// The `text/plain` body.
    pub text: Option<String>,
}

/// A received message's body, ready to store (SPEC Reading).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Body {
    /// Sanitized HTML above the quoted history.
    pub html: String,
    /// Sanitized quoted history, shown behind the `•••` toggle.
    pub quote: Option<String>,
    /// Text for search and snippets: the `text/plain` body, else the HTML's text.
    pub text: String,
    /// The message had no HTML body, so `html` was made from its text.
    pub is_plain: bool,
}

/// A draft read from Gmail, split for the composer (SPEC Reply composer).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DraftBody {
    /// Sanitized HTML for the editor: everything above the `gmail_quote`.
    pub editor_html: String,
    /// The sanitized quote block, kept as it is.
    pub quote_html: Option<String>,
}

impl RawBody {
    /// The HTML body, or the text body as HTML, and whether it came from text.
    fn into_html(self) -> (String, Option<String>, bool) {
        match (self.html, self.text) {
            (Some(html), text) => (html, text, false),
            (None, Some(text)) => (text_to_html(&text), Some(text), true),
            (None, None) => (String::new(), None, true),
        }
    }

    /// Sanitizes a received body for storage. `images` are its inline images,
    /// which replace their `cid:` references (SPEC Reading).
    #[must_use]
    pub fn render(self, images: &CidImages) -> Body {
        let (html, text, is_plain) = self.into_html();
        let (visible, quote) = split_quoted_history(&html);
        Body {
            html: sanitize_html(visible, images),
            quote: quote.map(|q| sanitize_html(q, images)),
            text: text.unwrap_or_else(|| html_to_text(&html)),
            is_plain,
        }
    }

    /// Splits and sanitizes the body of a draft read from Gmail. Its inline
    /// images stay `cid:` references, which the sanitizer drops.
    #[must_use]
    pub fn into_draft(self) -> DraftBody {
        let (html, _, _) = self.into_html();
        let (editor, quote) = split_gmail_quote(&html);
        let images = CidImages::new();
        DraftBody {
            editor_html: sanitize_html(editor, &images),
            quote_html: quote.map(|q| sanitize_html(q, &images)),
        }
    }
}
