//! A draft as its composer shows it.

use sm_domain::{AccountId, DraftId, DraftKind, Rev};
use sm_ipc::{Addr, DraftAttachment, DraftState};
use sm_store::{Reads, StoreError};

use crate::drafts::lifecycle;

/// The draft for a composer subscription. A draft that is gone gives
/// `gone`, which closes the composer.
pub(crate) fn draft_state(reads: Reads<'_>, id: &DraftId) -> Result<DraftState, StoreError> {
    let Some(record) = reads.drafts().get(id)? else {
        return Ok(DraftState {
            id: id.clone(),
            account: AccountId::NONE,
            kind: DraftKind::New,
            reply_all: false,
            thread: None,
            target: None,
            from: None,
            to: Vec::new(),
            cc: Vec::new(),
            bcc: Vec::new(),
            subject: String::new(),
            body_html: String::new(),
            quote_html: None,
            attachments: Vec::new(),
            resumed: false,
            gone: true,
            rev: Rev(0),
        });
    };
    let d = record.draft;
    let resumed = !lifecycle::is_fresh(&d);
    let addrs = |list: Vec<sm_domain::Addr>| list.into_iter().map(Addr::from).collect();
    Ok(DraftState {
        id: d.id,
        account: d.account,
        kind: d.kind,
        reply_all: d.reply_all,
        thread: d.thread,
        target: d.target,
        from: d.from.map(Addr::from),
        to: addrs(d.to),
        cc: addrs(d.cc),
        bcc: addrs(d.bcc),
        subject: d.subject,
        body_html: d.body_html,
        quote_html: d.quote_html,
        attachments: d
            .attachments
            .into_iter()
            .map(|a| DraftAttachment {
                id: a.id,
                filename: a.filename,
                mime_type: a.mime_type,
                size: a.size,
                path: a.path,
                source: a.source,
            })
            .collect(),
        resumed,
        gone: false,
        rev: record.rev,
    })
}
