// SPDX-License-Identifier: GPL-3.0-only

//! View code, and the small vocabulary the views share.

pub mod accounts;
pub mod agenda;
pub mod editor;
pub mod month;
pub mod search;
pub mod sidebar;
pub mod task_editor;
pub mod tasks;
pub mod timegrid;
pub mod year;

use crate::config::Config;
use crate::fl;
use crate::model::Rgb;
use chrono::{Datelike, Duration, NaiveDate, NaiveDateTime, NaiveTime, Weekday};
use cosmic::iced::Color;

/// Rows in a month grid. Six always covers a month, whatever weekday it starts on,
/// and keeping it fixed stops the grid resizing as you page through the year.
pub const MONTH_ROWS: i64 = 6;

/// Height of one hour in the week and day grids.
pub const HOUR_HEIGHT: f32 = 52.0;

/// Floor on a block's drawn height, so a 10-minute event stays readable.
pub const MIN_BLOCK_HEIGHT: f32 = 18.0;

/// Diameter of the accent badge drawn behind today's date.
pub const TODAY_BADGE: f32 = 26.0;

/// Padding inside an activatable list row, so the focus ring has room to sit
/// clear of the text.
pub const ROW_PADDING: u16 = 2;

/// Most events a month cell shows before collapsing into "+N more".
pub const MAX_CHIPS_PER_DAY: usize = 3;

/// Converts a calendar's colour into the toolkit's colour type.
#[must_use]
pub fn color(rgb: Rgb) -> Color {
    Color::from_rgb8(rgb.0, rgb.1, rgb.2)
}

/// A translucent version of a calendar's colour, for event chip backgrounds.
#[must_use]
pub fn chip_color(rgb: Rgb, alpha: f32) -> Color {
    let c = color(rgb);
    Color { a: alpha, ..c }
}

/// Whether the active theme wants frosted (blurred, translucent) surfaces.
///
/// libcosmic sets [`cosmic::Theme::transparent`] from the compositor's blur
/// state, so reading it here keeps our custom surfaces in step with the panel
/// and the rest of the desktop instead of punching an opaque hole through the
/// window's frosted background.
#[must_use]
pub fn frosted(theme: &cosmic::Theme) -> bool {
    theme.transparent
}

/// The window's own background — respects frosted glass automatically.
#[must_use]
pub fn window_surface() -> cosmic::theme::Container<'static> {
    cosmic::theme::Container::WindowBackground
}

/// A raised surface (the sidebar, the all-day band) that stays translucent when
/// the theme is frosted.
#[must_use]
pub fn panel_surface() -> cosmic::theme::Container<'static> {
    cosmic::theme::Container::custom(|theme| {
        let cosmic = theme.cosmic();
        let container = cosmic.primary(theme.transparent);
        cosmic::iced::widget::container::Style {
            icon_color: Some(Color::from(container.on)),
            text_color: Some(Color::from(container.on)),
            background: Some(cosmic::iced::Background::Color(container.base.into())),
            border: cosmic::iced::Border {
                radius: cosmic.corner_radii.radius_s.into(),
                ..Default::default()
            },
            shadow: cosmic::iced::Shadow::default(),
            snap: true,
        }
    })
}

/// A day cell in the month grid.
///
/// Deliberately unfilled. Painting every cell produces a heavy checkerboard and,
/// worse, hides the compositor's blur behind 42 opaque rectangles. A hairline
/// border carries the grid instead, and today is marked on the date badge rather
/// than by flooding the whole cell.
#[must_use]
pub fn day_cell(today: bool, outside: bool) -> cosmic::theme::Container<'static> {
    cosmic::theme::Container::custom(move |theme| {
        let cosmic = theme.cosmic();
        let on = cosmic.primary(theme.transparent).on;

        let mut line: Color = on.into();
        line.a = if outside { 0.04 } else { 0.09 };

        let background = if today {
            // A whisper of accent, just enough to find today at a glance.
            let mut tint: Color = cosmic.accent_color().into();
            tint.a = 0.07;
            Some(cosmic::iced::Background::Color(tint))
        } else {
            None
        };

        let mut text: Color = on.into();
        if outside {
            text.a = 0.45;
        }

        cosmic::iced::widget::container::Style {
            icon_color: Some(text),
            text_color: Some(text),
            background,
            border: cosmic::iced::Border {
                radius: cosmic.corner_radii.radius_xs.into(),
                width: 1.0,
                color: line,
            },
            shadow: cosmic::iced::Shadow::default(),
            snap: true,
        }
    })
}

