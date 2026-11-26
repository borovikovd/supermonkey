use chrono::{
    DateTime, Datelike as _, Days, Duration as ChronoDuration, LocalResult, Months, NaiveDate,
    NaiveDateTime, NaiveTime, TimeZone, Timelike as _, Weekday,
};

use super::Millis;

pub const LATER_TODAY_HOUR: u32 = 18;
pub const MORNING_HOUR: u32 = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnoozeKind {
    LaterToday,
    TomorrowMorning,
    NextWeek,
}

impl SnoozeKind {
    #[must_use]
    pub const fn title(self) -> &'static str {
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnoozePick {
    pub label: String,
    pub at: Millis,
}

#[must_use]
pub fn resolve_local<Tz: TimeZone>(tz: &Tz, naive: NaiveDateTime) -> Option<DateTime<Tz>> {
    match tz.from_local_datetime(&naive) {
        LocalResult::Single(dt) | LocalResult::Ambiguous(dt, _) => Some(dt),
        LocalResult::None => {
            for mins in [30_i64, 60, 90, 120] {
                let shifted = naive + ChronoDuration::minutes(mins);
                match tz.from_local_datetime(&shifted) {
                    LocalResult::Single(dt) | LocalResult::Ambiguous(dt, _) => return Some(dt),
                    LocalResult::None => {}
                }
            }
            for mins in 1_i64..=1440 {
                let shifted = naive + ChronoDuration::minutes(mins);
                match tz.from_local_datetime(&shifted) {
                    LocalResult::Single(dt) | LocalResult::Ambiguous(dt, _) => return Some(dt),
                    LocalResult::None => {}
                }
            }
            None
        }
    }
}

#[must_use]
pub fn snooze_options<Tz: TimeZone>(now: &DateTime<Tz>) -> Vec<SnoozeOption<Tz>> {
    let tz = now.timezone();
    let today = now.date_naive();
    let mut out = Vec::with_capacity(3);

    if now.hour() < LATER_TODAY_HOUR
        && let Some(time) = NaiveTime::from_hms_opt(LATER_TODAY_HOUR, 0, 0)
        && let Some(at) = resolve_local(&tz, today.and_time(time))
        && at > *now
    {
        out.push(SnoozeOption {
            kind: SnoozeKind::LaterToday,
            at,
        });
    }

    if let Some(tomorrow) = today.checked_add_days(Days::new(1))
        && let Some(time) = NaiveTime::from_hms_opt(MORNING_HOUR, 0, 0)
        && let Some(at) = resolve_local(&tz, tomorrow.and_time(time))
    {
        out.push(SnoozeOption {
            kind: SnoozeKind::TomorrowMorning,
            at,
        });
    }

    let days_to_mon = match today.weekday() {
        Weekday::Mon => 7,
        Weekday::Tue => 6,
        Weekday::Wed => 5,
        Weekday::Thu => 4,
        Weekday::Fri => 3,
        Weekday::Sat => 2,
        Weekday::Sun => 1,
    };
    if let Some(next_mon) = today.checked_add_days(Days::new(days_to_mon))
        && let Some(time) = NaiveTime::from_hms_opt(MORNING_HOUR, 0, 0)
        && let Some(at) = resolve_local(&tz, next_mon.and_time(time))
    {
        out.push(SnoozeOption {
            kind: SnoozeKind::NextWeek,
            at,
        });
    }

    out
}

#[must_use]
pub fn format_datetime<Tz: TimeZone>(dt: &DateTime<Tz>) -> String {
    let (pm, hour12) = dt.hour12();
    let ampm = if pm { "PM" } else { "AM" };
    format!(
        "{} {} {}, {}:{:02} {ampm}",
        weekday_short(dt.weekday()),
        month_short(dt.month()),
        dt.day(),
        hour12,
        dt.minute(),
    )
}

#[must_use]
pub fn snooze_picker<Tz: TimeZone>(text: &str, now: &DateTime<Tz>) -> Vec<SnoozePick> {
    let query = text.trim();
    let presets = snooze_options(now);
    if query.is_empty() {
        return presets
            .into_iter()
            .map(|opt| SnoozePick {
                label: format!("{} · {}", opt.kind.title(), format_datetime(&opt.at)),
                at: Millis(opt.at.timestamp_millis()),
            })
            .collect();
    }

    let mut out = Vec::new();
    if let Some(parsed) = parse_snooze_text(query, now) {
        out.push(SnoozePick {
            label: format_datetime(&parsed),
            at: Millis(parsed.timestamp_millis()),
        });
    }

    let q_lower = query.to_lowercase();
    for opt in presets {
        if opt.kind.title().to_lowercase().contains(&q_lower) {
            out.push(SnoozePick {
                label: format!("{} · {}", opt.kind.title(), format_datetime(&opt.at)),
                at: Millis(opt.at.timestamp_millis()),
            });
        }
    }
    out
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Roll {
    Never,
    Day,
    Week,
    Year,
}

#[derive(Debug, Default, Clone, Copy)]
struct Parsed {
    date: Option<NaiveDate>,
    time: Option<NaiveTime>,
    roll: Option<Roll>,
    relative: Option<ChronoDuration>,
    days: Option<u64>,
    months: Option<u32>,
}

#[must_use]
pub fn parse_snooze_text<Tz: TimeZone>(input: &str, now: &DateTime<Tz>) -> Option<DateTime<Tz>> {
    let tokens: Vec<String> = input
        .split(|c: char| c.is_whitespace() || c == ',')
        .filter(|s| !s.is_empty())
        .map(str::to_lowercase)
        .collect();
    if tokens.is_empty() {
        return None;
    }

    let today = now.date_naive();
    let mut parsed = Parsed::default();
    let mut i = 0;
    while i < tokens.len() {
        let consumed = consume_token(&tokens, i, today, &mut parsed)?;
        i += consumed;
    }

    apply_parsed(parsed, now)
}

fn consume_token(
    tokens: &[String],
    i: usize,
    today: NaiveDate,
    parsed: &mut Parsed,
) -> Option<usize> {
    let tok = tokens.get(i)?.as_str();
    if matches!(tok, "at" | "on" | "next") && i + 1 < tokens.len() {
        if tok == "next"
            && let Some(wd) = parse_weekday(&tokens[i + 1])
        {
            let date = next_weekday(today, wd, true)?;
            set_date(parsed, date, Roll::Never)?;
            return Some(2);
        }
        return Some(1);
    }
    if tok == "in" && i + 2 < tokens.len() {
        return parse_relative(tokens, i + 1, parsed).map(|n| n + 1);
    }
    if let Some(n) = parse_day_word(tok, today, parsed) {
        return Some(n);
    }
    if let Some(n) = parse_iso_date(tok, parsed) {
        return Some(n);
    }
    if let Some(n) = parse_month_day(tokens, i, today, parsed) {
        return Some(n);
    }
    if let Some(n) = parse_time_tokens(tokens, i, parsed) {
        return Some(n);
    }
    None
}

const fn set_date(parsed: &mut Parsed, date: NaiveDate, roll: Roll) -> Option<()> {
    if parsed.date.is_some() {
        return None;
    }
    parsed.date = Some(date);
    parsed.roll = Some(roll);
    Some(())
}

const fn set_time(parsed: &mut Parsed, time: NaiveTime) -> Option<()> {
    if parsed.time.is_some() {
        return None;
    }
    parsed.time = Some(time);
    Some(())
}

fn parse_relative(tokens: &[String], i: usize, parsed: &mut Parsed) -> Option<usize> {
    let count: u32 = tokens.get(i)?.parse().ok()?;
    if count == 0 {
        return None;
    }
    let unit = tokens.get(i + 1)?.as_str();
    match unit {
        "m" | "min" | "mins" | "minute" | "minutes" => {
            parsed.relative = Some(ChronoDuration::minutes(i64::from(count)));
        }
        "h" | "hr" | "hrs" | "hour" | "hours" => {
            parsed.relative = Some(ChronoDuration::hours(i64::from(count)));
        }
        "d" | "day" | "days" => {
            parsed.days = Some(u64::from(count));
        }
        "w" | "wk" | "wks" | "week" | "weeks" => {
            parsed.days = Some(u64::from(count).checked_mul(7)?);
        }
        "mo" | "month" | "months" => {
            parsed.months = Some(count);
        }
        _ => return None,
    }
    Some(2)
}

fn parse_day_word(tok: &str, today: NaiveDate, parsed: &mut Parsed) -> Option<usize> {
    match tok {
        "today" => set_date(parsed, today, Roll::Never)?,
        "tomorrow" | "tmrw" | "tmr" => {
            let date = today.checked_add_days(Days::new(1))?;
            set_date(parsed, date, Roll::Never)?;
        }
        "tonight" => {
            set_date(parsed, today, Roll::Never)?;
            set_time(parsed, NaiveTime::from_hms_opt(20, 0, 0)?)?;
        }
        _ => {
            let wd = parse_weekday(tok)?;
            let date = next_weekday(today, wd, false)?;
            set_date(parsed, date, Roll::Week)?;
        }
    }
    Some(1)
}

fn parse_iso_date(tok: &str, parsed: &mut Parsed) -> Option<usize> {
    let mut parts = tok.split(['-', '/']);
    let year: i32 = parts.next()?.parse().ok()?;
    let month: u32 = parts.next()?.parse().ok()?;
    let day: u32 = parts.next()?.parse().ok()?;
    if parts.next().is_some() || year < 1970 {
        return None;
    }
    let date = NaiveDate::from_ymd_opt(year, month, day)?;
    set_date(parsed, date, Roll::Never)?;
    Some(1)
}

fn parse_month_day(
    tokens: &[String],
    i: usize,
    today: NaiveDate,
    parsed: &mut Parsed,
) -> Option<usize> {
    let first = tokens.get(i)?.as_str();
    let second = tokens.get(i + 1)?.as_str();
    let (month, day, mut consumed) = if let Some(m) = parse_month(first) {
        (m, parse_ordinal_day(second)?, 2)
    } else {
        let d = parse_ordinal_day(first)?;
        (parse_month(second)?, d, 2)
    };
    let (year, roll) = match tokens.get(i + 2).and_then(|s| s.parse::<i32>().ok()) {
        Some(y) if (1970..=2100).contains(&y) => {
            consumed += 1;
            (y, Roll::Never)
        }
        _ => (today.year(), Roll::Year),
    };
    let date = NaiveDate::from_ymd_opt(year, month, day)?;
    set_date(parsed, date, roll)?;
    Some(consumed)
}

fn parse_time_tokens(tokens: &[String], i: usize, parsed: &mut Parsed) -> Option<usize> {
    let tok = tokens.get(i)?.as_str();
    match tok {
        "morning" => {
            set_time(parsed, NaiveTime::from_hms_opt(MORNING_HOUR, 0, 0)?)?;
            return Some(1);
        }
        "noon" => {
            set_time(parsed, NaiveTime::from_hms_opt(12, 0, 0)?)?;
            return Some(1);
        }
        "afternoon" => {
            set_time(parsed, NaiveTime::from_hms_opt(13, 0, 0)?)?;
            return Some(1);
        }
        "evening" => {
            set_time(parsed, NaiveTime::from_hms_opt(LATER_TODAY_HOUR, 0, 0)?)?;
            return Some(1);
        }
        "night" => {
            set_time(parsed, NaiveTime::from_hms_opt(20, 0, 0)?)?;
            return Some(1);
        }
        _ => {}
    }

    if let Some(time) = parse_clock(tok, None) {
        if let Some(ampm) = tokens.get(i + 1).map(String::as_str)
            && matches!(ampm, "am" | "pm")
            && !tok.ends_with("am")
            && !tok.ends_with("pm")
        {
            let adjusted = parse_clock(tok, Some(ampm))?;
            set_time(parsed, adjusted)?;
            return Some(2);
        }
        set_time(parsed, time)?;
        return Some(1);
    }
    None
}

fn parse_clock(tok: &str, suffix: Option<&str>) -> Option<NaiveTime> {
    let (num, ampm) = if let Some(rest) = tok.strip_suffix("am") {
        (rest, Some("am"))
    } else if let Some(rest) = tok.strip_suffix("pm") {
        (rest, Some("pm"))
    } else {
        (tok, suffix)
    };
    let (hour, min) = if let Some((h, m)) = num.split_once(':') {
        (h.parse::<u32>().ok()?, m.parse::<u32>().ok()?)
    } else {
        ampm?;
        (num.parse::<u32>().ok()?, 0)
    };
    if min >= 60 {
        return None;
    }
    let hour24 = match ampm {
        Some("am") => {
            if !(1..=12).contains(&hour) {
                return None;
            }
            if hour == 12 { 0 } else { hour }
        }
        Some("pm") => {
            if !(1..=12).contains(&hour) {
                return None;
            }
            if hour == 12 { 12 } else { hour + 12 }
        }
        None => {
            if hour >= 24 {
                return None;
            }
            hour
        }
        Some(_) => return None,
    };
    NaiveTime::from_hms_opt(hour24, min, 0)
}

fn apply_parsed<Tz: TimeZone>(parsed: Parsed, now: &DateTime<Tz>) -> Option<DateTime<Tz>> {
    let tz = now.timezone();
    if let Some(delta) = parsed.relative {
        let target = now.clone().checked_add_signed(delta)?;
        if let Some(time) = parsed.time {
            return resolve_local(&tz, target.date_naive().and_time(time));
        }
        return Some(target);
    }
    if let Some(days) = parsed.days {
        let date = now.date_naive().checked_add_days(Days::new(days))?;
        let time = parsed.time.unwrap_or_else(|| now.time());
        return resolve_local(&tz, date.and_time(time));
    }
    if let Some(months) = parsed.months {
        let date = now.date_naive().checked_add_months(Months::new(months))?;
        let time = parsed.time.unwrap_or_else(|| {
            NaiveTime::from_hms_opt(MORNING_HOUR, 0, 0).unwrap_or(NaiveTime::MIN)
        });
        return resolve_local(&tz, date.and_time(time));
    }

    let (mut date, roll) = if let Some(d) = parsed.date {
        (d, parsed.roll.unwrap_or(Roll::Never))
    } else {
        parsed.time?;
        (now.date_naive(), Roll::Day)
    };
    let time = parsed
        .time
        .unwrap_or_else(|| NaiveTime::from_hms_opt(MORNING_HOUR, 0, 0).unwrap_or(NaiveTime::MIN));

    let mut dt = resolve_local(&tz, date.and_time(time))?;
    if dt <= *now {
        date = match roll {
            Roll::Never => return None,
            Roll::Day => date.checked_add_days(Days::new(1))?,
            Roll::Week => date.checked_add_days(Days::new(7))?,
            Roll::Year => date.with_year(date.year() + 1)?,
        };
        dt = resolve_local(&tz, date.and_time(time))?;
    }
    (dt > *now).then_some(dt)
}

fn next_weekday(today: NaiveDate, target: Weekday, force_future: bool) -> Option<NaiveDate> {
    let cur = today.weekday().num_days_from_monday();
    let want = target.num_days_from_monday();
    let mut delta = (want + 7 - cur) % 7;
    if force_future && delta == 0 {
        delta = 7;
    }
    today.checked_add_days(Days::new(u64::from(delta)))
}

fn parse_weekday(tok: &str) -> Option<Weekday> {
    match tok {
        "mon" | "monday" => Some(Weekday::Mon),
        "tue" | "tues" | "tuesday" => Some(Weekday::Tue),
        "wed" | "wednesday" => Some(Weekday::Wed),
        "thu" | "thur" | "thurs" | "thursday" => Some(Weekday::Thu),
        "fri" | "friday" => Some(Weekday::Fri),
        "sat" | "saturday" => Some(Weekday::Sat),
        "sun" | "sunday" => Some(Weekday::Sun),
        _ => None,
    }
}

fn parse_month(tok: &str) -> Option<u32> {
    match tok {
        "jan" | "january" => Some(1),
        "feb" | "february" => Some(2),
        "mar" | "march" => Some(3),
        "apr" | "april" => Some(4),
        "may" => Some(5),
        "jun" | "june" => Some(6),
        "jul" | "july" => Some(7),
        "aug" | "august" => Some(8),
        "sep" | "sept" | "september" => Some(9),
        "oct" | "october" => Some(10),
        "nov" | "november" => Some(11),
        "dec" | "december" => Some(12),
        _ => None,
    }
}

fn parse_ordinal_day(tok: &str) -> Option<u32> {
    let digits = tok
        .strip_suffix("st")
        .or_else(|| tok.strip_suffix("nd"))
        .or_else(|| tok.strip_suffix("rd"))
        .or_else(|| tok.strip_suffix("th"))
        .unwrap_or(tok);
    let day: u32 = digits.parse().ok()?;
    (1..=31).contains(&day).then_some(day)
}

const fn weekday_short(w: Weekday) -> &'static str {
    match w {
        Weekday::Mon => "Mon",
        Weekday::Tue => "Tue",
        Weekday::Wed => "Wed",
        Weekday::Thu => "Thu",
        Weekday::Fri => "Fri",
        Weekday::Sat => "Sat",
        Weekday::Sun => "Sun",
    }
}

const fn month_short(m: u32) -> &'static str {
    match m {
        1 => "Jan",
        2 => "Feb",
        3 => "Mar",
        4 => "Apr",
        5 => "May",
        6 => "Jun",
        7 => "Jul",
        8 => "Aug",
        9 => "Sep",
        10 => "Oct",
        11 => "Nov",
        _ => "Dec",
    }
}

