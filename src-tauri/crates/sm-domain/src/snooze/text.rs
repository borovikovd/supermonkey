use chrono::{
    DateTime, Datelike, Days, NaiveDate, NaiveDateTime, NaiveTime, TimeDelta, TimeZone, Timelike,
    Weekday,
};

use super::options::{LATER_TODAY, MORNING, at_hour, resolve_local};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Roll {
    Never,
    Day,
    Week,
    Year,
}

#[derive(Debug, Clone, Copy)]
enum Span {
    Minutes(i64),
    Days(u64),
}

#[derive(Debug, Clone, Copy, Default)]
struct Parsed {
    date: Option<NaiveDate>,
    time: Option<NaiveTime>,
}

impl Parsed {
    fn set_date(&mut self, date: NaiveDate) -> Option<()> {
        self.date.is_none().then(|| self.date = Some(date))
    }

    fn set_time(&mut self, time: NaiveTime) -> Option<()> {
        self.time.is_none().then(|| self.time = Some(time))
    }
}

#[derive(Debug)]
struct Reading {
    parsed: Parsed,
    roll: Roll,
    default_time: NaiveTime,
}

impl Reading {
    fn take_date(&mut self, date: NaiveDate, roll: Option<Roll>) -> Option<()> {
        self.parsed.set_date(date)?;
        if let Some(roll) = roll {
            self.roll = roll;
        }
        Some(())
    }

    fn take_days(&mut self, days: u64, local: NaiveDateTime) -> Option<()> {
        self.parsed
            .set_date(local.date().checked_add_days(Days::new(days))?)?;
        self.default_time = NaiveTime::from_hms_opt(local.hour(), local.minute(), 0)?;
        Some(())
    }

    fn take_word(&mut self, toks: &[&str], today: NaiveDate) -> Option<usize> {
        let t = *toks.first()?;
        if let Some((date, roll)) = day_word(t, today) {
            self.take_date(date, roll)?;
            return Some(0);
        }
        if let Some(named) = named_time(t) {
            self.parsed.set_time(named)?;
            return Some(0);
        }
        if let Some((date, used, rolls)) = parse_month_day(toks, today) {
            self.take_date(date, rolls.then_some(Roll::Year))?;
            return Some(used - 1);
        }
        let meridiem = toks.get(1).copied().filter(|next| is_meridiem(next));
        self.parsed.set_time(parse_time(t, meridiem)?)?;
        Some(usize::from(meridiem.is_some()))
    }
}

pub fn parse_snooze_text<Tz: TimeZone>(text: &str, now: &DateTime<Tz>) -> Option<DateTime<Tz>> {
    let lower = text.to_lowercase().replace(',', " ");
    let toks: Vec<&str> = lower.split_whitespace().collect();
    let local = now.naive_local();
    let mut reading = Reading {
        parsed: Parsed::default(),
        roll: Roll::Never,
        default_time: MORNING,
    };

    let mut i = 0;
    while let Some(&t) = toks.get(i) {
        i += 1;
        if matches!(t, "at" | "on" | "next" | "this") {
            continue;
        }
        if t != "in" {
            i += reading.take_word(toks.get(i - 1..)?, local.date())?;
            continue;
        }
        let (span, used) = parse_span(toks.get(i..)?)?;
        i += used;
        match span {
            Span::Minutes(minutes) => {
                let rest = toks.get(i..).unwrap_or_default();
                return after_minutes(now, minutes, reading.parsed, rest);
            }
            Span::Days(days) => reading.take_days(days, local)?,
        }
    }
    resolve(now, reading.parsed, reading.default_time, reading.roll)
}

fn day_word(t: &str, today: NaiveDate) -> Option<(NaiveDate, Option<Roll>)> {
    if let Some(days) = relative_day(t) {
        return Some((today.checked_add_days(Days::new(days))?, None));
    }
    if let Some(weekday) = weekday(t) {
        let ahead =
            (7 + weekday.num_days_from_monday() - today.weekday().num_days_from_monday()) % 7;
        let date = today.checked_add_days(Days::new(u64::from(ahead)))?;
        return Some((date, Some(Roll::Week)));
    }
    iso_date(t).map(|d| (d, None))
}

fn after_minutes<Tz: TimeZone>(
    now: &DateTime<Tz>,
    minutes: i64,
    parsed: Parsed,
    rest: &[&str],
) -> Option<DateTime<Tz>> {
    let rest_is_filler = rest.iter().all(|t| matches!(*t, "at" | "on"));
    if parsed.date.is_some() || parsed.time.is_some() || !rest_is_filler {
        return None;
    }
    now.clone()
        .checked_add_signed(TimeDelta::try_minutes(minutes)?)
}

