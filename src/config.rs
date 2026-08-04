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
}

impl ViewKind {
    pub const ALL: &'static [ViewKind] = &[ViewKind::Month, ViewKind::Week, ViewKind::Day];

    /// How many days this view steps by when paging forward or back.
    #[must_use]
    pub fn page_days(self) -> i64 {
        match self {
            // Month is handled separately — calendar months are not a fixed length.
            ViewKind::Month => 0,
            ViewKind::Week => 7,
            ViewKind::Day => 1,
        }
    }
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