/// The filled accent circle behind today's date number.
#[must_use]
pub fn today_badge() -> cosmic::theme::Container<'static> {
    cosmic::theme::Container::custom(|theme| {
        let cosmic = theme.cosmic();
        cosmic::iced::widget::container::Style {
            icon_color: Some(Color::from(cosmic.on_accent_color())),
            text_color: Some(Color::from(cosmic.on_accent_color())),
            background: Some(cosmic::iced::Background::Color(
                cosmic.accent_color().into(),
            )),
            border: cosmic::iced::Border {
                // Radius far larger than the box gives a circle.
                radius: (TODAY_BADGE / 2.0).into(),
                ..Default::default()
            },
            shadow: cosmic::iced::Shadow::default(),
            snap: false,
        }
    })
}

/// The translucent target a drag paints where the event will land.
#[must_use]
pub fn ghost_chip() -> cosmic::theme::Container<'static> {
    cosmic::theme::Container::custom(|theme| {
        let cosmic = theme.cosmic();
        let mut fill: Color = cosmic.accent_color().into();
        fill.a = 0.25;
        cosmic::iced::widget::container::Style {
            background: Some(cosmic::iced::Background::Color(fill)),
            border: cosmic::iced::Border {
                radius: cosmic.corner_radii.radius_xs.into(),
                width: 1.0,
                color: cosmic.accent_color().into(),
            },
            ..Default::default()
        }
    })
}

/// The tint behind hours outside 09:00–18:00 in the time grid, so working
/// hours read as the bright band without a single line of chrome.
#[must_use]
pub fn off_hours_shade() -> cosmic::theme::Container<'static> {
    cosmic::theme::Container::custom(|theme| {
        let mut shade: Color = theme.cosmic().primary(theme.transparent).on.into();
        shade.a = 0.035;
        cosmic::iced::widget::container::Style {
            background: Some(cosmic::iced::Background::Color(shade)),
            ..Default::default()
        }
    })
}

/// A day square in the year view, tinted by how busy the day is.
///
/// `level` is 0 (nothing) to 3 (busiest). The accent carries the scale rather
/// than a separate heat palette, so the view stays in the desktop's own colour
/// and a themed accent takes the heatmap with it.
#[must_use]
pub fn heat_cell(level: usize) -> cosmic::theme::Container<'static> {
    cosmic::theme::Container::custom(move |theme| {
        let cosmic = theme.cosmic();
        let background = match level {
            0 => None,
            n => {
                let mut tint: Color = cosmic.accent_color().into();
                // Four steps, far enough apart to be told apart at 22px.
                tint.a = match n {
                    1 => 0.16,
                    2 => 0.34,
                    _ => 0.55,
                };
                Some(cosmic::iced::Background::Color(tint))
            }
        };
        cosmic::iced::widget::container::Style {
            background,
            border: cosmic::iced::Border {
                radius: cosmic.corner_radii.radius_xs.into(),
                ..Default::default()
            },
            ..Default::default()
        }
    })
}

/// A solid bar of the calendar's colour, used as an event's leading accent.
#[must_use]
pub fn color_bar(rgb: Rgb) -> cosmic::theme::Container<'static> {
    cosmic::theme::Container::custom(move |theme| {
        let cosmic = theme.cosmic();
        cosmic::iced::widget::container::Style {
            background: Some(cosmic::iced::Background::Color(color(rgb))),
            border: cosmic::iced::Border {
                radius: cosmic.corner_radii.radius_xs.into(),
                ..Default::default()
            },
            ..Default::default()
        }
    })
}

/// One of the hairlines ruling the hour grid.
#[must_use]
pub fn hour_rule(emphasis: bool) -> cosmic::theme::Container<'static> {
    cosmic::theme::Container::custom(move |theme| {
        let cosmic = theme.cosmic();
        let mut line: Color = cosmic.primary(theme.transparent).on.into();
        line.a = if emphasis { 0.12 } else { 0.06 };

        cosmic::iced::widget::container::Style {
            background: Some(cosmic::iced::Background::Color(line)),
            ..Default::default()
        }
    })
}

