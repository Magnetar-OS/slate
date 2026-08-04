// SPDX-License-Identifier: GPL-3.0-only

//! The event editor, shown in the context drawer.

use super::format_time;
use crate::app::Message;
use crate::config::Config;
use crate::fl;
use crate::model::{CalendarMeta, Event, EventTime, Freq, Recurrence, RepeatEnd};
use chrono::{NaiveDate, NaiveDateTime, NaiveTime, Timelike};
use chrono_tz::Tz;
use cosmic::iced::{Alignment, Length};
use cosmic::widget;
use cosmic::{Apply, Element};

/// Which date field the inline date picker is currently attached to.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DateField {
    Start,
    End,
    Until,
}

/// Editor state. Kept as strings where the user types, so a half-typed time does
/// not have to round-trip through a parser on every keystroke.
pub struct Editor {
    /// `None` when creating; `Some` carries the file name and UID we must keep.
    pub original: Option<Event>,
    pub calendar_id: String,
    pub summary: String,
    pub location: String,
    pub description: String,
    pub all_day: bool,
    pub start_date: NaiveDate,
    pub start_time: String,
    pub end_date: NaiveDate,
    pub end_time: String,
    pub freq: Freq,
    pub interval: String,
    pub repeat_end: RepeatEnd,
    pub count: String,
    pub until: NaiveDate,
    /// Set when the event carries an `RRULE` the simple editor cannot represent.
    /// The rule is preserved on save rather than being flattened.
    pub custom_rrule: Option<String>,
    pub picker: widget::calendar::CalendarModel,
    pub picking: Option<DateField>,
    pub error: Option<String>,
}

impl Editor {
    /// A blank event at `start`.
    #[must_use]
    pub fn new(calendar_id: String, start: NaiveDateTime, all_day: bool) -> Self {
        let end = start + chrono::Duration::hours(1);
        Self {
            original: None,
            calendar_id,
            summary: String::new(),
            location: String::new(),
            description: String::new(),
            all_day,
            start_date: start.date(),
            start_time: format!("{:02}:{:02}", start.hour(), start.minute()),
            end_date: end.date(),
            end_time: format!("{:02}:{:02}", end.hour(), end.minute()),
            freq: Freq::Never,
            interval: "1".into(),
            repeat_end: RepeatEnd::Never,
            count: "10".into(),
            until: start.date() + chrono::Duration::days(30),
            custom_rrule: None,
            picker: to_picker(start.date()),
            picking: None,
            error: None,
        }
    }

    /// Loads an existing event for editing.
    #[must_use]
    pub fn from_event(event: &Event, local: Tz) -> Self {
        let start = event.start.naive_local(local);
        let end = event.end.naive_local(local);

        // The editor understands a subset of RRULE. Anything richer is kept
        // verbatim in `custom_rrule` and written back untouched.
        let (freq, interval, repeat_end, custom) = match event.recurrence() {
            Some(recurrence) => (
                recurrence.freq,
                recurrence.interval.to_string(),
                recurrence.end,
                None,
            ),
            None => (
                Freq::Never,
                "1".to_owned(),
                RepeatEnd::Never,
                event.rrule.clone(),
            ),
        };

        Self {
            calendar_id: event.calendar_id.clone(),
            summary: event.summary.clone(),
            location: event.location.clone().unwrap_or_default(),
            description: event.description.clone().unwrap_or_default(),
            all_day: event.is_all_day(),
            start_date: start.date(),
            start_time: format!("{:02}:{:02}", start.hour(), start.minute()),
            // An all-day DTEND is exclusive; show the user the last day it covers.
            end_date: if event.is_all_day() {
                end.date() - chrono::Duration::days(1)
            } else {
                end.date()
            },
            end_time: format!("{:02}:{:02}", end.hour(), end.minute()),
            freq,
            interval,
            count: match repeat_end {
                RepeatEnd::After(n) => n.to_string(),
                _ => "10".to_owned(),
            },
            until: match repeat_end {
                RepeatEnd::On(d) => d,
                _ => start.date() + chrono::Duration::days(30),
            },
            repeat_end,
            custom_rrule: custom,
            picker: to_picker(start.date()),
            picking: None,
            error: None,
            original: Some(event.clone()),
        }
    }

