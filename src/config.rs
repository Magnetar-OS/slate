// SPDX-License-Identifier: GPL-3.0-only

//! Persisted settings, stored through `cosmic-config` so they live alongside
//! every other COSMIC app's configuration and are picked up live when changed.

use cosmic::cosmic_config::{self, CosmicConfigEntry, cosmic_config_derive::CosmicConfigEntry};
use serde::{Deserialize, Serialize};

/// Which grid the main area shows.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub enum ViewKind {
    #[default]
    Month,
    Week,
    Day,
    /// A flat list of the next month of events — the applet's model, with a
    /// full window's worth of room.
    Agenda,
    /// Twelve mini months, each day tinted by how busy it is. Answers "when am
    /// I busy" rather than "what is on".
    Year,
    /// The task list. Unlike the others this is not a date range — it is
    /// every VTODO in every visible collection — so the paging controls and
    /// the range title do not apply to it.
    Tasks,
}

impl ViewKind {
    pub const ALL: &'static [ViewKind] = &[
        ViewKind::Month,
        ViewKind::Week,
        ViewKind::Day,
        ViewKind::Agenda,
        ViewKind::Year,
        ViewKind::Tasks,
    ];

    /// Days the agenda view lists from its anchor.
    pub const AGENDA_DAYS: i64 = 30;

    /// Whether this view is anchored to a date range the user can page through.
    #[must_use]
    pub fn is_dated(self) -> bool {
        !matches!(self, ViewKind::Tasks)
    }

    /// How many days this view steps by when paging forward or back.
    #[must_use]
    pub fn page_days(self) -> i64 {
        match self {
            // Month is handled separately — calendar months are not a fixed length.
            ViewKind::Month => 0,
            ViewKind::Week => 7,
            ViewKind::Day => 1,
            ViewKind::Agenda => Self::AGENDA_DAYS,
            // Years are not a fixed number of days either; handled with month.
            ViewKind::Year => 0,
            // Never paged.
            ViewKind::Tasks => 0,
        }
    }
}

/// Per-calendar overrides of the two app-wide defaults.
///
/// Kept in this app's own config rather than in the collection directory: a
/// vdir holds what every client agrees on, and neither of these is a thing
/// khal or Thunderbird would know how to read. A calendar with no entry here
/// simply uses the app-wide values.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CalendarDefaults {
    pub calendar_id: String,
    /// Minutes before the event to remind. `0` means "use the app-wide
    /// setting", which is itself `0` for "no reminder".
    pub reminder_minutes: u32,
    /// How long a new event in this calendar lasts, in minutes. `0` means the
    /// usual hour.
    pub duration_minutes: u32,
}

#[derive(Clone, Debug, CosmicConfigEntry, Eq, PartialEq)]
#[version = 1]
pub struct Config {
    /// The view restored at startup.
    pub view: ViewKind,
    /// 0 = Monday … 6 = Sunday, matching `chrono::Weekday::num_days_from_monday`.
    pub first_day_of_week: u8,
    /// Calendars the user has unticked in the sidebar.
    pub hidden_calendars: Vec<String>,
    pub show_week_numbers: bool,
    /// 24-hour clock rather than am/pm.
    pub time_24h: bool,
    /// Minutes before an event to remind, for events that carry no `VALARM` of
    /// their own. `0` means no default reminder.
    pub default_reminder_minutes: u32,
    /// A second hour gutter in the week/day grids, as an IANA zone name.
    /// Empty means off. Stored as text rather than a `Tz` so a zone this
    /// build's tzdb does not know cannot corrupt the config.
    pub secondary_timezone: String,
    /// Grid drag operations round to this many minutes: 5, 10, or 15.
    pub snap_minutes: u8,
    /// Birthdays from the suite's address books, shown as all-day entries.
    pub show_birthdays: bool,
    /// Per-calendar overrides; calendars absent from this list use the
    /// app-wide defaults.
    pub calendar_defaults: Vec<CalendarDefaults>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            view: ViewKind::Month,
            first_day_of_week: 0,
            hidden_calendars: Vec::new(),
            show_week_numbers: false,
            // The majority of locales this app is likely to meet use a 24-hour
            // clock; the setting is one toggle away for the rest.
            time_24h: true,
            // Off by default: an app that starts notifying about every event
            // without being asked is an app people uninstall.
            default_reminder_minutes: 0,
            secondary_timezone: String::new(),
            snap_minutes: 15,
            // On by default: the address book already knows them, and an
            // empty book costs nothing.
            show_birthdays: true,
            calendar_defaults: Vec::new(),
        }
    }
}

impl Config {
    #[must_use]
    pub fn first_weekday(&self) -> chrono::Weekday {
        use chrono::Weekday;
        match self.first_day_of_week {
            1 => Weekday::Tue,
            2 => Weekday::Wed,
            3 => Weekday::Thu,
            4 => Weekday::Fri,
            5 => Weekday::Sat,
            6 => Weekday::Sun,
            _ => Weekday::Mon,
        }
    }

    /// The same weekday, in the `jiff` vocabulary libcosmic's calendar widget speaks.
    #[must_use]
    pub fn first_weekday_jiff(&self) -> jiff::civil::Weekday {
        use jiff::civil::Weekday;
        match self.first_day_of_week {
            1 => Weekday::Tuesday,
            2 => Weekday::Wednesday,
            3 => Weekday::Thursday,
            4 => Weekday::Friday,
            5 => Weekday::Saturday,
            6 => Weekday::Sunday,
            _ => Weekday::Monday,
        }
    }

