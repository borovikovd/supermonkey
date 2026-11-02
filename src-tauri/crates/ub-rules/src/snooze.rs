//! Snooze times: the picker's fixed choices and its free-text date-and-time field.
//!
//! Everything is generic over [`chrono::TimeZone`]; the app passes
//! `chrono::Local::now()`, tests pin a named zone. Wall-clock times that fall
//! in a DST gap resolve to the first valid instant after the gap, and times
//! that occur twice resolve to the earlier one (see [`resolve_local`]).

use chrono::{
    DateTime, Datelike, Days, NaiveDate, NaiveDateTime, NaiveTime, TimeDelta, TimeZone, Timelike,
    Weekday,
};

/// "Later today" is this local hour, and is offered only before it.
pub const LATER_TODAY_HOUR: u32 = 18;
/// "Tomorrow morning", "next week", and a date typed without a time use this local hour.
pub const MORNING_HOUR: u32 = 8;

/// One of the picker's fixed choices.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnoozeKind {
    /// 18:00 today.
    LaterToday,
    /// 08:00 on the next calendar day.
    TomorrowMorning,
    /// 08:00 on the coming Monday, or the Monday after when today is Monday.
    NextWeek,
}

/// A fixed choice and the instant it snoozes until.
#[derive(Debug, Clone, PartialEq)]
pub struct SnoozeOption<Tz: TimeZone> {
    pub kind: SnoozeKind,
    pub at: DateTime<Tz>,
}

fn hour(h: u32) -> NaiveTime {
    NaiveTime::from_hms_opt(h, 0, 0).expect("valid hour")
}

/// The picker's fixed choices for `now`, in display order. Later today is
/// left out at or after 18:00 local.
pub fn snooze_options<Tz: TimeZone>(now: DateTime<Tz>) -> Vec<SnoozeOption<Tz>> {
    let tz = now.timezone();
    let local = now.naive_local();
    let today = local.date();
    let mut options = Vec::with_capacity(3);

    if local.time() < hour(LATER_TODAY_HOUR) {
        options.push(SnoozeOption {
            kind: SnoozeKind::LaterToday,
            at: resolve_local(&tz, today.and_time(hour(LATER_TODAY_HOUR))),
        });
    }
    if let Some(tomorrow) = today.succ_opt() {
        options.push(SnoozeOption {
            kind: SnoozeKind::TomorrowMorning,
            at: resolve_local(&tz, tomorrow.and_time(hour(MORNING_HOUR))),
        });
    }
    // Monday is 0 days from Monday, so it gets 7: the Monday after.
    let to_monday = 7 - u64::from(today.weekday().num_days_from_monday());
    if let Some(monday) = today.checked_add_days(Days::new(to_monday)) {
        options.push(SnoozeOption {
            kind: SnoozeKind::NextWeek,
            at: resolve_local(&tz, monday.and_time(hour(MORNING_HOUR))),
        });
    }
    options
}

/// The instant a local wall-clock time names in `tz`. When the time occurs
/// twice (clocks fall back) this is the earlier one. When it does not exist
/// (clocks spring forward) this is the first instant after the gap, so 02:30
/// on a 02:00→03:00 spring-forward day is 03:00.
pub fn resolve_local<Tz: TimeZone>(tz: &Tz, naive: NaiveDateTime) -> DateTime<Tz> {
    if let Some(at) = tz.from_local_datetime(&naive).earliest() {
        return at;
    }
    // Gaps are whole minutes and never longer than a day.
    let mut t = naive;
    for _ in 0..24 * 60 {
        t += TimeDelta::minutes(1);
        if let Some(at) = tz.from_local_datetime(&t).earliest() {
            return at;
        }
    }
    tz.from_utc_datetime(&naive)
}

/// What to do when the parsed time is not in the future.
#[derive(Clone, Copy, PartialEq)]
enum Roll {
    /// The text named an exact moment; a past one is invalid.
    Never,
    /// Only a time was typed: take it tomorrow.
    Day,
    /// A weekday was typed and it is today: take next week's.
    Week,
    /// A month and day without a year: take next year's.
    Year,
}