    #[must_use]
    pub fn is_new(&self) -> bool {
        self.original.is_none()
    }

    /// Builds the event to save, or explains why it cannot.
    pub fn to_event(&self, local: Tz) -> Result<Event, String> {
        if self.summary.trim().is_empty() {
            return Err(fl!("error-summary-required"));
        }

        let (start, end) = if self.all_day {
            // DTEND is exclusive, so the stored end is the day after the last
            // day the user selected.
            let end = self.end_date.max(self.start_date) + chrono::Duration::days(1);
            (EventTime::Date(self.start_date), EventTime::Date(end))
        } else {
            let start_time =
                parse_time(&self.start_time).ok_or_else(|| fl!("error-invalid-time-range"))?;
            let end_time =
                parse_time(&self.end_time).ok_or_else(|| fl!("error-invalid-time-range"))?;

            let start = self.start_date.and_time(start_time);
            let end = self.end_date.and_time(end_time);

            if end <= start {
                return Err(fl!("error-invalid-time-range"));
            }

            (EventTime::Zoned(start, local), EventTime::Zoned(end, local))
        };

        let rrule = match &self.custom_rrule {
            // Never rewrite a rule we could not fully parse.
            Some(raw) => Some(raw.clone()),
            None => Recurrence {
                freq: self.freq,
                interval: self.interval.parse().unwrap_or(1).max(1),
                end: self.resolved_repeat_end(),
            }
            .to_rrule(),
        };

        let mut event = match &self.original {
            Some(original) => {
                let mut event = original.clone();
                // Bump SEQUENCE so CalDAV servers and other clients see a newer revision.
                event.sequence = event.sequence.saturating_add(1);
                event
            }
            None => Event::draft(
                &self.calendar_id,
                self.start_date.and_time(NaiveTime::MIN),
                local,
            ),
        };

        event.calendar_id = self.calendar_id.clone();
        event.summary = self.summary.trim().to_owned();
        event.location = non_empty(&self.location);
        event.description = non_empty(&self.description);
        event.start = start;
        event.end = end;
        event.rrule = rrule;
        event.last_modified = Some(chrono::Utc::now());

        Ok(event)
    }

    fn resolved_repeat_end(&self) -> RepeatEnd {
        match self.repeat_end {
            RepeatEnd::After(_) => RepeatEnd::After(self.count.parse().unwrap_or(10).max(1)),
            RepeatEnd::On(_) => RepeatEnd::On(self.until),
            RepeatEnd::Never => RepeatEnd::Never,
        }
    }

