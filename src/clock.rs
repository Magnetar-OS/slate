// SPDX-License-Identifier: GPL-3.0-only

//! "Now", in the zone the store expands occurrences in.
//!
//! Occurrence times are wall clock in the [`Store`]'s zone (jiff's reading of
//! the system zone, taken when the store opened). Comparing them against
//! `chrono::Local` mixes two readings of "local" that disagree after a
//! timezone change, or whenever jiff cannot name the zone and falls back to
//! UTC while chrono still applies the real offset: reminders then fire off by
//! the difference, and "today" is wrong. Every "now" that meets an occurrence
//! comes from here instead.

use crate::store::{Store, StoreError};
use chrono::NaiveDateTime;

/// The current wall-clock time in `zone`.
#[must_use]
pub fn now_in(zone: chrono_tz::Tz) -> NaiveDateTime {
    chrono::Utc::now().with_timezone(&zone).naive_local()
}

/// Reopens `store` when the system timezone is no longer the one it expands
/// occurrences in — travel, or automatic timezone updates. Returns whether it
/// did, so the caller can reload what it shows.
///
/// Reopening rebuilds the occurrence index for the new zone; the index keys
/// floating and all-day times by the zone they were resolved in.
///
/// # Errors
///
/// When the store cannot be reopened; the old one is kept.
pub fn follow_timezone(store: &mut Store) -> Result<bool, StoreError> {
    if crate::model::local_timezone() == store.local_timezone() {
        return Ok(false);
    }
    let reopened = Store::open(store.root(), &crate::store::index::default_path())?;
    tracing::info!(
        from = store.local_timezone().name(),
        to = reopened.local_timezone().name(),
        "the system timezone changed; following it"
    );
    *store = reopened;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn now_is_read_in_the_zone_asked_for() {
        // Two zones twelve-plus hours apart cannot share a wall clock, which
        // is exactly the disagreement `chrono::Local` let through.
        let tokyo = now_in(chrono_tz::Asia::Tokyo);
        let honolulu = now_in(chrono_tz::Pacific::Honolulu);
        let apart = (tokyo - honolulu).num_minutes();
        assert!((1139..=1141).contains(&apart), "{apart} minutes apart");
    }
}
