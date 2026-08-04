// SPDX-License-Identifier: GPL-3.0-only

//! The vdir layout: one directory per calendar, one `.ics` file per event.
//!
//! This is the layout `vdirsyncer` and `khal` use, which is the whole point —
//! events written here can be synced to a CalDAV server by pointing vdirsyncer
//! at the same directory, with no export step.
//!
//! ```text
//! ~/.local/share/calendars/
//! ├── personal/
//! │   ├── displayname        "Personal"
//! │   ├── color              "#2d7dd2"
//! │   ├── 9f3c…-a1.ics
//! │   └── b722…-04.ics
//! └── work/
//!     └── …
//! ```

use super::StoreError;
use crate::model::{CalendarMeta, Event, EventTime, Rgb};
use chrono::{DateTime, NaiveDate, NaiveDateTime, TimeZone, Utc};
use chrono_tz::Tz;
use icalendar::{
    Calendar, CalendarComponent, CalendarDateTime, Component, DatePerhapsTime, EventLike,
};
use std::path::{Path, PathBuf};

/// Where calendars live by default: `$XDG_DATA_HOME/calendars`.
///
/// `COSMIC_CALENDAR_DIR` overrides it, which is how the tests point at a tempdir.
#[must_use]
pub fn default_root() -> PathBuf {
    if let Some(dir) = std::env::var_os("COSMIC_CALENDAR_DIR") {
        return PathBuf::from(dir);
    }
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("calendars")
}

/// Lists every collection under `root`, sorted by display name.
///
/// Unreadable directories are skipped with a warning rather than failing the
/// whole load — one bad collection should not empty the app.
#[must_use]
pub fn collections(root: &Path) -> Vec<CalendarMeta> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };

    let mut out: Vec<CalendarMeta> = entries
        .filter_map(Result::ok)
        .filter(|e| e.path().is_dir())
        .filter(|e| !e.file_name().to_string_lossy().starts_with('.'))
        .filter_map(|e| CalendarMeta::load(&e.path()))
        .collect();

    out.sort_by_key(|c| c.name.to_lowercase());
    out
}

/// Creates a new collection directory with its metadata files.
pub fn create_collection(root: &Path, name: &str, color: Rgb) -> Result<CalendarMeta, StoreError> {
    std::fs::create_dir_all(root)?;

    let base = slugify(name);
    let mut dir = root.join(&base);
    let mut n = 2;
    while dir.exists() {
        dir = root.join(format!("{base}-{n}"));
        n += 1;
    }
    std::fs::create_dir(&dir)?;

    let meta = CalendarMeta {
        id: dir
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or(&base)
            .to_owned(),
        name: name.trim().to_owned(),
        color,
        path: dir,
        read_only: false,
    };
    meta.save_meta()?;
    Ok(meta)
}

/// Turns a display name into a safe directory name.
fn slugify(name: &str) -> String {
    let s: String = name
        .trim()
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();

    let s = s
        .split('-')
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join("-");

    if s.is_empty() {
        "calendar".to_owned()
    } else {
        s.chars().take(64).collect()
    }
}

/// Reads every event in a collection.
///
/// Malformed files are logged and skipped; a single corrupt `.ics` should not
/// hide the rest of the calendar.
#[must_use]
pub fn read_collection(meta: &CalendarMeta) -> Vec<Event> {
    let Ok(entries) = std::fs::read_dir(&meta.path) else {
        tracing::warn!(path = %meta.path.display(), "cannot read collection");
        return Vec::new();
    };

    let mut out = Vec::new();
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("ics") {
            continue;
        }
        let Some(file_name) = path.file_name().and_then(|s| s.to_str()) else {
            continue;
        };
        match std::fs::read_to_string(&path) {
            Ok(text) => out.extend(parse_ics(&text, &meta.id, file_name)),
            Err(why) => tracing::warn!(path = %path.display(), %why, "cannot read event file"),
        }
    }
    out
}