/// The "now" marker drawn across today's column.
#[must_use]
pub fn now_marker() -> cosmic::theme::Container<'static> {
    cosmic::theme::Container::custom(|theme| {
        let cosmic = theme.cosmic();
        cosmic::iced::widget::container::Style {
            background: Some(cosmic::iced::Background::Color(
                cosmic.destructive_color().into(),
            )),
            border: cosmic::iced::Border {
                radius: 1.5.into(),
                ..Default::default()
            },
            ..Default::default()
        }
    })
}

/// The dot that caps the "now" line.
#[must_use]
pub fn now_dot() -> cosmic::theme::Container<'static> {
    cosmic::theme::Container::custom(|theme| {
        let cosmic = theme.cosmic();
        cosmic::iced::widget::container::Style {
            background: Some(cosmic::iced::Background::Color(
                cosmic.destructive_color().into(),
            )),
            border: cosmic::iced::Border {
                radius: 4.0.into(),
                ..Default::default()
            },
            ..Default::default()
        }
    })
}

/// An event, tinted with its calendar's colour.
///
/// The tint is kept low-alpha so the compositor's blur still reads through it;
/// the solid colour lives in the leading bar instead, which is what makes the
/// calendar identifiable at a glance without turning the grid into blocks of paint.
#[must_use]
pub fn event_chip(rgb: Rgb) -> cosmic::theme::Button {
    cosmic::theme::Button::Custom {
        active: Box::new(move |_focused, theme| chip_style(rgb, theme, 0.20)),
        disabled: Box::new(move |theme| chip_style(rgb, theme, 0.10)),
        hovered: Box::new(move |_focused, theme| chip_style(rgb, theme, 0.32)),
        pressed: Box::new(move |_focused, theme| chip_style(rgb, theme, 0.42)),
    }
}

fn chip_style(rgb: Rgb, theme: &cosmic::Theme, alpha: f32) -> cosmic::widget::button::Style {
    let cosmic = theme.cosmic();
    cosmic::widget::button::Style {
        background: Some(cosmic::iced::Background::Color(chip_color(rgb, alpha))),
        border_radius: cosmic.corner_radii.radius_xs.into(),
        border_width: 0.0,
        border_color: Color::TRANSPARENT,
        icon_color: Some(color(rgb)),
        text_color: Some(Color::from(cosmic.primary(theme.transparent).on)),
        outline_width: 0.0,
        outline_color: Color::TRANSPARENT,
        ..Default::default()
    }
}

/// A whole row made activatable, without repainting what is inside it.
///
/// The list views need their rows to be reachable by keyboard — a `mouse_area`
/// takes a click but cannot take focus — while a stock button would impose its
/// own text colour on every label in the row, flattening the dimmed captions
/// and the overdue red. This keeps the button's focus ring and hover feedback
/// and leaves `text_color` unset, so each child keeps the class it chose.
#[must_use]
pub fn row_button() -> cosmic::theme::Button {
    fn style(theme: &cosmic::Theme, alpha: f32) -> cosmic::widget::button::Style {
        let cosmic = theme.cosmic();
        let mut fill: Color = cosmic.primary(theme.transparent).on.into();
        fill.a = alpha;
        cosmic::widget::button::Style {
            background: (alpha > 0.0).then_some(cosmic::iced::Background::Color(fill)),
            border_radius: cosmic.corner_radii.radius_xs.into(),
            // Deliberately not set: the row's own children carry their colours.
            text_color: None,
            ..Default::default()
        }
    }

    cosmic::theme::Button::Custom {
        active: Box::new(|_focused, theme| style(theme, 0.0)),
        disabled: Box::new(|theme| style(theme, 0.0)),
        hovered: Box::new(|_focused, theme| style(theme, 0.06)),
        pressed: Box::new(|_focused, theme| style(theme, 0.10)),
    }
}

/// Secondary text — column headers, times, hints. Dimmed rather than recoloured,
/// so it stays legible against whatever the theme and the blur put behind it.
#[must_use]
pub fn dim_text(theme: &cosmic::Theme) -> cosmic::iced::widget::text::Style {
    let mut color: Color = theme.cosmic().primary(theme.transparent).on.into();
    color.a = 0.6;
    cosmic::iced::widget::text::Style {
        color: Some(color),
        ..Default::default()
    }
}

