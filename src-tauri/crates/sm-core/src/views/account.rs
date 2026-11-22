use sm_domain::{AVATAR_HUES, AccountId, SettingKey, SyncStatus};
use sm_ipc::{
    AccountInfo, AccountSummary, AppState, GlobalSettings, LabelInfo, Notice, SendAs, SplitInfo,
    UndoEntryInfo,
};
use sm_store::{AccountRecord, Reads, StoreError, Table};

use super::outbox_row;

pub(crate) const APP_READS: &[Table] = &[Table::Account, Table::Threads, Table::Settings];

pub(crate) const SUMMARY_READS: &[Table] = &[
    Table::Account,
    Table::Threads,
    Table::Settings,
    Table::Labels,
    Table::Splits,
    Table::Outbox,
    Table::Undo,
    Table::Notices,
    Table::Ops,
];

pub(crate) const BADGE_READS: &[Table] = &[Table::Account, Table::Threads, Table::Settings];

fn is_muted(reads: Reads<'_>, account: AccountId) -> Result<bool, StoreError> {
    reads.settings().is_on(account, SettingKey::Muted)
}

fn account_info(record: AccountRecord, muted: bool) -> AccountInfo {
    let hue = record.hue().to_owned();
    let sync_status = record.status();
    AccountInfo {
        id: record.id,
        email: record.email,
        name: record.display_name,
        hue,
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
            })
            .collect(),
    }
}

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

pub(crate) fn account_summary(
    reads: Reads<'_>,
    account: AccountId,
) -> Result<Option<AccountSummary>, StoreError> {
    let Some(record) = reads.accounts().get(account)? else {
        return Ok(None);
    };
    let labels = labels(reads, account)?;
    let splits = splits(reads, account)?;
    let undo = undo_entries(reads, account)?;
    let notices = notices(reads, account)?;
    Ok(Some(AccountSummary {
        account: account_info(record, is_muted(reads, account)?),
        labels,
        splits,
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

fn labels(reads: Reads<'_>, account: AccountId) -> Result<Vec<LabelInfo>, StoreError> {
    Ok(reads
        .labels()
        .all(account)?
        .into_iter()
        .map(|l| LabelInfo {
            id: l.id,
            name: l.name,
            system: l.system,
        })
        .collect())
}

fn splits(reads: Reads<'_>, account: AccountId) -> Result<Vec<SplitInfo>, StoreError> {
    Ok(reads
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
        })
        .collect())
}

fn undo_entries(reads: Reads<'_>, account: AccountId) -> Result<Vec<UndoEntryInfo>, StoreError> {
    let entries = reads.undo().entries(account)?;
    Ok(entries
        .into_iter()
        .map(|e| UndoEntryInfo {
            id: e.id,
            outbox: e.action.outbox(),
        })
        .collect())
}

fn notices(reads: Reads<'_>, account: AccountId) -> Result<Vec<Notice>, StoreError> {
    let notices = reads.notices().for_account(account)?;
    Ok(notices
        .into_iter()
        .map(|n| Notice {
            id: n.id,
            kind: n.kind,
            message: n.message,
            draft: n.draft,
            thread: n.thread,
        })
        .collect())
}

pub(crate) fn empty_summary(account: AccountId) -> AccountSummary {
    AccountSummary {
        account: AccountInfo {
            id: account,
            email: String::new(),
            name: String::new(),
            hue: AVATAR_HUES[0].to_owned(),
            inbox_unread: 0,
            needs_signin: false,
            muted: false,
            sync_status: SyncStatus::Idle,
            send_as: Vec::new(),
        },
        labels: Vec::new(),
        splits: Vec::new(),
        outbox: Vec::new(),
        undo: Vec::new(),
        notices: Vec::new(),
        queued_ops: 0,
    }
}

pub(crate) fn badge_count(reads: Reads<'_>) -> Result<u32, StoreError> {
    let mut count = 0u32;
    for record in reads.accounts().all()? {
        if !is_muted(reads, record.id)? {
            count = count.saturating_add(record.inbox_unread);
        }
    }
    Ok(count)
}
