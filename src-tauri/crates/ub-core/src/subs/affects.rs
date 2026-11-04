//! Which subscriptions a commit can change (ARCHITECTURE.md, section 5.2),
//! from the ids the store recorded as it wrote.

use ub_domain::View;
use ub_ipc::SubKey;
use ub_store::Changes;

/// Whether a commit with `changes` can change what `key` shows. A false
/// "yes" only costs a rebuild; a false "no" leaves the mirror stale.
pub(crate) fn affects(key: &SubKey, changes: &Changes) -> bool {
    match key {
        // A muted account is on the rail, and a mute is an account setting.
        SubKey::App => changes.app || !changes.settings.is_empty(),
        SubKey::Account { account } => changes.summaries.contains(account),
        SubKey::List {
            account,
            view: View::Drafts,
        } => changes.draft_lists.contains(account),
        SubKey::List { account, .. } | SubKey::Search { account, .. } => {
            changes.lists.contains(account)
        }
        SubKey::Conversation { thread, .. } => changes.threads.contains(thread),
        SubKey::Composer { draft } => changes.drafts.contains(draft),
    }
}

#[cfg(test)]
mod tests {
    use ub_domain::{AccountId, DraftId, ThreadId};

    use super::*;

    const A: AccountId = AccountId(1);
    const B: AccountId = AccountId(2);

    fn inbox(account: AccountId) -> SubKey {
        SubKey::List {
            account,
            view: View::Inbox,
        }
    }

    fn drafts(account: AccountId) -> SubKey {
        SubKey::List {
            account,
            view: View::Drafts,
        }
    }

    #[test]
    fn nothing_changed_affects_nothing() {
        let none = Changes::default();
        for key in [
            SubKey::App,
            SubKey::Account { account: A },
            inbox(A),
            drafts(A),
            SubKey::Conversation {
                thread: ThreadId(1),
                trash_mode: false,
                open: true,
            },
            SubKey::Composer {
                draft: DraftId::from("d"),
            },
        ] {
            assert!(!affects(&key, &none), "{key:?}");
        }
    }

    #[test]
    fn a_thread_list_change_reaches_that_accounts_lists_and_searches_only() {
        let mut changes = Changes::default();
        changes.lists.insert(A);
        assert!(affects(&inbox(A), &changes));
        assert!(affects(
            &SubKey::Search {
                account: A,
                query: "x".to_owned()
            },
            &changes
        ));
        assert!(!affects(&inbox(B), &changes));
        assert!(!affects(&drafts(A), &changes), "Drafts lists drafts");
    }

    #[test]
    fn a_draft_list_change_reaches_the_drafts_list_only() {
        let mut changes = Changes::default();
        changes.draft_lists.insert(A);
        assert!(affects(&drafts(A), &changes));
        assert!(!affects(&inbox(A), &changes));
    }

    #[test]
    fn an_account_setting_reaches_the_rail() {
        let mut changes = Changes::default();
        changes.settings.insert(A);
        assert!(affects(&SubKey::App, &changes), "muted shows on the rail");
    }

    #[test]
    fn rows_reach_their_conversation_and_composer() {
        let mut changes = Changes::default();
        changes.threads.insert(ThreadId(7));
        changes.drafts.insert(DraftId::from("d"));
        let conversation = |thread| SubKey::Conversation {
            thread,
            trash_mode: true,
            open: false,
        };
        assert!(affects(&conversation(ThreadId(7)), &changes));
        assert!(!affects(&conversation(ThreadId(8)), &changes));
        assert!(affects(
            &SubKey::Composer {
                draft: DraftId::from("d")
            },
            &changes
        ));
        assert!(!affects(
            &SubKey::Composer {
                draft: DraftId::from("e")
            },
            &changes
        ));
    }
}
