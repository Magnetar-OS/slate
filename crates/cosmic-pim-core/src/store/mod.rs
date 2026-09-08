// SPDX-License-Identifier: GPL-3.0-only

//! Storage: iCalendar files on disk, with a SQLite index in front of them.
//!
//! The split is deliberate. [`vdir`] owns the files, which are the source of
//! truth and remain readable by khal, Thunderbird, or vdirsyncer. [`index`] is a
//! cache that makes range queries cheap; it can be deleted at any time and will
//! rebuild itself. [`Store`] is the only thing the UI talks to.

pub mod index;
pub mod vdir;
pub mod watcher;

use crate::model::{CalendarMeta, Event, Occurrence, Rgb, expand, local_timezone};
use chrono::{DateTime, Duration, NaiveDate, NaiveTime, TimeZone, Utc};
use chrono_tz::Tz;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("{0}")]
    Io(#[from] std::io::Error),

    #[error("index error: {0}")]
    Sqlite(#[from] rusqlite::Error),

    #[error("cannot watch the calendar directory: {0}")]
    Notify(#[from] notify::Error),

    #[error("“{0}” is read-only")]
    ReadOnly(String),

    #[error("no calendar named “{0}”")]
    UnknownCalendar(String),

    #[error("no calendars available")]
    NoCalendars,
}

/// What an import did, so the UI can say so.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ImportSummary {
    pub added: usize,
    pub updated: usize,
}

impl ImportSummary {
    #[must_use]
    pub fn total(&self) -> usize {
        self.added + self.updated
    }
}

/// Makes a UID safe to use as a file name.
///
/// UIDs are arbitrary text and routinely contain `/` and `@`; without this an
/// imported file could escape its collection directory.
fn sanitise_file_stem(uid: &str) -> String {
    let cleaned: String = uid
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' {
                c
            } else {
                '-'
            }
        })
        .collect();

    // Path separators are already gone, so this cannot traverse. Collapsing runs
    // of dots anyway keeps `..` out of file names entirely, which is one less
    // thing for a future reader to have to reason about.
    let mut collapsed = String::with_capacity(cleaned.len());
    let mut last_was_dot = false;
    for c in cleaned.chars() {
        if c == '.' {
            if !last_was_dot {
                collapsed.push(c);
            }
            last_was_dot = true;
        } else {
            collapsed.push(c);
            last_was_dot = false;
        }
    }

    let trimmed = collapsed.trim_matches('.').trim_matches('-');
    if trimmed.is_empty() {
        uuid::Uuid::new_v4().to_string()
    } else {
        trimmed.chars().take(120).collect()
    }
}

/// Everything the UI needs from disk.
pub struct Store {
    root: PathBuf,
    index: index::Index,
    calendars: Vec<CalendarMeta>,
    local: Tz,
}

impl Store {
    /// Opens the store at the default location, creating it if absent.
    pub fn open_default() -> Result<Self, StoreError> {
        Self::open(&vdir::default_root(), &index::default_path())
    }

