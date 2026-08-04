// SPDX-License-Identifier: GPL-3.0-only

//! Event reminders, delivered as desktop notifications.
//!
//! COSMIC implements the freedesktop notification spec through
//! `cosmic-notifications`, so a plain `org.freedesktop.Notifications` call lands
//! in the desktop's own notification centre with no COSMIC-specific code.
//!
//! The scheduling decision is kept separate from the delivery so it can be tested
//! against a fixed clock: [`Scheduler::due`] is pure, and [`notify`] is the only
//! part that touches D-Bus.

use crate::model::Occurrence;
use chrono::{Duration, NaiveDateTime};
use std::collections::HashSet;

/// Identifies one alarm on one occurrence, so it fires exactly once.
///
/// The recurrence-aware part is the occurrence's own start: two instances of a
/// weekly series are different reminders, but re-reading the same file is not.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ReminderId {
    pub uid: String,
    pub calendar_id: String,
    pub start: NaiveDateTime,
    /// Trigger offset in seconds, so two alarms on one event stay distinct.
    pub offset_secs: i64,
}

/// A reminder that is ready to be shown.
#[derive(Clone, Debug)]
pub struct Reminder {
    pub id: ReminderId,
    pub summary: String,
    pub location: Option<String>,
    pub start: NaiveDateTime,
    pub all_day: bool,
    /// How long until the event starts, at the moment the reminder fires.
    pub lead: Duration,
}

/// Remembers what has already been shown.
#[derive(Default)]
pub struct Scheduler {
    fired: HashSet<ReminderId>,
}

/// How far past its trigger a reminder may still fire.
///
/// Without a bound, launching the app would replay every alarm the day already
/// went through. Five minutes is enough to survive a suspend/resume or a slow
/// start without resurrecting the morning's notifications.
const GRACE: Duration = Duration::minutes(5);

impl Scheduler {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Reminders whose trigger has passed but which have not been shown yet.
    ///
    /// `occurrences` should cover at least the next day; anything outside it
    /// simply cannot fire.
    pub fn due(
        &mut self,
        occurrences: &[Occurrence],
        alarms_for: impl Fn(&Occurrence) -> Vec<Duration>,
        now: NaiveDateTime,
        default_lead: Option<Duration>,
    ) -> Vec<Reminder> {
        let mut out = Vec::new();

        for occurrence in occurrences {
            let mut alarms = alarms_for(occurrence);
            if alarms.is_empty() {
                // An event with no VALARM of its own uses the app-wide default,
                // if the user has set one.
                match default_lead {
                    Some(lead) => alarms.push(-lead),
                    None => continue,
                }
            }

            for alarm in alarms {
                let trigger = occurrence.start + alarm;

                if trigger > now || now - trigger > GRACE {
                    continue;
                }

                let id = ReminderId {
                    uid: occurrence.uid.clone(),
                    calendar_id: occurrence.calendar_id.clone(),
                    start: occurrence.start,
                    offset_secs: alarm.num_seconds(),
                };

                if !self.fired.insert(id.clone()) {
                    continue;
                }

                out.push(Reminder {
                    id,
                    summary: occurrence.summary.clone(),
                    location: occurrence.location.clone(),
                    start: occurrence.start,
                    all_day: occurrence.all_day,
                    lead: occurrence.start - now,
                });
            }
        }

        out
    }

    /// Drops memory of reminders whose events are long past, so the set does not
    /// grow for the lifetime of the process.
    pub fn forget_before(&mut self, cutoff: NaiveDateTime) {
        self.fired.retain(|id| id.start >= cutoff);
    }

    #[must_use]
    pub fn tracked(&self) -> usize {
        self.fired.len()
    }
}

/// Well-known bus name claimed by whichever process is responsible for firing
/// reminders.
///
/// The app and the background daemon can both be running, and both can see the
/// same events. Without an arbiter the user would get every reminder twice. The
/// daemon claims this name at startup; the app checks for it and stays quiet if
/// someone already holds it.
pub const OWNER_BUS_NAME: &str = "io.github.entro314labs.Calendar.Reminders";