#[derive(Clone, Copy)]
enum Unit {
    Minutes,
    Hours,
    Days,
    Weeks,
}

/// Parses the picker's date-and-time text relative to `now`. Returns `None`
/// unless the text is understood completely and names a future instant.
///
/// Understood, case-insensitive, in any order, with `at`, `on`, `next`,
/// `this`, and commas ignored:
/// - durations: `in 2 hours`, `in 30m`, `in an hour`, `in 3 days`, `in 2 weeks`
///   (minutes and hours are exact; days and weeks keep the time of day)
/// - days: `today`, `tomorrow` (`tmrw`, `tmr`, `tom`), weekday names (`fri`,
///   `friday`), meaning the next such day, or today if that time is still ahead
/// - dates: `dec 24`, `24 dec`, `december 24th 2026`, `2026-12-01`; without a
///   year, this year's date, or next year's once it has passed
/// - times: `9am`, `9 am`, `9:30pm`, `14:00`, `9` (24-hour), `noon`,
///   `morning` (08:00), `afternoon` (13:00), `evening` (18:00)
///
/// A date without a time means 08:00. A time without a date means today, or
/// tomorrow once it has passed. Numeric dates like `12/24` are not accepted,
/// because day/month order is ambiguous.
pub fn parse_snooze_text<Tz: TimeZone>(text: &str, now: DateTime<Tz>) -> Option<DateTime<Tz>> {
    let lower = text.to_lowercase().replace(',', " ");
    let toks: Vec<&str> = lower.split_whitespace().collect();
    let local = now.naive_local();
    let today = local.date();

    let mut date: Option<NaiveDate> = None;
    let mut time: Option<NaiveTime> = None;
    let mut roll = Roll::Never;
    let mut default_time = hour(MORNING_HOUR);

    // Setting a date or a time twice is an error.
    #[allow(clippy::items_after_statements)] // Local helper, kept next to its only use.
    fn set<T>(slot: &mut Option<T>, value: T) -> Option<()> {
        if slot.is_some() {
            None
        } else {
            *slot = Some(value);
            Some(())
        }
    }

    let mut i = 0;
    while i < toks.len() {
        let t = toks[i];
        i += 1;
        if matches!(t, "at" | "on" | "next" | "this") {
            continue;
        }
        if t == "in" {
            let (amount, unit, used) = parse_amount(&toks[i..])?;
            i += used;
            match unit {
                Unit::Minutes | Unit::Hours => {
                    let minutes = match unit {
                        Unit::Hours => amount.checked_mul(60)?,
                        _ => amount,
                    };
                    let rest_is_filler = toks[i..].iter().all(|t| matches!(*t, "at" | "on"));
                    if date.is_some() || time.is_some() || !rest_is_filler {
                        return None;
                    }
                    return now.checked_add_signed(TimeDelta::try_minutes(minutes)?);
                }
                Unit::Days | Unit::Weeks => {
                    let days = match unit {
                        Unit::Weeks => amount.checked_mul(7)?,
                        _ => amount,
                    };
                    set(&mut date, today.checked_add_days(Days::new(days as u64))?)?;
                    default_time = NaiveTime::from_hms_opt(local.hour(), local.minute(), 0)?;
                }
            }
            continue;
        }
        let relative_day = match t {
            "today" => Some(0),
            "tomorrow" | "tmrw" | "tmr" | "tom" => Some(1),
            _ => None,
        };
        if let Some(days) = relative_day {
            set(&mut date, today.checked_add_days(Days::new(days))?)?;
            continue;
        }
        if let Some(weekday) = weekday(t) {
            let ahead =
                (7 + weekday.num_days_from_monday() - today.weekday().num_days_from_monday()) % 7;
            set(
                &mut date,
                today.checked_add_days(Days::new(u64::from(ahead)))?,
            )?;
            roll = Roll::Week;
            continue;
        }
        if let Some(d) = iso_date(t) {
            set(&mut date, d)?;
            continue;
        }
        if let Some(named) = named_time(t) {
            set(&mut time, named)?;
            continue;
        }
        let next = toks.get(i).copied();
        let next_is_meridiem = next.is_some_and(is_meridiem);
        // `dec 24 [2026]` or `24 dec [2026]`
        let month_day = match (month(t), day_number(t), next) {
            (Some(m), _, Some(n)) if !toks.get(i + 1).copied().is_some_and(is_meridiem) => {
                day_number(n).map(|d| (m, d))
            }
            (None, Some(d), Some(n)) => month(n).map(|m| (m, d)),
            _ => None,
        };
        if let Some((m, d)) = month_day {
            i += 1;
            let year = toks.get(i).and_then(|y| year_number(y));
            let y = if let Some(y) = year {
                i += 1;
                y
            } else {
                roll = Roll::Year;
                today.year()
            };
            set(&mut date, NaiveDate::from_ymd_opt(y, m, d)?)?;
            continue;
        }
        let meridiem = if next_is_meridiem { next } else { None };
        if let Some(parsed) = parse_time(t, meridiem) {
            if meridiem.is_some() {
                i += 1;
            }
            set(&mut time, parsed)?;
            continue;
        }
        return None;
    }

    if date.is_none() && time.is_none() {
        return None;
    }
    if date.is_none() {
        roll = Roll::Day;
    }
    let day = date.unwrap_or(today);
    let wall_time = time.unwrap_or(default_time);
    let tz = now.timezone();
    let at = resolve_local(&tz, day.and_time(wall_time));
    if at > now {
        return Some(at);
    }
    let later_day = match roll {
        Roll::Never => return None,
        Roll::Day => day.checked_add_days(Days::new(1))?,
        Roll::Week => day.checked_add_days(Days::new(7))?,
        Roll::Year => day.with_year(day.year() + 1)?,
    };
    let at = resolve_local(&tz, later_day.and_time(wall_time));
    (at > now).then_some(at)
}

