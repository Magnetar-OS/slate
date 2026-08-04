// SPDX-License-Identifier: GPL-3.0-only

//! The week and day views: an hour-ruled grid with a separate all-day band.
//!
//! Events are placed in the row of the hour they start in. Drawing them at a
//! height proportional to their duration, with overlapping events side by side,
//! needs a custom layout widget; until that exists, an event that runs past its
//! slot says so in its label rather than silently looking like an hour long.

use super::{HOUR_HEIGHT, day_cell, format_time, panel_surface};
use crate::app::Message;
use crate::config::Config;
use crate::fl;
use crate::model::{CalendarMeta, Occurrence, Rgb};
use chrono::{Datelike, Duration, NaiveDate, NaiveTime, Timelike};
use cosmic::iced::{Alignment, Length};
use cosmic::widget;
use cosmic::{Apply, Element};
use std::collections::BTreeMap;

/// Identifies the hour-grid scrollable so the app can scroll it programmatically.
#[must_use]
pub fn scroll_id() -> cosmic::widget::Id {
    cosmic::widget::Id::new("cosmic-calendar-timegrid")
}

/// Vertical offset that puts `hour` at the top of the viewport.
#[must_use]
pub fn offset_for_hour(hour: u32) -> f32 {
    let row_spacing = cosmic::theme::spacing().space_xxxs as f32;
    hour as f32 * (HOUR_HEIGHT + row_spacing)
}

pub struct TimeGrid<'a> {
    pub start: NaiveDate,
    /// 7 for the week view, 1 for the day view.
    pub days: i64,
    pub today: NaiveDate,
    pub occurrences: &'a BTreeMap<NaiveDate, Vec<Occurrence>>,
    pub calendars: &'a [CalendarMeta],
    pub config: &'a Config,
}

impl<'a> TimeGrid<'a> {
    #[must_use]
    pub fn view(&self) -> Element<'a, Message> {
        let spacing = cosmic::theme::spacing();

