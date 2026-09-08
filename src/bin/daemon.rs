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
//! name: whoever holds it is the single writer, and the app defers to it.
//!
//! It claims [`OWNER_BUS_NAME`](slate::reminders::OWNER_BUS_NAME) on
//! startup. The app watches for that name and suppresses its own reminders while
//! it is held, so having both running does not double every notification.

use chrono::Duration;
use cosmic_pim_accounts::AccountStore;
use slate::config::Config;
use slate::model::Occurrence;
use slate::reminders::{self, Scheduler};
use slate::store::{Store, watcher};

const APP_ID: &str = "io.github.entro314labs.Slate";

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

    let mut sync_ticker = tokio::time::interval(SYNC_TICK);

    // The daemon is what is running while the machine sleeps, so the
    // missed-reminder digest is its job. `on_wake` never returns while login1
    // is reachable and simply ends where it is not.
    let (wake_tx, mut wake_rx) = tokio::sync::mpsc::channel::<()>(1);
    tokio::spawn(async move {
        reminders::on_wake(move || {
            let _ = wake_tx.try_send(());
        })
        .await;
    });
    // When the last sweep ran, which is the start of any sleep window.
    let mut last_sweep = chrono::Local::now().naive_local();
    // `Delay` rather than `Burst`: after a suspend-resume the missed ticks must
    // not all fire at once and start several overlapping sync passes.
    sync_ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    tracing::info!(root = %store.root().display(), "watching for reminders");

    let mut shutdown = signals();

    loop {
        tokio::select! {
            _ = ticker.tick() => {
                last_sweep = check(&mut store, &mut scheduler).await;
            }
            Some(()) = wake_rx.recv() => {
                // Back from suspend: say how many alarms passed, once, then
                // resume the normal sweep.
                if let Err(why) = store.refresh() {
                    tracing::warn!(%why, "refresh after resume failed");
                }
                report_missed(&mut store, &mut scheduler, last_sweep).await;
                last_sweep = check(&mut store, &mut scheduler).await;
            }
            _ = sync_ticker.tick() => {
                let synced = sync_once().await;
                let fed = refresh_feeds().await;
                if (synced || fed) && let Err(why) = store.refresh() {
                    tracing::warn!(%why, "refresh after a sync failed");
                }
            }
            Some(()) = recv(&mut changes) => {
                if let Err(why) = store.refresh() {
                    tracing::warn!(%why, "refresh after a file change failed");
                }
                last_sweep = check(&mut store, &mut scheduler).await;
            }
            () = &mut shutdown => {
                tracing::info!("shutting down");
                return std::process::ExitCode::SUCCESS;
            }
        }
    }
}

/// Runs one CalDAV sync pass. Returns whether anything landed on disk.
///
/// The whole pass runs on the blocking pool: the CalDAV client is synchronous
/// by design, and a five-second SSO handshake on an executor thread would stall
/// the reminder ticker sharing it.
async fn sync_once() -> bool {
    let handle = tokio::task::spawn_blocking(|| {
        let mut accounts = match AccountStore::open_default() {
            Ok(accounts) => accounts,
            Err(why) => {
                tracing::warn!(%why, "cannot open the account store; skipping sync");
                return false;
            }
        };

        if accounts.enabled().count() == 0 {
            return false;
        }

        let root = slate::store::vdir::default_root();
        // One pass covers CalDAV and CardDAV; the address books are written to
        // the suite's contacts root, which is why the unit file grants it too.
        let contacts_root = slate::store::contacts::default_root();
        // Provider manifests, for accounts that name a provider rather than a
        // raw URL. Reloaded each pass so a dropped-in manifest applies without
        // restarting the daemon.
        let registry = cosmic_pim_accounts::Registry::load();
        let reports = cosmic_pim_sync::sync_all(&mut accounts, &registry, &root, &contacts_root);

        let mut changed = false;
        for report in &reports {
            // One line per account whatever happened: a silent sync that is
            // quietly failing is indistinguishable from one that is working.
            match &report.collections {
                Ok(_) => tracing::info!("{}", report.summary()),
                Err(_) => tracing::warn!("{}", report.summary()),
            }
            changed |= report.changed();
        }
        changed
    });

    match handle.await {
        Ok(changed) => changed,
        Err(why) => {
            tracing::error!(%why, "the sync task panicked");
            false
        }
    }
}

