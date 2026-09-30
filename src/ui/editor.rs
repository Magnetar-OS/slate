// SPDX-License-Identifier: GPL-3.0-only

//! The event editor, shown in the context drawer.

use super::format_time;
use crate::app::Message;
use crate::config::Config;
use crate::fl;
use crate::model::{Alarm, CalendarMeta, Event, EventTime, Freq, Recurrence, RepeatEnd};
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

/// Which zone the timezone picker is choosing for.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TzField {
    Start,
    /// The end's own zone — the flight case, where an event starts in one
    /// zone and ends in another.
    End,
}

/// Editor state. Kept as strings where the user types, so a half-typed time does
/// not have to round-trip through a parser on every keystroke.
pub struct Editor {
    /// `None` when creating; `Some` carries the file name and UID we must keep.
    pub original: Option<Event>,
    /// Set when the editor was opened from one *generated* instance of a
    /// series (not an override): the identity of that instance, which is what
    /// a "this event" edit or delete acts on. The scope prompt exists exactly
    /// when this does.
    pub occurrence: Option<chrono::DateTime<chrono::Utc>>,
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
    /// Explicit start zone; `None` means the system zone. The date and time
    /// fields show wall clock in this zone, which is what "09:00 in Tokyo"
    /// means whichever zone the viewer is in.
    pub start_tz: Option<Tz>,
    /// Explicit end zone when it differs from the start's — the flight case.
    /// `None` means "same zone as the start".
    pub end_tz: Option<Tz>,
    /// Which zone field the timezone picker is open for, if any.
    pub tz_picking: Option<TzField>,
    /// Live search text in the timezone picker.
    pub tz_query: String,
    pub picker: widget::calendar::CalendarModel,
    pub picking: Option<DateField>,
    pub error: Option<String>,
    /// The event's own reminders, as offsets from its start (negative is
    /// before). The calendar's default applies when this and
    /// `other_alarms` are both empty.
    pub alarms: Vec<chrono::Duration>,
    /// How the event's other reminders read — those set from its end or at
    /// a fixed time, which another app wrote. Shown so the event does not
    /// look reminder-less; not edited here, and every save leaves them in
    /// the file as they were.
    pub other_alarms: Vec<String>,
    /// Who is invited. Edited here; written back by `to_event`, which keeps
    /// each attendee's original line so nothing unmodelled is lost.
    pub attendees: Vec<crate::model::Attendee>,
    /// The address being typed into the add field.
    pub attendee_draft: String,
    /// What the server last said about the attendees' availability, if asked.
    pub availability: Option<AvailabilityView>,
    /// True while a free/busy request is in flight.
    pub checking_availability: bool,
}

/// The answer to "when are these people busy", shaped for display.
#[derive(Clone, Debug)]
pub enum AvailabilityView {
    /// The calendar belongs to no account, so there is no server to ask.
    NoAccount,
    /// The server runs no scheduling engine. Not the same as everyone free.
    Unsupported,
    /// The request failed; the message is the server's or the client's.
    Failed(String),
    /// One line per attendee, in the order they were asked about.
    Answers(Vec<AttendeeAvailability>),
}

/// One attendee's answer.
#[derive(Clone, Debug)]
pub struct AttendeeAvailability {
    pub email: String,
    /// `false` when the server declined to say — which must not be drawn as
    /// free, or the meeting gets booked over someone who simply would not
    /// answer.
    pub answered: bool,
    /// Whether any busy period overlaps the slot that was asked about.
    pub busy: bool,
}

/// The zone an [`EventTime`] carries explicitly, when it is worth showing.
///
/// The system zone and UTC stamps resolve to `None`: both display and save as
/// "the viewer's time", which is also what every event this editor creates
/// starts as. Only a genuinely foreign `TZID` is an explicit choice to keep.
fn explicit_zone(time: EventTime, local: Tz) -> Option<Tz> {
    match time {
        EventTime::Zoned(_, tz) if tz != local && tz != chrono_tz::UTC => Some(tz),
        _ => None,
    }
}

/// The time to save for `wall`, a wall clock the editor showed in `picked`
/// (a zone the user chose) or, without one, in the viewer's `local` zone.
///
/// A chosen zone is written as that zone. Otherwise the value keeps the form
/// it had: a floating time ("09:00 wherever I am") stays floating, and a UTC
/// stamp stays UTC — at the same instant the fields showed in local time.
/// Rewriting either as `TZID=local` would change what the event means to
/// every other client, over a title edit.
fn in_original_form(
    wall: NaiveDateTime,
    picked: Option<Tz>,
    original: Option<EventTime>,
    local: Tz,
) -> EventTime {
    if let Some(zone) = picked {
        return EventTime::Zoned(wall, zone);
    }
    match original {
        Some(EventTime::Floating(_)) => EventTime::Floating(wall),
        Some(EventTime::Zoned(_, zone)) if zone == chrono_tz::UTC => EventTime::Zoned(
            EventTime::Zoned(wall, local).to_utc(local).naive_utc(),
            chrono_tz::UTC,
        ),
        _ => EventTime::Zoned(wall, local),
    }
}

/// The reminders the editor offers, as offsets from the event's start.
///
/// The presets the common clients share, up to Outlook's longest; anything
/// else an event already carries is kept and shown as it is.
pub const ALARM_PRESETS: &[chrono::Duration] = &[
    chrono::Duration::zero(),
    chrono::Duration::minutes(-5),
    chrono::Duration::minutes(-10),
    chrono::Duration::minutes(-15),
    chrono::Duration::minutes(-30),
    chrono::Duration::hours(-1),
    chrono::Duration::hours(-2),
    chrono::Duration::days(-1),
    chrono::Duration::days(-2),
    chrono::Duration::weeks(-1),
];

