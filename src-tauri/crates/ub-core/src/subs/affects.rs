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

#[cfg(test)]
mod tests {
    use ub_domain::{DraftId, ThreadId};

    use super::*;

    const A: AccountId = AccountId(1);
    const B: AccountId = AccountId(2);

    fn conversation(thread: ThreadId) -> SubKey {
        SubKey::Conversation {
            thread,
            trash_mode: false,
            open: true,
        }
    }

    fn composer(draft: &str) -> SubKey {
        SubKey::Composer {
            draft: DraftId::from(draft),
        }
    }

    fn list(view: View) -> SubKey {
        SubKey::List { account: A, view }
    }

    /// Every kind of subscription on account `A`, with a name to report.
    fn keys() -> Vec<(&'static str, SubKey)> {
        vec![
            ("app", SubKey::App),
            ("summary", SubKey::Account { account: A }),
            ("inbox", list(View::Inbox)),
            ("starred", list(View::Starred)),
            ("drafts", list(View::Drafts)),
            (
                "search",
                SubKey::Search {
                    account: A,
                    query: "x".to_owned(),
                },
            ),
            ("conversation", conversation(ThreadId(7))),
            ("composer", composer("d")),
        ]
    }

    fn touching(account: AccountId, table: Table) -> Changes {
        let mut changes = Changes::default();
        changes.tables.insert((account, table));
        changes
    }

    fn reached(changes: &Changes) -> Vec<&'static str> {
        keys()
            .into_iter()
            .filter(|(_, key)| affects(key, changes))
            .map(|(name, _)| name)
            .collect()
    }

    #[test]
    fn nothing_changed_affects_nothing() {
        assert_eq!(reached(&Changes::default()), Vec::<&str>::new());
        assert!(!affects_badge(&Changes::default()));
    }

    /// What each table's write reaches, worked out from what each read
    /// model shows (SPEC UI state, Subscriptions).
    #[test]
    fn each_table_reaches_the_read_models_that_show_it() {
        let expected: [(Table, &[&str]); 13] = [
            // The rail, the summary's account, and a search's `from:me`.
            (Table::Account, &["app", "summary", "search"]),
            // Nothing shows a history cursor.
            (Table::SyncCursor, &[]),
            // The summary's labels and a search's `label:`.
            (Table::Labels, &["summary", "search"]),
            // Lists and searches, and the unread count on the rail and in
            // the summary; rows reach their conversation by id.
            (
                Table::Threads,
                &["app", "summary", "inbox", "starred", "search"],
            ),
            (Table::Drafts, &["drafts"]),
            // The summary's sends; a draft with an outbox row leaves Drafts.
            (Table::Outbox, &["summary", "drafts"]),
            (Table::Ops, &["summary"]),
            (Table::Undo, &["summary"]),
            (Table::Notices, &["summary"]),
            // The summary's splits, and the Inbox sorts by split.
            (Table::Splits, &["summary", "inbox", "starred"]),
            (Table::Pins, &["summary"]),
            // The account's mute, on the rail and in its summary.
            (Table::Settings, &["app", "summary"]),
            // Autocomplete is a lookup, not a subscription.
            (Table::Contacts, &[]),
        ];
        assert_eq!(
            expected.map(|(table, _)| table),
            Table::ALL,
            "every table has its expectation"
        );
        for (table, reaches) in expected {
            assert_eq!(reached(&touching(A, table)), reaches, "{table:?}");
            assert_eq!(
                reached(&touching(B, table)),
                if reaches.contains(&"app") {
                    &["app"][..]
                } else {
                    &[]
                },
                "{table:?} of another account reaches only the rail"
            );
        }
    }

    #[test]
    fn the_badge_counts_unread_mail_of_accounts_not_muted() {
        for table in Table::ALL {
            let expected = matches!(table, Table::Account | Table::Threads | Table::Settings);
            assert_eq!(affects_badge(&touching(B, table)), expected, "{table:?}");
        }
    }

    #[test]
    fn a_global_setting_reaches_the_rail_only() {
        assert_eq!(
            reached(&touching(AccountId::NONE, Table::Settings)),
            ["app"]
        );
    }

    #[test]
    fn rows_reach_their_conversation_and_composer() {
        let mut changes = Changes::default();
        changes.threads.insert(ThreadId(7));
        changes.drafts.insert(DraftId::from("d"));
        assert_eq!(reached(&changes), ["conversation", "composer"]);
        assert!(!affects(&conversation(ThreadId(8)), &changes));
        assert!(!affects(&composer("e"), &changes));
    }
}
