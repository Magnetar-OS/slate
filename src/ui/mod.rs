// SPDX-License-Identifier: GPL-3.0-only

//! View code, and the small vocabulary the views share.

pub mod editor;
pub mod month;
pub mod sidebar;
pub mod timegrid;

use crate::config::Config;
use crate::fl;
use crate::model::Rgb;
use chrono::{Datelike, Duration, NaiveDate, NaiveDateTime, NaiveTime, Weekday};
use cosmic::iced::Color;

/// Rows in a month grid. Six always covers a month, whatever weekday it starts on,
/// and keeping it fixed stops the grid resizing as you page through the year.
pub const MONTH_ROWS: i64 = 6;

/// Height of one hour in the week and day grids.
pub const HOUR_HEIGHT: f32 = 44.0;

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

/// A raised surface (grid cells, the sidebar) that stays translucent when the
/// theme is frosted.
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

/// A single day cell. `today` and `outside` (a day from an adjacent month) get
/// distinct treatments.
#[must_use]
pub fn day_cell(today: bool, outside: bool) -> cosmic::theme::Container<'static> {
    cosmic::theme::Container::custom(move |theme| {
        let cosmic = theme.cosmic();
        let container = cosmic.primary(theme.transparent);

        let mut background: Color = container.base.into();
        if outside {
            // Recede rather than repaint: dropping alpha keeps whatever the
            // compositor blurred behind us visible.
            background.a *= 0.45;
        }

        let border = if today {
            cosmic::iced::Border {
                radius: cosmic.corner_radii.radius_s.into(),
                width: 1.5,
                color: cosmic.accent_color().into(),
            }
        } else {
            cosmic::iced::Border {
                radius: cosmic.corner_radii.radius_s.into(),
                ..Default::default()
            }
        };

        cosmic::iced::widget::container::Style {
            icon_color: Some(Color::from(container.on)),
            text_color: Some(Color::from(container.on)),
            background: Some(cosmic::iced::Background::Color(background)),
            border,
            shadow: cosmic::iced::Shadow::default(),
            snap: true,
        }
    })
}

/// An event chip tinted with its calendar's colour.
#[must_use]
pub fn event_chip(rgb: Rgb) -> cosmic::theme::Button {
    cosmic::theme::Button::Custom {
        active: Box::new(move |_focused, theme| chip_style(rgb, theme, 0.22)),
        disabled: Box::new(move |theme| chip_style(rgb, theme, 0.12)),
        hovered: Box::new(move |_focused, theme| chip_style(rgb, theme, 0.34)),
        pressed: Box::new(move |_focused, theme| chip_style(rgb, theme, 0.44)),
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
        ViewKind::Day => (anchor, anchor + Duration::days(1)),
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
