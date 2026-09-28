// SPDX-License-Identifier: GPL-3.0-only

//! The background work — CalDAV/CardDAV sync and ICS feed refreshes — and the
//! bus interface that keeps it in one process at a time.
//!
//! Sync reads and rewrites each collection's `.caldav-state.json` whole. Two
//! passes running at once — the daemon's five-minute tick and the app's
//! "Sync now" — each load the sidecar, spend seconds on the network, and save
//! their own copy last, so one pass's etags, conflicts and queue entries are
//! lost to the other's. The queue is the only road a local edit has to the
//! server; an entry lost there never arrives.
//!
//! So sync has one owner, the same one reminders have: whoever holds
//! [`OWNER_BUS_NAME`](crate::reminders::OWNER_BUS_NAME). The daemon holds it and serves [`Service`] on it,
//! which runs one pass at a time. The app, while the name is held, asks the
//! daemon over [`BackgroundProxy`] instead of running its own pass, and runs
//! [`sync_accounts`] itself only when no daemon is there to ask.
//!
//! This orders *passes*. An individual edit still writes its queue entry into
//! the sidecar from the app while a daemon pass may hold an older copy; making
//! that read-modify-write safe needs a lock in the substrate's sidecar store,
//! which is where it belongs.

/// Where the daemon serves [`Service`].
pub const OBJECT_PATH: &str = "/com/magnetaros/Slate/Background";

/// One sync pass's report: a line per account, and whether anything landed on
/// disk.
pub type SyncReport = (Vec<String>, bool);

/// Runs one CalDAV/CardDAV sync pass over every enabled account. Blocking.
///
/// Every account gets a line whatever happened, logged as well as returned: a
/// sync that is quietly failing is indistinguishable from one that works.
#[must_use]
pub fn sync_accounts() -> SyncReport {
    let mut accounts = match cosmic_pim_accounts::AccountStore::open_default() {
        Ok(accounts) => accounts,
        Err(why) => {
            tracing::warn!(%why, "cannot open the account store; skipping sync");
            return (vec![why.to_string()], false);
        }
    };
    if accounts.enabled().count() == 0 {
        return (Vec::new(), false);
    }

    let root = crate::store::vdir::default_root();
    // One pass covers CalDAV and CardDAV; the address books land in the
    // suite's contacts root for Circle to read.
    let contacts_root = crate::store::contacts::default_root();
    // Provider manifests, reloaded each pass so a dropped-in manifest applies
    // without a restart.
    let registry = cosmic_pim_accounts::Registry::load();
    let reports = cosmic_pim_sync::sync_all(&mut accounts, &registry, &root, &contacts_root);

    let mut lines = Vec::with_capacity(reports.len());
    let mut changed = false;
    for report in &reports {
        let line = report.summary();
        if report.collections.is_ok() {
            tracing::info!("{line}");
        } else {
            tracing::warn!("{line}");
        }
        lines.push(line);
        changed |= report.changed();
    }
    (lines, changed)
}

/// Refreshes the ICS feed subscriptions that are due — or all of them when
/// `force`, which is what a just-added subscription wants. Blocking. Returns
/// whether any calendar changed on disk.
///
/// Feeds ride the sync tick but not the sync engine: a subscription is a URL
/// with no account behind it, so `sync_all` never schedules one. Each feed
/// carries its own interval and HTTP validators in its sidecar, and `due`
/// keeps an idle pass down to a directory scan.
#[must_use]
pub fn refresh_feeds(force: bool) -> bool {
    use cosmic_pim_caldav::feed;

    let root = crate::store::vdir::default_root();
    let now_ms = chrono::Utc::now().timestamp_millis();
    let mut changed = false;

    for meta in crate::store::vdir::collections(&root) {
        if !feed::is_feed(&meta.path) {
            continue;
        }
        let due = feed::FeedState::load(&meta.path).is_some_and(|state| state.due(now_ms));
        if !(force || due) {
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
            // Logged, not surfaced: a feed that is down is retried on its own
            // schedule, and there is nothing for the user to do about it now.
            Err(why) => tracing::warn!(feed = %meta.name, %why, "feed refresh failed"),
        }
    }
    changed
}

/// The daemon's side: sync and feed passes, strictly one at a time.
///
/// The work itself is passed in rather than hard-wired so the ordering can be
/// tested on a private bus without touching the user's accounts; the daemon
/// passes [`sync_accounts`] and [`refresh_feeds`].
pub struct Service {
    running: std::sync::Arc<tokio::sync::Mutex<()>>,
    sync: fn() -> SyncReport,
    feeds: fn(bool) -> bool,
}

impl Service {
    /// `running` is shared with the daemon's own timer, so a pass the timer
    /// started and one a caller asked for never overlap either.
    #[must_use]
    pub fn new(
        running: std::sync::Arc<tokio::sync::Mutex<()>>,
        sync: fn() -> SyncReport,
        feeds: fn(bool) -> bool,
    ) -> Self {
        Self {
            running,
            sync,
            feeds,
        }
    }
}

