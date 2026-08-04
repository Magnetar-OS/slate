// SPDX-License-Identifier: GPL-3.0-only

//! Fires calendar reminders while the calendar window is closed.
//!
//! The app itself only notifies while it is running, which is not much use for a
//! meeting reminder. This daemon shares the same [`Store`] and
//! [`Scheduler`](cosmic_calendar::reminders::Scheduler) and does nothing else, so
//! it stays small enough to leave running.
//!
//! It claims [`OWNER_BUS_NAME`](cosmic_calendar::reminders::OWNER_BUS_NAME) on
//! startup. The app watches for that name and suppresses its own reminders while
//! it is held, so having both running does not double every notification.

use chrono::Duration;
use cosmic_calendar::config::Config;
use cosmic_calendar::model::Occurrence;
use cosmic_calendar::reminders::{self, Scheduler};
use cosmic_calendar::store::{Store, watcher};

const APP_ID: &str = "io.github.entro314labs.Calendar";

/// How often to re-check for due reminders.
///
/// Reminder triggers land on whole minutes, and the scheduler tolerates a five
/// minute lag, so half a minute is plenty and keeps the daemon effectively idle.
const TICK: std::time::Duration = std::time::Duration::from_secs(30);

#[tokio::main]
async fn main() -> std::process::ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "cosmic_calendar=info".into()),
        )
        .init();

    // Held for the lifetime of the process: dropping it releases the name and
    // would silently hand reminders back to the app.
    let _connection = match reminders::claim_ownership().await {
        Ok(Some(connection)) => connection,
        Ok(None) => {
            tracing::info!("another process already owns reminders; exiting");
            return std::process::ExitCode::SUCCESS;
        }
        Err(why) => {
            tracing::error!(%why, "cannot reach the session bus");
            return std::process::ExitCode::FAILURE;
        }
    };

    let mut store = match Store::open_default() {
        Ok(store) => store,
        Err(why) => {
            tracing::error!(%why, "cannot open the calendar store");
            return std::process::ExitCode::FAILURE;
        }
    };

    // Watching means a vdirsyncer run is picked up without waiting for the next
    // tick to notice the mtimes changed.
    let watch = watcher::watch(store.root());
    let (_watch, mut changes) = match watch {
        Ok(pair) => (Some(pair.0), Some(pair.1)),
        Err(why) => {
            tracing::warn!(%why, "cannot watch for changes; falling back to polling");
            (None, None)
        }
    };

    let mut scheduler = Scheduler::new();
    let mut ticker = tokio::time::interval(TICK);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    tracing::info!(root = %store.root().display(), "watching for reminders");

    let mut shutdown = signals();

    loop {
        tokio::select! {
            _ = ticker.tick() => {
                check(&mut store, &mut scheduler).await;
            }
            Some(()) = recv(&mut changes) => {
                if let Err(why) = store.refresh() {
                    tracing::warn!(%why, "refresh after a file change failed");
                }
                check(&mut store, &mut scheduler).await;
            }
            () = &mut shutdown => {
                tracing::info!("shutting down");
                return std::process::ExitCode::SUCCESS;
            }
        }
    }
}

/// Receives from the watcher if there is one, otherwise never resolves.
async fn recv(rx: &mut Option<tokio::sync::mpsc::Receiver<()>>) -> Option<()> {
    match rx {
        Some(rx) => rx.recv().await,
        None => std::future::pending().await,
    }
}

fn signals() -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> {
    Box::pin(async {
        let mut term =
            match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
                Ok(signal) => signal,
                Err(why) => {
                    tracing::warn!(%why, "cannot listen for SIGTERM");
                    std::future::pending::<()>().await;
                    return;
                }
            };

        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = term.recv() => {}
        }
    })
}

/// One pass: reload config, look at the next two days, notify whatever is due.
async fn check(store: &mut Store, scheduler: &mut Scheduler) {
    // Re-read each pass so a settings change takes effect without a restart.
    let config = load_config();
    let now = chrono::Local::now().naive_local();
    let today = now.date();

    let occurrences =
        match store.occurrences(today, today + Duration::days(2), &config.hidden_set()) {
            Ok(occurrences) => occurrences,
            Err(why) => {
                tracing::warn!(%why, "could not load occurrences");
                return;
            }
        };

    let alarms_for = |occurrence: &Occurrence| {
        store
            .event(&occurrence.calendar_id, &occurrence.uid)
            .ok()
            .flatten()
            .map(|event| event.alarms)
            .unwrap_or_default()
    };

    let due = scheduler.due(&occurrences, alarms_for, now, config.default_reminder());

    for reminder in &due {
        tracing::info!(summary = %reminder.summary, "reminder due");
        reminders::notify(reminder, APP_ID, body(reminder, &config)).await;
    }

    scheduler.forget_before(now - Duration::days(1));
}

fn body(reminder: &reminders::Reminder, config: &Config) -> String {
    let when = if reminder.all_day {
        "All day".to_owned()
    } else {
        let minutes = reminder.lead.num_minutes();
        if minutes <= 0 {
            "Starting now".to_owned()
        } else if minutes < 60 {
            let unit = if minutes == 1 { "minute" } else { "minutes" };
            format!("In {minutes} {unit}")
        } else {
            format!(
                "At {}",
                cosmic_calendar::ui::format_time(reminder.start.time(), config)
            )
        }
    };

    match &reminder.location {
        Some(location) if !location.is_empty() => format!("{when} · {location}"),
        _ => when,
    }
}

/// Reads the app's settings straight from `cosmic-config`.
///
/// The daemon has no UI, so it only ever reads; defaults are fine if the app has
/// never been run.
fn load_config() -> Config {
    use cosmic::cosmic_config::CosmicConfigEntry;

    cosmic::cosmic_config::Config::new(APP_ID, Config::VERSION)
        .ok()
        .map(|handler| match Config::get_entry(&handler) {
            Ok(config) => config,
            Err((_errors, config)) => config,
        })
        .unwrap_or_default()
}