/// Formats a time of day, honouring the 24-hour setting.
#[must_use]
pub fn format_time(time: NaiveTime, config: &Config) -> String {
    if config.time_24h {
        time.format("%H:%M").to_string()
    } else {
        time.format("%-I:%M %p").to_string()
    }
}

/// Formats an occurrence's time range for a chip label.
#[must_use]
pub fn format_range(start: NaiveDateTime, end: NaiveDateTime, config: &Config) -> String {
    if start.date() == end.date() {
        format!(
            "{} – {}",
            format_time(start.time(), config),
            format_time(end.time(), config)
        )
    } else {
        format!(
            "{} {} – {} {}",
            format_date_short(start.date()),
            format_time(start.time(), config),
            format_date_short(end.date()),
            format_time(end.time(), config)
        )
    }
}

/// Localised full month name.
#[must_use]
pub fn month_name(month: u32) -> String {
    match month {
        1 => fl!("month-january"),
        2 => fl!("month-february"),
        3 => fl!("month-march"),
        4 => fl!("month-april"),
        5 => fl!("month-may"),
        6 => fl!("month-june"),
        7 => fl!("month-july"),
        8 => fl!("month-august"),
        9 => fl!("month-september"),
        10 => fl!("month-october"),
        11 => fl!("month-november"),
        _ => fl!("month-december"),
    }
}

/// Localised short weekday name, for column headers.
#[must_use]
pub fn weekday_short(weekday: Weekday) -> String {
    match weekday {
        Weekday::Mon => fl!("weekday-mon"),
        Weekday::Tue => fl!("weekday-tue"),
        Weekday::Wed => fl!("weekday-wed"),
        Weekday::Thu => fl!("weekday-thu"),
        Weekday::Fri => fl!("weekday-fri"),
        Weekday::Sat => fl!("weekday-sat"),
        Weekday::Sun => fl!("weekday-sun"),
    }
}

#[must_use]
pub fn format_date_short(date: NaiveDate) -> String {
    format!("{} {}", date.day(), month_name(date.month()))
}

/// The title shown in the header for a given view and anchor date.
#[must_use]
pub fn range_title(view: crate::config::ViewKind, anchor: NaiveDate, config: &Config) -> String {
    use crate::config::ViewKind;
    match view {
        ViewKind::Month => format!("{} {}", month_name(anchor.month()), anchor.year()),
        ViewKind::Week => {
            let start = week_start(anchor, config.first_weekday());
            let end = start + Duration::days(6);
            if start.month() == end.month() {
                format!(
                    "{} – {} {}",
                    start.day(),
                    format_date_short(end),
                    end.year()
                )
            } else {
                format!(
                    "{} – {} {}",
                    format_date_short(start),
                    format_date_short(end),
                    end.year()
                )
            }
        }
        ViewKind::Day => format!(
            "{} {} {}",
            weekday_short(anchor.weekday()),
            format_date_short(anchor),
            anchor.year()
        ),
        ViewKind::Agenda => {
            let end = anchor + Duration::days(ViewKind::AGENDA_DAYS - 1);
            format!(
                "{} – {} {}",
                format_date_short(anchor),
                format_date_short(end),
                end.year()
            )
        }
        ViewKind::Year => anchor.year().to_string(),
        ViewKind::Tasks => fl!("tasks"),
    }
}

/// The first day of the week containing `date`, given the user's week start.
#[must_use]
pub fn week_start(date: NaiveDate, first: Weekday) -> NaiveDate {
    let offset = (date.weekday().num_days_from_monday() as i64
        - first.num_days_from_monday() as i64)
        .rem_euclid(7);
    date - Duration::days(offset)
}

/// The inclusive-exclusive date range a view covers.
///
/// Month view deliberately spills into the adjacent months so the grid is always
/// a full six rows.
#[must_use]
pub fn visible_range(
    view: crate::config::ViewKind,
    anchor: NaiveDate,
    config: &Config,
) -> (NaiveDate, NaiveDate) {
    use crate::config::ViewKind;
    match view {
        ViewKind::Month => {
            let first_of_month = anchor.with_day(1).unwrap_or(anchor);
            let start = week_start(first_of_month, config.first_weekday());
            (start, start + Duration::days(MONTH_ROWS * 7))
        }
        ViewKind::Week => {
            let start = week_start(anchor, config.first_weekday());
            (start, start + Duration::days(7))
        }
        ViewKind::Agenda => (anchor, anchor + Duration::days(ViewKind::AGENDA_DAYS)),
        // The whole calendar year, plus the leading and trailing days the mini
        // months' week rows reach into.
        ViewKind::Year => {
            let first = NaiveDate::from_ymd_opt(anchor.year(), 1, 1).unwrap_or(anchor);
            let last = NaiveDate::from_ymd_opt(anchor.year(), 12, 31).unwrap_or(anchor);
            (
                week_start(first, config.first_weekday()),
                week_start(last, config.first_weekday()) + Duration::days(7),
            )
        }
        ViewKind::Day | ViewKind::Tasks => (anchor, anchor + Duration::days(1)),
    }
}