#[cfg(test)]
mod tests {
    use chrono_tz::America::New_York;

    use super::*;

    #[test]
    fn presets_before_and_after_six_pm() {
        let morning = New_York.with_ymd_and_hms(2026, 11, 2, 10, 0, 0).unwrap();
        let opts = snooze_options(&morning);
        assert_eq!(opts.len(), 3, "before 18:00 has 3 presets");
        assert_eq!(format_datetime(&opts[0].at), "Mon Nov 2, 6:00 PM");
        assert_eq!(format_datetime(&opts[1].at), "Tue Nov 3, 8:00 AM");
        assert_eq!(format_datetime(&opts[2].at), "Mon Nov 9, 8:00 AM");

        let evening = New_York.with_ymd_and_hms(2026, 11, 2, 18, 0, 0).unwrap();
        let opts = snooze_options(&evening);
        assert_eq!(opts.len(), 2, "at 18:00 drops Later today");
        assert_eq!(opts[0].kind, SnoozeKind::TomorrowMorning);
    }

    #[test]
    fn parses_free_text_and_handles_dst_gap() {
        let now = New_York.with_ymd_and_hms(2026, 11, 2, 10, 0, 0).unwrap();
        let fri = parse_snooze_text("fri 9am", &now).unwrap();
        assert_eq!(format_datetime(&fri), "Fri Nov 6, 9:00 AM");
        let hm = parse_snooze_text("10:05", &now).unwrap();
        assert_eq!(format_datetime(&hm), "Mon Nov 2, 10:05 AM");
        let rel = parse_snooze_text("in 2 hours", &now).unwrap();
        assert_eq!(format_datetime(&rel), "Mon Nov 2, 12:00 PM");

        let spring_gap = NaiveDate::from_ymd_opt(2026, 3, 8)
            .unwrap()
            .and_hms_opt(2, 30, 0)
            .unwrap();
        let resolved = resolve_local(&New_York, spring_gap).unwrap();
        assert_eq!(resolved.hour(), 3, "spring-forward gap moves into 3:xx");
    }

    #[test]
    fn relative_days_keep_the_time_on_the_clock_across_a_clock_change() {
        let before = New_York.with_ymd_and_hms(2027, 3, 13, 10, 0, 0).unwrap();
        let two_days = parse_snooze_text("in 2 days", &before).unwrap();
        assert_eq!(format_datetime(&two_days), "Mon Mar 15, 10:00 AM");
        let week = parse_snooze_text("in 1 week", &before).unwrap();
        assert_eq!(format_datetime(&week), "Sat Mar 20, 10:00 AM");
        let evening = parse_snooze_text("in 2 days 6pm", &before).unwrap();
        assert_eq!(format_datetime(&evening), "Mon Mar 15, 6:00 PM");
        let hours = parse_snooze_text("in 24 hours", &before).unwrap();
        assert_eq!(format_datetime(&hours), "Sun Mar 14, 11:00 AM");
    }
}