/// Claims responsibility for firing reminders.
///
/// Returns the connection on success — it must be kept alive, since dropping it
/// releases the name. `Ok(None)` means someone else already owns it.
pub async fn claim_ownership() -> Result<Option<zbus::Connection>, zbus::Error> {
    use zbus::fdo::RequestNameFlags;
    use zbus::fdo::RequestNameReply;

    let connection = zbus::Connection::session().await?;
    let reply = connection
        .request_name_with_flags(
            OWNER_BUS_NAME,
            // Do not queue: if someone else has it, we want to know now rather
            // than silently take over later.
            RequestNameFlags::DoNotQueue.into(),
        )
        .await;

    match reply {
        Ok(RequestNameReply::PrimaryOwner | RequestNameReply::AlreadyOwner) => Ok(Some(connection)),
        Ok(_) => Ok(None),

        // With `DoNotQueue`, zbus reports a name that is already held as an
        // error rather than a reply variant. That is a normal outcome here — a
        // second daemon should bow out quietly — so it must not be reported as a
        // failure, or systemd's `Restart=on-failure` would spin forever.
        Err(zbus::Error::NameTaken) => Ok(None),

        Err(why) => Err(why),
    }
}

/// Whether some other process is already firing reminders.
///
/// Any failure to reach the bus answers "no": a calendar with no reminders is a
/// worse outcome than one that occasionally shows a duplicate.
pub async fn someone_else_owns_reminders() -> bool {
    let Ok(connection) = zbus::Connection::session().await else {
        return false;
    };
    let Ok(proxy) = zbus::fdo::DBusProxy::new(&connection).await else {
        return false;
    };
    let Ok(name) = zbus::names::BusName::try_from(OWNER_BUS_NAME) else {
        return false;
    };

    proxy.name_has_owner(name).await.unwrap_or(false)
}

