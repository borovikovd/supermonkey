use sm_domain::{AccountId, View};
use sm_ipc::SubKey;
use sm_store::{Changes, Table};

use super::lists::{DRAFTS_READS, SEARCH_READS, VIEW_READS};
use crate::views::{APP_READS, BADGE_READS, SUMMARY_READS};

pub(crate) fn affects(key: &SubKey, changes: &Changes) -> bool {
    match key {
        SubKey::App => touched_any(changes, APP_READS),
        SubKey::Account { account } => touched(changes, *account, SUMMARY_READS),
        SubKey::List {
            account,
            view: View::Drafts,
        } => touched(changes, *account, DRAFTS_READS),
        SubKey::List { account, .. } => touched(changes, *account, VIEW_READS),
        SubKey::Search { account, .. } => touched(changes, *account, SEARCH_READS),
        SubKey::Conversation { thread, .. } => changes.threads.contains(thread),
        SubKey::Composer { draft } => changes.drafts.contains(draft),
    }
}

pub(crate) fn affects_badge(changes: &Changes) -> bool {
    touched_any(changes, BADGE_READS)
}

fn touched(changes: &Changes, account: AccountId, reads: &[Table]) -> bool {
    reads.iter().any(|&table| changes.touched(account, table))
}

fn touched_any(changes: &Changes, reads: &[Table]) -> bool {
    reads.iter().any(|&table| changes.touched_any(table))
}