/// Parses the `VEVENT`s out of one `.ics` file.
#[must_use]
pub fn parse_ics(text: &str, calendar_id: &str, file_name: &str) -> Vec<Event> {
    let parsed: Calendar = match text.parse() {
        Ok(c) => c,
        Err(why) => {
            tracing::warn!(file_name, %why, "unparseable iCalendar file");
            return Vec::new();
        }
    };

    parsed
        .components
        .iter()
        .filter_map(|component| match component {
            CalendarComponent::Event(ev) => convert_event(ev, calendar_id, file_name),
            _ => None,
        })
        .collect()
}

fn convert_event(ev: &icalendar::Event, calendar_id: &str, file_name: &str) -> Option<Event> {
    // An event with no DTSTART cannot be placed on a grid, so it is not
    // something this app can show.
    let start = to_event_time(&ev.get_start()?);

    let end = ev
        .get_end()
        .map(|e| to_event_time(&e))
        .unwrap_or_else(|| default_end(start));

    let uid = ev
        .get_uid()
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| format!("{}@cosmic-calendar", uuid::Uuid::new_v4()));

    Some(Event {
        uid,
        calendar_id: calendar_id.to_owned(),
        summary: ev.get_summary().unwrap_or_default().to_owned(),
        description: ev
            .get_description()
            .map(ToOwned::to_owned)
            .filter(|s| !s.is_empty()),
        location: ev
            .property_value("LOCATION")
            .map(ToOwned::to_owned)
            .filter(|s| !s.is_empty()),
        start,
        end,
        rrule: ev.property_value("RRULE").map(ToOwned::to_owned),
        exdates: parse_exdates(ev),
        sequence: ev.get_sequence().unwrap_or(0) as i32,
        created: ev.get_created(),
        last_modified: ev.get_last_modified(),
        file_name: file_name.to_owned(),
    })
}

/// RFC 5545: a `DATE`-valued event with no `DTEND` lasts one day; a `DATE-TIME`
/// one is zero-length. We give timed events an hour so they stay clickable.
fn default_end(start: EventTime) -> EventTime {
    match start {
        EventTime::Date(d) => EventTime::Date(d + chrono::Duration::days(1)),
        EventTime::Floating(dt) => EventTime::Floating(dt + chrono::Duration::hours(1)),
        EventTime::Zoned(dt, tz) => EventTime::Zoned(dt + chrono::Duration::hours(1), tz),
    }
}

fn parse_exdates(ev: &icalendar::Event) -> Vec<NaiveDateTime> {
    let mut out = Vec::new();

    // EXDATE may appear several times, and each line may hold a comma-separated list.
    let lines = ev
        .multi_properties()
        .get("EXDATE")
        .map(|props| {
            props
                .iter()
                .map(icalendar::Property::value)
                .collect::<Vec<_>>()
        })
        .unwrap_or_else(|| ev.property_value("EXDATE").into_iter().collect());

    for line in lines {
        for value in line.split(',') {
            if let Some(dt) = parse_ical_datetime(value.trim()) {
                out.push(dt);
            }
        }
    }
    out
}

/// Parses the raw `YYYYMMDD[THHMMSS[Z]]` forms used by EXDATE.
fn parse_ical_datetime(value: &str) -> Option<NaiveDateTime> {
    let v = value.trim_end_matches('Z');
    NaiveDateTime::parse_from_str(v, "%Y%m%dT%H%M%S")
        .ok()
        .or_else(|| {
            NaiveDate::parse_from_str(v, "%Y%m%d")
                .ok()
                .map(|d| d.and_time(chrono::NaiveTime::MIN))
        })
}