/// Sends one reminder to the desktop's notification service.
///
/// Async deliberately. Showing a notification is a D-Bus round trip, and
/// `notify-rust`'s blocking variant spins up its own runtime to do it — which
/// panics outright when called from inside one, as both the daemon and the app's
/// executor are.
///
/// Failure is logged, not surfaced: a missing notification daemon should not
/// interrupt whatever the user is doing in the calendar.
pub async fn notify(reminder: &Reminder, app_id: &str, body: String) {
    let result = notify_rust::Notification::new()
        .appname("Calendar")
        .summary(&reminder.summary)
        .body(&body)
        .icon(app_id)
        // Calendar reminders should persist until dismissed rather than sliding
        // away after a few seconds.
        .hint(notify_rust::Hint::Category(
            "appointment.reminded".to_owned(),
        ))
        .timeout(notify_rust::Timeout::Never)
        .show_async()
        .await;

    match result {
        Ok(_) => tracing::debug!(summary = %reminder.summary, "reminder delivered"),
        Err(why) => tracing::warn!(%why, "could not deliver a reminder notification"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    fn at(h: u32, m: u32) -> NaiveDateTime {
        NaiveDate::from_ymd_opt(2026, 8, 4)
            .unwrap()
            .and_hms_opt(h, m, 0)
            .unwrap()
    }

    fn occurrence(uid: &str, start: NaiveDateTime) -> Occurrence {
        Occurrence {
            uid: uid.into(),
            calendar_id: "personal".into(),
            summary: uid.into(),
            location: None,
            all_day: false,
            start,
            end: start + Duration::hours(1),
            recurrence_id: None,
        }
    }

    fn ten_minutes_before(_: &Occurrence) -> Vec<Duration> {
        vec![Duration::minutes(-10)]
    }

    fn no_alarms(_: &Occurrence) -> Vec<Duration> {
        Vec::new()
    }

    #[test]
    fn fires_once_the_trigger_has_passed() {
        let mut scheduler = Scheduler::new();
        let events = [occurrence("Standup", at(9, 0))];

        assert!(
            scheduler
                .due(&events, ten_minutes_before, at(8, 45), None)
                .is_empty(),
            "fired before the trigger"
        );

        let got = scheduler.due(&events, ten_minutes_before, at(8, 50), None);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].summary, "Standup");
    }

    #[test]
    fn never_fires_the_same_reminder_twice() {
        let mut scheduler = Scheduler::new();
        let events = [occurrence("Standup", at(9, 0))];

        assert_eq!(
            scheduler
                .due(&events, ten_minutes_before, at(8, 50), None)
                .len(),
            1
        );
        for minute in 51..55 {
            assert!(
                scheduler
                    .due(&events, ten_minutes_before, at(8, minute), None)
                    .is_empty(),
                "re-fired at 8:{minute}"
            );
        }
    }

    #[test]
    fn does_not_replay_the_whole_day_on_a_late_start() {
        // The app opens at 17:00; this morning's alarms must stay quiet.
        let mut scheduler = Scheduler::new();
        let events = [
            occurrence("Breakfast", at(8, 0)),
            occurrence("Standup", at(9, 0)),
            occurrence("Lunch", at(13, 0)),
        ];
        assert!(
            scheduler
                .due(&events, ten_minutes_before, at(17, 0), None)
                .is_empty(),
            "replayed reminders from earlier in the day"
        );
    }

    #[test]
    fn a_reminder_inside_the_grace_window_still_fires() {
        // Suspended over the trigger, resumed two minutes later.
        let mut scheduler = Scheduler::new();
        let events = [occurrence("Standup", at(9, 0))];
        assert_eq!(
            scheduler
                .due(&events, ten_minutes_before, at(8, 52), None)
                .len(),
            1
        );
    }

    #[test]
    fn events_without_alarms_are_silent_unless_a_default_is_set() {
        let mut scheduler = Scheduler::new();
        let events = [occurrence("Standup", at(9, 0))];

        assert!(
            scheduler
                .due(&events, no_alarms, at(8, 50), None)
                .is_empty(),
            "notified without any alarm configured"
        );
        assert_eq!(
            scheduler
                .due(&events, no_alarms, at(8, 50), Some(Duration::minutes(10)))
                .len(),
            1,
            "the default reminder did not apply"
        );
    }

    #[test]
    fn an_events_own_alarm_wins_over_the_default() {
        let mut scheduler = Scheduler::new();
        let events = [occurrence("Standup", at(9, 0))];

        // Its own alarm is 10 minutes; the default of 60 must not also fire.
        let got = scheduler.due(
            &events,
            ten_minutes_before,
            at(8, 50),
            Some(Duration::minutes(60)),
        );
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].id.offset_secs, -600);
    }

    #[test]
    fn multiple_alarms_on_one_event_each_fire() {
        let mut scheduler = Scheduler::new();
        let events = [occurrence("Flight", at(9, 0))];
        let two = |_: &Occurrence| vec![Duration::minutes(-60), Duration::minutes(-10)];

        assert_eq!(scheduler.due(&events, two, at(8, 5), None).len(), 1);
        assert_eq!(scheduler.due(&events, two, at(8, 51), None).len(), 1);
    }

    #[test]
    fn two_instances_of_a_series_are_separate_reminders() {
        let mut scheduler = Scheduler::new();
        let monday = occurrence("Standup", at(9, 0));
        let tuesday = occurrence("Standup", at(9, 0) + Duration::days(1));

        assert_eq!(
            scheduler
                .due(&[monday], ten_minutes_before, at(8, 50), None)
                .len(),
            1
        );
        assert_eq!(
            scheduler
                .due(
                    &[tuesday],
                    ten_minutes_before,
                    at(8, 50) + Duration::days(1),
                    None
                )
                .len(),
            1,
            "the next instance of the series was suppressed"
        );
    }

    #[test]
    fn forgetting_prunes_old_reminders() {
        let mut scheduler = Scheduler::new();
        scheduler.due(
            &[occurrence("Standup", at(9, 0))],
            ten_minutes_before,
            at(8, 50),
            None,
        );
        assert_eq!(scheduler.tracked(), 1);

        scheduler.forget_before(at(9, 0) + Duration::days(1));
        assert_eq!(scheduler.tracked(), 0);
    }
}
