// SPDX-License-Identifier: GPL-3.0-only

//! The month grid: six rows of seven days.

use super::{
    MAX_CHIPS_PER_DAY, MONTH_ROWS, TODAY_BADGE, color_bar, day_cell, dim_text, event_chip,
    format_time, today_badge, week_number, weekday_order,
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

/// Width of the week-number gutter.
const GUTTER: f32 = 30.0;

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

        grid.padding(spacing.space_xs).into()
    }

    fn header_row(&self) -> Element<'a, Message> {
        let spacing = cosmic::theme::spacing();
        let mut row = widget::row::with_capacity(8).spacing(spacing.space_xxxs);

        if self.config.show_week_numbers {
            row = row.push(widget::Space::new().width(Length::Fixed(GUTTER)));
        }

        for weekday in weekday_order(self.config.first_weekday()) {
            // Uppercase and dimmed: the column headers are a reference, not content.
            row = row.push(
                widget::text::caption(super::weekday_short(weekday).to_uppercase())
                    .class(cosmic::theme::Text::Custom(dim_text))
                    .apply(widget::container)
                    .width(Length::Fill)
                    .align_x(Alignment::Center),
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
                    .class(cosmic::theme::Text::Custom(dim_text))
                    .apply(widget::container)
                    .width(Length::Fixed(GUTTER))
                    .height(Length::Fill)
                    .align_x(Alignment::Center)
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
            .spacing(1)
            .push(day_number(date, is_today));

        for occurrence in occurrences.iter().take(MAX_CHIPS_PER_DAY) {
            cell = cell.push(self.chip(occurrence, date));
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

        // The whole cell is a click target for creating an event that day.
        widget::mouse_area(
            cell.apply(widget::container)
                .padding(spacing.space_xxxs)
                .width(Length::Fill)
                .height(Length::Fill)
                .class(day_cell(is_today, outside)),
        )
        .on_press(Message::NewEventOn(date))
        .into()
    }

    fn chip(&self, occurrence: &Occurrence, date: NaiveDate) -> Element<'a, Message> {
        let color = self
            .calendars
            .iter()
            .find(|c| c.id == occurrence.calendar_id)
            .map_or(crate::model::DEFAULT_CALENDAR_COLOR, |c| c.color);

        chip_button(occurrence, color, self.config, true, Some(date))
    }
}

/// The date, as a plain number — or as a filled accent circle when it is today.
fn day_number(date: NaiveDate, is_today: bool) -> Element<'static, Message> {
    let label = date.day().to_string();

    let inner: Element<'static, Message> = if is_today {
        widget::text::body(label)
            .apply(widget::container)
            .class(today_badge())
            .width(Length::Fixed(TODAY_BADGE))
            .height(Length::Fixed(TODAY_BADGE))
            .align_x(Alignment::Center)
            .align_y(Alignment::Center)
            .into()
    } else {
        widget::text::body(label)
            .apply(widget::container)
            .height(Length::Fixed(TODAY_BADGE))
            .padding([0, 6])
            .align_y(Alignment::Center)
            .into()
    };

    widget::container(inner)
        .width(Length::Fill)
        .align_x(Alignment::Start)
        .into()
}

/// The shared event chip: a solid bar of the calendar's colour, then the label.
///
/// `on_date` is the day the chip is being drawn on. An event that started before
/// it is marked as a continuation rather than advertising yesterday's start time.
#[must_use]
pub fn chip_button(
    occurrence: &Occurrence,
    color: Rgb,
    config: &Config,
    show_time: bool,
    on_date: Option<NaiveDate>,
) -> Element<'static, Message> {
    let spacing = cosmic::theme::spacing();

    let summary = if occurrence.summary.trim().is_empty() {
        fl!("untitled-event")
    } else {
        occurrence.summary.clone()
    };

    let mut row = widget::row::with_capacity(3)
        .align_y(Alignment::Center)
        .spacing(spacing.space_xxxs)
        .push(
            // The solid colour lives here rather than in the background, so the
            // chip stays translucent enough for the blur to read through.
            widget::container(widget::Space::new().width(Length::Fixed(3.0)))
                .height(Length::Fixed(12.0))
                .class(color_bar(color)),
        );

    let continues = on_date.is_some_and(|date| occurrence.start.date() < date);

    if show_time && !occurrence.all_day {
        // A run-over from a previous day shows when it *ends*, which is the only
        // part of it that concerns today.
        let label = if continues {
            format!("↳ {}", format_time(occurrence.end.time(), config))
        } else {
            format_time(occurrence.start.time(), config)
        };
        row = row.push(
            widget::text::caption(label)
                .wrapping(cosmic::iced::core::text::Wrapping::None)
                .class(cosmic::theme::Text::Custom(dim_text)),
        );
    }

    row = row.push(
        widget::text::caption(summary)
            .wrapping(cosmic::iced::core::text::Wrapping::None)
            .width(Length::Fill),
    );

    widget::button::custom(row)
        .class(event_chip(color))
        .padding([1, spacing.space_xxxs])
        .width(Length::Fill)
        .on_press(Message::OpenEvent(
            occurrence.calendar_id.clone(),
            occurrence.uid.clone(),
        ))
        .into()
}
