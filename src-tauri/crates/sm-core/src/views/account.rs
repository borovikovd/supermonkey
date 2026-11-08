//! The app state and account summaries (SPEC UI state, Subscriptions).

use sm_domain::{AVATAR_HUES, AccountId, SettingKey, SyncStatus};
use sm_ipc::{
    AccountInfo, AccountSummary, AppState, GlobalSettings, LabelInfo, Notice, PinInfo, SendAs,
    SplitInfo, UndoEntryInfo,
};
use sm_store::{AccountRecord, Reads, StoreError, Table};

use super::outbox_row;

/// What [`app_state`] reads, of every account: the rail's account rows and
/// their unread Inbox counts (from the threads), each account's mute, and
/// the global settings ([`AccountId::NONE`]).
pub(crate) const APP_READS: &[Table] = &[Table::Account, Table::Threads, Table::Settings];

/// What [`account_summary`] reads, of its own account. Keep it in step with
/// the summary's fields: a table left out leaves the mirror stale.
pub(crate) const SUMMARY_READS: &[Table] = &[
    Table::Account,
    // The unread Inbox count.
    Table::Threads,
    // The mute.
    Table::Settings,
    Table::Labels,
    Table::Splits,
    Table::Pins,
    Table::Outbox,
    Table::Undo,
    Table::Notices,
    // The queued op count.
    Table::Ops,
];

/// What [`badge_count`] reads, of every account.
pub(crate) const BADGE_READS: &[Table] = &[Table::Account, Table::Threads, Table::Settings];

/// Whether the account's notifications are muted.
fn is_muted(reads: Reads<'_>, account: AccountId) -> Result<bool, StoreError> {
    reads.settings().is_on(account, SettingKey::Muted)
}

/// An account as the rail shows it.
fn account_info(record: AccountRecord, muted: bool) -> AccountInfo {
    let hue = record.hue().to_owned();
    let sync_status = record.status();
    AccountInfo {
        id: record.id,
        email: record.email,
        name: record.display_name,
        hue,
        position: record.position,
        inbox_unread: record.inbox_unread,
        needs_signin: record.needs_signin,
        muted,
        sync_status,
        send_as: record
            .send_as
            .into_iter()
            .map(|s| SendAs {
                email: s.email,
                name: s.name,
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
        .into_iter()
        .map(|record| {
            let muted = is_muted(reads, record.id)?;
            Ok(account_info(record, muted))
        })
        .collect::<Result<_, StoreError>>()?;
    let remote_images = reads
        .settings()
        .is_on(AccountId::NONE, SettingKey::RemoteImages)?;
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
        .into_iter()
        .map(|p| PinInfo {
            kind: p.kind,
            split: p.split,
            value: p.into_value(),
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
        account: account_info(record, is_muted(reads, account)?),
        labels,
        splits,
        pins,
        outbox: reads
            .outbox()
            .for_account(account)?
            .into_iter()
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