/// Refreshes every ICS feed subscription that is due. Returns whether any
/// calendar changed on disk.
///
/// Feeds ride the sync tick but not the sync engine: a subscription is a URL
/// with no account behind it, so `sync_all` never schedules one. Each feed
/// carries its own interval and HTTP validators in its sidecar, and `due`
/// keeps an idle pass down to a directory scan — most ticks touch nothing.
async fn refresh_feeds() -> bool {
    use cosmic_pim_caldav::feed;

    let handle = tokio::task::spawn_blocking(|| {
        let root = slate::store::vdir::default_root();
        let now_ms = chrono::Utc::now().timestamp_millis();
        let mut changed = false;

        for meta in slate::store::vdir::collections(&root) {
            if !feed::is_feed(&meta.path) {
                continue;
            }
            if !feed::FeedState::load(&meta.path).is_some_and(|state| state.due(now_ms)) {
                continue;
            }

            match feed::refresh(&meta.path, now_ms) {
                Ok(outcome) => {
                    // One line per actual fetch, so a quietly failing feed is
                    // distinguishable from one that is simply unchanged.
                    tracing::info!(
                        feed = %meta.name,
                        updated = outcome.updated,
                        removed = outcome.removed,
                        unchanged = outcome.unchanged,
                        guard_tripped = outcome.guard_tripped,
                        "feed refreshed"
                    );
                    changed |= outcome.changed();
                }
                Err(why) => tracing::warn!(feed = %meta.name, %why, "feed refresh failed"),
            }
        }
        changed
    });

    match handle.await {
        Ok(changed) => changed,
        Err(why) => {
            tracing::error!(%why, "the feed refresh task panicked");
            false
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
/// Fires whatever is due, and returns the instant it looked — the start of
/// the next sleep window.
async fn check(store: &mut Store, scheduler: &mut Scheduler) -> chrono::NaiveDateTime {
    // Re-read each pass so a settings change takes effect without a restart.
    let config = load_config();
    let now = chrono::Local::now().naive_local();
    let today = now.date();

    let occurrences =
        match store.occurrences(today, today + Duration::days(2), &config.hidden_set()) {
            Ok(occurrences) => occurrences,
            Err(why) => {
                tracing::warn!(%why, "could not load occurrences");
                return now;
            }
        };

    // Instance-aware: an overridden occurrence carries its own VALARMs, and
    // only falls back to the series master's.
    let alarms_for = |occurrence: &Occurrence| {
        store
            .event_instance(
                &occurrence.calendar_id,
                &occurrence.uid,
                occurrence.recurrence_id,
            )
            .ok()
            .flatten()
            .map(|event| event.alarms)
            .unwrap_or_default()
    };

    let due = scheduler.due(&occurrences, alarms_for, now, |occurrence| {
        config.reminder_for(&occurrence.calendar_id)
    });

    for reminder in &due {
        tracing::info!(summary = %reminder.summary, "reminder due");
        reminders::notify(reminder, APP_ID, reminder.body(&config)).await;
    }

    scheduler.forget_before(now - Duration::days(1));
    now
}

/// One notification for the alarms that passed while the machine slept.
async fn report_missed(
    store: &mut Store,
    scheduler: &mut Scheduler,
    slept_at: chrono::NaiveDateTime,
) {
    let config = load_config();
    let now = chrono::Local::now().naive_local();

    // A day either side of the window covers any plausible suspend.
    let Ok(occurrences) = store.occurrences(
        slept_at.date() - Duration::days(1),
        now.date() + Duration::days(1),
        &config.hidden_set(),
    ) else {
        return;
    };

    let alarms_for = |occurrence: &Occurrence| {
        store
            .event_instance(
                &occurrence.calendar_id,
                &occurrence.uid,
                occurrence.recurrence_id,
            )
            .ok()
            .flatten()
            .map(|event| event.alarms)
            .unwrap_or_default()
    };

    let missed = reminders::sweep_missed(
        scheduler,
        &occurrences,
        alarms_for,
        slept_at,
        now,
        |occurrence| config.reminder_for(&occurrence.calendar_id),
    );
    if missed > 0 {
        tracing::info!(missed, "reminders passed while asleep");
        reminders::notify_missed(missed, APP_ID).await;
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
