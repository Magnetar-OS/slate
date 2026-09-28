// SPDX-License-Identifier: GPL-3.0-only

//! Keeps the calendar current while the window is closed: fires reminders, and
//! runs CalDAV sync.
//!
//! The app itself only notifies while it is running, which is not much use for a
//! meeting reminder — and only syncs while it is running, which is not much use
//! for a calendar. This daemon shares the same [`Store`] and
//! [`Scheduler`](slate::reminders::Scheduler), so it stays small
//! enough to leave running.
//!
//! # Why sync lives here rather than in the app
//!
//! Sync must happen whether or not anyone has the window open, and it must not
//! happen twice at once. Both fall out of the daemon already owning a D-Bus
//! name: whoever holds it runs every sync pass, one at a time, and serves
//! [`slate::background::Service`] on it so the app's "Sync now" asks the
//! daemon instead of starting a second pass of its own.
//!
//! It claims [`OWNER_BUS_NAME`](slate::reminders::OWNER_BUS_NAME) on
//! startup. The app follows that name for as long as it runs and suppresses its
//! own reminders exactly while it is held, so having both running does not
//! double every notification, and the daemon stopping does not silence them.

use slate::background;
use slate::clock::{follow_timezone, now_in};
use slate::config::Config;
use slate::reminders::{self, Scheduler, SleepWindow};
use slate::store::{Store, watcher};

const APP_ID: &str = "com.magnetaros.Slate";

/// How often to re-check for due reminders.
///
/// Reminder triggers land on whole minutes, and the scheduler tolerates a five
/// minute lag, so half a minute is plenty and keeps the daemon effectively idle.
const TICK: std::time::Duration = std::time::Duration::from_secs(30);

/// How often to run a CalDAV sync cycle.
///
/// Five minutes is the interval every desktop calendar has converged on, and
/// the ctag check makes an idle cycle one cheap PROPFIND per collection rather
/// than a full listing — so this is far less traffic than it sounds.
const SYNC_TICK: std::time::Duration = std::time::Duration::from_secs(5 * 60);

