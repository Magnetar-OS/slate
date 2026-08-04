// SPDX-License-Identifier: GPL-3.0-only

//! The sidebar: a mini month for jumping around, and the calendar list.

use super::{color, panel_surface};
use crate::app::Message;
use crate::config::Config;
use crate::fl;
use crate::model::CalendarMeta;
use cosmic::iced::{Alignment, Length};
use cosmic::widget;
use cosmic::{Apply, Element};

/// Width libcosmic's `calendar` widget lays itself out at.
const MINI_CALENDAR_WIDTH: f32 = 360.0;
/// Padding either side of the sidebar's contents.
const GUTTER: f32 = 8.0;

pub struct Sidebar<'a> {
    pub mini: &'a widget::calendar::CalendarModel,
    pub calendars: &'a [CalendarMeta],
    pub config: &'a Config,
    /// Set while the "new calendar" row is being filled in.
    pub new_calendar_name: Option<&'a String>,
}

impl<'a> Sidebar<'a> {
    #[must_use]
    pub fn view(&self) -> Element<'a, Message> {
        let spacing = cosmic::theme::spacing();

        widget::column::with_capacity(3)
            .spacing(spacing.space_s)
            .push(self.mini_month())
            .push(self.calendar_list())
            // libcosmic's calendar widget hard-codes a 360px content width, so
            // anything narrower makes its 7-column grid wrap onto two rows.
            .width(Length::Fixed(MINI_CALENDAR_WIDTH + 2.0 * GUTTER))
            .padding(GUTTER as u16)
            .into()
    }

    fn mini_month(&self) -> Element<'a, Message> {
        widget::calendar(
            self.mini,
            Message::MiniDateSelected,
            || Message::MiniPrevMonth,
            || Message::MiniNextMonth,
            self.config.first_weekday_jiff(),
        )
        .apply(widget::container)
        .class(panel_surface())
        .width(Length::Fixed(MINI_CALENDAR_WIDTH))
        .into()
    }

    fn calendar_list(&self) -> Element<'a, Message> {
        let spacing = cosmic::theme::spacing();

        let mut column = widget::column::with_capacity(self.calendars.len() + 3)
            .spacing(spacing.space_xxs)
            .push(widget::text::heading(fl!("calendars")));

        if self.calendars.is_empty() && self.new_calendar_name.is_none() {
            column = column.push(
                widget::text::caption(fl!("no-calendars"))
                    .wrapping(cosmic::iced::core::text::Wrapping::Word),
            );
        }

        for calendar in self.calendars {
            column = column.push(self.calendar_row(calendar));
        }

        let add_row: Element<'a, Message> = match self.new_calendar_name {
            Some(name) => widget::row::with_capacity(2)
                .spacing(spacing.space_xxs)
                .push(
                    widget::text_input(fl!("calendar-name"), name)
                        .on_input(Message::NewCalendarNameChanged)
                        .on_submit(|_| Message::NewCalendarConfirm)
                        .width(Length::Fill),
                )
                .push(
                    widget::button::icon(widget::icon::from_name("window-close-symbolic"))
                        .on_press(Message::NewCalendarCancel),
                )
                .into(),
            None => widget::button::text(fl!("new-calendar"))
                .leading_icon(widget::icon::from_name("list-add-symbolic"))
                .on_press(Message::NewCalendarStart)
                .width(Length::Fill)
                .into(),
        };
        column = column.push(add_row);

        column
            .apply(widget::container)
            .class(panel_surface())
            .padding(spacing.space_xs)
            .width(Length::Fill)
            .into()
    }

    fn calendar_row(&self, calendar: &'a CalendarMeta) -> Element<'a, Message> {
        let spacing = cosmic::theme::spacing();
        let visible = !self.config.is_hidden(&calendar.id);

        let swatch = widget::container(
            widget::Space::new()
                .width(Length::Fixed(12.0))
                .height(Length::Fixed(12.0)),
        )
        .class(swatch_style(calendar.color, visible));

        let mut label = widget::text::body(calendar.name.clone())
            .wrapping(cosmic::iced::core::text::Wrapping::None)
            .width(Length::Fill);

        if !visible {
            label = label.class(cosmic::theme::Text::Default);
        }

        let mut row = widget::row::with_capacity(4)
            .align_y(Alignment::Center)
            .spacing(spacing.space_xxs)
            .push(swatch)
            .push(label);

        if calendar.read_only {
            row = row.push(
                widget::icon::from_name("changes-prevent-symbolic")
                    .size(14)
                    .apply(widget::container),
            );
        }

        // The whole row toggles visibility; a checkbox would only duplicate it.
        widget::button::custom(row)
            .class(cosmic::theme::Button::Text)
            .padding([spacing.space_xxxs, spacing.space_xxs])
            .width(Length::Fill)
            .on_press(Message::ToggleCalendar(calendar.id.clone()))
            .into()
    }
}

fn swatch_style(rgb: crate::model::Rgb, visible: bool) -> cosmic::theme::Container<'static> {
    cosmic::theme::Container::custom(move |theme| {
        let cosmic = theme.cosmic();
        let mut fill = color(rgb);
        if !visible {
            fill.a = 0.0;
        }

        cosmic::iced::widget::container::Style {
            background: Some(cosmic::iced::Background::Color(fill)),
            border: cosmic::iced::Border {
                radius: cosmic.corner_radii.radius_xs.into(),
                width: 1.5,
                color: color(rgb),
            },
            ..Default::default()
        }
    })
}
