use chrono::NaiveDate;

use crate::Millis;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Age {
    Days,
    Months,
    Years,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum DateArg {
    Day(NaiveDate),
    Epoch(Millis),
    Ago(u32, Age),
}

pub(super) fn day_or_epoch(value: &str) -> Option<DateArg> {
    if value.bytes().all(|b| b.is_ascii_digit()) {
        let seconds: i64 = value.parse().ok()?;
        return Some(DateArg::Epoch(Millis(seconds.checked_mul(1000)?)));
    }
    let sep = if value.contains('/') { '/' } else { '-' };
    let parts: Vec<&str> = value.split(sep).collect();
    let [a, b, c] = parts.as_slice() else {
        return None;
    };
    let number = |s: &str, digits: std::ops::RangeInclusive<usize>| -> Option<u32> {
        (digits.contains(&s.len()) && s.bytes().all(|b| b.is_ascii_digit()))
            .then(|| s.parse().ok())
            .flatten()
    };
    let (year, month, day) = if a.len() == 4 {
        (number(a, 4..=4)?, number(b, 1..=2)?, number(c, 1..=2)?)
    } else {
        (number(c, 4..=4)?, number(a, 1..=2)?, number(b, 1..=2)?)
    };
    NaiveDate::from_ymd_opt(i32::try_from(year).ok()?, month, day).map(DateArg::Day)
}

pub(super) fn age(value: &str) -> Option<DateArg> {
    let unit = match value.chars().last()?.to_ascii_lowercase() {
        'd' => Age::Days,
        'm' => Age::Months,
        'y' => Age::Years,
        _ => return None,
    };
    let n = &value[..value.len() - 1];
    if n.is_empty() || !n.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some(DateArg::Ago(n.parse().ok()?, unit))
}