    /// The secondary-gutter zone, when one is set and resolvable.
    #[must_use]
    pub fn secondary_tz(&self) -> Option<chrono_tz::Tz> {
        self.secondary_timezone.trim().parse().ok()
    }

    /// The drag snap step, guarded against a hand-edited config saying `0`.
    #[must_use]
    pub fn snap(&self) -> i64 {
        match self.snap_minutes {
            5 | 10 => i64::from(self.snap_minutes),
            _ => 15,
        }
    }

    #[must_use]
    pub fn is_hidden(&self, calendar_id: &str) -> bool {
        self.hidden_calendars.iter().any(|c| c == calendar_id)
    }

    pub fn toggle_calendar(&mut self, calendar_id: &str) {
        if let Some(pos) = self.hidden_calendars.iter().position(|c| c == calendar_id) {
            self.hidden_calendars.remove(pos);
        } else {
            self.hidden_calendars.push(calendar_id.to_owned());
        }
    }

    /// The default reminder lead time, or `None` when disabled.
    #[must_use]
    pub fn default_reminder(&self) -> Option<chrono::Duration> {
        (self.default_reminder_minutes > 0)
            .then(|| chrono::Duration::minutes(i64::from(self.default_reminder_minutes)))
    }

    /// The reminder lead time for events in `calendar_id`: the calendar's own
    /// if it sets one, otherwise the app-wide default.
    #[must_use]
    pub fn reminder_for(&self, calendar_id: &str) -> Option<chrono::Duration> {
        match self.defaults_for(calendar_id) {
            Some(defaults) if defaults.reminder_minutes > 0 => Some(chrono::Duration::minutes(
                i64::from(defaults.reminder_minutes),
            )),
            _ => self.default_reminder(),
        }
    }

    /// How long a new event in `calendar_id` lasts. An hour unless the
    /// calendar says otherwise.
    #[must_use]
    pub fn duration_for(&self, calendar_id: &str) -> chrono::Duration {
        match self.defaults_for(calendar_id) {
            Some(defaults) if defaults.duration_minutes > 0 => {
                chrono::Duration::minutes(i64::from(defaults.duration_minutes))
            }
            _ => chrono::Duration::hours(1),
        }
    }

    #[must_use]
    pub fn defaults_for(&self, calendar_id: &str) -> Option<&CalendarDefaults> {
        self.calendar_defaults
            .iter()
            .find(|d| d.calendar_id == calendar_id)
    }

    /// Replaces one calendar's overrides, dropping the entry entirely when
    /// both fields are back to "use the app-wide value".
    pub fn set_defaults_for(&mut self, calendar_id: &str, reminder: u32, duration: u32) {
        self.calendar_defaults
            .retain(|d| d.calendar_id != calendar_id);
        if reminder > 0 || duration > 0 {
            self.calendar_defaults.push(CalendarDefaults {
                calendar_id: calendar_id.to_owned(),
                reminder_minutes: reminder,
                duration_minutes: duration,
            });
        }
    }

    #[must_use]
    pub fn hidden_set(&self) -> std::collections::HashSet<String> {
        self.hidden_calendars.iter().cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toggling_a_calendar_is_reversible() {
        let mut config = Config::default();
        assert!(!config.is_hidden("work"));

        config.toggle_calendar("work");
        assert!(config.is_hidden("work"));

        config.toggle_calendar("work");
        assert!(!config.is_hidden("work"));
        assert!(config.hidden_calendars.is_empty());
    }

    #[test]
    fn a_calendar_without_overrides_uses_the_app_defaults() {
        let config = Config {
            default_reminder_minutes: 15,
            ..Config::default()
        };
        assert_eq!(
            config.reminder_for("work"),
            Some(chrono::Duration::minutes(15))
        );
        assert_eq!(config.duration_for("work"), chrono::Duration::hours(1));
    }

    #[test]
    fn a_calendars_own_defaults_win() {
        let mut config = Config {
            default_reminder_minutes: 15,
            ..Config::default()
        };
        config.set_defaults_for("work", 30, 45);

        assert_eq!(
            config.reminder_for("work"),
            Some(chrono::Duration::minutes(30))
        );
        assert_eq!(config.duration_for("work"), chrono::Duration::minutes(45));
        // Other calendars are untouched.
        assert_eq!(
            config.reminder_for("personal"),
            Some(chrono::Duration::minutes(15))
        );
    }

    #[test]
    fn clearing_both_overrides_forgets_the_calendar() {
        // Otherwise the list would grow an inert entry per calendar ever
        // touched, and every one of them would be written back to disk.
        let mut config = Config::default();
        config.set_defaults_for("work", 30, 45);
        assert_eq!(config.calendar_defaults.len(), 1);

        config.set_defaults_for("work", 0, 0);
        assert!(config.calendar_defaults.is_empty());
        assert_eq!(config.duration_for("work"), chrono::Duration::hours(1));
    }

    #[test]
    fn setting_one_override_twice_replaces_rather_than_duplicates() {
        let mut config = Config::default();
        config.set_defaults_for("work", 30, 0);
        config.set_defaults_for("work", 60, 0);
        assert_eq!(config.calendar_defaults.len(), 1);
        assert_eq!(
            config.reminder_for("work"),
            Some(chrono::Duration::minutes(60))
        );
    }

    #[test]
    fn first_weekday_defaults_to_monday() {
        assert_eq!(Config::default().first_weekday(), chrono::Weekday::Mon);
    }

    #[test]
    fn out_of_range_weekday_falls_back_to_monday() {
        let config = Config {
            first_day_of_week: 42,
            ..Config::default()
        };
        assert_eq!(config.first_weekday(), chrono::Weekday::Mon);
    }
}
