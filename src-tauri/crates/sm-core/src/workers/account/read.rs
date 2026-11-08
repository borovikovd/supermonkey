//! Reading threads from Gmail, and parsing and sanitizing them before they
//! reach the writer thread (ARCHITECTURE.md, section 5.3; SPEC Reading).

use std::collections::HashSet;

use sm_domain::{Addr, GmailMessageId, GmailThreadId, LabelSet, Millis, SystemLabel};
use sm_gmail::{Format, GmailAttachmentId, GmailError, HistoryId, Message, Outcome, Priority};
use sm_mime::{CidImages, DraftBody, ParsedMessage, Part};
use sm_store::{NewAttachment, NewBody, NewMessage};

use super::ctx::AccountCtx;

/// The headers a metadata read asks for: what the app stores.
pub(crate) const METADATA_HEADERS: &[&str] = &[
    "From",
    "To",
    "Cc",
    "Bcc",
    "Reply-To",
    "Subject",
    "Date",
    "Message-ID",
    "In-Reply-To",
    "References",
    "List-Id",
    "List-Unsubscribe",
    "X-Supermonkey-Draft",
];

/// A thread as Gmail gave it, parsed, its bodies sanitized: ready to store.
#[derive(Debug, Clone)]
pub(crate) struct ReadThread {
    pub(crate) gmail_id: GmailThreadId,
    /// Gmail's `historyId` for the thread when it was read.
    pub(crate) history: Option<HistoryId>,
    /// Every message of the thread, oldest first.
    pub(crate) messages: Vec<ReadMessage>,
}

/// One message of a [`ReadThread`].
#[derive(Debug, Clone)]
pub(crate) struct ReadMessage {
    /// Headers and flags. Its labels are Gmail's ids until the writer maps
    /// them to local ones.
    pub(crate) message: NewMessage,
    /// The body, when the read had one the app does not store yet.
    pub(crate) body: Option<NewBody>,
}

/// Reads a thread at `priority` and parses it. With [`Format::Full`],
/// messages in `stored` keep the body they have, which never changes: they
/// are parsed without one, so their inline images are not fetched again.
pub(crate) async fn read_thread(
    ctx: &AccountCtx,
    thread: &GmailThreadId,
    format: Format,
    priority: Priority,
    stored: &HashSet<GmailMessageId>,
) -> Result<ReadThread, GmailError> {
    let headers = if format == Format::Metadata {
        METADATA_HEADERS
    } else {
        &[]
    };
    let read = ctx
        .gmail
        .get_thread(priority, thread, format, headers)
        .await?;
    let mut messages = Vec::with_capacity(read.messages.len());
    for m in &read.messages {
        let parsed = parse(m);
        let message = new_message(m, &parsed);
        let body = if format == Format::Full && !stored.contains(&m.id) {
            Some(body(ctx, m, parsed, priority).await?)
        } else {
            None
        };
        messages.push(ReadMessage { message, body });
    }
    Ok(ReadThread {
        gmail_id: read.id,
        history: read.history_id,
        messages,
    })
}

/// The message's payload, parsed; an empty one when Gmail sent none or one
/// that doesn't parse (logged).
pub(crate) fn parse(m: &Message) -> ParsedMessage {
    let Some(payload) = &m.payload else {
        return ParsedMessage::default();
    };
    sm_mime::parse_message(payload).unwrap_or_else(|e| {
        tracing::warn!(error = %e, "skipping a message payload that doesn't parse");
        ParsedMessage::default()
    })
}

/// The stored form of a message's headers. Its labels are Gmail's ids.
pub(crate) fn new_message(m: &Message, parsed: &ParsedMessage) -> NewMessage {
    let h = &parsed.headers;
    let labels: LabelSet = m.label_ids.iter().cloned().collect();
    NewMessage {
        gmail_id: m.id.clone(),
        date: m.internal_date.or(h.date).unwrap_or(Millis(0)),
        from: h.from.clone().unwrap_or_else(|| Addr::new(String::new())),
        to: h.to.clone(),
        cc: h.cc.clone(),
        bcc: h.bcc.clone(),
        reply_to: h.reply_to.clone(),
        subject: h.subject.clone(),
        snippet: sm_mime::snippet_text(&m.snippet).into_owned(),
        message_id_header: h.message_id.clone(),
        in_reply_to: h.in_reply_to.clone(),
        references: h.references.join(" "),
        is_list: h.is_list,
        is_draft: labels.has(SystemLabel::Draft),
        sm_draft: h.draft_id.clone(),
        labels,
        has_attachments: parsed.parts.iter().any(|p| !p.is_inline),
    }
}

/// The body of a full read: its inline images fetched (a missing one is
/// left out), then rendered and sanitized off the async threads.
async fn body(
    ctx: &AccountCtx,
    m: &Message,
    parsed: ParsedMessage,
    priority: Priority,
) -> Result<NewBody, GmailError> {
    let mut images = parsed.cid_images();
    let fetches: Vec<(String, GmailAttachmentId, String)> = parsed
        .inline_images_to_fetch()
        .map(|f| {
            (
                f.content_id.to_owned(),
                GmailAttachmentId::from(f.attachment_id),
                f.mime_type.to_owned(),
            )
        })
        .collect();
    for (content_id, attachment, mime_type) in fetches {
        match ctx.gmail.get_attachment(priority, &m.id, &attachment).await {
            Ok(bytes) => images.insert(&content_id, &mime_type, &bytes),
            Err(e) if *e.outcome() == Outcome::NotFound => {
                tracing::debug!("an inline image is gone; leaving it out");
            }
            Err(e) => return Err(e),
        }
    }
    let attachments = parsed.parts.iter().map(new_attachment).collect();
    let raw = parsed.body;
    let rendered = tokio::task::spawn_blocking(move || raw.render(&images)).await;
    let rendered = match rendered {
        Ok(body) => body,
        Err(e) => {
            tracing::error!(error = %e, "rendering a body failed; storing it empty");
            sm_mime::Body {
                html: String::new(),
                quote: None,
                text: String::new(),
                is_plain: true,
            }
        }
    };
    Ok(NewBody {
        html: Some(rendered.html),
        quote_html: rendered.quote,
        text: Some(rendered.text),
        is_plain: rendered.is_plain,
        attachments,
    })
}

/// A Gmail draft's message, parsed and sanitized for storing, and its HTML
/// split at the `gmail_quote` element: the editor's part and the quote
/// block (SPEC Addressing). Its inline images are not fetched: the composer
/// shows only the text above the quote. Slow on a big draft: call it off
/// the async threads.
pub(crate) fn draft_message(m: &Message) -> (ReadMessage, DraftBody) {
    let parsed = parse(m);
    let message = new_message(m, &parsed);
    let rendered = parsed.body.clone().render(&CidImages::new());
    let body = NewBody {
        html: Some(rendered.html),
        quote_html: rendered.quote,
        text: Some(rendered.text),
        is_plain: rendered.is_plain,
        attachments: parsed.parts.iter().map(new_attachment).collect(),
    };
    let draft = parsed.body.into_draft();
    (
        ReadMessage {
            message,
            body: Some(body),
        },
        draft,
    )
}

fn new_attachment(part: &Part) -> NewAttachment {
    NewAttachment {
        part_id: part.part_id.clone(),
        filename: part.filename.clone(),
        mime_type: part.mime_type.clone(),
        size: i64::try_from(part.size).unwrap_or(i64::MAX),
        gmail_attachment_id: part.attachment_id.clone(),
        content_id: part.content_id.clone(),
        is_inline: part.is_inline,
    }
}