/// "10 minutes before", "1 day before", "At the start".
#[must_use]
pub fn alarm_label(offset: chrono::Duration) -> String {
    let lead = -offset;
    if lead == chrono::Duration::zero() {
        fl!("reminder-at-start")
    } else if lead < chrono::Duration::zero() {
        fl!("reminder-after-start", minutes = (-lead).num_minutes())
    } else if lead.num_seconds() % 86_400 == 0 {
        fl!("reminder-days", days = lead.num_days())
    } else if lead.num_seconds() % 3_600 == 0 {
        fl!("reminder-hours", hours = lead.num_hours())
    } else {
        fl!("reminder-minutes", minutes = lead.num_minutes())
    }
}

/// "5 minutes before the end", "On Thu 6 Aug at 09:00" — a reminder the
/// editor shows but does not author. `None` for one set from the start,
/// which [`alarm_label`] words and the editor edits.
#[must_use]
pub fn other_alarm_label(alarm: Alarm, local: Tz, config: &Config) -> Option<String> {
    match alarm {
        Alarm::Start(_) => None,
        Alarm::End(offset) if offset == chrono::Duration::zero() => Some(fl!("reminder-at-end")),
        Alarm::End(offset) if offset < chrono::Duration::zero() => {
            Some(fl!("reminder-before-end", duration = span_label(-offset)))
        }
        Alarm::End(offset) => Some(fl!("reminder-after-end", duration = span_label(offset))),
        Alarm::At(instant) => {
            let wall = instant.with_timezone(&local).naive_local();
            Some(fl!(
                "reminder-on",
                day = super::format_day(wall.date()),
                time = format_time(wall.time(), config)
            ))
        }
    }
}

/// "2 days", "3 hours", "15 minutes": the largest whole unit.
fn span_label(span: chrono::Duration) -> String {
    if span.num_seconds() % 86_400 == 0 {
        fl!("span-days", days = span.num_days())
    } else if span.num_seconds() % 3_600 == 0 {
        fl!("span-hours", hours = span.num_hours())
    } else {
        fl!("span-minutes", minutes = span.num_minutes())
    }
}

/// Wall clock of `time` in `zone`. All-day and floating values are already
/// wall clock; zoned ones convert through their instant.
fn wall_in(time: EventTime, zone: Tz, local: Tz) -> NaiveDateTime {
    match time {
        EventTime::Date(_) | EventTime::Floating(_) => time.naive_local(local),
        EventTime::Zoned(..) => time.to_utc(local).with_timezone(&zone).naive_local(),
    }
}

impl Editor {
    /// A blank event at `start`, lasting `duration` — which the calendar it
    /// lands in may have its own opinion about.
    #[must_use]
    pub fn new(
        calendar_id: String,
        start: NaiveDateTime,
        all_day: bool,
        duration: chrono::Duration,
    ) -> Self {
        let end = start + duration;
        Self {
            occurrence: None,
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
            start_tz: None,
            end_tz: None,
            tz_picking: None,
            tz_query: String::new(),
            picker: to_picker(start.date()),
            picking: None,
            error: None,
            alarms: Vec::new(),
            other_alarms: Vec::new(),
            attendees: Vec::new(),
            attendee_draft: String::new(),
            availability: None,
            checking_availability: false,
        }
    }

    /// Opens one generated instance of a series: the fields show the
    /// instance's own dates and times, not the series' first ones, and the
    /// instance identity is kept so saving can ask "this event or all events?".
    #[must_use]
    pub fn from_series_occurrence(
        master: &Event,
        instant: chrono::DateTime<chrono::Utc>,
        local: Tz,
    ) -> Self {
        let mut editor = Self::from_event(master, local);
        editor.occurrence = Some(instant);

        // Place the instance in the zone the fields display in — the series'
        // own zone for a foreign-zone master, the viewer's otherwise (which is
        // also where floating and all-day instants were resolved).
        let start_zone = editor.start_tz.unwrap_or(local);
        let end_zone = editor.end_tz.unwrap_or(start_zone);
        let start = instant.with_timezone(&start_zone).naive_local();
        let duration = master.duration(local);
        let end = (instant + duration).with_timezone(&end_zone).naive_local();

        editor.start_date = start.date();
        editor.start_time = format!("{:02}:{:02}", start.hour(), start.minute());
        editor.end_date = if editor.all_day {
            // DTEND is exclusive; show the last covered day.
            end.date() - chrono::Duration::days(1)
        } else {
            end.date()
        };
        editor.end_time = format!("{:02}:{:02}", end.hour(), end.minute());
        editor
    }