#[zbus::interface(name = "com.magnetaros.Slate.Background")]
impl Service {
    /// Runs one sync pass and reports it. Waits for a pass already running
    /// rather than starting a second one beside it.
    async fn sync(&self) -> SyncReport {
        let _running = self.running.lock().await;
        let sync = self.sync;
        tokio::task::spawn_blocking(sync)
            .await
            .unwrap_or_else(|why| (vec![why.to_string()], false))
    }

    /// Refreshes due feeds (all of them when `force`); true if any changed.
    async fn refresh_feeds(&self, force: bool) -> bool {
        let _running = self.running.lock().await;
        let feeds = self.feeds;
        tokio::task::spawn_blocking(move || feeds(force))
            .await
            .unwrap_or(false)
    }
}

/// The app's side of [`Service`].
#[zbus::proxy(
    interface = "com.magnetaros.Slate.Background",
    default_service = "com.magnetaros.Slate.Reminders",
    default_path = "/com/magnetaros/Slate/Background"
)]
pub trait Background {
    fn sync(&self) -> zbus::Result<SyncReport>;
    fn refresh_feeds(&self, force: bool) -> zbus::Result<bool>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Serves `sync`/`feeds` under the owner name on a private bus, and
    /// returns the bus with a client connection to it.
    async fn daemon_on_private_bus(
        running: std::sync::Arc<tokio::sync::Mutex<()>>,
        sync: fn() -> SyncReport,
        feeds: fn(bool) -> bool,
    ) -> (
        crate::testbus::PrivateBus,
        zbus::Connection,
        zbus::Connection,
    ) {
        let bus = crate::testbus::PrivateBus::start();
        let daemon = crate::reminders::claim_ownership(bus.connect().await)
            .await
            .unwrap()
            .expect("the name was free");
        daemon
            .object_server()
            .at(OBJECT_PATH, Service::new(running, sync, feeds))
            .await
            .unwrap();
        let app = bus.connect().await;
        (bus, daemon, app)
    }

    fn one_account_synced() -> SyncReport {
        (vec!["Fastmail: 2 calendars, 1 changed".to_owned()], true)
    }

    fn feeds_unchanged(_force: bool) -> bool {
        false
    }

    #[tokio::test]
    async fn the_app_gets_the_daemons_report() {
        let (_bus, _daemon, app) = daemon_on_private_bus(
            std::sync::Arc::default(),
            one_account_synced,
            feeds_unchanged,
        )
        .await;

        let report = BackgroundProxy::new(&app)
            .await
            .unwrap()
            .sync()
            .await
            .unwrap();
        assert_eq!(report, one_account_synced());
    }

    static RUNNING: AtomicUsize = AtomicUsize::new(0);
    static MOST_AT_ONCE: AtomicUsize = AtomicUsize::new(0);

    fn a_slow_pass() -> SyncReport {
        let now = RUNNING.fetch_add(1, Ordering::SeqCst) + 1;
        MOST_AT_ONCE.fetch_max(now, Ordering::SeqCst);
        std::thread::sleep(std::time::Duration::from_millis(150));
        RUNNING.fetch_sub(1, Ordering::SeqCst);
        (Vec::new(), false)
    }

    fn a_slow_feed_pass(_force: bool) -> bool {
        a_slow_pass().1
    }

    #[tokio::test]
    async fn two_requests_never_run_two_passes_at_once() {
        let (_bus, _daemon, app) =
            daemon_on_private_bus(std::sync::Arc::default(), a_slow_pass, a_slow_feed_pass).await;
        let proxy = BackgroundProxy::new(&app).await.unwrap();

        let (first, second, feeds) =
            tokio::join!(proxy.sync(), proxy.sync(), proxy.refresh_feeds(true));
        first.unwrap();
        second.unwrap();
        feeds.unwrap();
        assert_eq!(
            MOST_AT_ONCE.load(Ordering::SeqCst),
            1,
            "two passes ran side by side"
        );
    }

    #[tokio::test]
    async fn a_request_waits_for_the_timers_pass() {
        let running = std::sync::Arc::<tokio::sync::Mutex<()>>::default();
        let (_bus, _daemon, app) =
            daemon_on_private_bus(running.clone(), one_account_synced, feeds_unchanged).await;
        let proxy = BackgroundProxy::new(&app).await.unwrap();

        // The daemon's five-minute pass is running.
        let timer_pass = running.lock().await;
        let asked = tokio::time::timeout(std::time::Duration::from_millis(300), proxy.sync()).await;
        assert!(asked.is_err(), "a second pass started beside the timer's");

        drop(timer_pass);
        assert_eq!(proxy.sync().await.unwrap(), one_account_synced());
    }
}
