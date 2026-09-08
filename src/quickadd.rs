// SPDX-License-Identifier: GPL-3.0-only

//! Quick-add: "lunch with Maria Thu 13:00 at Kolonaki" → an event.
//!
//! A deterministic grammar, English and Greek, not a language model: the same
//! input always parses the same way, and the caller shows the parse *before*
//! anything is committed, so a wrong guess is visibly wrong rather than
//! silently filed.
//!
//! Recognised, in any order after the summary words:
//! - a weekday (`thu`, `thursday`, `πέμπτη`, `πεμ`), `today`/`σήμερα`,
//!   `tomorrow`/`αύριο`, or a numeric date (`4/9`, `4/9/2026`);
//! - a time (`13:00`, `9`, `2pm`), optionally a range (`13:00-14:30`);
//! - a location after `at` / `@` / `στο` / `στη` / `στον` / `σε` — unless
//!   what follows is a time, which is what "at 13:00" means.
//!
//! No time means an all-day event; no date means today. Everything not
//! recognised is the summary, in the order it was typed.

use crate::ui::editor::parse_time;
use chrono::{Datelike, Duration, NaiveDate, NaiveDateTime, NaiveTime, Weekday};

/// What the input parses to; the preview renders this, and Add commits it.
#[derive(Clone, Debug, PartialEq)]
pub struct Parsed {
    pub summary: String,
    pub date: NaiveDate,
    /// `None` means all-day.
    pub start: Option<NaiveTime>,
    pub end: Option<NaiveTime>,
    pub location: Option<String>,
}

/// The location prepositions. English `at` doubles as the time preposition,
/// which the parser resolves by looking at what follows.
const LOCATION_PREPOSITIONS: &[&str] = &["at", "@", "στο", "στη", "στον", "στην", "σε"];

#[must_use]
pub fn parse(input: &str, today: NaiveDate) -> Option<Parsed> {
    let tokens: Vec<&str> = input.split_whitespace().collect();
    if tokens.is_empty() {
        return None;
    }

    let mut summary: Vec<&str> = Vec::new();
    let mut date: Option<NaiveDate> = None;
    let mut start: Option<NaiveTime> = None;
    let mut end: Option<NaiveTime> = None;
    let mut location: Option<String> = None;

    let mut index = 0;
    while index < tokens.len() {
        let token = tokens[index];
        let lower = token.to_lowercase();

        // A location preposition claims the rest of the input — unless what
        // follows is a time, which is the other thing "at" means.
        if LOCATION_PREPOSITIONS.contains(&lower.as_str()) && index + 1 < tokens.len() {
            let rest = &tokens[index + 1..];
            if parse_time(rest[0]).is_none() || lower != "at" && lower != "@" {
                location = Some(rest.join(" "));
                break;
            }
        }

        if let Some((from, to)) = parse_time_range(&lower) {
            start = Some(from);
            end = to;
        } else if let Some(d) = parse_date_word(&lower, today) {
            date = Some(d);
        } else if !LOCATION_PREPOSITIONS.contains(&lower.as_str()) {
            summary.push(token);
        }
        index += 1;
    }

    let summary = summary.join(" ");
    if summary.trim().is_empty() {
        return None;
    }

    Some(Parsed {
        summary,
        date: date.unwrap_or(today),
        start,
        end,
        location,
    })
}

impl Parsed {
    /// Start and end as wall-clock datetimes; all-day maps to midnight and a
    /// one-day span, matching what the editor stores.
    #[must_use]
    pub fn span(&self) -> (NaiveDateTime, NaiveDateTime) {
        let Some(start) = self.start else {
            let begin = self.date.and_time(NaiveTime::MIN);
            return (begin, begin + Duration::days(1));
        };
        let begin = self.date.and_time(start);
        let end = match self.end {
            Some(end) if end > start => self.date.and_time(end),
            // A range wrapping midnight, or no range at all: an hour is the
            // editor's own default.
            _ => begin + Duration::hours(1),
        };
        (begin, end)
    }

    #[must_use]
    pub fn all_day(&self) -> bool {
        self.start.is_none()
    }
}

/// `13:00`, `2pm`, or a `13:00-14:30` range. A bare number is only accepted
/// as a time by [`parse_time`] — `9` is 09:00 — which is why dates must be
/// written with a slash.
fn parse_time_range(token: &str) -> Option<(NaiveTime, Option<NaiveTime>)> {
    if let Some((from, to)) = token.split_once(['-', '–']) {
        let from = parse_time(from)?;
        let to = parse_time(to)?;
        return Some((from, Some(to)));
    }
    // A bare integer is ambiguous ("lunch 9" is more likely a title than
    // 09:00), so only accept it with a colon or a meridiem.
    if !token.contains(':') && !token.ends_with("am") && !token.ends_with("pm") {
        return None;
    }
    parse_time(token).map(|t| (t, None))
}