/// `2 hours`, `2h`, `2hours`, `an hour`, `a day`: amount, unit, tokens used.
fn parse_amount(toks: &[&str]) -> Option<(i64, Unit, usize)> {
    let first = *toks.first()?;
    let (amount, unit_text, used) = if first == "a" || first == "an" {
        (1, *toks.get(1)?, 2)
    } else {
        let digits_end = first
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(first.len());
        let amount: i64 = first[..digits_end].parse().ok()?;
        match &first[digits_end..] {
            "" => (amount, *toks.get(1)?, 2),
            rest => (amount, rest, 1),
        }
    };
    let unit = match unit_text {
        "m" | "min" | "mins" | "minute" | "minutes" => Unit::Minutes,
        "h" | "hr" | "hrs" | "hour" | "hours" => Unit::Hours,
        "d" | "day" | "days" => Unit::Days,
        "w" | "wk" | "wks" | "week" | "weeks" => Unit::Weeks,
        _ => return None,
    };
    if amount <= 0 || amount > 100_000 {
        return None;
    }
    Some((amount, unit, used))
}

fn is_meridiem(t: &str) -> bool {
    matches!(t, "am" | "pm" | "a.m." | "p.m.")
}

/// `9`, `9am`, `9:30pm`, `14:00`, or `9` + a separate `am`/`pm` token.
fn parse_time(token: &str, meridiem: Option<&str>) -> Option<NaiveTime> {
    let (body, meridiem) = match meridiem {
        Some(m) => (token, Some(m.starts_with('p'))),
        None => {
            if let Some(b) = token.strip_suffix("am").or(token.strip_suffix("a.m.")) {
                (b, Some(false))
            } else if let Some(b) = token.strip_suffix("pm").or(token.strip_suffix("p.m.")) {
                (b, Some(true))
            } else {
                (token, None)
            }
        }
    };
    let (h, m) = match body.split_once(':') {
        Some((h, m)) if m.len() == 2 => (h, m),
        Some(_) => return None,
        None => (body, "0"),
    };
    if h.is_empty() || h.len() > 2 || !h.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    if !m.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let mut h: u32 = h.parse().ok()?;
    let m: u32 = m.parse().ok()?;
    if let Some(pm) = meridiem {
        if !(1..=12).contains(&h) {
            return None;
        }
        h %= 12;
        if pm {
            h += 12;
        }
    }
    NaiveTime::from_hms_opt(h, m, 0)
}

