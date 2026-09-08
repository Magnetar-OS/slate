// SPDX-License-Identifier: GPL-3.0-only

//! In-app search: the launcher plugin's query, with a real results surface
//! and a wider window. Summaries and locations are matched; results come
//! back nearest-first and one row per event, so a daily standup does not
//! fill the list with itself.

use super::dim_text;
use crate::app::Message;
use crate::config::Config;
use crate::fl;
use crate::model::{CalendarMeta, Occurrence};
use chrono::{Datelike, NaiveDate};
use cosmic::Element;
use cosmic::iced::{Alignment, Length};
use cosmic::widget;

/// Identifies the query field so opening search can focus it.
#[must_use]
pub fn input_id() -> cosmic::widget::Id {
    cosmic::widget::Id::new("slate-search")
}

pub struct SearchView<'a> {
    pub query: &'a str,
    pub results: &'a [Occurrence],
    pub calendars: &'a [CalendarMeta],
    pub config: &'a Config,
    pub today: NaiveDate,
}

impl<'a> SearchView<'a> {
    #[must_use]
    pub fn view(&self) -> Element<'a, Message> {
        let spacing = cosmic::theme::spacing();

        let mut column = widget::column::with_capacity(3)
            .spacing(spacing.space_s)
            .padding(spacing.space_s)
            .push(
                widget::search_input(fl!("search-placeholder"), self.query)
                    .id(input_id())
                    .on_input(Message::SearchInput)
                    .on_clear(Message::SearchClose)
                    .width(Length::Fill),
            );

        if self.query.trim().is_empty() {
            column = column.push(
                widget::text::caption(fl!("search-hint"))
                    .class(cosmic::theme::Text::Custom(dim_text)),
            );
            return column.into();
        }

        if self.results.is_empty() {
            column = column.push(widget::text::body(fl!("search-no-results")));
            return column.into();
        }

        let mut list = widget::column::with_capacity(self.results.len()).spacing(spacing.space_xxs);
        for occurrence in self.results {
            list = list.push(self.row(occurrence));
        }

        column
            .push(
                widget::scrollable(list)
                    .height(Length::Fill)
                    .width(Length::Fill),
            )
            .into()
    }

    fn row(&self, occurrence: &'a Occurrence) -> Element<'a, Message> {
        let spacing = cosmic::theme::spacing();

        let color = self
            .calendars
            .iter()
            .find(|c| c.id == occurrence.calendar_id)
            .map_or(crate::model::DEFAULT_CALENDAR_COLOR, |c| c.color);

        let date = occurrence.start.date();
        let when = if occurrence.all_day {
            format!(
                "{} {} {}",
                super::weekday_short(date.weekday()),
                super::format_date_short(date),
                date.year()
            )
        } else {
            format!(
                "{} {} {} · {}",
                super::weekday_short(date.weekday()),
                super::format_date_short(date),
                date.year(),
                super::format_time(occurrence.start.time(), self.config)
            )
        };

        let summary = if occurrence.summary.trim().is_empty() {
            fl!("untitled-event")
        } else {
            occurrence.summary.clone()
        };

        let mut text = widget::column::with_capacity(2)
            .push(widget::text::body(summary).wrapping(cosmic::iced::core::text::Wrapping::None))
            .push(widget::text::caption(when).class(cosmic::theme::Text::Custom(dim_text)));
        if let Some(location) = occurrence.location.as_deref().filter(|s| !s.is_empty()) {
            text = text.push(
                widget::text::caption(location.to_owned())
                    .wrapping(cosmic::iced::core::text::Wrapping::None)
                    .class(cosmic::theme::Text::Custom(dim_text)),
            );
        }

        let row = widget::row::with_capacity(2)
            .spacing(spacing.space_xs)
            .align_y(Alignment::Center)
            .push(
                widget::container(widget::Space::new().width(Length::Fixed(3.0)))
                    .height(Length::Fixed(36.0))
                    .class(super::color_bar(color)),
            )
            .push(text.width(Length::Fill));

        // A button, not a mouse area: a result has to be reachable by keyboard,
        // and `row_button` keeps the row's own colours.
        widget::button::custom(row.width(Length::Fill))
            .class(super::row_button())
            .padding(super::ROW_PADDING)
            .width(Length::Fill)
            .on_press(Message::SearchHit(
                date,
                occurrence.calendar_id.clone(),
                occurrence.uid.clone(),
                occurrence.recurrence_id,
            ))
            .into()
    }
}
