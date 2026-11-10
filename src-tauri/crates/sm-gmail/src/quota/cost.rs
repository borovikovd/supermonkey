//! Quota units per Gmail method, from Google's published table (SPEC Sync).

use super::Units;
use crate::wire::Format;

pub(crate) const GET_PROFILE: Units = Units(1);
pub(crate) const SEND_AS_LIST: Units = Units(1);
pub(crate) const LABELS_LIST: Units = Units(1);
pub(crate) const LABELS_CREATE: Units = Units(5);
pub(crate) const HISTORY_LIST: Units = Units(2);
pub(crate) const THREADS_LIST: Units = Units(10);
pub(crate) const MESSAGES_BATCH_MODIFY: Units = Units(50);
pub(crate) const MESSAGES_TRASH: Units = Units(20);
pub(crate) const MESSAGES_SEND: Units = Units(100);
pub(crate) const ATTACHMENTS_GET: Units = Units(20);
pub(crate) const DRAFTS_LIST: Units = Units(5);
pub(crate) const DRAFTS_CREATE: Units = Units(10);
pub(crate) const DRAFTS_UPDATE: Units = Units(15);
pub(crate) const DRAFTS_SEND: Units = Units(100);
pub(crate) const DRAFTS_DELETE: Units = Units(10);
pub(crate) const FILTERS_LIST: Units = Units(1);
pub(crate) const FILTERS_CREATE: Units = Units(5);
pub(crate) const FILTERS_DELETE: Units = Units(5);

/// `threads.get`: 40, or 120 with `format=full`.
pub(crate) const fn threads_get(format: Format) -> Units {
    if format.is_full() {
        Units(120)
    } else {
        Units(40)
    }
}

/// `messages.get` and `drafts.get`: 20, or 60 with `format=full` or `raw`.
pub(crate) const fn messages_get(format: Format) -> Units {
    if format.is_full() {
        Units(60)
    } else {
        Units(20)
    }
}
