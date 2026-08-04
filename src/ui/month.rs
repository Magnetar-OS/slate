// SPDX-License-Identifier: GPL-3.0-only

//! The month grid: six rows of seven days.

use super::{
    MAX_CHIPS_PER_DAY, MONTH_ROWS, day_cell, event_chip, format_time, week_number, weekday_order,
};
use crate::app::Message;
use crate::config::Config;
use crate::fl;
use crate::model::{CalendarMeta, Occurrence, Rgb};
use chrono::{Datelike, Duration, NaiveDate};
use cosmic::iced::{Alignment, Length};
use cosmic::widget;
use cosmic::{Apply, Element};
use std::collections::BTreeMap;

pub struct MonthView<'a> {
    pub anchor: NaiveDate,
    pub today: NaiveDate,
    pub start: NaiveDate,
    pub days: &'a BTreeMap<NaiveDate, Vec<Occurrence>>,
    pub calendars: &'a [CalendarMeta],
    pub config: &'a Config,
}

impl<'a> MonthView<'a> {
    #[must_use]
    pub fn view(&self) -> Element<'a, Message> {
        let spacing = cosmic::theme::spacing();

        let mut grid = widget::column::with_capacity(MONTH_ROWS as usize + 1)
            .spacing(spacing.space_xxxs)
            .width(Length::Fill)
            .height(Length::Fill);

        grid = grid.push(self.header_row());

        for row in 0..MONTH_ROWS {
            grid = grid.push(self.week_row(self.start + Duration::days(row * 7)));
        }

        grid.padding(spacing.space_xxs).into()
    }

    fn header_row(&self) -> Element<'a, Message> {
        let spacing = cosmic::theme::spacing();
        let mut row = widget::row::with_capacity(8).spacing(spacing.space_xxxs);

        if self.config.show_week_numbers {
            // Spacer aligning the headers with the week-number gutter below.
            row = row.push(widget::container(widget::text::caption("")).width(Length::Fixed(28.0)));
        }

        for weekday in weekday_order(self.config.first_weekday()) {
            row = row.push(
                widget::text::caption_heading(super::weekday_short(weekday))
                    .width(Length::Fill)
                    .align_x(Alignment::Center)
                    .apply(widget::container)
                    .width(Length::Fill),
            );
        }

        row.into()
    }

    fn week_row(&self, week_start: NaiveDate) -> Element<'a, Message> {
        let spacing = cosmic::theme::spacing();
        let mut row = widget::row::with_capacity(8)
            .spacing(spacing.space_xxxs)
            .height(Length::Fill);

        if self.config.show_week_numbers {
            row = row.push(
                widget::text::caption(fl!("week-abbrev", number = week_number(week_start)))
                    .apply(widget::container)
                    .width(Length::Fixed(28.0))
                    .height(Length::Fill)
                    .align_y(Alignment::Center),
            );
        }

        for offset in 0..7 {
            row = row.push(self.day_cell(week_start + Duration::days(offset)));
        }

        row.into()
    }

    fn day_cell(&self, date: NaiveDate) -> Element<'a, Message> {
        let spacing = cosmic::theme::spacing();
        let is_today = date == self.today;
        let outside = date.month() != self.anchor.month();

        let occurrences: &'a [Occurrence] = self.days.get(&date).map_or(&[], Vec::as_slice);

        let mut cell = widget::column::with_capacity(MAX_CHIPS_PER_DAY + 2)
            .spacing(spacing.space_xxxs)
            .push(day_number(date, is_today, outside));

        for occurrence in occurrences.iter().take(MAX_CHIPS_PER_DAY) {
            cell = cell.push(self.chip(occurrence));
        }

        if occurrences.len() > MAX_CHIPS_PER_DAY {
            let extra = occurrences.len() - MAX_CHIPS_PER_DAY;
            cell = cell.push(
                widget::button::text(fl!("more-events", count = extra))
                    .class(cosmic::theme::Button::Link)
                    .padding([0, spacing.space_xxs])
                    // Jumping to the day view is the natural way to see the rest.
                    .on_press(Message::OpenDay(date)),
            );
        }

        // The whole cell is a click target for creating an event at 09:00 that day.
        widget::mouse_area(
            cell.apply(widget::container)
                .padding(spacing.space_xxs)
                .width(Length::Fill)
                .height(Length::Fill)
                .class(day_cell(is_today, outside)),
        )
        .on_press(Message::NewEventOn(date))
        .into()
    }

    fn chip(&self, occurrence: &Occurrence) -> Element<'a, Message> {
        let color = self
            .calendars
            .iter()
            .find(|c| c.id == occurrence.calendar_id)
            .map_or(crate::model::DEFAULT_CALENDAR_COLOR, |c| c.color);

        chip_button(occurrence, color, self.config, false)
    }
}

fn day_number(date: NaiveDate, is_today: bool, outside: bool) -> Element<'static, Message> {
    let label = date.day().to_string();

    let text = if is_today {
        widget::text::body(label).class(cosmic::theme::Text::Accent)
    } else if outside {
        widget::text::body(label).class(cosmic::theme::Text::Default)
    } else {
        widget::text::body(label)
    };

    text.apply(widget::container).width(Length::Fill).into()
}

/// The shared event-chip button used by every view.
#[must_use]
pub fn chip_button(
    occurrence: &Occurrence,
    color: Rgb,
    config: &Config,
    show_time: bool,
) -> Element<'static, Message> {
    let spacing = cosmic::theme::spacing();

    let summary = if occurrence.summary.trim().is_empty() {
        fl!("untitled-event")
    } else {
        occurrence.summary.clone()
    };

    let label = if occurrence.all_day || !show_time {
        summary
    } else {
        format!(
            "{} {}",
            format_time(occurrence.start.time(), config),
            summary
        )
    };

    widget::button::custom(
        widget::text::caption(label)
            .wrapping(cosmic::iced::core::text::Wrapping::None)
            .width(Length::Fill),
    )
    .class(event_chip(color))
    .padding([1, spacing.space_xxs])
    .width(Length::Fill)
    .on_press(Message::OpenEvent(
        occurrence.calendar_id.clone(),
        occurrence.uid.clone(),
    ))
    .into()
}
