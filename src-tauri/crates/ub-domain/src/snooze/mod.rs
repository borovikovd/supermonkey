//! Snooze times (SPEC Snooze): the picker's fixed choices and its free-text
//! date-and-time field.
//!
//! Everything takes "now" as a [`chrono::DateTime`] in the user's time zone;
//! nothing here reads the clock. Wall-clock times in a DST gap resolve to the
//! first instant after the gap, and times that occur twice to the earlier one
//! (see [`resolve_local`]).

mod options;
mod text;

pub use options::{
    LATER_TODAY_HOUR, MORNING_HOUR, SnoozeKind, SnoozeOption, SnoozePick, resolve_local,
    snooze_options, snooze_picker,
};
pub use text::parse_snooze_text;
