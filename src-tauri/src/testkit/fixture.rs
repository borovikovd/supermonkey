use std::path::Path;

use crate::domain::{
    Addr, GmailAttachmentId, GmailMessageId, GmailThreadId, HistoryId, LabelId, LabelSet, Millis,
};
use crate::store::{
    ContactSighting, MessageBodyWrite, NewAttachment, NewMessage, StoreError, with_write_tx,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PerfStats {
    pub threads: usize,
    pub messages: usize,
    pub contacts: usize,
}

const SENDERS: [(&str, &str); 10] = [
    ("Alice Adams", "alice@corp.test"),
    ("Bob Baker", "bob@corp.test"),
    ("Carol Chen", "carol@eng.test"),
    ("Dave Diaz", "dave@eng.test"),
    ("Eve Evans", "eve@design.test"),
    ("Frank Ford", "frank@finance.test"),
    ("Grace Green", "grace@ops.test"),
    ("Henry Hall", "henry@sales.test"),
    ("Ivy Ito", "ivy@support.test"),
    ("Jack Jones", "jack@partner.test"),
];

const TOPICS: [&str; 8] = [
    "Quarterly planning notes",
    "Invoice approval needed",
    "Engineering design review",
    "Customer rollout checklist",
    "Security audit follow-up",
    "Release calendar update",
    "Budget forecast draft",
    "Team offsite schedule",
];

#[expect(
    clippy::too_many_lines,
    reason = "populates a deterministic multi-thousand message perf fixture inside one SQLite transaction"
)]
pub fn generate_perf_db(path: &Path, thread_count: usize) -> Result<PerfStats, StoreError> {
    if path.exists() {
        std::fs::remove_file(path).map_err(|source| StoreError::Io {
            action: "remove existing perf db",
            path: path.to_path_buf(),
            source,
        })?;
    }
    let anchor_ms: i64 = 1_793_631_600_000;
    let mut total_messages = 0usize;
    let mut sightings = Vec::with_capacity(thread_count * 2);

    with_write_tx(path, |wtx| {
        let acct = wtx.create_account("ana@acme.test", "Ana Ames")?;
        let me = Addr::named("Ana Ames", "ana@acme.test");

        for i in 0..thread_count {
            let (name, email) = SENDERS[i % SENDERS.len()];
            let topic = TOPICS[i % TOPICS.len()];
            let subject = format!("{topic} #{i}");
            let thread_key = GmailThreadId(format!("perf-t-{i}"));
            let tid = wtx.place_thread(acct.id, &thread_key, Some(HistoryId(1000)))?;
            let msgs_in_thread = if i % 4 == 0 { 3 } else { 2 };

            for m in 0..msgs_in_thread {
                let seq = i * 4 + m;
                let msg_key = GmailMessageId(format!("perf-m-{seq}"));
                let sent_by_me = m == 1;
                let from = if sent_by_me {
                    me.clone()
                } else {
                    Addr::named(name, email)
                };
                let to = if sent_by_me {
                    vec![Addr::named(name, email)]
                } else {
                    vec![me.clone()]
                };
                let date = Millis(
                    anchor_ms - i64::try_from(i).unwrap_or(0) * 60_000
                        + i64::try_from(m).unwrap_or(0) * 1_000,
                );

                let mut raw_labels = Vec::new();
                if i < 500 {
                    raw_labels.push("INBOX".to_owned());
                }
                if i % 5 == 0 && m + 1 == msgs_in_thread {
                    raw_labels.push("UNREAD".to_owned());
                }
                if i % 7 == 0 {
                    raw_labels.push("STARRED".to_owned());
                }
                if sent_by_me {
                    raw_labels.push("SENT".to_owned());
                }
                raw_labels.push("CATEGORY_PERSONAL".to_owned());
                let labels: LabelSet = raw_labels.into_iter().map(LabelId).collect();

                let body_text = format!(
                    "Hi team, following up on {topic} for thread {i}, message {m}. Please review the quarterly metrics and invoice details."
                );
                let body_html = format!("<p>{body_text}</p>");
                let has_att = i % 6 == 0 && m == 0;
                let attachments = if has_att {
                    vec![NewAttachment {
                        part_id: "1".to_owned(),
                        filename: format!("invoice-{i}.pdf"),
                        mime_type: "application/pdf".to_owned(),
                        size: 24_576,
                        gmail_att_id: Some(GmailAttachmentId(format!("att-{seq}"))),
                        content_id: None,
                        is_inline: false,
                        local_path: None,
                    }]
                } else {
                    Vec::new()
                };

                let msg_id = wtx.upsert_message(
                    acct.id,
                    tid,
                    &NewMessage {
                        gmail_id: msg_key.clone(),
                        date,
                        from: from.clone(),
                        to: to.clone(),
                        cc: Vec::new(),
                        bcc: Vec::new(),
                        reply_to: Vec::new(),
                        subject: subject.clone(),
                        snippet: body_text.chars().take(120).collect(),
                        message_id_hdr: Some(format!("<perf-{seq}@corp.test>")),
                        in_reply_to: None,
                        references: Vec::new(),
                        is_list: false,
                        is_draft: false,
                        sm_draft_id: None,
                        labels,
                        has_attachments: Some(has_att),
                    },
                )?;

                wtx.store_message_body(
                    msg_id,
                    &MessageBodyWrite {
                        html: &body_html,
                        quote_html: None,
                        body_text: &body_text,
                        attachments: &attachments,
                    },
                )?;

                sightings.push(ContactSighting {
                    gmail_msg_id: msg_key,
                    date,
                    sent_by_me,
                    is_list: false,
                    from,
                    to,
                    cc: Vec::new(),
                });
                total_messages += 1;
            }
        }

        for c in 0..250usize {
            let email = format!("contact{c}@eng-partner{}.test", c % 20);
            let name = format!("Contact Person {c}");
            sightings.push(ContactSighting {
                gmail_msg_id: GmailMessageId(format!("perf-contact-{c}")),
                date: Millis(anchor_ms - i64::try_from(c).unwrap_or(0) * 3_600_000),
                sent_by_me: c % 2 == 0,
                is_list: false,
                from: Addr::named(name, email),
                to: vec![me.clone()],
                cc: Vec::new(),
            });
        }

        wtx.record_contacts(acct.id, &sightings)?;
        wtx.set_connected(acct.id, true)?;
        wtx.set_history(acct.id, HistoryId(1000))?;
        Ok(())
    })?;

    Ok(PerfStats {
        threads: thread_count,
        messages: total_messages,
        contacts: SENDERS.len() + 250,
    })
}