fn to_event_time(dpt: &DatePerhapsTime) -> EventTime {
    match dpt {
        DatePerhapsTime::Date(d) => EventTime::Date(*d),
        DatePerhapsTime::DateTime(cdt) => match cdt {
            CalendarDateTime::Floating(dt) => EventTime::Floating(*dt),
            CalendarDateTime::Utc(dt) => EventTime::Zoned(dt.naive_utc(), chrono_tz::UTC),
            CalendarDateTime::WithTimezone { date_time, tzid } => tzid
                .parse::<Tz>()
                .map_or(EventTime::Floating(*date_time), |tz| {
                    EventTime::Zoned(*date_time, tz)
                }),
        },
    }
}

fn from_event_time(t: EventTime) -> DatePerhapsTime {
    match t {
        EventTime::Date(d) => DatePerhapsTime::Date(d),
        EventTime::Floating(dt) => DatePerhapsTime::DateTime(CalendarDateTime::Floating(dt)),
        EventTime::Zoned(dt, tz) if tz == chrono_tz::UTC => {
            DatePerhapsTime::DateTime(CalendarDateTime::Utc(Utc.from_utc_datetime(&dt)))
        }
        EventTime::Zoned(dt, tz) => DatePerhapsTime::DateTime(CalendarDateTime::WithTimezone {
            date_time: dt,
            tzid: tz.name().to_owned(),
        }),
    }
}

/// Serialises an event to iCalendar text.
#[must_use]
pub fn to_ics(event: &Event) -> String {
    let mut ical = icalendar::Event::new();
    ical.uid(&event.uid)
        .summary(&event.summary)
        .starts(from_event_time(event.start))
        .ends(from_event_time(event.end))
        .sequence(event.sequence.max(0) as u32)
        .timestamp(Utc::now());

    if let Some(description) = &event.description {
        ical.description(description);
    }
    if let Some(location) = &event.location {
        ical.add_property("LOCATION", location.as_str());
    }
    if let Some(rrule) = &event.rrule {
        ical.add_property("RRULE", rrule.as_str());
    }
    if let Some(created) = event.created {
        ical.created(created);
    }
    ical.last_modified(event.last_modified.unwrap_or_else(Utc::now));

    for exdate in &event.exdates {
        ical.add_multi_property("EXDATE", &format_ical_datetime(*exdate, event.start));
    }

    let mut calendar = Calendar::new();
    calendar.push(ical.done());
    calendar.to_string()
}

fn format_ical_datetime(dt: NaiveDateTime, like: EventTime) -> String {
    if like.is_all_day() {
        dt.format("%Y%m%d").to_string()
    } else {
        dt.format("%Y%m%dT%H%M%S").to_string()
    }
}

/// Writes an event to its collection, replacing any existing file.
///
/// The write goes to a temporary file and is then renamed into place, so a sync
/// tool watching the directory never observes a half-written `.ics`.
pub fn write_event(meta: &CalendarMeta, event: &Event) -> Result<(), StoreError> {
    if meta.read_only {
        return Err(StoreError::ReadOnly(meta.name.clone()));
    }

    let target = meta.path.join(&event.file_name);
    let temp = meta.path.join(format!(".{}.tmp", event.file_name));

    std::fs::write(&temp, to_ics(event))?;
    std::fs::rename(&temp, &target)?;
    Ok(())
}

/// Deletes an event's file. A file that is already gone is not an error.
pub fn delete_event(meta: &CalendarMeta, file_name: &str) -> Result<(), StoreError> {
    if meta.read_only {
        return Err(StoreError::ReadOnly(meta.name.clone()));
    }
    match std::fs::remove_file(meta.path.join(file_name)) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}

