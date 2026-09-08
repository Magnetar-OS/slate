// SPDX-License-Identifier: GPL-3.0-only

//! A calendar for the COSMIC desktop.
//!
//! The crate is a library so the four binaries — the app, the panel applet, the
//! reminder daemon, and the launcher plugin — can share the model and storage
//! layers rather than talking to each other.
//!
//! [`model`] and [`store`] are re-exports from `cosmic-pim-core`, which is
//! MPL-2.0 rather than this package's GPL-3.0-only: they are the substrate the
//! whole suite shares, not calendar-specific code. Re-exporting rather than
//! having callers say `cosmic_pim_core::` keeps every in-app path
//! (`crate::model::Event`, `crate::store::Store`) unchanged. See LICENSING.md.

pub use cosmic_pim_core::{model, store};

pub mod app;
pub mod config;
pub mod i18n;
pub mod key_bind;
pub mod meeting;
pub mod quickadd;
pub mod reminders;
pub mod scheduling;
pub mod ui;
pub mod undo;

use std::path::PathBuf;

/// Runs the main application.
pub fn run() -> cosmic::iced::Result {
    // `RUST_LOG=slate=debug` turns on our own tracing without the
    // toolkit's chatter.
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "slate=warn".into()),
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

    // `run_single_instance` rather than `run`: it is the only entry point that
    // sets `Core::single_instance`, and libcosmic gates the D-Bus activation
    // subscription on that flag. Without it the `DBusActivatable=true` in the
    // desktop entry has nothing behind it, `AppModel::dbus_activation` never
    // fires, and a second launch opens a second window.
    cosmic::app::run_single_instance::<app::AppModel>(
        settings,
        parse_args(std::env::args().skip(1)),
    )
}

/// Parses the command line.
///
/// Deliberately tiny. `--date=YYYY-MM-DD` is what the applet and the launcher
/// plugin use to open us on a given day; `--today` and `--new-event` back the
/// desktop entry's two actions for the case where there is no running instance
/// to hand them to over the bus; and bare paths are `.ics` files to import,
/// which is how the desktop passes a file we are registered to handle.
#[must_use]
pub fn parse_args(args: impl Iterator<Item = String>) -> app::Flags {
    let mut initial_date = None;
    let mut import = Vec::new();
    let mut new_event = false;

    for arg in args {
        if let Some(value) = arg.strip_prefix("--date=") {
            match value.parse::<chrono::NaiveDate>() {
                Ok(date) => initial_date = Some(date),
                Err(why) => tracing::warn!(value, %why, "ignoring unparseable --date"),
            }
        } else if arg == "--today" {
            initial_date = Some(chrono::Local::now().date_naive());
        } else if arg == "--new-event" {
            new_event = true;
        } else if arg.starts_with('-') {
            tracing::warn!(arg, "ignoring unknown option");
        } else {
            import.push(PathBuf::from(arg));
        }
    }

    app::Flags::new(initial_date, import, new_event)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> app::Flags {
        parse_args(args.iter().map(ToString::to_string))
    }

    #[test]
    fn no_arguments_means_today_and_no_imports() {
        let flags = parse(&[]);
        assert!(flags.initial_date.is_none());
        assert!(flags.import.is_empty());
    }

    #[test]
    fn date_is_parsed() {
        assert_eq!(
            parse(&["--date=2026-08-04"]).initial_date,
            chrono::NaiveDate::from_ymd_opt(2026, 8, 4)
        );
    }

    #[test]
    fn a_bad_date_is_ignored_rather_than_fatal() {
        assert!(parse(&["--date=not-a-date"]).initial_date.is_none());
    }

    #[test]
    fn bare_paths_are_imports() {
        let flags = parse(&["/tmp/a.ics", "/tmp/b.ics"]);
        assert_eq!(flags.import.len(), 2);
        assert_eq!(flags.import[0], PathBuf::from("/tmp/a.ics"));
    }

    #[test]
    fn unknown_options_do_not_become_imports() {
        // Otherwise `--verbose` would be treated as a file to open.
        let flags = parse(&["--verbose", "/tmp/a.ics"]);
        assert_eq!(flags.import, vec![PathBuf::from("/tmp/a.ics")]);
    }

    #[test]
    fn desktop_actions_have_command_line_equivalents() {
        assert!(parse(&["--new-event"]).new_event);
        assert_eq!(
            parse(&["--today"]).initial_date,
            Some(chrono::Local::now().date_naive())
        );
    }

    #[test]
    fn an_empty_command_line_asks_a_running_instance_for_nothing() {
        use cosmic::app::CosmicFlags;
        // A `None` action is what tells libcosmic to just raise the window.
        assert!(parse(&[]).action().is_none());
    }

    #[test]
    fn a_request_survives_the_round_trip_across_the_bus() {
        use cosmic::app::CosmicFlags;

        let flags = parse(&["--date=2026-08-04", "--new-event", "/tmp/a.ics"]);
        let encoded = flags.action().expect("a request to hand over").to_string();
        let decoded: app::SlateTask = encoded.parse().expect("round trip");

        assert_eq!(decoded.date, chrono::NaiveDate::from_ymd_opt(2026, 8, 4));
        assert!(decoded.new_event);
        // Paths travel separately, as the activation argument list.
        assert_eq!(flags.args(), vec!["/tmp/a.ics"]);
    }
}