    /// Loads an existing event for editing.
    #[must_use]
    pub fn from_event(event: &Event, local: Tz) -> Self {
        // A foreign TZID is kept and displayed in its own zone — "09:00 in
        // Tokyo" shows 09:00 — rather than being flattened to the viewer's.
        let start_tz = explicit_zone(event.start, local);
        let start_zone = start_tz.unwrap_or(local);
        let end_zone_actual = match event.end {
            EventTime::Zoned(_, tz) if tz != chrono_tz::UTC => tz,
            _ => local,
        };
        let end_tz = (end_zone_actual != start_zone).then_some(end_zone_actual);

        let start = wall_in(event.start, start_zone, local);
        let end = wall_in(event.end, end_tz.unwrap_or(start_zone), local);

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
            occurrence: None,
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
            start_tz,
            end_tz,
            tz_picking: None,
            tz_query: String::new(),
            picker: to_picker(start.date()),
            picking: None,
            error: None,
            alarms: event.alarms.clone(),
            other_alarms: Vec::new(),
            attendees: event.attendees.clone(),
            attendee_draft: String::new(),
            availability: None,
            checking_availability: false,
            original: Some(event.clone()),
        }
    }

    /// Shows `alarms` — every alarm the event's file holds — beside the
    /// start-relative ones the editor edits.
    #[must_use]
    pub fn showing_alarms(mut self, alarms: &[Alarm], local: Tz, config: &Config) -> Self {
        self.other_alarms = alarms
            .iter()
            .filter_map(|alarm| other_alarm_label(*alarm, local, config))
            .collect();
        self
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

            let original = self.original.as_ref();
            let start = in_original_form(
                self.start_date.and_time(start_time),
                self.start_tz,
                original.map(|event| event.start),
                local,
            );
            let end = in_original_form(
                self.end_date.and_time(end_time),
                self.end_tz.or(self.start_tz),
                original.map(|event| event.end),
                local,
            );

            // Compared as instants, not wall clocks: a flight can land at an
            // earlier wall-clock time than it took off.
            if end.to_utc(local) <= start.to_utc(local) {
                return Err(fl!("error-invalid-time-range"));
            }

            (start, end)
        };

        let recurrence = Recurrence {
            freq: self.freq,
            interval: self.interval.parse().unwrap_or(1).max(1),
            end: self.resolved_repeat_end(),
        };
        let rrule = if self.is_override() {
            // An override describes one instance of somebody else's series; a
            // rule on it would fork a second series under the same UID.
            None
        } else if let Some(raw) = &self.custom_rrule {
            // Never rewrite a rule we could not fully parse.
            Some(raw.clone())
        } else if let Some(original) = self.original.as_ref().and_then(|e| e.rrule.as_deref()) {
            // Patched over the rule the series has: what the user did not
            // change keeps its spelling, and what the fields do not model
            // (`WKST`, the order of the parts) is kept, so an untouched rule
            // is written back byte for byte. `UNTIL` is written in the
            // series' own terms, so it needs the start the series is saved
            // with.
            recurrence.to_rrule_keeping(start, original)
        } else {
            recurrence.to_rrule(start)
        };

        let mut event = match &self.original {
            Some(original) => {
                let mut event = original.clone();
                // Bump SEQUENCE so CalDAV servers and other clients see a newer
                // revision — unless the event has an ORGANIZER. SEQUENCE is the
                // organizer's revision counter (RFC 5546), and Slate never sends
                // REQUESTs, so on a scheduled event it belongs to the organizer's
                // client: bumping it on this copy would make the organizer's next
                // update or CANCEL compare as stale in `itip::apply`.
                if event.organizer.is_none() {
                    event.sequence = event.sequence.saturating_add(1);
                }
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
        event.attendees = self.attendees.clone();
        event.alarms.clone_from(&self.alarms);
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
            .push(self.reminders_section())
            .push(self.attendees_section())
            .push(self.actions());

        widget::scrollable(column).into()
    }

    fn details_section<'a>(&'a self, calendars: &'a [CalendarMeta]) -> Element<'a, Message> {
        // Read-only calendars (ICS feeds, unwritable directories) are not
        // valid destinations; the index space here must match the handler's,
        // which filters the same way.
        let writable: Vec<&CalendarMeta> = calendars.iter().filter(|c| !c.read_only).collect();
        let names: Vec<String> = writable.iter().map(|c| c.name.clone()).collect();
        let selected = writable.iter().position(|c| c.id == self.calendar_id);

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

        // Timezone rows: meaningless for all-day events, which are dates and
        // never shift.
        if !self.all_day {
            section = section.add(
                widget::settings::item::builder(fl!("time-zone")).control(
                    widget::button::standard(zone_label(self.start_tz))
                        .on_press(Message::EditorTzToggle(TzField::Start)),
                ),
            );

            // The end's own zone appears only once it exists (or is being
            // chosen) — the flight case is rare and should not cost everyone
            // else a row.
            if self.end_tz.is_some() || self.tz_picking == Some(TzField::End) {
                section =
                    section.add(
                        widget::settings::item::builder(fl!("time-zone-ends-in")).control(
                            widget::button::standard(self.end_tz.map_or_else(
                                || fl!("time-zone-same-as-start"),
                                |tz| tz.to_string(),
                            ))
                            .on_press(Message::EditorTzToggle(TzField::End)),
                        ),
                    );
            }

            if let Some(field) = self.tz_picking {
                section = section.add(self.tz_picker(field));
            }
        }

        section.into()
    }

    /// The timezone picker: a search box over the IANA names, the first few
    /// matches as buttons, and the way back to the default.
    fn tz_picker(&self, field: TzField) -> Element<'_, Message> {
        let spacing = cosmic::theme::spacing();

        let mut column = widget::column::with_capacity(4)
            .spacing(spacing.space_xxs)
            .push(
                widget::text_input(fl!("time-zone-search-hint"), &self.tz_query)
                    .on_input(Message::EditorTzQuery),
            );

        if !self.tz_query.trim().is_empty() {
            let query = self.tz_query.trim().to_ascii_lowercase();
            let mut matches = chrono_tz::TZ_VARIANTS
                .iter()
                .filter(|tz| tz.name().to_ascii_lowercase().contains(&query))
                .take(6)
                .peekable();

            if matches.peek().is_none() {
                column = column.push(widget::text::caption(fl!("time-zone-no-match")));
            }
            for tz in matches {
                column = column.push(
                    widget::button::text(tz.name().to_owned())
                        .on_press(Message::EditorTzChosen(tz.name().to_owned())),
                );
            }
        }

        let clear_label = match field {
            TzField::Start => fl!("time-zone-system"),
            TzField::End => fl!("time-zone-same-as-start"),
        };
        column = column.push(widget::button::text(clear_label).on_press(Message::EditorTzClear));

        if field == TzField::Start && self.end_tz.is_none() {
            column = column.push(
                widget::button::text(fl!("time-zone-different-end"))
                    .on_press(Message::EditorTzToggle(TzField::End)),
            );
        }

        widget::container(column).padding(spacing.space_xxs).into()
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

    /// Whether the editor is open on a `RECURRENCE-ID` override — a single
    /// modified instance of a series, rather than the series itself.
    #[must_use]
    pub fn is_override(&self) -> bool {
        self.original
            .as_ref()
            .is_some_and(|event| event.recurrence_id.is_some())
    }

    /// Adds the preset at `index` in [`ALARM_PRESETS`], once.
    pub fn add_alarm(&mut self, index: usize) {
        if let Some(offset) = ALARM_PRESETS.get(index).copied()
            && !self.alarms.contains(&offset)
        {
            self.alarms.push(offset);
            // Earliest first, the order they will fire in.
            self.alarms.sort();
        }
    }

    pub fn remove_alarm(&mut self, index: usize) {
        if index < self.alarms.len() {
            self.alarms.remove(index);
        }
    }

    fn reminders_section(&self) -> Element<'_, Message> {
        let mut section = widget::settings::section().title(fl!("event-reminders"));
        if self.alarms.is_empty() && self.other_alarms.is_empty() {
            section = section.add(widget::text::caption(fl!("event-reminders-default")));
        }
        for label in &self.other_alarms {
            section = section.add(
                widget::settings::item::builder(label.clone())
                    .control(widget::text::caption(fl!("event-reminder-kept"))),
            );
        }
        for (index, offset) in self.alarms.iter().enumerate() {
            section = section.add(
                widget::settings::item::builder(alarm_label(*offset)).control(
                    widget::button::text(fl!("remove")).on_press(Message::EditorAlarmRemove(index)),
                ),
            );
        }
        let presets: Vec<String> = ALARM_PRESETS.iter().map(|o| alarm_label(*o)).collect();
        section
            .add(
                widget::settings::item::builder(fl!("event-reminder-add")).control(
                    widget::dropdown(presets, None, Message::EditorAlarmAdd)
                        .width(Length::Fixed(220.0)),
                ),
            )
            .into()
    }

    fn repeat_section(&self) -> Element<'_, Message> {
        let spacing = cosmic::theme::spacing();

        // One instance of a series: the recurrence belongs to the master, so
        // showing dropdowns here would offer an edit that cannot mean anything.
        if self.is_override() {
            return widget::settings::section()
                .add(
                    widget::settings::item::builder(fl!("repeats"))
                        .control(widget::text::caption(fl!("modified-occurrence"))),
                )
                .into();
        }

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

    /// Who is invited, and — when the calendar's server can answer — whether
    /// they are free when this event is.
    fn attendees_section(&self) -> Element<'_, Message> {
        let spacing = cosmic::theme::spacing();
        let mut section = widget::settings::section().title(fl!("attendees"));

        for (index, attendee) in self.attendees.iter().enumerate() {
            let status = self.availability_for(&attendee.email);
            let mut row = widget::settings::item::builder(attendee.display().to_owned());
            // The address is worth showing under a display name, since that is
            // what the server was actually asked about.
            if attendee.name.is_some() {
                row = row.description(attendee.email.clone());
            }
            if let Some(status) = status {
                row = row.description(status);
            }
            section = section.add(
                row.control(
                    widget::button::icon(widget::icon::from_name("list-remove-symbolic"))
                        .on_press(Message::EditorAttendeeRemove(index)),
                ),
            );
        }

        if self.attendees.is_empty() {
            section = section.add(
                widget::settings::item::builder(fl!("no-attendees")).control(widget::Space::new()),
            );
        }

        section = section.add(
            widget::settings::item::builder(fl!("attendee-add")).control(
                widget::text_input("name@example.com", &self.attendee_draft)
                    .on_input(Message::EditorAttendeeDraft)
                    .on_submit(|_| Message::EditorAttendeeAdd)
                    .width(Length::Fixed(220.0)),
            ),
        );

        let mut column = widget::column::with_capacity(3)
            .spacing(spacing.space_xxs)
            .push(section);

        if !self.attendees.is_empty() {
            let label = if self.checking_availability {
                fl!("checking-availability")
            } else {
                fl!("check-availability")
            };
            let button = widget::button::standard(label);
            column = column.push(if self.checking_availability {
                button
            } else {
                button.on_press(Message::EditorCheckAvailability)
            });
        }

        // The two answers that are not per-attendee: no server to ask, or a
        // server that does not do scheduling. Both must read as "unknown",
        // never as "everyone is free".
        let note = match &self.availability {
            Some(AvailabilityView::NoAccount) => Some(fl!("availability-no-account")),
            Some(AvailabilityView::Unsupported) => Some(fl!("availability-unsupported")),
            Some(AvailabilityView::Failed(why)) => {
                Some(fl!("availability-failed", why = why.clone()))
            }
            _ => None,
        };
        if let Some(note) = note {
            column = column.push(
                widget::text::caption(note).wrapping(cosmic::iced::core::text::Wrapping::Word),
            );
        }

        column.into()
    }

    /// One attendee's availability, as a line to sit under their name.
    fn availability_for(&self, email: &str) -> Option<String> {
        let Some(AvailabilityView::Answers(answers)) = &self.availability else {
            return None;
        };
        let answer = answers.iter().find(|a| a.email == email)?;
        Some(if !answer.answered {
            // The server was asked and would not say. Reporting this as free
            // is how meetings get booked over people.
            fl!("availability-unknown")
        } else if answer.busy {
            fl!("availability-busy")
        } else {
            fl!("availability-free")
        })
    }

    fn actions(&self) -> Element<'_, Message> {
        let spacing = cosmic::theme::spacing();

        let mut row = widget::row::with_capacity(4)
            .spacing(spacing.space_xxs)
            .push(widget::button::suggested(fl!("save")).on_press(Message::EditorSave))
            .push(widget::button::standard(fl!("cancel")).on_press(Message::EditorCancel));

        if let Some(url) =
            crate::meeting::meeting_link(Some(&self.location), Some(&self.description)).or_else(
                || {
                    self.original
                        .as_ref()
                        .and_then(|event| crate::meeting::conference_link(&event.other))
                },
            )
        {
            row =
                row.push(widget::button::text(fl!("join-call")).on_press(Message::LaunchUrl(url)));
        }

        if !self.is_new() {
            row = row
                .push(widget::Space::new().width(Length::Fill))
                .push(widget::button::destructive(fl!("delete")).on_press(Message::EditorDelete));
        }

        row.width(Length::Fill).into()
    }
}

