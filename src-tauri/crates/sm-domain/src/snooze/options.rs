use std::fmt;

use chrono::{DateTime, Datelike, Days, NaiveDateTime, NaiveTime, TimeDelta, TimeZone};

use super::parse_snooze_text;
use crate::Millis;

pub const LATER_TODAY_HOUR: u32 = 18;
pub const MORNING_HOUR: u32 = 8;

pub(super) const fn at_hour(h: u32) -> NaiveTime {
    match NaiveTime::from_hms_opt(h, 0, 0) {
        Some(t) => t,
        None => NaiveTime::MIN,
    }
}

pub(super) const LATER_TODAY: NaiveTime = at_hour(LATER_TODAY_HOUR);
pub(super) const MORNING: NaiveTime = at_hour(MORNING_HOUR);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnoozeKind {
    LaterToday,
    TomorrowMorning,
    NextWeek,
}

impl SnoozeKind {
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::LaterToday => "Later today",
            Self::TomorrowMorning => "Tomorrow morning",
            Self::NextWeek => "Next week",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnoozeOption<Tz: TimeZone> {
    pub kind: SnoozeKind,
    pub at: DateTime<Tz>,
}

pub fn snooze_options<Tz: TimeZone>(now: &DateTime<Tz>) -> Vec<SnoozeOption<Tz>> {
    let tz = now.timezone();
    let local = now.naive_local();
    let today = local.date();
    let mut options = Vec::with_capacity(3);
    if local.time() < LATER_TODAY {
        options.push(SnoozeOption {
            kind: SnoozeKind::LaterToday,
            at: resolve_local(&tz, today.and_time(LATER_TODAY)),
        });
    }
    if let Some(tomorrow) = today.succ_opt() {
        options.push(SnoozeOption {
            kind: SnoozeKind::TomorrowMorning,
            at: resolve_local(&tz, tomorrow.and_time(MORNING)),
        });
    }
    let to_monday = 7 - u64::from(today.weekday().num_days_from_monday());
    if let Some(monday) = today.checked_add_days(Days::new(to_monday)) {
        options.push(SnoozeOption {
            kind: SnoozeKind::NextWeek,
            at: resolve_local(&tz, monday.and_time(MORNING)),
        });
    }
    options
}

pub fn resolve_local<Tz: TimeZone>(tz: &Tz, naive: NaiveDateTime) -> DateTime<Tz> {
    if let Some(at) = tz.from_local_datetime(&naive).earliest() {
        return at;
    }
    let mut t = naive;
    for _ in 0..24 * 60 {
        let Some(next) = t.checked_add_signed(TimeDelta::minutes(1)) else {
            break;
        };
        t = next;
        if let Some(at) = tz.from_local_datetime(&t).earliest() {
            return at;
        }
    }
    tz.from_utc_datetime(&naive)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnoozePick {
    pub label: String,
    pub at: Millis,
}

pub fn snooze_picker<Tz: TimeZone>(text: &str, now: &DateTime<Tz>) -> Vec<SnoozePick>
where
    Tz::Offset: fmt::Display,
{
    let format = |at: &DateTime<Tz>| at.format("%a %b %-d, %-I:%M %p").to_string();
    let text = text.trim();
    let mut picks = Vec::new();
    if !text.is_empty()
        && let Some(at) = parse_snooze_text(text, now)
    {
        picks.push(SnoozePick {
            label: format(&at),
            at: Millis(at.timestamp_millis()),
        });
    }
    let needle = text.to_lowercase();
    for option in snooze_options(now) {
        let name = option.kind.name();
        if name.to_lowercase().contains(&needle) {
            picks.push(SnoozePick {
                label: format!("{name} · {}", format(&option.at)),
                at: Millis(option.at.timestamp_millis()),
            });
        }
    }
    picks
}

#[cfg(test)]
pub(super) mod tests {
    use chrono::NaiveDate;
    use chrono_tz::America::New_York;
    use chrono_tz::Tz;

    use super::*;

    pub(crate) fn ny(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> DateTime<Tz> {
        New_York
            .with_ymd_and_hms(y, mo, d, h, mi, 0)
            .single()
            .unwrap()
    }

    fn kinds(options: &[SnoozeOption<Tz>]) -> Vec<SnoozeKind> {
        options.iter().map(|o| o.kind).collect()
    }

    fn at(options: &[SnoozeOption<Tz>], kind: SnoozeKind) -> DateTime<Tz> {
        options.iter().find(|o| o.kind == kind).unwrap().at
    }

    #[test]
    fn options_before_six() {
        let options = snooze_options(&ny(2026, 11, 3, 17, 59));
        assert_eq!(
            kinds(&options),
            [
                SnoozeKind::LaterToday,
                SnoozeKind::TomorrowMorning,
                SnoozeKind::NextWeek
            ]
        );
        assert_eq!(at(&options, SnoozeKind::LaterToday), ny(2026, 11, 3, 18, 0));
        assert_eq!(
            at(&options, SnoozeKind::TomorrowMorning),
            ny(2026, 11, 4, 8, 0)
        );
        assert_eq!(at(&options, SnoozeKind::NextWeek), ny(2026, 11, 9, 8, 0));
    }

    #[test]
    fn later_today_omitted_at_and_after_six() {
        for (h, m) in [(18, 0), (18, 1), (23, 59)] {
            let options = snooze_options(&ny(2026, 11, 3, h, m));
            assert_eq!(
                kinds(&options),
                [SnoozeKind::TomorrowMorning, SnoozeKind::NextWeek]
            );
        }
        let late = snooze_options(&ny(2026, 11, 3, 23, 59));
        assert_eq!(
            at(&late, SnoozeKind::TomorrowMorning),
            ny(2026, 11, 4, 8, 0)
        );
        let early = snooze_options(&ny(2026, 11, 4, 0, 5));
        assert_eq!(
            at(&early, SnoozeKind::TomorrowMorning),
            ny(2026, 11, 5, 8, 0)
        );
        assert_eq!(at(&early, SnoozeKind::LaterToday), ny(2026, 11, 4, 18, 0));
    }

    #[test]
    fn next_week_on_each_weekday() {
        let cases = [(1, 2), (2, 9), (3, 9), (7, 9), (8, 9), (9, 16)];
        for (day, monday) in cases {
            let options = snooze_options(&ny(2026, 11, day, 10, 0));
            assert_eq!(
                at(&options, SnoozeKind::NextWeek),
                ny(2026, 11, monday, 8, 0),
                "from Nov {day}"
            );
        }
    }

    #[test]
    fn options_across_dst_changes() {
        let options = snooze_options(&ny(2026, 3, 7, 10, 0));
        let tomorrow = at(&options, SnoozeKind::TomorrowMorning);
        assert_eq!(tomorrow.naive_local(), ny(2026, 3, 8, 8, 0).naive_local());
        assert_eq!(tomorrow.naive_utc().to_string(), "2026-03-08 12:00:00");
        let later = at(&options, SnoozeKind::LaterToday);
        assert_eq!(later.naive_utc().to_string(), "2026-03-07 23:00:00");

        let options = snooze_options(&ny(2026, 10, 31, 10, 0));
        let monday = at(&options, SnoozeKind::NextWeek);
        assert_eq!(monday.naive_utc().to_string(), "2026-11-02 13:00:00");
    }

    #[test]
    fn resolve_local_gap_and_fold() {
        let gap = NaiveDate::from_ymd_opt(2026, 3, 8)
            .unwrap()
            .and_hms_opt(2, 30, 0)
            .unwrap();
        let resolved = resolve_local(&New_York, gap);
        assert_eq!(resolved.naive_local().to_string(), "2026-03-08 03:00:00");
        assert_eq!(resolved.naive_utc().to_string(), "2026-03-08 07:00:00");

        let fold = NaiveDate::from_ymd_opt(2026, 11, 1)
            .unwrap()
            .and_hms_opt(1, 30, 0)
            .unwrap();
        let resolved = resolve_local(&New_York, fold);
        assert_eq!(resolved.naive_utc().to_string(), "2026-11-01 05:30:00");
    }

    #[test]
    fn works_with_local_and_utc() {
        let local = chrono::Local
            .with_ymd_and_hms(2026, 11, 2, 9, 0, 0)
            .earliest()
            .unwrap();
        let later = &snooze_options(&local)[0];
        assert_eq!(later.kind, SnoozeKind::LaterToday);
        assert_eq!(later.at.naive_local().to_string(), "2026-11-02 18:00:00");

        let utc = chrono::Utc.with_ymd_and_hms(2026, 11, 2, 9, 0, 0).unwrap();
        let times: Vec<String> = snooze_options(&utc)
            .iter()
            .map(|o| o.at.to_string())
            .collect();
        assert_eq!(
            times,
            [
                "2026-11-02 18:00:00 UTC",
                "2026-11-03 08:00:00 UTC",
                "2026-11-09 08:00:00 UTC"
            ]
        );
    }

    #[test]
    fn picker_lists_the_typed_time_then_matching_choices() {
        let now = ny(2026, 11, 2, 10, 0);
        let labels = |text: &str| -> Vec<String> {
            snooze_picker(text, &now)
                .into_iter()
                .map(|p| p.label)
                .collect()
        };
        assert_eq!(
            labels(""),
            [
                "Later today · Mon Nov 2, 6:00 PM",
                "Tomorrow morning · Tue Nov 3, 8:00 AM",
                "Next week · Mon Nov 9, 8:00 AM",
            ]
        );
        assert_eq!(
            labels("  TOMORROW "),
            [
                "Tue Nov 3, 8:00 AM",
                "Tomorrow morning · Tue Nov 3, 8:00 AM"
            ]
        );
        assert_eq!(labels("fri 9am"), ["Fri Nov 6, 9:00 AM"]);
        assert_eq!(labels("week"), ["Next week · Mon Nov 9, 8:00 AM"]);
        assert_eq!(labels("soon"), Vec::<String>::new());
        let picks = snooze_picker("in 2 hours", &now);
        assert_eq!(
            picks[0].at,
            Millis(ny(2026, 11, 2, 12, 0).timestamp_millis())
        );
    }
}