#[tokio::main]
async fn main() -> std::process::ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "slate=info".into()),
        )
        .init();

    // Reminder bodies are Fluent strings, so the daemon needs the catalogue
    // selected too — otherwise notifications fired with the window closed come
    // out in English while the same reminder from the app is translated.
    slate::i18n::init(&i18n_embed::DesktopLanguageRequester::requested_languages());

    // Held while a sync or feed pass runs, whoever started it: the timer
    // below, or the app over the bus.
    let running = std::sync::Arc::new(tokio::sync::Mutex::new(()));

    // Held for the lifetime of the process: dropping it releases the name and
    // would silently hand reminders back to the app.
    let _connection = match become_owner(running.clone()).await {
        Ok(Some(connection)) => connection,
        Ok(None) => {
            tracing::info!("another process already owns reminders; exiting");
            return std::process::ExitCode::SUCCESS;
        }
        Err(why) => {
            tracing::error!(%why, "cannot claim the reminder name on the session bus");
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

    // What already fired survives a restart: `Restart=on-failure`, a package
    // upgrade or a new login must not replay alarms still inside their grace.
    let mut scheduler = Scheduler::persistent(reminders::fired_path());
    let mut ticker = tokio::time::interval(TICK);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    let mut sync_ticker = tokio::time::interval(SYNC_TICK);

    // The daemon is what is running while the machine sleeps, so the
    // missed-reminder digest is its job. `on_sleep` never returns while login1
    // is reachable and simply ends where it is not.
    let (sleep_tx, mut sleep_rx) = tokio::sync::mpsc::channel::<bool>(4);
    tokio::spawn(async move {
        reminders::on_sleep(move |going_down| {
            let _ = sleep_tx.try_send(going_down);
        })
        .await;
    });
    // Where the sleep window being reported on began.
    let mut sleep = SleepWindow::new(now_in(store.local_timezone()));
    // `Delay` rather than `Burst`: after a suspend-resume the missed ticks must
    // not all fire at once and start several overlapping sync passes.
    sync_ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    tracing::info!(root = %store.root().display(), "watching for reminders");

    let mut shutdown = signals();
    // A background pass reporting that something landed on disk.
    let (landed_tx, mut landed_rx) = tokio::sync::mpsc::channel::<()>(1);

    loop {
        tokio::select! {
            _ = ticker.tick() => {
                // A timezone change moves every occurrence's wall clock.
                if let Err(why) = follow_timezone(&mut store) {
                    tracing::warn!(%why, "could not follow the timezone change");
                }
                sleep.swept(check(&mut store, &mut scheduler));
            }
            Some(going_down) = sleep_rx.recv() => {
                if going_down {
                    sleep.going_down(now_in(store.local_timezone()));
                    continue;
                }
                // Back from suspend: say how many alarms passed, once, then
                // resume the normal sweep.
                if let Err(why) = store.refresh() {
                    tracing::warn!(%why, "refresh after resume failed");
                }
                report_missed(&mut store, &mut scheduler, sleep.woke()).await;
                sleep.swept(check(&mut store, &mut scheduler));
            }
            _ = sync_ticker.tick() => spawn_background_pass(&running, landed_tx.clone()),
            Some(()) = landed_rx.recv() => {
                if let Err(why) = store.refresh() {
                    tracing::warn!(%why, "refresh after a sync failed");
                }
            }
            Some(()) = recv(&mut changes) => {
                if let Err(why) = store.refresh() {
                    tracing::warn!(%why, "refresh after a file change failed");
                }
                sleep.swept(check(&mut store, &mut scheduler));
            }
            () = &mut shutdown => {
                tracing::info!("shutting down");
                return std::process::ExitCode::SUCCESS;
            }
        }
    }
}

/// Claims the owner name and serves the sync interface on it. `Ok(None)`
/// means another daemon already holds the name.
async fn become_owner(
    running: std::sync::Arc<tokio::sync::Mutex<()>>,
) -> Result<Option<zbus::Connection>, zbus::Error> {
    let session = zbus::Connection::session().await?;
    let Some(connection) = reminders::claim_ownership(session).await? else {
        return Ok(None);
    };
    if let Err(why) = connection
        .object_server()
        .at(
            background::OBJECT_PATH,
            background::Service::new(
                running,
                background::sync_accounts,
                background::refresh_feeds,
            ),
        )
        .await
    {
        // Reminders still work; the app's "Sync now" will report the error.
        tracing::warn!(%why, "could not serve the sync interface");
    }
    Ok(Some(connection))
}

/// One background pass — accounts, then due feeds — unless one is already
/// running (the timer's last, or one the app asked for over the bus). Tells
/// `landed` when anything changed on disk.
///
/// Spawned rather than awaited in the main loop: a pass can spend many
/// seconds on the network, and reminders must keep firing meanwhile.
fn spawn_background_pass(
    running: &std::sync::Arc<tokio::sync::Mutex<()>>,
    landed: tokio::sync::mpsc::Sender<()>,
) {
    let Ok(guard) = running.clone().try_lock_owned() else {
        tracing::debug!("a sync pass is still running; skipping this tick");
        return;
    };
    tokio::spawn(async move {
        let _guard = guard;
        let changed = tokio::task::spawn_blocking(|| {
            let (_, synced) = background::sync_accounts();
            let fed = background::refresh_feeds(false);
            synced || fed
        })
        .await
        .unwrap_or_else(|why| {
            tracing::error!(%why, "the sync task panicked");
            false
        });
        if changed {
            let _ = landed.try_send(());
        }
    });
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

/// One pass: reload config, notify whatever is due. Returns the instant it
/// looked — the start of the next sleep window.
fn check(store: &mut Store, scheduler: &mut Scheduler) -> chrono::NaiveDateTime {
    // Re-read each pass so a settings change takes effect without a restart.
    let config = load_config();
    let now = now_in(store.local_timezone());

    match reminders::due_reminders(store, scheduler, &config, now) {
        Ok(due) => {
            for reminder in due {
                tracing::info!(summary = %reminder.summary, "reminder due");
                let body = reminder.body(&config);
                // Spawned: a notification with a Join button stays with its
                // button until the meeting ends, and the sweep must not wait.
                tokio::spawn(async move { reminders::notify(&reminder, APP_ID, body).await });
            }
        }
        Err(why) => tracing::warn!(%why, "could not load occurrences"),
    }
    now
}

/// One notification for the alarms that passed while the machine slept.
async fn report_missed(
    store: &mut Store,
    scheduler: &mut Scheduler,
    slept_at: chrono::NaiveDateTime,
) {
    let config = load_config();
    let now = now_in(store.local_timezone());

    match reminders::missed_reminders(store, scheduler, &config, slept_at, now) {
        Ok(missed) if missed > 0 => {
            tracing::info!(missed, "reminders passed while asleep");
            reminders::notify_missed(missed, APP_ID).await;
        }
        Ok(_) => {}
        Err(why) => tracing::warn!(%why, "could not load occurrences"),
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
