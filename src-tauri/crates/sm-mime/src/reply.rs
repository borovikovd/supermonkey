//! Gmail-style quote blocks and subjects for replies and forwards (SPEC Reply
//! composer).

use std::fmt::Write as _;

use chrono::{DateTime, FixedOffset, Offset as _, Utc};
use sm_domain::{Addr, Millis};

/// The message a reply or forward quotes.
#[derive(Debug, Clone, Copy)]
pub struct QuoteSource<'a> {
    /// Its sender.
    pub from: &'a Addr,
    /// Its `To` recipients.
    pub to: &'a [Addr],
    /// Its `Cc` recipients.
    pub cc: &'a [Addr],
    /// When it was sent.
    pub date: Millis,
    /// Its subject.
    pub subject: &'a str,
    /// Its stored, sanitized HTML. For a plain-text message, pass
    /// [`text_to_html`](crate::text_to_html) of its text.
    pub html: &'a str,
}

/// The user's offset from UTC, in minutes east (UTC+2 is `120`; JavaScript's
/// `getTimezoneOffset()` has the opposite sign). Quote dates are shown in it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct UtcOffset(pub i32);

/// Gmail's reply quote block:
///
/// ```html
/// <div class="gmail_quote"><div dir="ltr" class="gmail_attr">On Mon, Jan 6, 2025 at 10:00 AM
/// Alice &lt;<a href="mailto:a@x.com">a@x.com</a>&gt; wrote:<br></div><blockquote
/// class="gmail_quote" style="…">original html</blockquote></div>
/// ```
pub fn reply_quote_html(source: &QuoteSource<'_>, offset: UtcOffset) -> String {
    format!(
        "<div class=\"gmail_quote\"><div dir=\"ltr\" class=\"gmail_attr\">On {} {} wrote:<br></div>\
         <blockquote class=\"gmail_quote\" style=\"margin:0px 0px 0px 0.8ex;border-left:1px solid rgb(204,204,204);padding-left:1ex\">\
         {}</blockquote></div>",
        quote_date(source.date, offset),
        addr_html(source.from),
        source.html
    )
}

/// Gmail's forward block: `---------- Forwarded message ---------`, the
/// original From, Date, Subject, To (and Cc when there is one), then the
/// original HTML, all inside `<div class="gmail_quote">`.
pub fn forward_quote_html(source: &QuoteSource<'_>, offset: UtcOffset) -> String {
    let email = mailto_html(&source.from.email);
    let from = match display_name(source.from) {
        Some(name) => format!(
            "<strong class=\"gmail_sendername\" dir=\"auto\">{}</strong> <span dir=\"auto\">&lt;{email}&gt;</span>",
            html_escape::encode_text(name),
        ),
        None => format!("<span dir=\"auto\">&lt;{email}&gt;</span>"),
    };
    let mut out = format!(
        "<div class=\"gmail_quote\"><div dir=\"ltr\" class=\"gmail_attr\">---------- Forwarded message ---------<br>\
         From: {from}<br>Date: {}<br>Subject: {}<br>To: {}<br>",
        quote_date(source.date, offset),
        html_escape::encode_text(source.subject),
        addr_list_html(source.to),
    );
    if !source.cc.is_empty() {
        // Writing to a String cannot fail.
        let _ = write!(out, "Cc: {}<br>", addr_list_html(source.cc));
    }
    out.push_str("</div><br><br>");
    out.push_str(source.html);
    out.push_str("</div>");
    out
}

/// `Mon, Jan 6, 2025 at 10:00 AM`, Gmail's quote date format. An offset out of
/// range is read as UTC.
fn quote_date(date: Millis, offset: UtcOffset) -> String {
    let zone = offset
        .0
        .checked_mul(60)
        .and_then(FixedOffset::east_opt)
        .unwrap_or_else(|| Utc.fix());
    DateTime::from_timestamp_millis(date.0)
        .unwrap_or_default()
        .with_timezone(&zone)
        .format("%a, %b %-d, %Y at %-I:%M %p")
        .to_string()
}

fn display_name(addr: &Addr) -> Option<&str> {
    addr.name.as_deref().filter(|name| !name.is_empty())
}

fn mailto_html(email: &str) -> String {
    format!(
        "<a href=\"mailto:{}\">{}</a>",
        html_escape::encode_double_quoted_attribute(email),
        html_escape::encode_text(email)
    )
}

/// `Name &lt;<a href="mailto:e">e</a>&gt;`, or the bracketed link alone.
fn addr_html(addr: &Addr) -> String {
    let email = mailto_html(&addr.email);
    match display_name(addr) {
        Some(name) => format!("{} &lt;{email}&gt;", html_escape::encode_text(name)),
        None => format!("&lt;{email}&gt;"),
    }
}

fn addr_list_html(list: &[Addr]) -> String {
    list.iter().map(addr_html).collect::<Vec<_>>().join(", ")
}

/// `Re: subject`, unless it already starts with `Re:` in any case.
pub fn reply_subject(subject: &str) -> String {
    prefixed(subject, "Re:", &["re:"])
}

/// `Fwd: subject`, unless it already starts with `Fwd:` or `Fw:` in any case.
pub fn forward_subject(subject: &str) -> String {
    prefixed(subject, "Fwd:", &["fwd:", "fw:"])
}

fn prefixed(subject: &str, prefix: &str, existing: &[&str]) -> String {
    let subject = subject.trim();
    let lower = subject.to_lowercase();
    if existing.iter().any(|p| lower.starts_with(p)) {
        subject.to_owned()
    } else if subject.is_empty() {
        prefix.to_owned()
    } else {
        format!("{prefix} {subject}")
    }
}