fn named_time(t: &str) -> Option<NaiveTime> {
    match t {
        "noon" | "midday" => Some(hour(12)),
        "morning" => Some(hour(MORNING_HOUR)),
        "afternoon" => Some(hour(13)),
        "evening" => Some(hour(LATER_TODAY_HOUR)),
        _ => None,
    }
}

fn weekday(t: &str) -> Option<Weekday> {
    Some(match t {
        "mon" | "monday" => Weekday::Mon,
        "tue" | "tues" | "tuesday" => Weekday::Tue,
        "wed" | "weds" | "wednesday" => Weekday::Wed,
        "thu" | "thur" | "thurs" | "thursday" => Weekday::Thu,
        "fri" | "friday" => Weekday::Fri,
        "sat" | "saturday" => Weekday::Sat,
        "sun" | "sunday" => Weekday::Sun,
        _ => return None,
    })
}

fn month(t: &str) -> Option<u32> {
    Some(match t {
        "jan" | "january" => 1,
        "feb" | "february" => 2,
        "mar" | "march" => 3,
        "apr" | "april" => 4,
        "may" => 5,
        "jun" | "june" => 6,
        "jul" | "july" => 7,
        "aug" | "august" => 8,
        "sep" | "sept" | "september" => 9,
        "oct" | "october" => 10,
        "nov" | "november" => 11,
        "dec" | "december" => 12,
        _ => return None,
    })
}

/// `24`, `24th`, `1st`, `2nd`, `3rd`.
fn day_number(t: &str) -> Option<u32> {
    let digits = ["st", "nd", "rd", "th"]
        .iter()
        .find_map(|s| t.strip_suffix(s))
        .unwrap_or(t);
    if digits.is_empty() || digits.len() > 2 || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let d: u32 = digits.parse().ok()?;
    (1..=31).contains(&d).then_some(d)
}

fn year_number(t: &str) -> Option<i32> {
    if t.len() == 4 && t.bytes().all(|b| b.is_ascii_digit()) {
        t.parse().ok()
    } else {
        None
    }
}