/// Last-modified time of a collection's newest file, used to spot external edits.
#[must_use]
pub fn collection_mtime(meta: &CalendarMeta) -> Option<DateTime<Utc>> {
    let entries = std::fs::read_dir(&meta.path).ok()?;
    entries
        .filter_map(Result::ok)
        .filter_map(|e| e.metadata().ok()?.modified().ok())
        .max()
        .map(DateTime::<Utc>::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Rgb;

    fn temp_root() -> tempfile::TempDir {
        tempfile::tempdir().expect("tempdir")
    }

    #[test]
    fn slugify_makes_safe_directory_names() {
        assert_eq!(slugify("Personal"), "personal");
        assert_eq!(slugify("Work / Projects"), "work-projects");
        assert_eq!(slugify("  ../../etc/passwd  "), "etc-passwd");
        assert_eq!(slugify("🎉"), "calendar");
        assert_eq!(slugify(""), "calendar");
    }

    #[test]
    fn create_and_list_collections() {
        let root = temp_root();
        let a = create_collection(root.path(), "Personal", Rgb(0x2d, 0x7d, 0xd2)).unwrap();
        let b = create_collection(root.path(), "Work", Rgb(0x24, 0x9b, 0x74)).unwrap();

        let found = collections(root.path());
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].name, "Personal");
        assert_eq!(found[1].name, "Work");
        assert_eq!(found[0].color, a.color);
        assert_eq!(found[1].id, b.id);
    }

    #[test]
    fn duplicate_names_get_distinct_directories() {
        let root = temp_root();
        let a = create_collection(root.path(), "Personal", Rgb(1, 2, 3)).unwrap();
        let b = create_collection(root.path(), "Personal", Rgb(1, 2, 3)).unwrap();
        assert_ne!(a.id, b.id);
        assert_eq!(collections(root.path()).len(), 2);
    }

    #[test]
    fn event_roundtrips_through_disk() {
        let root = temp_root();
        let cal = create_collection(root.path(), "Personal", Rgb(1, 2, 3)).unwrap();

        let mut event = Event::draft(
            &cal.id,
            NaiveDate::from_ymd_opt(2026, 8, 4)
                .unwrap()
                .and_hms_opt(9, 30, 0)
                .unwrap(),
            chrono_tz::Europe::Athens,
        );
        event.summary = "Standup".into();
        event.location = Some("Room 3".into());
        event.description = Some("Daily sync".into());
        event.rrule = Some("FREQ=WEEKLY".into());

        write_event(&cal, &event).unwrap();

        let read = read_collection(&cal);
        assert_eq!(read.len(), 1);
        let got = &read[0];
        assert_eq!(got.uid, event.uid);
        assert_eq!(got.summary, "Standup");
        assert_eq!(got.location.as_deref(), Some("Room 3"));
        assert_eq!(got.description.as_deref(), Some("Daily sync"));
        assert_eq!(got.rrule.as_deref(), Some("FREQ=WEEKLY"));
        assert_eq!(got.start, event.start);
        assert_eq!(got.end, event.end);
    }

    #[test]
    fn all_day_events_keep_date_semantics() {
        let root = temp_root();
        let cal = create_collection(root.path(), "Personal", Rgb(1, 2, 3)).unwrap();

        let mut event = Event::draft(
            &cal.id,
            NaiveDate::from_ymd_opt(2026, 8, 4)
                .unwrap()
                .and_hms_opt(0, 0, 0)
                .unwrap(),
            chrono_tz::UTC,
        );
        event.summary = "Holiday".into();
        event.start = EventTime::Date(NaiveDate::from_ymd_opt(2026, 8, 4).unwrap());
        event.end = EventTime::Date(NaiveDate::from_ymd_opt(2026, 8, 5).unwrap());

        write_event(&cal, &event).unwrap();
        let got = read_collection(&cal).remove(0);

        assert!(got.is_all_day(), "all-day event came back as timed");
        assert_eq!(
            got.start,
            EventTime::Date(NaiveDate::from_ymd_opt(2026, 8, 4).unwrap())
        );
    }

    #[test]
    fn timezone_survives_a_roundtrip() {
        let root = temp_root();
        let cal = create_collection(root.path(), "Personal", Rgb(1, 2, 3)).unwrap();

        let mut event = Event::draft(
            &cal.id,
            NaiveDate::from_ymd_opt(2026, 8, 4)
                .unwrap()
                .and_hms_opt(9, 0, 0)
                .unwrap(),
            chrono_tz::Europe::Athens,
        );
        event.summary = "Meeting".into();
        write_event(&cal, &event).unwrap();

        let got = read_collection(&cal).remove(0);
        assert_eq!(
            got.start,
            EventTime::Zoned(
                NaiveDate::from_ymd_opt(2026, 8, 4)
                    .unwrap()
                    .and_hms_opt(9, 0, 0)
                    .unwrap(),
                chrono_tz::Europe::Athens
            ),
            "TZID was not preserved"
        );
    }

    #[test]
    fn missing_dtend_gets_a_sensible_default() {
        let ics = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//test//EN\r\n\
                   BEGIN:VEVENT\r\nUID:x@test\r\nDTSTART:20260804T090000Z\r\n\
                   SUMMARY:No end\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
        let events = parse_ics(ics, "personal", "x.ics");
        assert_eq!(events.len(), 1);
        assert_eq!(
            events[0].end,
            EventTime::Zoned(
                NaiveDate::from_ymd_opt(2026, 8, 4)
                    .unwrap()
                    .and_hms_opt(10, 0, 0)
                    .unwrap(),
                chrono_tz::UTC
            )
        );
    }

    #[test]
    fn corrupt_file_does_not_hide_siblings() {
        let root = temp_root();
        let cal = create_collection(root.path(), "Personal", Rgb(1, 2, 3)).unwrap();

        std::fs::write(cal.path.join("broken.ics"), "this is not iCalendar at all").unwrap();

        let mut event = Event::draft(
            &cal.id,
            NaiveDate::from_ymd_opt(2026, 8, 4)
                .unwrap()
                .and_hms_opt(9, 0, 0)
                .unwrap(),
            chrono_tz::UTC,
        );
        event.summary = "Good".into();
        write_event(&cal, &event).unwrap();

        let read = read_collection(&cal);
        assert_eq!(read.len(), 1, "a corrupt file swallowed the valid one");
        assert_eq!(read[0].summary, "Good");
    }

    #[test]
    fn exdates_are_parsed_and_written() {
        let root = temp_root();
        let cal = create_collection(root.path(), "Personal", Rgb(1, 2, 3)).unwrap();

        let mut event = Event::draft(
            &cal.id,
            NaiveDate::from_ymd_opt(2026, 8, 3)
                .unwrap()
                .and_hms_opt(9, 0, 0)
                .unwrap(),
            chrono_tz::UTC,
        );
        event.summary = "Standup".into();
        event.rrule = Some("FREQ=WEEKLY".into());
        event.exdates = vec![
            NaiveDate::from_ymd_opt(2026, 8, 17)
                .unwrap()
                .and_hms_opt(9, 0, 0)
                .unwrap(),
        ];

        write_event(&cal, &event).unwrap();
        let got = read_collection(&cal).remove(0);
        assert_eq!(got.exdates, event.exdates);
    }

    #[test]
    fn delete_is_idempotent() {
        let root = temp_root();
        let cal = create_collection(root.path(), "Personal", Rgb(1, 2, 3)).unwrap();
        assert!(delete_event(&cal, "not-there.ics").is_ok());
    }

    #[test]
    fn writes_leave_no_temp_files_behind() {
        let root = temp_root();
        let cal = create_collection(root.path(), "Personal", Rgb(1, 2, 3)).unwrap();
        let mut event = Event::draft(
            &cal.id,
            NaiveDate::from_ymd_opt(2026, 8, 4)
                .unwrap()
                .and_hms_opt(9, 0, 0)
                .unwrap(),
            chrono_tz::UTC,
        );
        event.summary = "X".into();
        write_event(&cal, &event).unwrap();

        let temps: Vec<_> = std::fs::read_dir(&cal.path)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
            .collect();
        assert!(temps.is_empty(), "temp file left in the collection");
    }
}