fn parse_date_word(lower: &str, today: NaiveDate) -> Option<NaiveDate> {
    match lower {
        "today" | "σήμερα" | "σημερα" => return Some(today),
        "tomorrow" | "αύριο" | "αυριο" => return Some(today + Duration::days(1)),
        _ => {}
    }

    if let Some(weekday) = parse_weekday(lower) {
        let ahead = i64::from(
            (weekday.num_days_from_monday() + 7 - today.weekday().num_days_from_monday()) % 7,
        );
        return Some(today + Duration::days(ahead));
    }

    // 4/9 or 4/9/2026 — day first, the way the app's locales write dates.
    let mut parts = lower.split('/');
    let day: u32 = parts.next()?.parse().ok()?;
    let month: u32 = parts.next()?.parse().ok()?;
    let year: i32 = match parts.next() {
        Some(y) => y.parse().ok()?,
        None => today.year(),
    };
    if parts.next().is_some() {
        return None;
    }
    let date = NaiveDate::from_ymd_opt(year, month, day)?;
    // A bare day/month in the past means next year: "15/1" typed in September
    // is January's plans, not a retroactive entry.
    if date < today && lower.matches('/').count() == 1 {
        return NaiveDate::from_ymd_opt(year + 1, month, day);
    }
    Some(date)
}

fn parse_weekday(lower: &str) -> Option<Weekday> {
    Some(match lower {
        "monday" | "mon" | "δευτέρα" | "δευτερα" | "δευ" => Weekday::Mon,
        "tuesday" | "tue" | "τρίτη" | "τριτη" | "τρι" => Weekday::Tue,
        "wednesday" | "wed" | "τετάρτη" | "τεταρτη" | "τετ" => Weekday::Wed,
        "thursday" | "thu" | "πέμπτη" | "πεμπτη" | "πεμ" => Weekday::Thu,
        "friday" | "fri" | "παρασκευή" | "παρασκευη" | "παρ" => Weekday::Fri,
        "saturday" | "sat" | "σάββατο" | "σαββατο" | "σαβ" => Weekday::Sat,
        "sunday" | "sun" | "κυριακή" | "κυριακη" | "κυρ" => Weekday::Sun,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn today() -> NaiveDate {
        // A Tuesday.
        NaiveDate::from_ymd_opt(2026, 9, 1).unwrap()
    }

    fn t(h: u32, m: u32) -> NaiveTime {
        NaiveTime::from_hms_opt(h, m, 0).unwrap()
    }

    #[test]
    fn the_roadmap_example_parses() {
        let parsed = parse("lunch with Maria Thu 13:00 at Kolonaki", today()).unwrap();
        assert_eq!(parsed.summary, "lunch with Maria");
        assert_eq!(parsed.date, NaiveDate::from_ymd_opt(2026, 9, 3).unwrap());
        assert_eq!(parsed.start, Some(t(13, 0)));
        assert_eq!(parsed.location.as_deref(), Some("Kolonaki"));
    }

    #[test]
    fn greek_parses_the_same_way() {
        let parsed = parse("γεύμα με τη Μαρία πεμ 13:00 στο Κολωνάκι", today()).unwrap();
        assert_eq!(parsed.summary, "γεύμα με τη Μαρία");
        assert_eq!(parsed.date, NaiveDate::from_ymd_opt(2026, 9, 3).unwrap());
        assert_eq!(parsed.start, Some(t(13, 0)));
        assert_eq!(parsed.location.as_deref(), Some("Κολωνάκι"));
    }

    #[test]
    fn at_before_a_time_is_a_time_not_a_place() {
        let parsed = parse("standup tomorrow at 9:30", today()).unwrap();
        assert_eq!(parsed.summary, "standup");
        assert_eq!(parsed.date, NaiveDate::from_ymd_opt(2026, 9, 2).unwrap());
        assert_eq!(parsed.start, Some(t(9, 30)));
        assert_eq!(parsed.location, None);
    }

    #[test]
    fn a_range_sets_the_end() {
        let parsed = parse("workshop 13:00-14:30", today()).unwrap();
        assert_eq!(parsed.start, Some(t(13, 0)));
        assert_eq!(parsed.end, Some(t(14, 30)));
        let (from, to) = parsed.span();
        assert_eq!(to - from, Duration::minutes(90));
    }

    #[test]
    fn no_time_means_all_day_and_no_date_means_today() {
        let parsed = parse("conference tomorrow", today()).unwrap();
        assert!(parsed.all_day());

        let parsed = parse("write report", today()).unwrap();
        assert_eq!(parsed.date, today());
        assert!(parsed.all_day());
    }

    #[test]
    fn a_numeric_date_in_the_past_rolls_to_next_year() {
        let parsed = parse("ski trip 15/1", today()).unwrap();
        assert_eq!(parsed.date, NaiveDate::from_ymd_opt(2027, 1, 15).unwrap());
    }

    #[test]
    fn a_weekday_matching_today_is_today() {
        let parsed = parse("review tue 10:00", today()).unwrap();
        assert_eq!(parsed.date, today());
    }

    #[test]
    fn empty_or_summaryless_input_is_rejected() {
        assert!(parse("", today()).is_none());
        assert!(parse("13:00", today()).is_none());
    }

    #[test]
    fn a_bare_number_stays_in_the_summary() {
        let parsed = parse("route 66 documentary 20:00", today()).unwrap();
        assert_eq!(parsed.summary, "route 66 documentary");
        assert_eq!(parsed.start, Some(t(20, 0)));
    }
}