fn iso_date(t: &str) -> Option<NaiveDate> {
    let mut parts = t.split('-');
    let (y, m, d) = (parts.next()?, parts.next()?, parts.next()?);
    if parts.next().is_some() || y.len() != 4 || m.len() > 2 || d.len() > 2 {
        return None;
    }
    NaiveDate::from_ymd_opt(y.parse().ok()?, m.parse().ok()?, d.parse().ok()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono_tz::America::New_York;
    use chrono_tz::Tz;

    fn ny(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> DateTime<Tz> {
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
        // 2026-11-03 is a Tuesday.
        let options = snooze_options(ny(2026, 11, 3, 17, 59));
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
            let options = snooze_options(ny(2026, 11, 3, h, m));
            assert_eq!(
                kinds(&options),
                [SnoozeKind::TomorrowMorning, SnoozeKind::NextWeek]
            );
        }
        let late = snooze_options(ny(2026, 11, 3, 23, 59));
        assert_eq!(
            at(&late, SnoozeKind::TomorrowMorning),
            ny(2026, 11, 4, 8, 0)
        );
        // Just after midnight, tomorrow is the next calendar day, not today.
        let early = snooze_options(ny(2026, 11, 4, 0, 5));
        assert_eq!(
            at(&early, SnoozeKind::TomorrowMorning),
            ny(2026, 11, 5, 8, 0)
        );
        assert_eq!(at(&early, SnoozeKind::LaterToday), ny(2026, 11, 4, 18, 0));
    }

    #[test]
    fn next_week_on_each_weekday() {
        // 2026-11-01 is a Sunday.
        let cases = [
            (1, 2),  // Sunday -> tomorrow
            (2, 9),  // Monday -> the Monday after
            (3, 9),  // Tuesday
            (7, 9),  // Saturday
            (8, 9),  // Sunday
            (9, 16), // Monday
        ];
        for (day, monday) in cases {
            let options = snooze_options(ny(2026, 11, day, 10, 0));
            assert_eq!(
                at(&options, SnoozeKind::NextWeek),
                ny(2026, 11, monday, 8, 0),
                "from Nov {day}"
            );
        }
    }

    #[test]
    fn options_across_dst_changes() {
        // Spring forward on Sunday 2026-03-08: Saturday's "tomorrow" is EDT.
        let options = snooze_options(ny(2026, 3, 7, 10, 0));
        let tomorrow = at(&options, SnoozeKind::TomorrowMorning);
        assert_eq!(tomorrow.naive_local(), ny(2026, 3, 8, 8, 0).naive_local());
        assert_eq!(tomorrow.naive_utc().to_string(), "2026-03-08 12:00:00");
        // Later today on Saturday is still EST.
        let later = at(&options, SnoozeKind::LaterToday);
        assert_eq!(later.naive_utc().to_string(), "2026-03-07 23:00:00");

        // Fall back on Sunday 2026-11-01: next week's Monday 08:00 is EST.
        let options = snooze_options(ny(2026, 10, 31, 10, 0));
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
        assert!(snooze_options(chrono::Local::now()).len() >= 2);
        let utc = chrono::Utc.with_ymd_and_hms(2026, 11, 2, 9, 0, 0).unwrap();
        assert_eq!(snooze_options(utc).len(), 3);
    }

    // --- free text --------------------------------------------------------

    /// Monday 2026-11-02 10:00 New York.
    fn now() -> DateTime<Tz> {
        ny(2026, 11, 2, 10, 0)
    }

    fn parse_at(text: &str) -> Option<DateTime<Tz>> {
        parse_snooze_text(text, now())
    }

    #[test]
    fn text_examples_from_the_picker() {
        assert_eq!(parse_at("fri 9am"), Some(ny(2026, 11, 6, 9, 0)));
        assert_eq!(parse_at("tomorrow 3pm"), Some(ny(2026, 11, 3, 15, 0)));
        assert_eq!(parse_at("dec 24"), Some(ny(2026, 12, 24, 8, 0)));
        assert_eq!(parse_at("in 2 hours"), Some(ny(2026, 11, 2, 12, 0)));
        assert_eq!(parse_at("2026-12-01 14:00"), Some(ny(2026, 12, 1, 14, 0)));
    }

    #[test]
    fn text_times() {
        assert_eq!(parse_at("3pm"), Some(ny(2026, 11, 2, 15, 0)));
        assert_eq!(parse_at("3 PM"), Some(ny(2026, 11, 2, 15, 0)));
        assert_eq!(
            parse_at("9:30am"),
            Some(ny(2026, 11, 3, 9, 30)),
            "past today: tomorrow"
        );
        assert_eq!(
            parse_at("10:00"),
            Some(ny(2026, 11, 3, 10, 0)),
            "now is not the future"
        );
        assert_eq!(parse_at("10:01"), Some(ny(2026, 11, 2, 10, 1)));
        assert_eq!(parse_at("12pm"), Some(ny(2026, 11, 2, 12, 0)));
        assert_eq!(parse_at("12am"), Some(ny(2026, 11, 3, 0, 0)));
        assert_eq!(parse_at("noon"), Some(ny(2026, 11, 2, 12, 0)));
        assert_eq!(parse_at("evening"), Some(ny(2026, 11, 2, 18, 0)));
        assert_eq!(parse_at("at 17"), Some(ny(2026, 11, 2, 17, 0)));
    }

    #[test]
    fn text_days_and_weekdays() {
        assert_eq!(parse_at("today 5pm"), Some(ny(2026, 11, 2, 17, 0)));
        assert_eq!(parse_at("today 9am"), None, "explicit past time");
        assert_eq!(parse_at("Tomorrow"), Some(ny(2026, 11, 3, 8, 0)));
        assert_eq!(parse_at("tmrw at 7:15 pm"), Some(ny(2026, 11, 3, 19, 15)));
        assert_eq!(parse_at("friday"), Some(ny(2026, 11, 6, 8, 0)));
        assert_eq!(parse_at("next Fri, 9am"), Some(ny(2026, 11, 6, 9, 0)));
        assert_eq!(parse_at("sun"), Some(ny(2026, 11, 8, 8, 0)));
        // Today is Monday: later today counts, a past time rolls a week.
        assert_eq!(parse_at("mon 5pm"), Some(ny(2026, 11, 2, 17, 0)));
        assert_eq!(parse_at("mon"), Some(ny(2026, 11, 9, 8, 0)));
        assert_eq!(parse_at("9am fri"), Some(ny(2026, 11, 6, 9, 0)));
    }

    #[test]
    fn text_dates() {
        assert_eq!(parse_at("24 dec"), Some(ny(2026, 12, 24, 8, 0)));
        assert_eq!(parse_at("December 24th 6pm"), Some(ny(2026, 12, 24, 18, 0)));
        assert_eq!(parse_at("dec 24 2027"), Some(ny(2027, 12, 24, 8, 0)));
        assert_eq!(
            parse_at("jan 5"),
            Some(ny(2027, 1, 5, 8, 0)),
            "passed: next year"
        );
        assert_eq!(
            parse_at("nov 2"),
            Some(ny(2027, 11, 2, 8, 0)),
            "08:00 today passed"
        );
        assert_eq!(parse_at("nov 2 3pm"), Some(ny(2026, 11, 2, 15, 0)));
        assert_eq!(parse_at("dec 9 am"), None, "day 9 or 9 am is ambiguous");
        assert_eq!(parse_at("2026-12-01"), Some(ny(2026, 12, 1, 8, 0)));
        assert_eq!(parse_at("2026-01-01"), None, "explicit past date");
        assert_eq!(parse_at("2026-12-01 2pm"), Some(ny(2026, 12, 1, 14, 0)));
    }

    #[test]
    fn text_durations() {
        assert_eq!(parse_at("in 30 min"), Some(ny(2026, 11, 2, 10, 30)));
        assert_eq!(parse_at("in 2h"), Some(ny(2026, 11, 2, 12, 0)));
        assert_eq!(parse_at("in an hour"), Some(ny(2026, 11, 2, 11, 0)));
        assert_eq!(parse_at("in 3 days"), Some(ny(2026, 11, 5, 10, 0)));
        assert_eq!(parse_at("in 3 days 9am"), Some(ny(2026, 11, 5, 9, 0)));
        assert_eq!(parse_at("in a week"), Some(ny(2026, 11, 9, 10, 0)));
        assert_eq!(parse_at("in 2 weeks"), Some(ny(2026, 11, 16, 10, 0)));
        assert_eq!(parse_at("in 0 hours"), None);
        assert_eq!(parse_at("in 2 hours tomorrow"), None);
        assert_eq!(parse_at("in"), None);
        assert_eq!(parse_at("in 2 fortnights"), None);
    }

    #[test]
    fn text_rejects_garbage() {
        for text in [
            "",
            "   ",
            "soon",
            "fri fri",
            "fri tomorrow",
            "9am 10am",
            "25:00",
            "13pm",
            "0am",
            "9:5",
            "9:60",
            "feb 30",
            "dec",
            "dec 32",
            "2026-13-01",
            "12/24",
            "fri 9am blah",
            "in 99999999999999999999 hours",
        ] {
            assert_eq!(parse_at(text), None, "{text:?}");
        }
    }

    #[test]
    fn text_across_dst() {
        // Saturday before spring forward: "in 24 hours" is exact, so 11:00 EDT;
        // "in 1 day" keeps the wall time.
        let sat = ny(2026, 3, 7, 10, 0);
        let exact = parse_snooze_text("in 24 hours", sat).unwrap();
        assert_eq!(exact.naive_local().to_string(), "2026-03-08 11:00:00");
        let calendar = parse_snooze_text("in 1 day", sat).unwrap();
        assert_eq!(calendar.naive_local().to_string(), "2026-03-08 10:00:00");
        // 2:30am on the spring-forward day does not exist: first instant after the gap.
        let gap = parse_snooze_text("tomorrow 2:30am", sat).unwrap();
        assert_eq!(gap.naive_local().to_string(), "2026-03-08 03:00:00");
    }
}