    #[must_use]
    pub fn view<'a>(
        &'a self,
        calendars: &'a [CalendarMeta],
        config: &'a Config,
    ) -> Element<'a, Message> {
        let spacing = cosmic::theme::spacing();

        let mut column = widget::column::with_capacity(8).spacing(spacing.space_s);

        if let Some(error) = &self.error {
            column = column.push(
                widget::text::body(error.clone())
                    .class(cosmic::theme::Text::Custom(|theme| {
                        cosmic::iced::widget::text::Style {
                            color: Some(theme.cosmic().destructive_color().into()),
                            ..Default::default()
                        }
                    }))
                    .wrapping(cosmic::iced::core::text::Wrapping::Word),
            );
        }

        column = column
            .push(self.details_section(calendars))
            .push(self.time_section(config))
            .push(self.repeat_section())
            .push(self.actions());

        widget::scrollable(column).into()
    }

    fn details_section<'a>(&'a self, calendars: &'a [CalendarMeta]) -> Element<'a, Message> {
        let names: Vec<String> = calendars.iter().map(|c| c.name.clone()).collect();
        let selected = calendars.iter().position(|c| c.id == self.calendar_id);

        widget::settings::section()
            .add(
                widget::settings::item::builder(fl!("event-summary")).control(
                    widget::text_input(fl!("event-summary"), &self.summary)
                        .on_input(Message::EditorSummary)
                        .width(Length::Fixed(220.0)),
                ),
            )
            .add(
                widget::settings::item::builder(fl!("event-location")).control(
                    widget::text_input(fl!("event-location"), &self.location)
                        .on_input(Message::EditorLocation)
                        .width(Length::Fixed(220.0)),
                ),
            )
            .add(
                widget::settings::item::builder(fl!("event-calendar")).control(
                    widget::dropdown(names, selected, Message::EditorCalendar)
                        .width(Length::Fixed(220.0)),
                ),
            )
            .add(
                widget::settings::item::builder(fl!("event-description")).control(
                    widget::text_input(fl!("event-description"), &self.description)
                        .on_input(Message::EditorDescription)
                        .width(Length::Fixed(220.0)),
                ),
            )
            .into()
    }

    fn time_section(&self, config: &Config) -> Element<'_, Message> {
        let mut section = widget::settings::section()
            .add(
                widget::settings::item::builder(fl!("all-day"))
                    .toggler(self.all_day, Message::EditorAllDay),
            )
            .add(
                widget::settings::item::builder(fl!("event-starts")).control(self.date_control(
                    DateField::Start,
                    self.start_date,
                    &self.start_time,
                    Message::EditorStartTime,
                    config,
                )),
            )
            .add(
                widget::settings::item::builder(fl!("event-ends")).control(self.date_control(
                    DateField::End,
                    self.end_date,
                    &self.end_time,
                    Message::EditorEndTime,
                    config,
                )),
            );

        if let Some(field) = self.picking {
            section = section.add(self.picker_row(field));
        }

        section.into()
    }

    fn date_control<'a>(
        &'a self,
        field: DateField,
        date: NaiveDate,
        time: &'a str,
        on_time: fn(String) -> Message,
        config: &Config,
    ) -> Element<'a, Message> {
        let spacing = cosmic::theme::spacing();

        let mut row = widget::row::with_capacity(2)
            .spacing(spacing.space_xxs)
            .align_y(Alignment::Center)
            .push(
                widget::button::standard(super::format_date_short(date))
                    .on_press(Message::EditorPickDate(field)),
            );

        if !self.all_day {
            row = row.push(
                widget::text_input(
                    format_time(
                        NaiveTime::from_hms_opt(9, 0, 0).unwrap_or(NaiveTime::MIN),
                        config,
                    ),
                    time,
                )
                .on_input(on_time)
                .width(Length::Fixed(84.0)),
            );
        }

        row.into()
    }

    fn picker_row(&self, _field: DateField) -> Element<'_, Message> {
        widget::calendar(
            &self.picker,
            Message::EditorDatePicked,
            || Message::EditorPickerPrev,
            || Message::EditorPickerNext,
            jiff::civil::Weekday::Monday,
        )
        .apply(widget::container)
        .padding(cosmic::theme::spacing().space_xxs)
        .into()
    }

    fn repeat_section(&self) -> Element<'_, Message> {
        let spacing = cosmic::theme::spacing();

        // An RRULE we cannot express is shown read-only rather than silently
        // replaced by whatever the dropdowns happen to say.
        if let Some(raw) = &self.custom_rrule {
            return widget::settings::section()
                .add(
                    widget::settings::item::builder(fl!("repeats")).control(
                        widget::text::caption(raw.clone())
                            .wrapping(cosmic::iced::core::text::Wrapping::Word),
                    ),
                )
                .into();
        }

        let freq_labels: Vec<String> = Freq::ALL.iter().map(|f| freq_label(*f)).collect();
        let freq_index = Freq::ALL.iter().position(|f| *f == self.freq);

        let mut section = widget::settings::section().add(
            widget::settings::item::builder(fl!("repeats")).control(
                widget::dropdown(freq_labels, freq_index, Message::EditorFreq)
                    .width(Length::Fixed(220.0)),
            ),
        );

        if self.freq != Freq::Never {
            section = section
                .add(
                    widget::settings::item::builder(fl!("repeat-interval")).control(
                        widget::text_input("1", &self.interval)
                            .on_input(Message::EditorInterval)
                            .width(Length::Fixed(84.0)),
                    ),
                )
                .add(
                    widget::settings::item::builder(fl!("repeat-ends")).control(
                        widget::dropdown(
                            vec![
                                fl!("repeat-ends-never"),
                                fl!("repeat-ends-after"),
                                fl!("repeat-ends-on"),
                            ],
                            Some(match self.repeat_end {
                                RepeatEnd::Never => 0,
                                RepeatEnd::After(_) => 1,
                                RepeatEnd::On(_) => 2,
                            }),
                            Message::EditorRepeatEnd,
                        )
                        .width(Length::Fixed(220.0)),
                    ),
                );

            section = match self.repeat_end {
                RepeatEnd::After(_) => section.add(
                    widget::settings::item::builder(fl!("occurrences")).control(
                        widget::text_input("10", &self.count)
                            .on_input(Message::EditorCount)
                            .width(Length::Fixed(84.0)),
                    ),
                ),
                RepeatEnd::On(_) => section.add(
                    widget::settings::item::builder(fl!("repeat-ends-on")).control(
                        widget::button::standard(super::format_date_short(self.until))
                            .on_press(Message::EditorPickDate(DateField::Until)),
                    ),
                ),
                RepeatEnd::Never => section,
            };
        }

        section
            .apply(Element::from)
            .apply(|e| widget::container(e).padding([0, 0, spacing.space_xxs, 0]))
            .into()
    }

    fn actions(&self) -> Element<'_, Message> {
        let spacing = cosmic::theme::spacing();

        let mut row = widget::row::with_capacity(3)
            .spacing(spacing.space_xxs)
            .push(widget::button::suggested(fl!("save")).on_press(Message::EditorSave))
            .push(widget::button::standard(fl!("cancel")).on_press(Message::EditorCancel));

        if !self.is_new() {
            row = row
                .push(widget::Space::new().width(Length::Fill))
                .push(widget::button::destructive(fl!("delete")).on_press(Message::EditorDelete));
        }

        row.width(Length::Fill).into()
    }
}

