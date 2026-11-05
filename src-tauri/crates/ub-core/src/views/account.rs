//! The app state and account summaries (SPEC UI state, Subscriptions).

use ub_domain::{AVATAR_HUES, AccountId, SyncStatus};
use ub_ipc::{
    AccountInfo, AccountSummary, AppState, GlobalSettings, LabelInfo, Notice, PinInfo, SendAs,
    SplitInfo, UndoEntryInfo,
};
use ub_store::{AccountRecord, Reads, StoreError};

use super::outbox_row;

/// The settings the core reads, and the values they take.
pub(crate) mod setting {
    /// Per account: notifications off and out of the Dock badge.
    pub(crate) const MUTED: &str = "muted";
    /// Global: load remote images in messages.
    pub(crate) const REMOTE_IMAGES: &str = "remote_images";
    /// A setting turned on.
    pub(crate) const ON: &str = "on";
    /// A setting turned off.
    pub(crate) const OFF: &str = "off";
}

/// Whether the account's notifications are muted.
fn is_muted(reads: Reads<'_>, account: AccountId) -> Result<bool, StoreError> {
    Ok(reads.settings().get(account, setting::MUTED)?.as_deref() == Some(setting::ON))
}

/// An account as the rail shows it.
fn account_info(record: &AccountRecord, muted: bool) -> AccountInfo {
    AccountInfo {
        id: record.id,
        email: record.email.clone(),
        name: record.display_name.clone(),
        hue: record.hue().to_owned(),
        position: record.position,
        inbox_unread: record.inbox_unread,
        needs_signin: record.needs_signin,
        muted,
        sync_status: record.status(),
        send_as: record
            .send_as
            .iter()
            .map(|s| SendAs {
                email: s.email.clone(),
                name: s.name.clone(),
                is_default: s.is_default,
                is_primary: s.is_primary,
            })
            .collect(),
    }
}

/// The rail, the global settings, and whether Gmail is reachable.
pub(crate) fn app_state(reads: Reads<'_>, online: bool) -> Result<AppState, StoreError> {
    let accounts = reads
        .accounts()
        .all()?
        .iter()
        .map(|record| Ok(account_info(record, is_muted(reads, record.id)?)))
        .collect::<Result<_, StoreError>>()?;
    let remote_images = reads
        .settings()
        .get(AccountId::NONE, setting::REMOTE_IMAGES)?
        .as_deref()
        != Some(setting::OFF);
    Ok(AppState {
        accounts,
        settings: GlobalSettings { remote_images },
        online,
    })
}

/// An account's summary, or `None` when the account does not exist.
pub(crate) fn account_summary(
    reads: Reads<'_>,
    account: AccountId,
) -> Result<Option<AccountSummary>, StoreError> {
    let Some(record) = reads.accounts().get(account)? else {
        return Ok(None);
    };
    let labels = reads
        .labels()
        .all(account)?
        .into_iter()
        .map(|l| LabelInfo {
            id: l.id,
            name: l.name,
            system: l.system,
        })
        .collect();
    let splits = reads
        .splits()
        .all(account)?
        .into_iter()
        .map(|s| SplitInfo {
            id: s.id,
            name: s.name,
            kind: s.kind,
            query: s.query,
            enabled: s.enabled,
            hide_when_empty: s.hide_when_empty,
            position: s.position,
        })
        .collect();
    let pins = reads
        .splits()
        .pins(account)?
        .iter()
        .map(|p| PinInfo {
            kind: p.kind,
            value: p.value().to_owned(),
            split: p.split,
        })
        .collect();
    let undo = reads
        .undo()
        .entries(account)?
        .into_iter()
        .map(|e| UndoEntryInfo {
            id: e.id,
            kind: e.action.kind(),
            threads: e.action.threads(),
            outbox: e.action.outbox(),
            label: e.label,
        })
        .collect();
    let notices = reads
        .notices()
        .for_account(account)?
        .into_iter()
        .map(|n| Notice {
            id: n.id,
            kind: n.kind,
            message: n.message,
            draft: n.draft,
            thread: n.thread,
        })
        .collect();
    Ok(Some(AccountSummary {
        account: account_info(&record, is_muted(reads, account)?),
        labels,
        splits,
        pins,
        outbox: reads
            .outbox()
            .for_account(account)?
            .iter()
            .map(outbox_row)
            .collect(),
        undo,
        notices,
        queued_ops: reads.ops().count(account)?,
    }))
}

/// The first load of a summary for an account that does not exist (yet):
/// patches start once it does.
pub(crate) fn empty_summary(account: AccountId) -> AccountSummary {
    AccountSummary {
        account: AccountInfo {
            id: account,
            email: String::new(),
            name: String::new(),
            hue: AVATAR_HUES[0].to_owned(),
            position: 0,
            inbox_unread: 0,
            needs_signin: false,
            muted: false,
            sync_status: SyncStatus::Idle,
            send_as: Vec::new(),
        },
        labels: Vec::new(),
        splits: Vec::new(),
        pins: Vec::new(),
        outbox: Vec::new(),
        undo: Vec::new(),
        notices: Vec::new(),
        queued_ops: 0,
    }
}

/// The Dock badge (SPEC Notifications): unread Inbox threads across the
/// accounts that are not muted.
pub(crate) fn badge_count(reads: Reads<'_>) -> Result<u32, StoreError> {
    let mut count = 0u32;
    for record in reads.accounts().all()? {
        if !is_muted(reads, record.id)? {
            count = count.saturating_add(record.inbox_unread);
        }
    }
    Ok(count)
}
