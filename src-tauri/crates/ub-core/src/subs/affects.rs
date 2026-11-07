//! Which subscriptions a commit can change (ARCHITECTURE.md, section 5.2):
//! the one place that maps what the store wrote to the read models it makes
//! stale. Each read model lists the tables it reads next to the code that
//! builds it (`views`, `lists`); row-keyed ones go by the ids written.

use ub_domain::{AccountId, View};
use ub_ipc::SubKey;
use ub_store::{Changes, Table};

use super::lists::{DRAFTS_READS, SEARCH_READS, VIEW_READS};
use crate::views::{APP_READS, BADGE_READS, SUMMARY_READS};

/// Whether a commit with `changes` can change what `key` shows. A false
/// "yes" only costs a rebuild; a false "no" leaves the mirror stale.
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
        // A thread is stamped when its messages, drafts, or outbox rows
        // change, so its id is enough.
        SubKey::Conversation { thread, .. } => changes.threads.contains(thread),
        SubKey::Composer { draft } => changes.drafts.contains(draft),
    }
}

/// Whether a commit with `changes` can change the Dock badge's count.
pub(crate) fn affects_badge(changes: &Changes) -> bool {
    touched_any(changes, BADGE_READS)
}

fn touched(changes: &Changes, account: AccountId, reads: &[Table]) -> bool {
    reads.iter().any(|&table| changes.touched(account, table))
}

fn touched_any(changes: &Changes, reads: &[Table]) -> bool {
    reads.iter().any(|&table| changes.touched_any(table))
}