/// The label on the timezone button: the zone's IANA name, or the default.
fn zone_label(tz: Option<Tz>) -> String {
    tz.map_or_else(|| fl!("time-zone-system"), |tz| tz.to_string())
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

    fn with_availability(answers: Vec<AttendeeAvailability>) -> Editor {
        let mut editor = Editor::new(
            "personal".into(),
            NaiveDate::from_ymd_opt(2026, 8, 4)
                .unwrap()
                .and_hms_opt(9, 0, 0)
                .unwrap(),
            false,
            chrono::Duration::hours(1),
        );
        editor.availability = Some(AvailabilityView::Answers(answers));
        editor
    }

    #[test]
    fn a_server_that_would_not_say_is_never_shown_as_free() {
        // The whole reason `answered` exists: an unanswered attendee comes
        // back with an empty busy list, and calling that free books the
        // meeting over someone who simply did not reply.
        let editor = with_availability(vec![AttendeeAvailability {
            email: "bob@example.com".into(),
            answered: false,
            busy: false,
        }]);
        let shown = editor.availability_for("bob@example.com").unwrap();
        assert_ne!(shown, fl!("availability-free"));
        assert_eq!(shown, fl!("availability-unknown"));
    }

    #[test]
    fn an_answered_attendee_reads_free_or_busy() {
        let editor = with_availability(vec![
            AttendeeAvailability {
                email: "free@example.com".into(),
                answered: true,
                busy: false,
            },
            AttendeeAvailability {
                email: "busy@example.com".into(),
                answered: true,
                busy: true,
            },
        ]);
        assert_eq!(
            editor.availability_for("free@example.com").unwrap(),
            fl!("availability-free")
        );
        assert_eq!(
            editor.availability_for("busy@example.com").unwrap(),
            fl!("availability-busy")
        );
    }

    #[test]
    fn an_attendee_nobody_asked_about_shows_nothing() {
        let editor = with_availability(Vec::new());
        assert!(editor.availability_for("stranger@example.com").is_none());
    }

    #[test]
    fn attendees_survive_the_editor_round_trip() {
        let mut event = Event::draft(
            "personal",
            NaiveDate::from_ymd_opt(2026, 8, 4)
                .unwrap()
                .and_hms_opt(9, 0, 0)
                .unwrap(),
            chrono_tz::UTC,
        );
        event.summary = "Planning".into();
        event.attendees = vec![crate::model::Attendee {
            email: "bob@example.com".into(),
            name: Some("Bob".into()),
            partstat: Some("ACCEPTED".into()),
            raw: Some("ATTENDEE;CN=Bob;PARTSTAT=ACCEPTED:mailto:bob@example.com".into()),
        }];

        let mut editor = Editor::from_event(&event, chrono_tz::UTC);
        assert_eq!(editor.attendees.len(), 1);
        editor.summary = "Planning (renamed)".into();

        let saved = editor.to_event(chrono_tz::UTC).unwrap();
        assert_eq!(saved.attendees.len(), 1, "the editor dropped the attendee");
        // The source line rides along, so PARTSTAT and any unmodelled
        // parameter survive an edit made here.
        assert_eq!(
            saved.attendees[0].raw.as_deref(),
            Some("ATTENDEE;CN=Bob;PARTSTAT=ACCEPTED:mailto:bob@example.com")
        );
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
            chrono::Duration::hours(1),
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
            chrono::Duration::hours(1),
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
            chrono::Duration::hours(1),
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
    fn reminders_are_edited_and_saved() {
        let athens = chrono_tz::Europe::Athens;
        let nine = NaiveDate::from_ymd_opt(2026, 8, 4)
            .unwrap()
            .and_hms_opt(9, 0, 0)
            .unwrap();
        let mut event = Event::draft("personal", nine, athens);
        event.summary = "Dentist".into();
        event.alarms = vec![chrono::Duration::minutes(-10)];

        let mut editor = Editor::from_event(&event, athens);
        // A day before, then the same again, which is not added twice.
        let day = ALARM_PRESETS
            .iter()
            .position(|o| *o == chrono::Duration::days(-1))
            .unwrap();
        editor.add_alarm(day);
        editor.add_alarm(day);
        assert_eq!(
            editor.to_event(athens).unwrap().alarms,
            vec![chrono::Duration::days(-1), chrono::Duration::minutes(-10)]
        );

        editor.remove_alarm(1);
        assert_eq!(
            editor.to_event(athens).unwrap().alarms,
            vec![chrono::Duration::days(-1)]
        );
    }

    #[test]
    fn a_reminder_edit_on_a_saved_event_reaches_its_file() {
        // The save patches an existing file property by property, and a
        // reminder is a nested VALARM: before cosmic-pim 2.1 the edit was
        // accepted here and then dropped on the way to disk.
        let dir = tempfile::tempdir().unwrap();
        let mut store = crate::store::Store::open(
            &dir.path().join("calendars"),
            &dir.path().join("index.sqlite"),
        )
        .unwrap();
        let calendar = store
            .create_calendar("Personal", crate::model::PALETTE[0])
            .unwrap();
        let local = store.local_timezone();
        let nine = NaiveDate::from_ymd_opt(2026, 8, 4)
            .unwrap()
            .and_hms_opt(9, 0, 0)
            .unwrap();
        let mut event = Event::draft(&calendar.id, nine, local);
        event.summary = "Dentist".into();
        event.alarms = vec![chrono::Duration::minutes(-10)];
        store.save(&event).unwrap();

        let saved = store.event(&calendar.id, &event.uid).unwrap().unwrap();
        let mut editor = Editor::from_event(&saved, local);
        editor.add_alarm(
            ALARM_PRESETS
                .iter()
                .position(|o| *o == chrono::Duration::days(-1))
                .unwrap(),
        );
        editor.remove_alarm(1);
        store.save(&editor.to_event(local).unwrap()).unwrap();

        assert_eq!(
            store
                .event(&calendar.id, &event.uid)
                .unwrap()
                .unwrap()
                .alarms,
            vec![chrono::Duration::days(-1)],
            "the reminder edit was not written"
        );
    }

    #[test]
    fn reminders_from_the_end_or_at_a_time_are_shown_and_kept() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = crate::store::Store::open(
            &dir.path().join("calendars"),
            &dir.path().join("index.sqlite"),
        )
        .unwrap();
        let calendar = store
            .create_calendar("Personal", crate::model::PALETTE[0])
            .unwrap();
        // Written by another app: one alarm of each kind.
        std::fs::write(
            calendar.path.join("exam.ics"),
            "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Other//EN\r\n\
             BEGIN:VEVENT\r\nUID:exam\r\nDTSTAMP:20260801T000000Z\r\n\
             DTSTART:20260804T090000Z\r\nDTEND:20260804T100000Z\r\nSUMMARY:Exam\r\n\
             BEGIN:VALARM\r\nACTION:DISPLAY\r\nDESCRIPTION:Exam\r\n\
             TRIGGER:-PT10M\r\nEND:VALARM\r\n\
             BEGIN:VALARM\r\nACTION:DISPLAY\r\nDESCRIPTION:Exam\r\n\
             TRIGGER;RELATED=END:-PT15M\r\nEND:VALARM\r\n\
             BEGIN:VALARM\r\nACTION:AUDIO\r\n\
             TRIGGER;VALUE=DATE-TIME:20260803T170000Z\r\nEND:VALARM\r\n\
             END:VEVENT\r\nEND:VCALENDAR\r\n",
        )
        .unwrap();
        store.refresh().unwrap();
        let event = store.event(&calendar.id, "exam").unwrap().unwrap();
        let alarms = store.alarms(&event).unwrap();

        let config = Config::default();
        let mut editor = Editor::from_event(&event, chrono_tz::UTC).showing_alarms(
            &alarms,
            chrono_tz::UTC,
            &config,
        );
        assert_eq!(editor.alarms, vec![chrono::Duration::minutes(-10)]);
        assert_eq!(
            editor.other_alarms,
            vec![
                fl!(
                    "reminder-before-end",
                    duration = fl!("span-minutes", minutes = 15)
                ),
                fl!(
                    "reminder-on",
                    day = super::super::format_day(NaiveDate::from_ymd_opt(2026, 8, 3).unwrap()),
                    time = format_time(NaiveTime::from_hms_opt(17, 0, 0).unwrap(), &config)
                ),
            ]
        );

        // Editing the ones the editor owns leaves the others as written.
        editor.remove_alarm(0);
        editor.summary = "Final exam".into();
        store
            .save(&editor.to_event(chrono_tz::UTC).unwrap())
            .unwrap();
        let saved = store.event(&calendar.id, "exam").unwrap().unwrap();
        assert_eq!(saved.summary, "Final exam");
        assert_eq!(
            store.alarms(&saved).unwrap(),
            alarms[1..],
            "a reminder the editor does not author was lost"
        );
    }

    #[test]
    fn a_floating_time_stays_floating() {
        let athens = chrono_tz::Europe::Athens;
        let nine = NaiveDate::from_ymd_opt(2026, 8, 4)
            .unwrap()
            .and_hms_opt(9, 0, 0)
            .unwrap();
        let mut event = Event::draft("personal", nine, athens);
        event.summary = "Stretch".into();
        event.start = EventTime::Floating(nine);
        event.end = EventTime::Floating(nine + chrono::Duration::minutes(15));

        let mut editor = Editor::from_event(&event, athens);
        editor.summary = "Stretch properly".into();
        let saved = editor.to_event(athens).unwrap();

        assert_eq!(saved.start, EventTime::Floating(nine));
        assert_eq!(
            saved.end,
            EventTime::Floating(nine + chrono::Duration::minutes(15))
        );
    }

    #[test]
    fn a_utc_time_stays_utc_at_the_same_instant() {
        let athens = chrono_tz::Europe::Athens;
        let six_utc = NaiveDate::from_ymd_opt(2026, 8, 4)
            .unwrap()
            .and_hms_opt(6, 0, 0)
            .unwrap();
        let mut event = Event::draft("personal", six_utc, athens);
        event.summary = "Call".into();
        event.start = EventTime::Zoned(six_utc, chrono_tz::UTC);
        event.end = EventTime::Zoned(six_utc + chrono::Duration::hours(1), chrono_tz::UTC);

        // Shown as 09:00 in Athens; only the title changes.
        let mut editor = Editor::from_event(&event, athens);
        assert_eq!(editor.start_time, "09:00");
        editor.summary = "Call with the team".into();
        let saved = editor.to_event(athens).unwrap();

        assert_eq!(saved.start, EventTime::Zoned(six_utc, chrono_tz::UTC));
        assert_eq!(
            saved.end,
            EventTime::Zoned(six_utc + chrono::Duration::hours(1), chrono_tz::UTC)
        );
    }

    #[test]
    fn an_untouched_simple_rule_is_written_back_verbatim() {
        // The end of 30 September in Los Angeles, as a client there writes
        // it. The editor reads this as "ends 1 October" (the UTC date), and
        // re-rendering it would widen the bound to 1 Oct 23:59:59Z — one more
        // 09:00 PDT instance than the series had.
        let mut event = Event::draft(
            "personal",
            NaiveDate::from_ymd_opt(2026, 9, 1)
                .unwrap()
                .and_hms_opt(16, 0, 0)
                .unwrap(),
            chrono_tz::UTC,
        );
        event.summary = "Standup".into();
        event.rrule = Some("FREQ=DAILY;WKST=MO;UNTIL=20261001T065959Z".into());

        let mut editor = Editor::from_event(&event, chrono_tz::UTC);
        editor.summary = "Renamed standup".into();

        let saved = editor.to_event(chrono_tz::UTC).unwrap();
        assert_eq!(
            saved.rrule.as_deref(),
            Some("FREQ=DAILY;WKST=MO;UNTIL=20261001T065959Z"),
            "a title edit rewrote the recurrence rule"
        );
    }

    #[test]
    fn a_changed_simple_rule_takes_the_fields() {
        let mut event = Event::draft(
            "personal",
            NaiveDate::from_ymd_opt(2026, 9, 1)
                .unwrap()
                .and_hms_opt(9, 0, 0)
                .unwrap(),
            chrono_tz::UTC,
        );
        event.summary = "Standup".into();
        event.rrule = Some("FREQ=DAILY;COUNT=5".into());

        let mut editor = Editor::from_event(&event, chrono_tz::UTC);
        editor.interval = "2".into();

        // The new part goes after the ones the rule had, which keep their
        // order.
        let saved = editor.to_event(chrono_tz::UTC).unwrap();
        assert_eq!(
            saved.rrule.as_deref(),
            Some("FREQ=DAILY;COUNT=5;INTERVAL=2")
        );
    }

    #[test]
    fn a_changed_rule_keeps_what_the_fields_do_not_show() {
        // The same Los Angeles rule as above, now every other day: WKST and
        // the UTC spelling of the unchanged UNTIL stay as the other client
        // wrote them. Rendering from the fields dropped WKST and re-spelled
        // UNTIL a day wider (audit F-04).
        let mut event = Event::draft(
            "personal",
            NaiveDate::from_ymd_opt(2026, 9, 1)
                .unwrap()
                .and_hms_opt(16, 0, 0)
                .unwrap(),
            chrono_tz::UTC,
        );
        event.summary = "Standup".into();
        event.rrule = Some("FREQ=DAILY;WKST=MO;UNTIL=20261001T065959Z".into());

        let mut editor = Editor::from_event(&event, chrono_tz::UTC);
        editor.interval = "2".into();

        let saved = editor.to_event(chrono_tz::UTC).unwrap();
        assert_eq!(
            saved.rrule.as_deref(),
            Some("FREQ=DAILY;WKST=MO;UNTIL=20261001T065959Z;INTERVAL=2")
        );
    }

    #[test]
    fn an_invitation_keeps_the_organizers_sequence() {
        // SEQUENCE is the organizer's revision. Bumping it on the attendee's
        // copy makes the organizer's next update or CANCEL compare as stale
        // in `itip::apply`, and it is silently dropped.
        let mut event = Event::draft(
            "personal",
            NaiveDate::from_ymd_opt(2026, 8, 4)
                .unwrap()
                .and_hms_opt(9, 0, 0)
                .unwrap(),
            chrono_tz::UTC,
        );
        event.summary = "Quarterly review".into();
        event.sequence = 2;
        event.organizer = Some(crate::model::Attendee::new("boss@example.com", None));

        let mut editor = Editor::from_event(&event, chrono_tz::UTC);
        editor.description = "bring the numbers".into();

        assert_eq!(editor.to_event(chrono_tz::UTC).unwrap().sequence, 2);
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
    fn opening_a_series_instance_shows_its_own_dates() {
        let mut master = Event::draft(
            "personal",
            NaiveDate::from_ymd_opt(2026, 8, 4)
                .unwrap()
                .and_hms_opt(9, 0, 0)
                .unwrap(),
            chrono_tz::UTC,
        );
        master.summary = "Standup".into();
        master.rrule = Some("FREQ=WEEKLY".into());

        // The user clicked the 18 Aug instance.
        let instant = NaiveDate::from_ymd_opt(2026, 8, 18)
            .unwrap()
            .and_hms_opt(9, 0, 0)
            .unwrap()
            .and_utc();
        let editor = Editor::from_series_occurrence(&master, instant, chrono_tz::UTC);

        assert_eq!(
            editor.start_date,
            NaiveDate::from_ymd_opt(2026, 8, 18).unwrap(),
            "the editor must show the clicked instance, not the series start"
        );
        assert_eq!(editor.start_time, "09:00");
        assert_eq!(editor.occurrence, Some(instant));

        // And what it builds carries the instance's dates — which is exactly
        // what a "this event" save turns into an override.
        let event = editor.to_event(chrono_tz::UTC).unwrap();
        assert_eq!(
            event.start.naive_local(chrono_tz::UTC).date(),
            NaiveDate::from_ymd_opt(2026, 8, 18).unwrap()
        );
        assert_eq!(event.uid, master.uid, "identity is the master's");
    }

    #[test]
    fn an_override_never_grows_a_rule() {
        let mut over = Event::draft(
            "personal",
            NaiveDate::from_ymd_opt(2026, 8, 18)
                .unwrap()
                .and_hms_opt(14, 0, 0)
                .unwrap(),
            chrono_tz::UTC,
        );
        over.summary = "Moved".into();
        over.recurrence_id = Some(crate::model::EventTime::Zoned(
            NaiveDate::from_ymd_opt(2026, 8, 18)
                .unwrap()
                .and_hms_opt(9, 0, 0)
                .unwrap(),
            chrono_tz::UTC,
        ));

        let mut editor = Editor::from_event(&over, chrono_tz::UTC);
        assert!(editor.is_override());

        // Even if the recurrence state somehow says "weekly", the built event
        // must not carry a rule: a rule on an override forks the series.
        editor.freq = Freq::Weekly;
        let event = editor.to_event(chrono_tz::UTC).unwrap();
        assert_eq!(event.rrule, None);
        assert!(event.recurrence_id.is_some());
    }

    #[test]
    fn a_foreign_zone_is_preserved_and_shown_in_its_own_wall_clock() {
        let athens = chrono_tz::Europe::Athens;
        let tokyo = chrono_tz::Asia::Tokyo;
        let nine = NaiveDate::from_ymd_opt(2026, 8, 4)
            .unwrap()
            .and_hms_opt(9, 0, 0)
            .unwrap();

        let mut event = Event::draft("personal", nine, athens);
        event.summary = "Tokyo call".into();
        event.start = EventTime::Zoned(nine, tokyo);
        event.end = EventTime::Zoned(nine + chrono::Duration::hours(1), tokyo);

        let editor = Editor::from_event(&event, athens);
        assert_eq!(editor.start_tz, Some(tokyo));
        assert_eq!(editor.end_tz, None, "same zone at both ends");
        assert_eq!(
            editor.start_time, "09:00",
            "displayed in the event's own zone, not flattened to the viewer's"
        );

        let saved = editor.to_event(athens).unwrap();
        assert_eq!(saved.start, EventTime::Zoned(nine, tokyo));
        assert_eq!(
            saved.end,
            EventTime::Zoned(nine + chrono::Duration::hours(1), tokyo)
        );
    }

    #[test]
    fn the_flight_case_keeps_two_zones_and_validates_by_instant() {
        let athens = chrono_tz::Europe::Athens;
        let los_angeles = chrono_tz::America::Los_Angeles;

        let mut editor = Editor::new(
            "personal".into(),
            NaiveDate::from_ymd_opt(2026, 8, 4)
                .unwrap()
                .and_hms_opt(10, 0, 0)
                .unwrap(),
            false,
            chrono::Duration::hours(1),
        );
        editor.summary = "ATH → LAX".into();
        editor.start_tz = Some(athens);
        editor.end_tz = Some(los_angeles);
        editor.start_time = "10:00".into();
        editor.end_time = "13:00".into();

        // Lands the same calendar day at an earlier-looking hour difference —
        // valid, because the instants are 16 hours apart.
        let event = editor.to_event(athens).unwrap();
        assert!(matches!(event.start, EventTime::Zoned(_, tz) if tz == athens));
        assert!(matches!(event.end, EventTime::Zoned(_, tz) if tz == los_angeles));

        // 00:00 in Los Angeles is 07:00Z — the same instant as the 10:00
        // Athens departure, so it must be rejected as not-after.
        editor.end_time = "00:00".into();
        assert!(editor.to_event(athens).is_err());
    }

    #[test]
    fn a_utc_stamped_event_still_edits_as_local_time() {
        let athens = chrono_tz::Europe::Athens; // +03:00 in August
        let six_utc = NaiveDate::from_ymd_opt(2026, 8, 4)
            .unwrap()
            .and_hms_opt(6, 0, 0)
            .unwrap();

        let mut event = Event::draft("personal", six_utc, athens);
        event.summary = "Imported".into();
        event.start = EventTime::Zoned(six_utc, chrono_tz::UTC);
        event.end = EventTime::Zoned(six_utc + chrono::Duration::hours(1), chrono_tz::UTC);

        let editor = Editor::from_event(&event, athens);
        assert_eq!(editor.start_tz, None, "a UTC stamp is not a zone choice");
        assert_eq!(editor.start_time, "09:00", "shown as local wall clock");
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