fn relative_day(t: &str) -> Option<u64> {
    match t {
        "today" => Some(0),
        "tomorrow" | "tmrw" | "tmr" | "tom" => Some(1),
        _ => None,
    }
}

fn resolve<Tz: TimeZone>(
    now: &DateTime<Tz>,
    parsed: Parsed,
    default_time: NaiveTime,
    roll: Roll,
) -> Option<DateTime<Tz>> {
    let (day, roll) = match (parsed.date, parsed.time) {
        (None, None) => return None,
        (None, Some(_)) => (now.naive_local().date(), Roll::Day),
        (Some(date), _) => (date, roll),
    };
    let wall_time = parsed.time.unwrap_or(default_time);
    let tz = now.timezone();
    let at = resolve_local(&tz, day.and_time(wall_time));
    if at > *now {
        return Some(at);
    }
    let later_day = match roll {
        Roll::Never => return None,
        Roll::Day => day.checked_add_days(Days::new(1))?,
        Roll::Week => day.checked_add_days(Days::new(7))?,
        Roll::Year => day.with_year(day.year().checked_add(1)?)?,
    };
    let at = resolve_local(&tz, later_day.and_time(wall_time));
    (at > *now).then_some(at)
}

fn parse_span(toks: &[&str]) -> Option<(Span, usize)> {
    let first = *toks.first()?;
    let (amount, unit, used) = if first == "a" || first == "an" {
        (1, *toks.get(1)?, 2)
    } else {
        let digits_end = first
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(first.len());
        let amount: u32 = first.get(..digits_end)?.parse().ok()?;
        match first.get(digits_end..)? {
            "" => (amount, *toks.get(1)?, 2),
            rest => (amount, rest, 1),
        }
    };
    if !(1..=100_000).contains(&amount) {
        return None;
    }
    let amount = u64::from(amount);
    let minutes = |per: u64| i64::try_from(amount * per).ok().map(Span::Minutes);
    let span = match unit {
        "m" | "min" | "mins" | "minute" | "minutes" => minutes(1)?,
        "h" | "hr" | "hrs" | "hour" | "hours" => minutes(60)?,
        "d" | "day" | "days" => Span::Days(amount),
        "w" | "wk" | "wks" | "week" | "weeks" => Span::Days(amount * 7),
        _ => return None,
    };
    Some((span, used))
}

fn parse_month_day(toks: &[&str], today: NaiveDate) -> Option<(NaiveDate, usize, bool)> {
    let first = *toks.first()?;
    let second = toks.get(1).copied();
    let (m, d) = match (month(first), day_number(first), second) {
        (Some(m), _, Some(n)) if !toks.get(2).copied().is_some_and(is_meridiem) => {
            (m, day_number(n)?)
        }
        (None, Some(d), Some(n)) => (month(n)?, d),
        _ => return None,
    };
    match toks.get(2).and_then(|y| year_number(y)) {
        Some(y) => Some((NaiveDate::from_ymd_opt(y, m, d)?, 3, false)),
        None => Some((NaiveDate::from_ymd_opt(today.year(), m, d)?, 2, true)),
    }
}

fn is_meridiem(t: &str) -> bool {
    matches!(t, "am" | "pm" | "a.m." | "p.m.")
}

