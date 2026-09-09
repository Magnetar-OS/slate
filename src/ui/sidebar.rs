// SPDX-License-Identifier: GPL-3.0-only

//! The sidebar: a mini month for jumping around, and the calendar list.

use super::color;
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

        // Rendered into libcosmic's nav-bar slot, so it wears the desktop's own
        // nav-bar surface — same background, corner radii, and blur behaviour as
        // the sidebar in cosmic-files or cosmic-settings.
        // Scrollable, because the mini month alone is over 300px tall and the
        // window may be 560: without this the calendar list — and with it
        // every visibility toggle and the "new calendar" button — sits below
        // the bottom of a short window with no way to reach it.
        //
        // No `Fill` spacer inside: a scrollable gives its content unbounded
        // height, so one would grow without limit instead of pushing content
        // to the top. The container below fills the column either way.
        widget::column::with_capacity(2)
            .spacing(spacing.space_s)
            .push(self.mini_month())
            .push(self.calendar_list())
            // libcosmic's calendar widget hard-codes a 360px content width, so
            // anything narrower makes its 7-column grid wrap onto two rows.
            .width(Length::Fixed(MINI_CALENDAR_WIDTH + 2.0 * GUTTER))
            .padding(GUTTER as u16)
            .apply(widget::scrollable)
            .height(Length::Fill)
            .apply(widget::container)
            .class(cosmic::theme::Container::custom(
                cosmic::widget::nav_bar::nav_bar_style,
            ))
            .height(Length::Fill)
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
                .push(widget::tooltip(
                    widget::button::icon(widget::icon::from_name("window-close-symbolic"))
                        .on_press(Message::NewCalendarCancel),
                    widget::text::caption(fl!("cancel")),
                    widget::tooltip::Position::Bottom,
                ))
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
            .padding([spacing.space_xxs, 0])
            .width(Length::Fill)
            .into()
    }

    /// The icon-only sidebar: one swatch per calendar, name in a tooltip,
    /// press to toggle. The mini month has no narrow form and is simply
    /// absent here.
    #[must_use]
    pub fn rail(&self) -> Element<'a, Message> {
        cosmic_ext_widgets::rail(self.calendars.iter().map(|calendar| {
            let visible = !self.config.is_hidden(&calendar.id);
            cosmic_ext_widgets::rail_item(
                swatch(calendar, visible, 16.0),
                calendar.name.clone(),
                false,
                Message::ToggleCalendar(calendar.id.clone()),
            )
        }))
    }

    fn calendar_row(&self, calendar: &'a CalendarMeta) -> Element<'a, Message> {
        let spacing = cosmic::theme::spacing();
        let visible = !self.config.is_hidden(&calendar.id);
        let swatch = swatch(calendar, visible, 12.0);

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

/// The calendar's colour as a rounded square: filled while it is shown, an
/// outline while it is hidden.
fn swatch<'a>(calendar: &CalendarMeta, visible: bool, size: f32) -> Element<'a, Message> {
    widget::container(
        widget::Space::new()
            .width(Length::Fixed(size))
            .height(Length::Fixed(size)),
    )
    .class(swatch_style(calendar.color, visible))
    .into()
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
