use std::borrow::Cow;

use crate::cid::CidImages;
use crate::quote::{split_gmail_quote, split_quoted_history};
use crate::sanitize::sanitize_html;
use crate::text::html_to_text;
use crate::text_html::text_to_html;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RawBody {
    pub html: Option<String>,
    pub text: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Body {
    pub html: String,
    pub quote: Option<String>,
    pub text: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DraftBody {
    pub editor_html: String,
    pub quote_html: Option<String>,
}

impl RawBody {
    fn into_html(self) -> (String, Option<String>) {
        match (self.html, self.text) {
            (Some(html), text) => (html, text),
            (None, Some(text)) => (text_to_html(&text), Some(text)),
            (None, None) => (String::new(), None),
        }
    }

    #[must_use]
    pub fn render(self, images: &CidImages) -> Body {
        let (html, text) = self.into_html();
        let (visible, quote) = split_quoted_history(&html);
        Body {
            html: sanitize_html(visible, images),
            quote: quote.map(|q| sanitize_html(q, images)),
            text: text.unwrap_or_else(|| html_to_text(&html)),
        }
    }

    fn html(&self) -> Cow<'_, str> {
        match (&self.html, &self.text) {
            (Some(html), _) => Cow::Borrowed(html),
            (None, Some(text)) => Cow::Owned(text_to_html(text)),
            (None, None) => Cow::Borrowed(""),
        }
    }

    #[must_use]
    pub fn to_draft(&self) -> DraftBody {
        let html = self.html();
        let (editor, quote) = split_gmail_quote(&html);
        let images = CidImages::new();
        DraftBody {
            editor_html: sanitize_html(editor, &images),
            quote_html: quote.map(|q| sanitize_html(q, &images)),
        }
    }
}