fn parse_time(token: &str, meridiem: Option<&str>) -> Option<NaiveTime> {
    let (body, pm) = match meridiem {
        Some(m) => (token, Some(m.starts_with('p'))),
        None => {
            if let Some(b) = token
                .strip_suffix("am")
                .or_else(|| token.strip_suffix("a.m."))
            {
                (b, Some(false))
            } else if let Some(b) = token
                .strip_suffix("pm")
                .or_else(|| token.strip_suffix("p.m."))
            {
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
    if let Some(pm) = pm {
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
        "noon" | "midday" => Some(at_hour(12)),
        "morning" => Some(MORNING),
        "afternoon" => Some(at_hour(13)),
        "evening" => Some(LATER_TODAY),
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
    (t.len() == 4 && t.bytes().all(|b| b.is_ascii_digit()))
        .then(|| t.parse().ok())
        .flatten()
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
    use chrono_tz::Tz;

    use super::*;
    use crate::snooze::options::tests::ny;

    fn now() -> DateTime<Tz> {
        ny(2026, 11, 2, 10, 0)
    }

    fn parse_at(text: &str) -> Option<DateTime<Tz>> {
        parse_snooze_text(text, &now())
    }

    #[test]
    fn text_examples_from_the_picker() {
        assert_eq!(parse_at("fri 9am"), Some(ny(2026, 11, 6, 9, 0)));
        assert_eq!(parse_at("tomorrow 3pm"), Some(ny(2026, 11, 3, 15, 0)));
        assert_eq!(parse_at("dec 24"), Some(ny(2026, 12, 24, 8, 0)));
        assert_eq!(parse_at("in 2 hours"), Some(ny(2026, 11, 2, 12, 0)));
        assert_eq!(parse_at("2026-12-01 14:00"), Some(ny(2026, 12, 1, 14, 0)));
    }

    fn assert_cases(cases: &[(&str, Option<DateTime<Tz>>)]) {
        for (text, want) in cases {
            assert_eq!(&parse_at(text), want, "{text:?}");
        }
    }

    #[test]
    fn text_times() {
        assert_cases(&[
            ("3pm", Some(ny(2026, 11, 2, 15, 0))),
            ("3 PM", Some(ny(2026, 11, 2, 15, 0))),
            ("10:01", Some(ny(2026, 11, 2, 10, 1))),
            ("12pm", Some(ny(2026, 11, 2, 12, 0))),
            ("12am", Some(ny(2026, 11, 3, 0, 0))),
            ("noon", Some(ny(2026, 11, 2, 12, 0))),
            ("evening", Some(ny(2026, 11, 2, 18, 0))),
            ("at 17", Some(ny(2026, 11, 2, 17, 0))),
        ]);
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
    }

    #[test]
    fn text_days_and_weekdays() {
        assert_cases(&[
            ("today 5pm", Some(ny(2026, 11, 2, 17, 0))),
            ("Tomorrow", Some(ny(2026, 11, 3, 8, 0))),
            ("tmrw at 7:15 pm", Some(ny(2026, 11, 3, 19, 15))),
            ("friday", Some(ny(2026, 11, 6, 8, 0))),
            ("next Fri, 9am", Some(ny(2026, 11, 6, 9, 0))),
            ("sun", Some(ny(2026, 11, 8, 8, 0))),
            ("mon 5pm", Some(ny(2026, 11, 2, 17, 0))),
            ("mon", Some(ny(2026, 11, 9, 8, 0))),
            ("9am fri", Some(ny(2026, 11, 6, 9, 0))),
        ]);
        assert_eq!(parse_at("today 9am"), None, "explicit past time");
    }

    #[test]
    fn text_dates() {
        assert_cases(&[
            ("24 dec", Some(ny(2026, 12, 24, 8, 0))),
            ("December 24th 6pm", Some(ny(2026, 12, 24, 18, 0))),
            ("dec 24 2027", Some(ny(2027, 12, 24, 8, 0))),
            ("nov 2 3pm", Some(ny(2026, 11, 2, 15, 0))),
            ("2026-12-01", Some(ny(2026, 12, 1, 8, 0))),
            ("2026-12-01 2pm", Some(ny(2026, 12, 1, 14, 0))),
        ]);
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
        assert_eq!(parse_at("dec 9 am"), None, "day 9 or 9 am is ambiguous");
        assert_eq!(parse_at("2026-01-01"), None, "explicit past date");
    }

    #[test]
    fn text_durations() {
        assert_cases(&[
            ("in 30 min", Some(ny(2026, 11, 2, 10, 30))),
            ("in 2h", Some(ny(2026, 11, 2, 12, 0))),
            ("in an hour", Some(ny(2026, 11, 2, 11, 0))),
            ("in 3 days", Some(ny(2026, 11, 5, 10, 0))),
            ("in 3 days 9am", Some(ny(2026, 11, 5, 9, 0))),
            ("in a week", Some(ny(2026, 11, 9, 10, 0))),
            ("in 2 weeks", Some(ny(2026, 11, 16, 10, 0))),
            ("in 0 hours", None),
            ("in 2 hours tomorrow", None),
            ("tomorrow in 2 hours", None),
            ("in", None),
            ("in 2 fortnights", None),
        ]);
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
            "in 999999999999999999 hours",
        ] {
            assert_eq!(parse_at(text), None, "{text:?}");
        }
    }

    #[test]
    fn text_across_dst() {
        let sat = ny(2026, 3, 7, 10, 0);
        let exact = parse_snooze_text("in 24 hours", &sat).unwrap();
        assert_eq!(exact.naive_local().to_string(), "2026-03-08 11:00:00");
        let calendar = parse_snooze_text("in 1 day", &sat).unwrap();
        assert_eq!(calendar.naive_local().to_string(), "2026-03-08 10:00:00");
        let gap = parse_snooze_text("tomorrow 2:30am", &sat).unwrap();
        assert_eq!(gap.naive_local().to_string(), "2026-03-08 03:00:00");
    }
}
