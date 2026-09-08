// SPDX-License-Identifier: GPL-3.0-only

//! The agenda view: the next month of events as a flat, scannable list.
//!
//! The same model as the panel applet's popup — a date heading whenever the
//! day changes, one row per occurrence — with a full window's worth of room.
//! Deliberately no grid: this is the view for "what's coming", not "what's
//! where".

use super::dim_text;
use crate::app::Message;
use crate::config::Config;
use crate::fl;
use crate::model::{CalendarMeta, Occurrence};
use chrono::{Datelike, NaiveDate};
use cosmic::iced::{Alignment, Length};
use cosmic::widget;
use cosmic::{Apply, Element};
use std::collections::BTreeMap;

/// Width of the time column, so summaries align down the page.
const TIME_WIDTH: f32 = 88.0;

pub struct Agenda<'a> {
    pub occurrences: &'a BTreeMap<NaiveDate, Vec<Occurrence>>,
    pub calendars: &'a [CalendarMeta],
    pub config: &'a Config,
    pub today: NaiveDate,
}

impl<'a> Agenda<'a> {
    #[must_use]
    pub fn view(&self) -> Element<'a, Message> {
        let spacing = cosmic::theme::spacing();

        let mut column = widget::column::with_capacity(self.occurrences.len() * 4)
            .spacing(spacing.space_xxs)
            .padding(spacing.space_s);

        let mut any = false;
        for (date, occurrences) in self.occurrences {
            if occurrences.is_empty() {
                continue;
            }
            any = true;

            column = column.push(
                widget::text::heading(self.day_heading(*date))
                    .apply(widget::container)
                    .padding([spacing.space_xs, 0, spacing.space_xxxs, 0]),
            );
            for occurrence in occurrences {
                column = column.push(self.row(*date, occurrence));
            }
        }

        if !any {
            return widget::text::body(fl!("no-events-range"))
                .apply(widget::container)
                .center(Length::Fill)
                .into();
        }

        widget::scrollable(column)
            .height(Length::Fill)
            .width(Length::Fill)
            .into()
    }

    fn day_heading(&self, date: NaiveDate) -> String {
        if date == self.today {
            fl!("today")
        } else if date == self.today + chrono::Duration::days(1) {
            fl!("tomorrow")
        } else {
            format!(
                "{} {} {}",
                super::weekday_short(date.weekday()),
                super::format_date_short(date),
                date.year()
            )
        }
    }

    fn row(&self, date: NaiveDate, occurrence: &'a Occurrence) -> Element<'a, Message> {
        let spacing = cosmic::theme::spacing();

        let color = self
            .calendars
            .iter()
            .find(|c| c.id == occurrence.calendar_id)
            .map_or(crate::model::DEFAULT_CALENDAR_COLOR, |c| c.color);

        // A multi-day event appears under every day it touches; days after
        // the first carry the continuation marker instead of a start time.
        let time = if occurrence.all_day {
            fl!("all-day-events")
        } else if occurrence.start.date() < date {
            "↳".to_owned()
        } else {
            super::format_time(occurrence.start.time(), self.config)
        };

        let summary = if occurrence.summary.trim().is_empty() {
            fl!("untitled-event")
        } else {
            occurrence.summary.clone()
        };

        let mut text = widget::column::with_capacity(2)
            .push(widget::text::body(summary).wrapping(cosmic::iced::core::text::Wrapping::None));
        if let Some(location) = occurrence.location.as_deref().filter(|s| !s.is_empty()) {
            text = text.push(
                widget::text::caption(location.to_owned())
                    .wrapping(cosmic::iced::core::text::Wrapping::None)
                    .class(cosmic::theme::Text::Custom(dim_text)),
            );
        }

        let row = widget::row::with_capacity(3)
            .spacing(spacing.space_xs)
            .align_y(Alignment::Center)
            .push(
                widget::container(widget::Space::new().width(Length::Fixed(3.0)))
                    .height(Length::Fixed(28.0))
                    .class(super::color_bar(color)),
            )
            .push(
                widget::text::caption(time)
                    .class(cosmic::theme::Text::Custom(dim_text))
                    .apply(widget::container)
                    .width(Length::Fixed(TIME_WIDTH)),
            )
            .push(text.width(Length::Fill));

        // A button, not a mouse area: every row has to be reachable by
        // keyboard, and `row_button` keeps the row's own colours.
        widget::button::custom(row.width(Length::Fill))
            .class(super::row_button())
            .padding(super::ROW_PADDING)
            .width(Length::Fill)
            .on_press(Message::OpenEvent(
                occurrence.calendar_id.clone(),
                occurrence.uid.clone(),
                occurrence.recurrence_id,
            ))
            .into()
    }
}