        widget::column::with_capacity(3)
            .spacing(spacing.space_xxs)
            .push(self.header_row())
            .push(self.all_day_band())
            .push(
                widget::scrollable(self.hour_grid())
                    .id(scroll_id())
                    .height(Length::Fill)
                    .width(Length::Fill),
            )
            .padding(spacing.space_xxs)
            .into()
    }

    /// Copies the bounds out so the iterator borrows nothing from `self`.
    fn dates(&self) -> impl Iterator<Item = NaiveDate> + use<> {
        let start = self.start;
        (0..self.days).map(move |i| start + Duration::days(i))
    }

    fn header_row(&self) -> Element<'a, Message> {
        let spacing = cosmic::theme::spacing();
        let mut row = widget::row::with_capacity(self.days as usize + 1)
            .spacing(spacing.space_xxxs)
            .push(gutter_spacer());

        for date in self.dates() {
            let is_today = date == self.today;

            let weekday = widget::text::caption(super::weekday_short(date.weekday()));
            let number = if is_today {
                widget::text::title4(date.day().to_string()).class(cosmic::theme::Text::Accent)
            } else {
                widget::text::title4(date.day().to_string())
            };

            row = row.push(
                widget::column::with_capacity(2)
                    .push(weekday)
                    .push(number)
                    .align_x(Alignment::Center)
                    .apply(widget::container)
                    .width(Length::Fill)
                    .align_x(Alignment::Center),
            );
        }

        row.into()
    }

    /// All-day events get their own band above the ruler; they have no place on
    /// an hour axis.
    fn all_day_band(&self) -> Element<'a, Message> {
        let spacing = cosmic::theme::spacing();

        let has_any = self
            .dates()
            .any(|d| self.day_occurrences(d).iter().any(|o| o.all_day));

        if !has_any {
            return widget::Space::new()
                .width(Length::Fill)
                .height(Length::Fixed(0.0))
                .into();
        }

        let mut row = widget::row::with_capacity(self.days as usize + 1)
            .spacing(spacing.space_xxxs)
            .push(
                widget::text::caption(fl!("all-day-events"))
                    .apply(widget::container)
                    .width(Length::Fixed(56.0))
                    .align_y(Alignment::Center),
            );

        for date in self.dates() {
            let mut column = widget::column::with_capacity(2).spacing(spacing.space_xxxs);
            for occurrence in self.day_occurrences(date).iter().filter(|o| o.all_day) {
                column = column.push(super::month::chip_button(
                    occurrence,
                    self.color_of(occurrence),
                    self.config,
                    false,
                ));
            }
            row = row.push(column.width(Length::Fill));
        }

        row.apply(widget::container)
            .padding(spacing.space_xxs)
            .class(panel_surface())
            .width(Length::Fill)
            .into()
    }

    fn hour_grid(&self) -> Element<'a, Message> {
        let spacing = cosmic::theme::spacing();
        let mut rows = widget::column::with_capacity(24).spacing(spacing.space_xxxs);

        for hour in 0..24u32 {
            let mut row = widget::row::with_capacity(self.days as usize + 1)
                .spacing(spacing.space_xxxs)
                .height(Length::Fixed(HOUR_HEIGHT))
                .push(hour_label(hour, self.config));

            for date in self.dates() {
                row = row.push(self.hour_cell(date, hour));
            }

            rows = rows.push(row);
        }

        rows.into()
    }

    fn hour_cell(&self, date: NaiveDate, hour: u32) -> Element<'a, Message> {
        let is_today = date == self.today;

        let mut column = widget::column::with_capacity(2).spacing(1);

        for occurrence in self
            .day_occurrences(date)
            .iter()
            .filter(|o| !o.all_day)
            .filter(|o| self.starts_in_hour(o, date, hour))
        {
            column = column.push(self.timed_chip(occurrence));
        }

        widget::mouse_area(
            column
                .apply(widget::container)
                .padding(1)
                .width(Length::Fill)
                .height(Length::Fill)
                .class(day_cell(is_today && self.is_current_hour(hour), false)),
        )
        .on_press(Message::NewEventAt(
            date.and_hms_opt(hour, 0, 0)
                .unwrap_or_else(|| date.and_time(NaiveTime::MIN)),
        ))
        .into()
    }

    /// Whether this occurrence should be drawn in `hour` on `date`.
    ///
    /// An event that began on an earlier day is drawn in the first row so it is
    /// not lost off the top of the column.
    fn starts_in_hour(&self, occurrence: &Occurrence, date: NaiveDate, hour: u32) -> bool {
        if occurrence.start.date() == date {
            occurrence.start.hour() == hour
        } else {
            hour == 0
        }
    }

    fn is_current_hour(&self, hour: u32) -> bool {
        chrono::Local::now().hour() == hour
    }

    fn timed_chip(&self, occurrence: &Occurrence) -> Element<'a, Message> {
        let color = self.color_of(occurrence);
        let spacing = cosmic::theme::spacing();

        let summary = if occurrence.summary.trim().is_empty() {
            fl!("untitled-event")
        } else {
            occurrence.summary.clone()
        };

        // Say how long it runs, since the chip's height cannot.
        let mut label = format!(
            "{}–{}  {}",
            format_time(occurrence.start.time(), self.config),
            format_time(occurrence.end.time(), self.config),
            summary
        );

        // The day view has room for the location; the week view does not.
        if self.days == 1
            && let Some(location) = occurrence.location.as_deref().filter(|s| !s.is_empty())
        {
            label.push_str("  · ");
            label.push_str(location);
        }

        widget::button::custom(
            widget::text::caption(label)
                .wrapping(cosmic::iced::core::text::Wrapping::None)
                .width(Length::Fill),
        )
        .class(super::event_chip(color))
        .padding([1, spacing.space_xxs])
        .width(Length::Fill)
        .on_press(Message::OpenEvent(
            occurrence.calendar_id.clone(),
            occurrence.uid.clone(),
        ))
        .into()
    }

    fn day_occurrences(&self, date: NaiveDate) -> &'a [Occurrence] {
        self.occurrences.get(&date).map_or(&[], Vec::as_slice)
    }

    fn color_of(&self, occurrence: &Occurrence) -> Rgb {
        self.calendars
            .iter()
            .find(|c| c.id == occurrence.calendar_id)
            .map_or(crate::model::DEFAULT_CALENDAR_COLOR, |c| c.color)
    }
}

fn gutter_spacer() -> Element<'static, Message> {
    widget::Space::new().width(Length::Fixed(56.0)).into()
}

fn hour_label(hour: u32, config: &Config) -> Element<'static, Message> {
    let time = NaiveTime::from_hms_opt(hour, 0, 0).unwrap_or(NaiveTime::MIN);

    widget::text::caption(format_time(time, config))
        .apply(widget::container)
        .width(Length::Fixed(56.0))
        .height(Length::Fill)
        .align_y(Alignment::Start)
        .into()
}