fn freq_label(freq: Freq) -> String {
    match freq {
        Freq::Never => fl!("repeat-never"),
        Freq::Daily => fl!("repeat-daily"),
        Freq::Weekly => fl!("repeat-weekly"),
        Freq::Monthly => fl!("repeat-monthly"),
        Freq::Yearly => fl!("repeat-yearly"),
    }
}

fn non_empty(s: &str) -> Option<String> {
    let t = s.trim();
    if t.is_empty() {
        None
    } else {
        Some(t.to_owned())
    }
}

/// Parses the forms a user is likely to type: `9:30`, `09:30`, `0930`, `9`.
#[must_use]
pub fn parse_time(input: &str) -> Option<NaiveTime> {
    let s = input.trim();
    if s.is_empty() {
        return None;
    }

    // Accept a trailing am/pm marker.
    let lower = s.to_ascii_lowercase();
    let (body, pm, has_meridiem) = if let Some(rest) = lower.strip_suffix("pm") {
        (rest.trim().to_owned(), true, true)
    } else if let Some(rest) = lower.strip_suffix("am") {
        (rest.trim().to_owned(), false, true)
    } else {
        (lower, false, false)
    };

    let (h, m) = if let Some((h, m)) = body.split_once(':') {
        (h.trim().parse::<u32>().ok()?, m.trim().parse::<u32>().ok()?)
    } else if body.len() == 4 && body.chars().all(|c| c.is_ascii_digit()) {
        (body[..2].parse().ok()?, body[2..].parse().ok()?)
    } else {
        (body.trim().parse::<u32>().ok()?, 0)
    };

    let h = if has_meridiem {
        match (h, pm) {
            (12, false) => 0,
            (12, true) => 12,
            (h, true) if h < 12 => h + 12,
            (h, _) => h,
        }
    } else {
        h
    };

    NaiveTime::from_hms_opt(h, m, 0)
}

