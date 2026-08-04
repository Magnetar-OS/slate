// SPDX-License-Identifier: GPL-3.0-only

//! A calendar for the COSMIC desktop.
//!
//! The crate is split so the parts that do not need a display can be tested
//! without one:
//!
//! - [`model`] — events, calendars, and recurrence expansion. No toolkit types.
//! - [`store`] — iCalendar files in a vdir, with a SQLite index in front.
//! - [`ui`] / [`app`] — the libcosmic front end.

pub mod app;
pub mod config;
pub mod i18n;
pub mod model;
pub mod store;
pub mod ui;

/// Starts the application.
pub fn run() -> cosmic::iced::Result {
    // `RUST_LOG=cosmic_calendar=debug` turns on our own tracing without the
    // toolkit's chatter.
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "cosmic_calendar=warn".into()),
        )
        .init();

    let requested_languages = i18n_embed::DesktopLanguageRequester::requested_languages();
    i18n::init(&requested_languages);

    // `Settings::transparent` defaults to true, and `Core::auto_blur` defaults to
    // covering windows and popups. Together those are what let the compositor
    // apply COSMIC's frosted-glass effect behind us, so neither is overridden here.
    let settings = cosmic::app::Settings::default()
        // Wide enough that the sidebar, the grid, and the full header bar all
        // fit without the header collapsing.
        .size(cosmic::iced::Size::new(1360.0, 860.0))
        .size_limits(
            cosmic::iced::Limits::NONE
                .min_width(920.0)
                .min_height(560.0),
        );

    cosmic::app::run::<app::AppModel>(settings, ())
}