/// Weekday headers in the user's preferred order.
#[must_use]
pub fn weekday_order(first: Weekday) -> Vec<Weekday> {
    let mut day = first;
    let mut out = Vec::with_capacity(7);
    for _ in 0..7 {
        out.push(day);
        day = day.succ();
    }
    out
}

/// ISO week number, shown in the gutter when the setting is on.
#[must_use]
pub fn week_number(date: NaiveDate) -> u32 {
    date.iso_week().week()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn day(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    #[test]
    fn the_year_range_covers_every_day_of_the_year() {
        let config = Config::default();
        let (from, to) = visible_range(crate::config::ViewKind::Year, day(2026, 6, 15), &config);

        // Every mini month's grid is drawn from a week boundary, so the range
        // has to start on one and reach past 31 December.
        assert_eq!(from.weekday(), config.first_weekday());
        assert!(from <= day(2026, 1, 1), "January is not covered: {from}");
        assert!(to > day(2026, 12, 31), "December is not covered: {to}");
    }

    #[test]
    fn the_year_range_follows_the_anchor_year() {
        let config = Config::default();
        let (from, to) = visible_range(crate::config::ViewKind::Year, day(2027, 3, 2), &config);
        assert!(from <= day(2027, 1, 1) && to > day(2027, 12, 31));
        assert!(from > day(2026, 12, 1), "leaked into the previous year");
    }

    #[test]
    fn week_start_respects_the_configured_first_day() {
        // 2026-08-04 is a Tuesday.
        let tuesday = day(2026, 8, 4);
        assert_eq!(week_start(tuesday, Weekday::Mon), day(2026, 8, 3));
        assert_eq!(week_start(tuesday, Weekday::Sun), day(2026, 8, 2));
        assert_eq!(week_start(tuesday, Weekday::Sat), day(2026, 8, 1));
    }

    #[test]
    fn week_start_of_the_first_day_is_itself() {
        assert_eq!(week_start(day(2026, 8, 3), Weekday::Mon), day(2026, 8, 3));
    }

    #[test]
    fn month_range_is_always_six_full_weeks() {
        let config = Config::default();
        for month in 1..=12 {
            let (start, end) = visible_range(
                crate::config::ViewKind::Month,
                day(2026, month, 15),
                &config,
            );
            assert_eq!(
                (end - start).num_days(),
                42,
                "month {month} did not produce a 6x7 grid"
            );
            assert_eq!(start.weekday(), Weekday::Mon);
        }
    }

    #[test]
    fn month_range_covers_the_whole_month() {
        let config = Config::default();
        let (start, end) = visible_range(crate::config::ViewKind::Month, day(2026, 8, 15), &config);
        assert!(start <= day(2026, 8, 1));
        assert!(end > day(2026, 8, 31));
    }

    #[test]
    fn weekday_order_starts_where_asked() {
        assert_eq!(weekday_order(Weekday::Mon)[0], Weekday::Mon);
        assert_eq!(weekday_order(Weekday::Sun)[0], Weekday::Sun);
        assert_eq!(weekday_order(Weekday::Sun)[6], Weekday::Sat);
        assert_eq!(weekday_order(Weekday::Mon).len(), 7);
    }

    #[test]
    fn time_format_follows_the_setting() {
        let t = NaiveTime::from_hms_opt(14, 5, 0).unwrap();
        let c24 = Config {
            time_24h: true,
            ..Config::default()
        };
        let c12 = Config {
            time_24h: false,
            ..Config::default()
        };
        assert_eq!(format_time(t, &c24), "14:05");
        assert_eq!(format_time(t, &c12), "2:05 PM");
    }
}
