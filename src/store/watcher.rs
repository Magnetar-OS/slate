// SPDX-License-Identifier: GPL-3.0-only

//! Watches the vdir for changes made by anything other than us.
//!
//! The point is that `vdirsyncer`, `khal`, or a text editor can change files
//! underneath a running app, and the view should follow along without the user
//! having to reopen anything.
//!
//! Raw filesystem events are coalesced before being forwarded: a sync run
//! rewrites dozens of files in a burst, and re-indexing once at the end is both
//! cheaper and less visually jarring than reacting to each one.

use super::StoreError;
use notify::{RecommendedWatcher, RecursiveMode, Watcher as _};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// How long the directory must be quiet before we act on a burst of changes.
const QUIET_PERIOD: Duration = Duration::from_millis(400);

/// Upper bound on how long a continuous stream of events can delay a refresh.
const MAX_DELAY: Duration = Duration::from_secs(3);

/// Keeps the underlying watcher alive. Dropping this stops the watch.
pub struct Watch {
    _watcher: RecommendedWatcher,
}

/// Starts watching `root`, returning a receiver that yields one message per
/// settled burst of changes.
///
/// The returned [`Watch`] must be held for as long as you want notifications;
/// dropping it tears the watch down.
pub fn watch(root: &Path) -> Result<(Watch, tokio::sync::mpsc::Receiver<()>), StoreError> {
    std::fs::create_dir_all(root)?;

    let (raw_tx, raw_rx) = std::sync::mpsc::channel::<notify::Result<notify::Event>>();

    let mut watcher = notify::recommended_watcher(move |res| {
        // A send failure just means the debounce thread has gone away.
        let _ = raw_tx.send(res);
    })?;
    watcher.watch(root, RecursiveMode::Recursive)?;

    // Depth 1 is plenty: a burst that arrives while the UI is still handling the
    // previous one collapses into the pending slot rather than queueing up.
    let (tx, rx) = tokio::sync::mpsc::channel::<()>(1);

    std::thread::Builder::new()
        .name("calendar-watch".into())
        .spawn(move || debounce_loop(&raw_rx, &tx))
        .map_err(StoreError::Io)?;

    Ok((Watch { _watcher: watcher }, rx))
}

fn debounce_loop(
    raw_rx: &std::sync::mpsc::Receiver<notify::Result<notify::Event>>,
    tx: &tokio::sync::mpsc::Sender<()>,
) {
    loop {
        // Block until something interesting happens.
        match raw_rx.recv() {
            Ok(event) => {
                if !is_interesting(&event) {
                    continue;
                }
            }
            // The watcher was dropped; we are done.
            Err(_) => return,
        }

        // Absorb the rest of the burst.
        let burst_started = std::time::Instant::now();
        loop {
            match raw_rx.recv_timeout(QUIET_PERIOD) {
                Ok(event) => {
                    let _ = is_interesting(&event);
                    if burst_started.elapsed() >= MAX_DELAY {
                        break;
                    }
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => break,
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return,
            }
        }

        // `try_send` failing means a refresh is already pending, which is exactly
        // the coalescing we want — no reason to queue a second one.
        match tx.try_send(()) {
            Ok(()) => {}
            Err(tokio::sync::mpsc::error::TrySendError::Full(())) => {
                tracing::trace!("refresh already pending; coalescing");
            }
            Err(tokio::sync::mpsc::error::TrySendError::Closed(())) => return,
        }
    }
}

/// Filters out noise: our own atomic-write temp files, and anything that is not
/// calendar data.
fn is_interesting(event: &notify::Result<notify::Event>) -> bool {
    let Ok(event) = event else {
        return false;
    };

    if !matches!(
        event.kind,
        notify::EventKind::Create(_) | notify::EventKind::Modify(_) | notify::EventKind::Remove(_)
    ) {
        return false;
    }

    event.paths.iter().any(|p| is_calendar_path(p))
}

fn is_calendar_path(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
        return false;
    };

    // Our own `.name.ics.tmp` staging files land here a moment before the rename;
    // reacting to them would just cause a redundant scan.
    if name.starts_with('.') && name.ends_with(".tmp") {
        return false;
    }

    name.ends_with(".ics")
        || name == "displayname"
        || name == "color"
        // A whole collection appearing or disappearing has no extension at all.
        || path.extension().is_none()
}

/// A directory being watched, exposed so the caller can restart the watch when
/// the root moves.
impl Watch {
    #[must_use]
    pub fn root_exists(path: &Path) -> bool {
        path.is_dir()
    }
}

/// Convenience wrapper used by the app's subscription.
#[must_use]
pub fn default_root() -> PathBuf {
    super::vdir::default_root()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ignores_our_own_temp_files() {
        assert!(!is_calendar_path(Path::new("/c/personal/.abc.ics.tmp")));
        assert!(is_calendar_path(Path::new("/c/personal/abc.ics")));
    }

    #[test]
    fn watches_metadata_files() {
        assert!(is_calendar_path(Path::new("/c/personal/displayname")));
        assert!(is_calendar_path(Path::new("/c/personal/color")));
    }

    #[test]
    fn ignores_unrelated_files() {
        assert!(!is_calendar_path(Path::new("/c/personal/notes.txt")));
        assert!(!is_calendar_path(Path::new("/c/personal/photo.png")));
    }

    #[tokio::test]
    async fn fires_once_for_a_burst_of_writes() {
        let dir = tempfile::tempdir().unwrap();
        let collection = dir.path().join("personal");
        std::fs::create_dir_all(&collection).unwrap();

        let (_watch, mut rx) = watch(dir.path()).unwrap();

        // Let the watcher settle before touching anything.
        tokio::time::sleep(Duration::from_millis(200)).await;

        for i in 0..10 {
            std::fs::write(
                collection.join(format!("event-{i}.ics")),
                "BEGIN:VCALENDAR\r\nEND:VCALENDAR\r\n",
            )
            .unwrap();
        }

        // One signal should arrive for the whole burst.
        let first = tokio::time::timeout(Duration::from_secs(5), rx.recv()).await;
        assert!(first.is_ok(), "watcher never fired");

        // And nothing more should be queued behind it.
        let second = tokio::time::timeout(Duration::from_millis(800), rx.recv()).await;
        assert!(second.is_err(), "burst was not coalesced into one signal");
    }
}
