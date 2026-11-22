use std::fmt::Write as _;

use chrono::{DateTime, FixedOffset, Offset as _, Utc};
use sm_domain::{Addr, Millis};

#[derive(Debug, Clone, Copy)]
pub struct QuoteSource<'a> {
    pub from: &'a Addr,
    pub to: &'a [Addr],
    pub cc: &'a [Addr],
    pub date: Millis,
    pub subject: &'a str,
    pub html: &'a str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct UtcOffset(pub i32);

#[must_use]
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

#[must_use]
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
        _ = write!(out, "Cc: {}<br>", addr_list_html(source.cc));
    }
    out.push_str("</div><br><br>");
    out.push_str(source.html);
    out.push_str("</div>");
    out
}

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

#[must_use]
pub fn reply_subject(subject: &str) -> String {
    prefixed(subject, "Re:", &["re:"])
}

#[must_use]
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
