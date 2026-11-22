use std::collections::HashSet;

use sm_domain::{Addr, GmailMessageId, GmailThreadId, LabelSet, Millis, SystemLabel};
use sm_gmail::{Format, GmailAttachmentId, GmailError, HistoryId, Message, Outcome, Priority};
use sm_mime::{CidImages, DraftBody, MessageHeaders, ParsedMessage, Part, RawBody};
use sm_store::{NewAttachment, NewBody, NewMessage};

use super::ctx::AccountCtx;

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

#[derive(Debug, Clone)]
pub(crate) struct ReadThread {
    pub(crate) gmail_id: GmailThreadId,
    pub(crate) history: Option<HistoryId>,
    pub(crate) messages: Vec<ReadMessage>,
}

#[derive(Debug, Clone)]
pub(crate) struct ReadMessage {
    pub(crate) message: NewMessage,
    pub(crate) body: Option<NewBody>,
}

impl ReadMessage {
    pub(crate) const fn brought(&self) -> (&GmailMessageId, Option<&NewBody>) {
        (&self.message.gmail_id, self.body.as_ref())
    }
}

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
    for m in read.messages {
        let mut parsed = parse(&m);
        let message = new_message(m, std::mem::take(&mut parsed.headers), &parsed.parts);
        let body = if format == Format::Full && !stored.contains(&message.gmail_id) {
            Some(body(ctx, &message.gmail_id, parsed, priority).await?)
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

pub(crate) fn parse(m: &Message) -> ParsedMessage {
    let Some(payload) = &m.payload else {
        return ParsedMessage::default();
    };
    sm_mime::parse_message(payload).unwrap_or_else(|e| {
        tracing::warn!(error = %e, "skipping a message payload that doesn't parse");
        ParsedMessage::default()
    })
}

pub(crate) fn new_message(m: Message, headers: MessageHeaders, parts: &[Part]) -> NewMessage {
    let labels: LabelSet = m.label_ids.into_iter().collect();
    NewMessage {
        gmail_id: m.id,
        date: m.internal_date.or(headers.date).unwrap_or(Millis(0)),
        from: headers.from.unwrap_or_else(|| Addr::new(String::new())),
        to: headers.to,
        cc: headers.cc,
        bcc: headers.bcc,
        reply_to: headers.reply_to,
        subject: headers.subject,
        snippet: sm_mime::snippet_text(&m.snippet).into_owned(),
        message_id_header: headers.message_id,
        in_reply_to: headers.in_reply_to,
        references: headers.references.join(" "),
        is_list: headers.is_list,
        is_draft: labels.has(SystemLabel::Draft),
        sm_draft: headers.draft_id,
        labels,
        has_attachments: parts.iter().any(|p| !p.is_inline),
    }
}

async fn body(
    ctx: &AccountCtx,
    message: &GmailMessageId,
    parsed: ParsedMessage,
    priority: Priority,
) -> Result<NewBody, GmailError> {
    let images = inline_images(ctx, message, &parsed, priority).await?;
    let attachments = parsed.parts.iter().map(new_attachment).collect();
    let rendered = render(parsed.body, images).await;
    Ok(NewBody {
        html: Some(rendered.html),
        quote_html: rendered.quote,
        text: Some(rendered.text),
        attachments,
    })
}

async fn inline_images(
    ctx: &AccountCtx,
    message: &GmailMessageId,
    parsed: &ParsedMessage,
    priority: Priority,
) -> Result<CidImages, GmailError> {
    let mut images = parsed.cid_images();
    for fetch in parsed.inline_images_to_fetch() {
        let attachment = GmailAttachmentId::from(fetch.attachment_id);
        let read = ctx
            .gmail
            .get_attachment(priority, message, &attachment)
            .await;
        if let Some(bytes) = unless_gone(read)? {
            images.insert(fetch.content_id, fetch.mime_type, &bytes);
        }
    }
    Ok(images)
}

fn unless_gone(read: Result<Vec<u8>, GmailError>) -> Result<Option<Vec<u8>>, GmailError> {
    match read {
        Ok(bytes) => Ok(Some(bytes)),
        Err(e) if *e.outcome() == Outcome::NotFound => {
            tracing::debug!("an inline image is gone; leaving it out");
            Ok(None)
        }
        Err(e) => Err(e),
    }
}

async fn render(raw: RawBody, images: CidImages) -> sm_mime::Body {
    let rendered = tokio::task::spawn_blocking(move || raw.render(&images)).await;
    match rendered {
        Ok(body) => body,
        Err(e) => {
            tracing::error!(error = %e, "rendering a body failed; storing it empty");
            sm_mime::Body {
                html: String::new(),
                quote: None,
                text: String::new(),
            }
        }
    }
}

pub(crate) fn draft_message(m: Message) -> (ReadMessage, DraftBody) {
    let parsed = parse(&m);
    let message = new_message(m, parsed.headers, &parsed.parts);
    let draft = parsed.body.to_draft();
    let rendered = parsed.body.render(&CidImages::new());
    let body = NewBody {
        html: Some(rendered.html),
        quote_html: rendered.quote,
        text: Some(rendered.text),
        attachments: parsed.parts.iter().map(new_attachment).collect(),
    };
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
        part_id: part.id.clone(),
        filename: part.filename.clone(),
        mime_type: part.mime_type.clone(),
        size: i64::try_from(part.size).unwrap_or(i64::MAX),
        gmail_attachment_id: part.attachment_id.clone(),
        content_id: part.content_id.clone(),
        is_inline: part.is_inline,
    }
}
