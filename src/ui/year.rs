// SPDX-License-Identifier: GPL-3.0-only

//! The year view: twelve mini months, each day tinted by how busy it is.
//!
//! A year holds far too many events to name, so this view answers a different
//! question from the others — not "what is on" but "when am I busy" — and the
//! only way to read that at this scale is density. Clicking a day opens it.

use super::{dim_text, heat_cell, today_badge, weekday_order};
use crate::app::Message;
use crate::config::Config;
use crate::model::Occurrence;
use chrono::{Datelike, Duration, NaiveDate};
use cosmic::iced::{Alignment, Length};
use cosmic::widget;
use cosmic::{Apply, Element};
use std::collections::BTreeMap;

/// Months per row. Four by three fits the default window without scrolling.
const COLUMNS: usize = 4;

/// One day square. Small enough for a year on screen, large enough to hit.
const CELL: f32 = 22.0;

/// Busyness bands. A day with more events than the last threshold gets the
/// darkest tint; the bands are coarse on purpose, because the eye reads four
/// steps and not fourteen.
const BANDS: [usize; 3] = [1, 3, 5];

pub struct YearView<'a> {
    /// Any date within the year being shown.
    pub anchor: NaiveDate,
    pub today: NaiveDate,
    pub days: &'a BTreeMap<NaiveDate, Vec<Occurrence>>,
    pub config: &'a Config,
}

impl<'a> YearView<'a> {
    #[must_use]
    pub fn view(&self) -> Element<'a, Message> {
        let spacing = cosmic::theme::spacing();
        let year = self.anchor.year();

        let mut grid = widget::column::with_capacity(3).spacing(spacing.space_m);
        for row in 0..(12 / COLUMNS) {
            let mut months = widget::row::with_capacity(COLUMNS).spacing(spacing.space_m);
            for column in 0..COLUMNS {
                let month = (row * COLUMNS + column + 1) as u32;
                months = months.push(self.month(year, month));
            }
            grid = grid.push(months);
        }

        widget::scrollable(grid.padding(spacing.space_s))
            .height(Length::Fill)
            .width(Length::Fill)
            .into()
    }

    /// One mini month: its name, the weekday initials, and up to six week rows.
    fn month(&self, year: i32, month: u32) -> Element<'a, Message> {
        let spacing = cosmic::theme::spacing();

        let Some(first) = NaiveDate::from_ymd_opt(year, month, 1) else {
            return widget::Space::new().into();
        };
        let start = super::week_start(first, self.config.first_weekday());

        let mut column = widget::column::with_capacity(8)
            .spacing(2)
            .push(widget::text::heading(super::month_name(month)))
            .push(self.weekday_row());

        // Six rows always: a fixed height keeps the twelve months aligned
        // whatever weekday each one starts on.
        for row in 0..super::MONTH_ROWS {
            let mut week = widget::row::with_capacity(7).spacing(2);
            for day in 0..7 {
                let date = start + Duration::days(row * 7 + day);
                week = week.push(if date.month() == month {
                    self.day_cell(date)
                } else {
                    // Days from the neighbouring month are left blank rather
                    // than dimmed: at this size a grey number reads as noise.
                    widget::Space::new()
                        .width(Length::Fixed(CELL))
                        .height(Length::Fixed(CELL))
                        .into()
                });
            }
            column = column.push(week);
        }

        column
            .apply(widget::container)
            .padding(spacing.space_xxs)
            .into()
    }

    fn weekday_row(&self) -> Element<'a, Message> {
        let mut row = widget::row::with_capacity(7).spacing(2);
        for weekday in weekday_order(self.config.first_weekday()) {
            // One letter: seven three-letter names do not fit a mini month.
            let initial: String = super::weekday_short(weekday).chars().take(1).collect();
            row = row.push(
                widget::text::caption(initial.to_uppercase())
                    .class(cosmic::theme::Text::Custom(dim_text))
                    .apply(widget::container)
                    .width(Length::Fixed(CELL))
                    .align_x(Alignment::Center),
            );
        }
        row.into()
    }

    fn day_cell(&self, date: NaiveDate) -> Element<'a, Message> {
        let count = self.days.get(&date).map_or(0, Vec::len);
        let count_i64 = i64::try_from(count).unwrap_or(i64::MAX);
        let level = BANDS
            .iter()
            .filter(|threshold| count >= **threshold)
            .count();

        let number = widget::text::caption(date.day().to_string())
            .apply(widget::container)
            .width(Length::Fixed(CELL))
            .height(Length::Fixed(CELL))
            .align_x(Alignment::Center)
            .align_y(Alignment::Center);

        let cell = if date == self.today {
            number.class(today_badge())
        } else {
            number.class(heat_cell(level))
        };

        // A button so every day of the year can be reached by keyboard.
        let hit = widget::button::custom(cell)
            .class(super::row_button())
            .padding(0)
            .on_press(Message::OpenDay(date));

        // The count is only in the tint, so the tooltip is where a day says
        // how busy it actually is.
        if count == 0 {
            hit.into()
        } else {
            widget::tooltip(
                hit,
                widget::text::caption(crate::fl!("year-day-events", count = count_i64)),
                widget::tooltip::Position::Top,
            )
            .into()
        }
    }
}