fn to_picker(date: NaiveDate) -> widget::calendar::CalendarModel {
    let d = to_jiff(date);
    widget::calendar::CalendarModel::new(d, d)
}

/// chrono → jiff, for the toolkit's calendar widget.
#[must_use]
pub fn to_jiff(date: NaiveDate) -> jiff::civil::Date {
    use chrono::Datelike;
    jiff::civil::date(
        i16::try_from(date.year()).unwrap_or(0),
        i8::try_from(date.month()).unwrap_or(1),
        i8::try_from(date.day()).unwrap_or(1),
    )
}

/// jiff → chrono.
#[must_use]
pub fn from_jiff(date: jiff::civil::Date) -> Option<NaiveDate> {
    NaiveDate::from_ymd_opt(
        i32::from(date.year()),
        u32::try_from(date.month()).ok()?,
        u32::try_from(date.day()).ok()?,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(h: u32, m: u32) -> NaiveTime {
        NaiveTime::from_hms_opt(h, m, 0).unwrap()
    }

    #[test]
    fn parses_the_time_formats_people_type() {
        assert_eq!(parse_time("9:30"), Some(t(9, 30)));
        assert_eq!(parse_time("09:30"), Some(t(9, 30)));
        assert_eq!(parse_time("0930"), Some(t(9, 30)));
        assert_eq!(parse_time("9"), Some(t(9, 0)));
        assert_eq!(parse_time(" 14:05 "), Some(t(14, 5)));
    }

    #[test]
    fn parses_meridiem() {
        assert_eq!(parse_time("2:05 pm"), Some(t(14, 5)));
        assert_eq!(parse_time("2:05pm"), Some(t(14, 5)));
        assert_eq!(parse_time("12:00 am"), Some(t(0, 0)));
        assert_eq!(parse_time("12:00 pm"), Some(t(12, 0)));
        assert_eq!(parse_time("11:59 pm"), Some(t(23, 59)));
    }

    #[test]
    fn rejects_nonsense() {
        assert_eq!(parse_time(""), None);
        assert_eq!(parse_time("abc"), None);
        assert_eq!(parse_time("25:00"), None);
        assert_eq!(parse_time("9:70"), None);
    }

    #[test]
    fn jiff_roundtrip() {
        let d = NaiveDate::from_ymd_opt(2026, 8, 4).unwrap();
        assert_eq!(from_jiff(to_jiff(d)), Some(d));
    }

    #[test]
    fn new_event_requires_a_title() {
        let editor = Editor::new(
            "personal".into(),
            NaiveDate::from_ymd_opt(2026, 8, 4)
                .unwrap()
                .and_hms_opt(9, 0, 0)
                .unwrap(),
            false,
        );
        assert!(editor.to_event(chrono_tz::UTC).is_err());
    }

    #[test]
    fn end_before_start_is_rejected() {
        let mut editor = Editor::new(
            "personal".into(),
            NaiveDate::from_ymd_opt(2026, 8, 4)
                .unwrap()
                .and_hms_opt(9, 0, 0)
                .unwrap(),
            false,
        );
        editor.summary = "Standup".into();
        editor.end_time = "08:00".into();
        assert!(editor.to_event(chrono_tz::UTC).is_err());
    }

    #[test]
    fn all_day_end_is_stored_exclusive() {
        let mut editor = Editor::new(
            "personal".into(),
            NaiveDate::from_ymd_opt(2026, 8, 4)
                .unwrap()
                .and_hms_opt(0, 0, 0)
                .unwrap(),
            true,
        );
        editor.summary = "Conference".into();
        editor.start_date = NaiveDate::from_ymd_opt(2026, 8, 4).unwrap();
        editor.end_date = NaiveDate::from_ymd_opt(2026, 8, 6).unwrap();

        let event = editor.to_event(chrono_tz::UTC).unwrap();
        assert_eq!(
            event.end,
            EventTime::Date(NaiveDate::from_ymd_opt(2026, 8, 7).unwrap()),
            "the exclusive DTEND was not derived from the last covered day"
        );
    }

    #[test]
    fn editing_an_all_day_event_shows_the_inclusive_end() {
        let mut event = Event::draft(
            "personal",
            NaiveDate::from_ymd_opt(2026, 8, 4)
                .unwrap()
                .and_hms_opt(0, 0, 0)
                .unwrap(),
            chrono_tz::UTC,
        );
        event.summary = "Conference".into();
        event.start = EventTime::Date(NaiveDate::from_ymd_opt(2026, 8, 4).unwrap());
        event.end = EventTime::Date(NaiveDate::from_ymd_opt(2026, 8, 7).unwrap());

        let editor = Editor::from_event(&event, chrono_tz::UTC);
        assert_eq!(
            editor.end_date,
            NaiveDate::from_ymd_opt(2026, 8, 6).unwrap()
        );

        // …and saving it again must not shift the stored end.
        let saved = editor.to_event(chrono_tz::UTC).unwrap();
        assert_eq!(
            saved.end, event.end,
            "round-trip moved an all-day event's end"
        );
    }

    #[test]
    fn complex_rrule_survives_an_edit() {
        let mut event = Event::draft(
            "personal",
            NaiveDate::from_ymd_opt(2026, 8, 4)
                .unwrap()
                .and_hms_opt(9, 0, 0)
                .unwrap(),
            chrono_tz::UTC,
        );
        event.summary = "Standup".into();
        event.rrule = Some("FREQ=WEEKLY;BYDAY=MO,WE,FR".into());

        let mut editor = Editor::from_event(&event, chrono_tz::UTC);
        editor.summary = "Renamed standup".into();

        let saved = editor.to_event(chrono_tz::UTC).unwrap();
        assert_eq!(
            saved.rrule.as_deref(),
            Some("FREQ=WEEKLY;BYDAY=MO,WE,FR"),
            "editing the title destroyed the recurrence rule"
        );
        assert_eq!(saved.summary, "Renamed standup");
    }

    #[test]
    fn saving_bumps_the_sequence() {
        let mut event = Event::draft(
            "personal",
            NaiveDate::from_ymd_opt(2026, 8, 4)
                .unwrap()
                .and_hms_opt(9, 0, 0)
                .unwrap(),
            chrono_tz::UTC,
        );
        event.summary = "Standup".into();
        event.sequence = 3;

        let editor = Editor::from_event(&event, chrono_tz::UTC);
        assert_eq!(editor.to_event(chrono_tz::UTC).unwrap().sequence, 4);
    }

    #[test]
    fn editing_preserves_uid_and_file_name() {
        let mut event = Event::draft(
            "personal",
            NaiveDate::from_ymd_opt(2026, 8, 4)
                .unwrap()
                .and_hms_opt(9, 0, 0)
                .unwrap(),
            chrono_tz::UTC,
        );
        event.summary = "Standup".into();

        let editor = Editor::from_event(&event, chrono_tz::UTC);
        let saved = editor.to_event(chrono_tz::UTC).unwrap();
        assert_eq!(saved.uid, event.uid);
        assert_eq!(saved.file_name, event.file_name);
    }
}