    pub fn open(root: &Path, index_path: &Path) -> Result<Self, StoreError> {
        std::fs::create_dir_all(root)?;

        let local = local_timezone();
        let index = index::Index::open(index_path, local)?;

        let mut store = Self {
            root: root.to_path_buf(),
            index,
            calendars: Vec::new(),
            local,
        };
        store.refresh()?;
        Ok(store)
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    #[must_use]
    pub fn local_timezone(&self) -> Tz {
        self.local
    }

    #[must_use]
    pub fn calendars(&self) -> &[CalendarMeta] {
        &self.calendars
    }

    #[must_use]
    pub fn calendar(&self, id: &str) -> Option<&CalendarMeta> {
        self.calendars.iter().find(|c| c.id == id)
    }

    /// The calendar new events land in unless the user picks another.
    #[must_use]
    pub fn default_calendar(&self) -> Option<&CalendarMeta> {
        self.calendars.iter().find(|c| !c.read_only)
    }

    /// Rescans collections and brings the index up to date.
    ///
    /// Returns `true` if anything changed, so the caller can skip a redraw when
    /// nothing did.
    pub fn refresh(&mut self) -> Result<bool, StoreError> {
        let found = vdir::collections(&self.root);
        let mut changed = found.len() != self.calendars.len()
            || found
                .iter()
                .zip(&self.calendars)
                .any(|(a, b)| a.id != b.id || a.name != b.name || a.color != b.color);

        self.calendars = found;
        self.index.prune_missing_calendars(&self.calendars)?;

        // `calendars` is borrowed immutably by the loop while `index` is borrowed
        // mutably, so take a cheap clone of the list to keep the borrow checker happy.
        let metas = self.calendars.clone();
        for meta in &metas {
            if self.index.sync_calendar(meta)? {
                changed = true;
            }
        }

        Ok(changed)
    }

    /// Every occurrence between `from` (inclusive) and `to` (exclusive), local dates.
    ///
    /// `hidden` names calendars the user has unticked in the sidebar.
    pub fn occurrences(
        &self,
        from: NaiveDate,
        to: NaiveDate,
        hidden: &HashSet<String>,
    ) -> Result<Vec<Occurrence>, StoreError> {
        let visible: Vec<String> = self
            .calendars
            .iter()
            .filter(|c| !hidden.contains(&c.id))
            .map(|c| c.id.clone())
            .collect();

        if visible.is_empty() {
            return Ok(Vec::new());
        }

        let from_utc = self.day_start_utc(from);
        let to_utc = self.day_start_utc(to);

        let mut out = Vec::new();
        for event in self.index.candidates(&visible, from_utc, to_utc)? {
            out.extend(expand(&event, from_utc, to_utc, self.local));
        }

        out.sort_by_key(Occurrence::sort_key);
        Ok(out)
    }

    /// Occurrences grouped per day, ready for a month or week grid.
    pub fn occurrences_by_day(
        &self,
        from: NaiveDate,
        to: NaiveDate,
        hidden: &HashSet<String>,
    ) -> Result<std::collections::BTreeMap<NaiveDate, Vec<Occurrence>>, StoreError> {
        let mut map: std::collections::BTreeMap<NaiveDate, Vec<Occurrence>> =
            std::collections::BTreeMap::new();

        for occurrence in self.occurrences(from, to, hidden)? {
            // A multi-day event belongs to every day it touches, so the grid can
            // draw it in each cell.
            let mut day = occurrence.start.date().max(from);
            let last = occurrence.end.date();
            let last = if occurrence.end.time() == NaiveTime::MIN {
                // End is exclusive: a 09:00–00:00 event does not touch the next day.
                last - Duration::days(1)
            } else {
                last
            };

            while day <= last && day < to {
                map.entry(day).or_default().push(occurrence.clone());
                day += Duration::days(1);
            }
        }

        Ok(map)
    }

    fn day_start_utc(&self, date: NaiveDate) -> DateTime<Utc> {
        use chrono::offset::LocalResult;
        let naive = date.and_time(NaiveTime::MIN);
        match self.local.from_local_datetime(&naive) {
            LocalResult::Single(t) | LocalResult::Ambiguous(t, _) => t.with_timezone(&Utc),
            LocalResult::None => Utc.from_utc_datetime(&naive),
        }
    }

    /// Looks up one event for editing.
    pub fn event(&self, calendar_id: &str, uid: &str) -> Result<Option<Event>, StoreError> {
        self.index.event(calendar_id, uid)
    }

    /// Writes an event and re-indexes its calendar.
    pub fn save(&mut self, event: &Event) -> Result<(), StoreError> {
        let meta = self
            .calendar(&event.calendar_id)
            .ok_or_else(|| StoreError::UnknownCalendar(event.calendar_id.clone()))?
            .clone();

        vdir::write_event(&meta, event)?;
        self.index.sync_calendar(&meta)?;
        Ok(())
    }

    /// Deletes an event and re-indexes its calendar.
    pub fn delete(&mut self, calendar_id: &str, uid: &str) -> Result<(), StoreError> {
        let meta = self
            .calendar(calendar_id)
            .ok_or_else(|| StoreError::UnknownCalendar(calendar_id.to_owned()))?
            .clone();

        if let Some(event) = self.index.event(calendar_id, uid)? {
            vdir::delete_event(&meta, &event.file_name)?;
        }
        self.index.sync_calendar(&meta)?;
        Ok(())
    }

    /// Moves an event to a different calendar, preserving its UID.
    pub fn move_to_calendar(&mut self, event: &Event, to: &str) -> Result<Event, StoreError> {
        if event.calendar_id == to {
            return Ok(event.clone());
        }
        let from_meta = self
            .calendar(&event.calendar_id)
            .ok_or_else(|| StoreError::UnknownCalendar(event.calendar_id.clone()))?
            .clone();

        let mut moved = event.clone();
        moved.calendar_id = to.to_owned();
        self.save(&moved)?;

        vdir::delete_event(&from_meta, &event.file_name)?;
        self.index.sync_calendar(&from_meta)?;
        Ok(moved)
    }

    /// Imports the events from an iCalendar document into `calendar_id`.
    ///
    /// An event whose UID is already present is updated rather than duplicated,
    /// which is what makes re-importing the same file safe — and what makes this
    /// usable as the handler for opening a `.ics` from a file manager or a mail
    /// attachment.
    pub fn import_ics(
        &mut self,
        text: &str,
        calendar_id: &str,
    ) -> Result<ImportSummary, StoreError> {
        let meta = self
            .calendar(calendar_id)
            .ok_or_else(|| StoreError::UnknownCalendar(calendar_id.to_owned()))?
            .clone();

        if meta.read_only {
            return Err(StoreError::ReadOnly(meta.name));
        }

        let incoming = vdir::parse_ics(text, calendar_id, "");
        let mut summary = ImportSummary::default();

        for mut event in incoming {
            // Reuse the existing file when the UID is already known, so a second
            // import overwrites instead of accumulating copies.
            match self.index.event(calendar_id, &event.uid)? {
                Some(existing) => {
                    event.file_name = existing.file_name;
                    summary.updated += 1;
                }
                None => {
                    event.file_name = format!("{}.ics", sanitise_file_stem(&event.uid));
                    summary.added += 1;
                }
            }

            event.calendar_id = calendar_id.to_owned();
            vdir::write_event(&meta, &event)?;
        }

        self.index.sync_calendar(&meta)?;
        Ok(summary)
    }

    /// Serialises a whole calendar as one iCalendar document.
    pub fn export_calendar(&self, calendar_id: &str) -> Result<String, StoreError> {
        let meta = self
            .calendar(calendar_id)
            .ok_or_else(|| StoreError::UnknownCalendar(calendar_id.to_owned()))?;

        Ok(vdir::export_collection(meta))
    }

    /// Creates a new collection on disk.
    pub fn create_calendar(&mut self, name: &str, color: Rgb) -> Result<CalendarMeta, StoreError> {
        let meta = vdir::create_collection(&self.root, name, color)?;
        self.refresh()?;
        Ok(meta)
    }

    /// Renames a calendar and/or changes its colour, writing the vdir metadata files.
    pub fn update_calendar(&mut self, id: &str, name: &str, color: Rgb) -> Result<(), StoreError> {
        let mut meta = self
            .calendar(id)
            .ok_or_else(|| StoreError::UnknownCalendar(id.to_owned()))?
            .clone();

        if meta.read_only {
            return Err(StoreError::ReadOnly(meta.name));
        }

        meta.name = name.trim().to_owned();
        meta.color = color;
        meta.save_meta()?;
        self.refresh()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::EventTime;

    fn store() -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("calendars");
        let index = dir.path().join("index.sqlite");
        let store = Store::open(&root, &index).unwrap();
        (dir, store)
    }

    fn day(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    #[test]
    fn opens_empty_and_reports_no_calendars() {
        let (_dir, store) = store();
        assert!(store.calendars().is_empty());
        assert!(store.default_calendar().is_none());
        assert!(
            store
                .occurrences(day(2026, 8, 1), day(2026, 9, 1), &HashSet::new())
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn create_save_and_query() {
        let (_dir, mut store) = store();
        let cal = store.create_calendar("Personal", Rgb(1, 2, 3)).unwrap();

        let mut event = Event::draft(
            &cal.id,
            day(2026, 8, 4).and_hms_opt(9, 0, 0).unwrap(),
            store.local,
        );
        event.summary = "Standup".into();
        store.save(&event).unwrap();

        let got = store
            .occurrences(day(2026, 8, 1), day(2026, 9, 1), &HashSet::new())
            .unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].summary, "Standup");
    }

    #[test]
    fn hidden_calendars_are_excluded() {
        let (_dir, mut store) = store();
        let cal = store.create_calendar("Personal", Rgb(1, 2, 3)).unwrap();

        let mut event = Event::draft(
            &cal.id,
            day(2026, 8, 4).and_hms_opt(9, 0, 0).unwrap(),
            store.local,
        );
        event.summary = "Standup".into();
        store.save(&event).unwrap();

        let hidden: HashSet<String> = [cal.id.clone()].into_iter().collect();
        assert!(
            store
                .occurrences(day(2026, 8, 1), day(2026, 9, 1), &hidden)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn delete_removes_the_event() {
        let (_dir, mut store) = store();
        let cal = store.create_calendar("Personal", Rgb(1, 2, 3)).unwrap();

        let mut event = Event::draft(
            &cal.id,
            day(2026, 8, 4).and_hms_opt(9, 0, 0).unwrap(),
            store.local,
        );
        event.summary = "Standup".into();
        store.save(&event).unwrap();
        store.delete(&cal.id, &event.uid).unwrap();

        assert!(
            store
                .occurrences(day(2026, 8, 1), day(2026, 9, 1), &HashSet::new())
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn multi_day_event_lands_in_every_day_it_touches() {
        let (_dir, mut store) = store();
        let cal = store.create_calendar("Personal", Rgb(1, 2, 3)).unwrap();

        let mut event = Event::draft(
            &cal.id,
            day(2026, 8, 4).and_hms_opt(0, 0, 0).unwrap(),
            store.local,
        );
        event.summary = "Conference".into();
        event.start = EventTime::Date(day(2026, 8, 4));
        event.end = EventTime::Date(day(2026, 8, 7)); // exclusive: 4th, 5th, 6th
        store.save(&event).unwrap();

        let by_day = store
            .occurrences_by_day(day(2026, 8, 1), day(2026, 9, 1), &HashSet::new())
            .unwrap();

        assert!(by_day.contains_key(&day(2026, 8, 4)));
        assert!(by_day.contains_key(&day(2026, 8, 5)));
        assert!(by_day.contains_key(&day(2026, 8, 6)));
        assert!(
            !by_day.contains_key(&day(2026, 8, 7)),
            "exclusive DTEND leaked into an extra day"
        );
    }

    #[test]
    fn recurring_event_appears_on_each_occurrence_day() {
        let (_dir, mut store) = store();
        let cal = store.create_calendar("Personal", Rgb(1, 2, 3)).unwrap();

        let mut event = Event::draft(
            &cal.id,
            day(2026, 8, 3).and_hms_opt(9, 0, 0).unwrap(),
            store.local,
        );
        event.summary = "Standup".into();
        event.rrule = Some("FREQ=WEEKLY".into());
        store.save(&event).unwrap();

        let by_day = store
            .occurrences_by_day(day(2026, 8, 1), day(2026, 9, 1), &HashSet::new())
            .unwrap();

        for d in [3u32, 10, 17, 24, 31] {
            assert!(by_day.contains_key(&day(2026, 8, d)), "missing {d} Aug");
        }
    }

    #[test]
    fn moving_between_calendars_leaves_one_copy() {
        let (_dir, mut store) = store();
        let a = store.create_calendar("Personal", Rgb(1, 2, 3)).unwrap();
        let b = store.create_calendar("Work", Rgb(4, 5, 6)).unwrap();

        let mut event = Event::draft(
            &a.id,
            day(2026, 8, 4).and_hms_opt(9, 0, 0).unwrap(),
            store.local,
        );
        event.summary = "Standup".into();
        store.save(&event).unwrap();

        let moved = store.move_to_calendar(&event, &b.id).unwrap();
        assert_eq!(moved.calendar_id, b.id);

        let all = store
            .occurrences(day(2026, 8, 1), day(2026, 9, 1), &HashSet::new())
            .unwrap();
        assert_eq!(all.len(), 1, "move left a duplicate behind");
        assert_eq!(all[0].calendar_id, b.id);
    }

    #[test]
    fn refresh_picks_up_externally_written_files() {
        let (_dir, mut store) = store();
        let cal = store.create_calendar("Personal", Rgb(1, 2, 3)).unwrap();

        // Simulate vdirsyncer dropping a file in.
        let ics = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//x//EN\r\n\
                   BEGIN:VEVENT\r\nUID:external@test\r\nDTSTART:20260804T120000Z\r\n\
                   DTEND:20260804T130000Z\r\nSUMMARY:From sync\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
        std::fs::write(cal.path.join("external.ics"), ics).unwrap();

        assert!(
            store.refresh().unwrap(),
            "refresh did not notice the new file"
        );
        let got = store
            .occurrences(day(2026, 8, 1), day(2026, 9, 1), &HashSet::new())
            .unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].summary, "From sync");
    }

    const SAMPLE: &str = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//x//EN\r\n\
         BEGIN:VEVENT\r\nUID:imported-1@test\r\nDTSTART:20260804T090000Z\r\n\
         DTEND:20260804T100000Z\r\nSUMMARY:Imported one\r\nEND:VEVENT\r\n\
         BEGIN:VEVENT\r\nUID:imported-2@test\r\nDTSTART:20260805T090000Z\r\n\
         DTEND:20260805T100000Z\r\nSUMMARY:Imported two\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";

    #[test]
    fn import_adds_every_event() {
        let (_dir, mut store) = store();
        let cal = store.create_calendar("Personal", Rgb(1, 2, 3)).unwrap();

        let summary = store.import_ics(SAMPLE, &cal.id).unwrap();
        assert_eq!(summary.added, 2);
        assert_eq!(summary.updated, 0);

        let got = store
            .occurrences(day(2026, 8, 1), day(2026, 9, 1), &HashSet::new())
            .unwrap();
        assert_eq!(got.len(), 2);
    }

    #[test]
    fn importing_the_same_file_twice_updates_rather_than_duplicates() {
        let (_dir, mut store) = store();
        let cal = store.create_calendar("Personal", Rgb(1, 2, 3)).unwrap();

        store.import_ics(SAMPLE, &cal.id).unwrap();
        let second = store.import_ics(SAMPLE, &cal.id).unwrap();

        assert_eq!(second.added, 0, "re-import created duplicates");
        assert_eq!(second.updated, 2);
        assert_eq!(
            store
                .occurrences(day(2026, 8, 1), day(2026, 9, 1), &HashSet::new())
                .unwrap()
                .len(),
            2
        );
    }

    #[test]
    fn a_uid_with_path_separators_cannot_escape_the_collection() {
        let (_dir, mut store) = store();
        let cal = store.create_calendar("Personal", Rgb(1, 2, 3)).unwrap();

        let nasty = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//x//EN\r\n\
                     BEGIN:VEVENT\r\nUID:../../../../tmp/escaped\r\n\
                     DTSTART:20260804T090000Z\r\nDTEND:20260804T100000Z\r\n\
                     SUMMARY:Nasty\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";

        store.import_ics(nasty, &cal.id).unwrap();

        let written: Vec<String> = std::fs::read_dir(&cal.path)
            .unwrap()
            .filter_map(Result::ok)
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.ends_with(".ics"))
            .collect();

        assert_eq!(written.len(), 1);
        assert!(
            !written[0].contains('/') && !written[0].contains(".."),
            "unsanitised UID became the file name: {}",
            written[0]
        );

        // The property that actually matters: the file landed inside the collection.
        let written_path = cal.path.join(&written[0]).canonicalize().unwrap();
        assert!(
            written_path.starts_with(cal.path.canonicalize().unwrap()),
            "import escaped the collection directory: {}",
            written_path.display()
        );
    }

    #[test]
    fn export_round_trips_through_import() {
        let (_dir, mut store) = store();
        let a = store.create_calendar("Personal", Rgb(1, 2, 3)).unwrap();
        let b = store.create_calendar("Work", Rgb(4, 5, 6)).unwrap();

        store.import_ics(SAMPLE, &a.id).unwrap();
        let exported = store.export_calendar(&a.id).unwrap();

        let summary = store.import_ics(&exported, &b.id).unwrap();
        assert_eq!(summary.added, 2, "export lost events on the way back in");

        let hidden: HashSet<String> = [a.id.clone()].into_iter().collect();
        let in_b = store
            .occurrences(day(2026, 8, 1), day(2026, 9, 1), &hidden)
            .unwrap();
        assert_eq!(in_b.len(), 2);
        assert!(in_b.iter().all(|o| o.calendar_id == b.id));
    }

    #[test]
    fn exporting_an_unknown_calendar_is_an_error() {
        let (_dir, store) = store();
        assert!(matches!(
            store.export_calendar("nope"),
            Err(StoreError::UnknownCalendar(_))
        ));
    }

    #[test]
    fn sanitised_stems_stay_usable() {
        assert_eq!(sanitise_file_stem("abc-123"), "abc-123");
        assert_eq!(sanitise_file_stem("a@b.com"), "a-b.com");
        assert_eq!(
            sanitise_file_stem("a..b"),
            "a.b",
            "consecutive dots survived"
        );
        assert!(!sanitise_file_stem("../../etc/passwd").contains(".."));
        // An entirely unusable UID still yields something writable.
        assert!(!sanitise_file_stem("///").is_empty());
    }

    #[test]
    fn saving_to_an_unknown_calendar_is_an_error() {
        let (_dir, mut store) = store();
        let event = Event::draft(
            "nope",
            day(2026, 8, 4).and_hms_opt(9, 0, 0).unwrap(),
            store.local,
        );
        assert!(matches!(
            store.save(&event),
            Err(StoreError::UnknownCalendar(_))
        ));
    }
}
