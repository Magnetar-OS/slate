// SPDX-License-Identifier: GPL-3.0-only

//! Event reminders, delivered as desktop notifications.
//!
//! COSMIC implements the freedesktop notification spec through
//! `cosmic-notifications`, so a plain `org.freedesktop.Notifications` call lands
//! in the desktop's own notification centre with no COSMIC-specific code.
//!
//! The scheduling decision is kept separate from the delivery so it can be tested
//! against a fixed clock: [`Scheduler::due`] is pure, and [`notify`] is the only
//! part that shows anything. [`due_reminders`] and [`missed_reminders`] put the
//! two together over a [`Store`] — the one sweep the app and the daemon share.

use crate::config::Config;
use crate::fl;
use crate::model::Occurrence;
use crate::store::Store;
use chrono::{Duration, NaiveDateTime};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// Identifies one alarm on one occurrence, so it fires exactly once.
///
/// The recurrence-aware part is the occurrence's own start: two instances of a
/// weekly series are different reminders, but re-reading the same file is not.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
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
    pub end: NaiveDateTime,
    pub all_day: bool,
    /// How long until the event starts, at the moment the reminder fires.
    pub lead: Duration,
    /// Which instance of a series this is, for looking the event back up.
    pub recurrence_id: Option<chrono::DateTime<chrono::Utc>>,
    /// The video-call link the notification's Join button opens, if any.
    pub join: Option<String>,
}

impl Reminder {
    /// The notification body: when the event starts, and where.
    ///
    /// Lives here rather than in either caller because the app and the daemon
    /// deliver the *same* reminder — whichever of them owns notifications at the
    /// time — and two copies of this text drifted apart once already.
    #[must_use]
    pub fn body(&self, config: &Config) -> String {
        let fired_on = (self.start - self.lead).date();
        let when = if self.start.date() != fired_on {
            // A reminder days ahead: "At 14:00" would read as today.
            let day = crate::ui::format_day(self.start.date());
            if self.all_day {
                day
            } else {
                fl!(
                    "reminder-on",
                    day = day,
                    time = crate::ui::format_time(self.start.time(), config)
                )
            }
        } else if self.all_day {
            fl!("all-day")
        } else {
            let minutes = self.lead.num_minutes();
            if minutes <= 0 {
                fl!("reminder-now")
            } else if minutes < 60 {
                fl!("reminder-in", minutes = minutes)
            } else {
                // Past an hour, a wall-clock time is easier to act on than a
                // countdown: "At 14:00" beats "In 90 minutes".
                fl!(
                    "reminder-at",
                    time = crate::ui::format_time(self.start.time(), config)
                )
            }
        };

        match &self.location {
            Some(location) if !location.is_empty() => format!("{when} · {location}"),
            _ => when,
        }
    }

    /// How long, from the moment it fired, the reminder's Join button is
    /// worth waiting on: until the event is over.
    #[must_use]
    pub fn joinable_for(&self) -> std::time::Duration {
        // The reminder fired at the start minus the lead.
        (self.end - self.start + self.lead)
            .to_std()
            .unwrap_or_default()
    }
}

/// Remembers what has already been shown.
///
/// In memory for the pure decisions, and — for the daemon and the app — on
/// disk too ([`Scheduler::persistent`]): the memory has to outlive the
/// process, or every restart (a package upgrade, `Restart=on-failure`, a new
/// login) and every hand-over between the app and the daemon replays each
/// alarm still inside its grace period.
#[derive(Default)]
pub struct Scheduler {
    fired: HashSet<ReminderId>,
    /// Where the fired set is shared with the other process, if anywhere.
    path: Option<PathBuf>,
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

    /// A scheduler whose memory is shared through `path`, starting from what
    /// is already recorded there.
    #[must_use]
    pub fn persistent(path: PathBuf) -> Self {
        let mut scheduler = Self {
            fired: HashSet::new(),
            path: Some(path),
        };
        scheduler.reload();
        scheduler
    }

    /// Merges in whatever the other process recorded since the last look.
    ///
    /// A missing or unreadable file is an empty memory: the worst case is one
    /// repeated notification, which is better than none.
    pub fn reload(&mut self) {
        let Some(path) = &self.path else {
            return;
        };
        let Ok(text) = std::fs::read_to_string(path) else {
            return;
        };
        match serde_json::from_str::<Vec<ReminderId>>(&text) {
            Ok(ids) => self.fired.extend(ids),
            Err(why) => {
                tracing::warn!(%why, path = %path.display(), "ignoring an unreadable fired-reminder file");
            }
        }
    }

    /// Writes the memory out for the other process, atomically.
    ///
    /// # Errors
    ///
    /// When the file cannot be written.
    pub fn persist(&self) -> std::io::Result<()> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut ids: Vec<&ReminderId> = self.fired.iter().collect();
        // Stable output, so an unchanged set rewrites identical bytes.
        ids.sort_by(|a, b| {
            (a.start, &a.calendar_id, &a.uid, a.offset_secs).cmp(&(
                b.start,
                &b.calendar_id,
                &b.uid,
                b.offset_secs,
            ))
        });
        let text = serde_json::to_string(&ids).map_err(std::io::Error::other)?;
        cosmic_pim_core::atomic::write(path, &text, None)
            .map(|_| ())
            .map_err(|why| std::io::Error::other(why.to_string()))
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
        default_lead: impl Fn(&Occurrence) -> Option<Duration>,
    ) -> Vec<Reminder> {
        let mut out = Vec::new();

        for occurrence in occurrences {
            let mut alarms = alarms_for(occurrence);
            if alarms.is_empty() {
                // An event with no VALARM of its own uses its calendar's
                // default, or the app-wide one behind it.
                match default_lead(occurrence) {
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
                    end: occurrence.end,
                    all_day: occurrence.all_day,
                    lead: occurrence.start - now,
                    recurrence_id: occurrence.recurrence_id,
                    join: None,
                });
            }
        }

        out
    }

    /// Remembers a reminder as already dealt with, without showing it.
    ///
    /// Returns `false` if it was already known. [`sweep_missed`] uses this to
    /// account for the alarms that passed during a suspend: they are counted
    /// once, into the digest, and can never fire late.
    pub fn mark_fired(&mut self, id: ReminderId) -> bool {
        self.fired.insert(id)
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

/// How far ahead of today a sweep looks for events whose alarms are due.
///
/// An alarm fires relative to its event, so an event two weeks out can have a
/// reminder due today. Two weeks and a day covers every lead the common
/// clients offer (Thunderbird's longest preset is a week, Outlook's two); a
/// longer one fires late, once its event comes inside this window, rather
/// than never.
pub const LOOKAHEAD_DAYS: i64 = 15;

/// Where the app and the daemon share what they have fired:
/// `$XDG_STATE_HOME/slate/fired-reminders.json`.
///
/// The same directory the daemon's unit declares as its `StateDirectory=`, so
/// the sandboxed daemon can write it.
#[must_use]
pub fn fired_path() -> PathBuf {
    let state = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|home| Path::new(&home).join(".local/state")))
        .unwrap_or_else(std::env::temp_dir);
    state.join("slate").join("fired-reminders.json")
}

/// The component behind one occurrence, for its alarms and its links.
///
/// Instance-aware: an overridden occurrence carries its own VALARMs (a moved
/// meeting reminds relative to its new time), and only falls back to the
/// series master's.
fn instance(store: &Store, occurrence: &Occurrence) -> Option<crate::model::Event> {
    store
        .event_instance(
            &occurrence.calendar_id,
            &occurrence.uid,
            occurrence.recurrence_id,
        )
        .ok()
        .flatten()
}

/// The sweep both the app and the daemon run: every reminder due at `now`,
/// each at most once, with its Join link resolved.
///
/// Reloads the shared memory first and persists it after, so whichever
/// process owns reminders sees what the other already showed.
///
/// # Errors
///
/// When the store cannot list occurrences.
pub fn due_reminders(
    store: &Store,
    scheduler: &mut Scheduler,
    config: &Config,
    now: NaiveDateTime,
) -> Result<Vec<Reminder>, crate::store::StoreError> {
    let today = now.date();
    let occurrences = store.occurrences(
        today,
        today + Duration::days(LOOKAHEAD_DAYS),
        &config.hidden_set(),
    )?;

    scheduler.reload();
    let known = scheduler.tracked();
    let mut due = scheduler.due(
        &occurrences,
        |occurrence| {
            instance(store, occurrence)
                .map(|event| event.alarms)
                .unwrap_or_default()
        },
        now,
        |occurrence| config.reminder_for(&occurrence.calendar_id),
    );

    for reminder in &mut due {
        // Invitations mostly carry the link in DESCRIPTION; the occurrence
        // alone only knows the location.
        reminder.join = store
            .event_instance(
                &reminder.id.calendar_id,
                &reminder.id.uid,
                reminder.recurrence_id,
            )
            .ok()
            .flatten()
            .and_then(|event| {
                crate::meeting::meeting_link(
                    event.location.as_deref(),
                    event.description.as_deref(),
                )
                .or_else(|| crate::meeting::conference_link(&event.other))
            })
            .or_else(|| crate::meeting::meeting_link(reminder.location.as_deref(), None));
    }

    // Keep the fired set from growing for the lifetime of the process.
    scheduler.forget_before(now - Duration::days(1));
    if (!due.is_empty() || scheduler.tracked() != known)
        && let Err(why) = scheduler.persist()
    {
        tracing::warn!(%why, "could not record the fired reminders; a restart may repeat one");
    }
    Ok(due)
}

/// The digest's count: the alarms that passed between `slept_at` and `now`,
/// marked shown so none of them fires late. Shares the memory like
/// [`due_reminders`].
///
/// # Errors
///
/// When the store cannot list occurrences.
pub fn missed_reminders(
    store: &Store,
    scheduler: &mut Scheduler,
    config: &Config,
    slept_at: NaiveDateTime,
    now: NaiveDateTime,
) -> Result<usize, crate::store::StoreError> {
    // A day before the window covers any plausible suspend; the lookahead
    // covers the long-lead alarms `due_reminders` would have seen.
    let occurrences = store.occurrences(
        slept_at.date() - Duration::days(1),
        now.date() + Duration::days(LOOKAHEAD_DAYS),
        &config.hidden_set(),
    )?;

    scheduler.reload();
    let missed = sweep_missed(
        scheduler,
        &occurrences,
        |occurrence| {
            instance(store, occurrence)
                .map(|event| event.alarms)
                .unwrap_or_default()
        },
        slept_at,
        now,
        |occurrence| config.reminder_for(&occurrence.calendar_id),
    );
    if missed > 0
        && let Err(why) = scheduler.persist()
    {
        tracing::warn!(%why, "could not record the missed reminders");
    }
    Ok(missed)
}

/// Well-known bus name claimed by whichever process is responsible for firing
/// reminders — and, while it is held, for syncing.
///
/// The app and the background daemon can both be running, and both can see the
/// same events. Without an arbiter the user would get every reminder twice. The
/// daemon claims this name at startup; the app watches it ([`watch_owner`])
/// and stays quiet for exactly as long as someone holds it.
pub const OWNER_BUS_NAME: &str = "com.magnetaros.Slate.Reminders";

/// Claims responsibility for firing reminders on `connection`.
///
/// Returns the connection on success — it must be kept alive, since dropping it
/// releases the name. `Ok(None)` means someone else already owns it.
///
/// # Errors
///
/// When the bus refuses the request for any other reason.
pub async fn claim_ownership(
    connection: zbus::Connection,
) -> Result<Option<zbus::Connection>, zbus::Error> {
    use zbus::fdo::RequestNameFlags;
    use zbus::fdo::RequestNameReply;

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

/// Reports whether someone owns [`OWNER_BUS_NAME`] — once now, then again
/// every time that changes — until the bus goes away.
///
/// Ownership is a runtime fact, not a startup one: the daemon can be enabled
/// after the app opened (both would fire), or crash and restart while it is
/// open (neither would). Following `NameOwnerChanged` is what keeps exactly
/// one of them firing through all of that.
///
/// The signal subscription is made *before* the initial query, so a change
/// between the two is reported rather than lost.
///
/// # Errors
///
/// When the bus cannot be queried or subscribed to; the caller then has to
/// decide ownership without it.
pub async fn watch_owner(
    connection: &zbus::Connection,
    mut on_change: impl FnMut(bool),
) -> Result<(), zbus::Error> {
    use cosmic::iced::futures::StreamExt;

    let proxy = zbus::fdo::DBusProxy::new(connection).await?;
    let mut changes = proxy
        .receive_name_owner_changed_with_args(&[(0, OWNER_BUS_NAME)])
        .await?;

    let name = zbus::names::BusName::try_from(OWNER_BUS_NAME)?;
    let mut owned = proxy.name_has_owner(name).await?;
    on_change(owned);

    while let Some(signal) = changes.next().await {
        let Ok(args) = signal.args() else {
            continue;
        };
        let now_owned = args.new_owner().is_some();
        if now_owned != owned {
            owned = now_owned;
            on_change(owned);
        }
    }
    Ok(())
}

/// Counts the reminders whose moment passed while the machine was asleep, and
/// marks them shown so they cannot fire late.
///
/// The staleness rule in [`Scheduler::due`] is the right default — waking a
/// laptop at seven should not replay the whole day — but silence is the wrong
/// answer to "you missed four reminders". This is the middle: one number, on
/// resume, for the triggers that fell inside the sleep window.
///
/// Marking them fired is the point: without it the same triggers would be
/// counted again on the next sweep, or fire individually if a later pass
/// happened to catch one inside its grace period.
pub fn sweep_missed(
    scheduler: &mut Scheduler,
    occurrences: &[Occurrence],
    alarms_for: impl Fn(&Occurrence) -> Vec<Duration>,
    slept_at: NaiveDateTime,
    now: NaiveDateTime,
    default_lead: impl Fn(&Occurrence) -> Option<Duration>,
) -> usize {
    let mut missed = 0;

    for occurrence in occurrences {
        let mut alarms = alarms_for(occurrence);
        if alarms.is_empty() {
            match default_lead(occurrence) {
                Some(lead) => alarms.push(-lead),
                None => continue,
            }
        }

        for alarm in alarms {
            let trigger = occurrence.start + alarm;

            // Inside the sleep window, and now too stale for `due` to show.
            if trigger < slept_at || trigger > now || now - trigger <= GRACE {
                continue;
            }

            let id = ReminderId {
                uid: occurrence.uid.clone(),
                calendar_id: occurrence.calendar_id.clone(),
                start: occurrence.start,
                offset_secs: alarm.num_seconds(),
            };
            if scheduler.mark_fired(id) {
                missed += 1;
            }
        }
    }

    missed
}

/// Tells the user how many reminders passed while the machine slept.
///
/// One notification however many were missed: the whole reason the individual
/// ones were suppressed is that a queue of them is noise.
pub async fn notify_missed(count: usize, app_id: &str) {
    if count == 0 {
        return;
    }

    let result = notify_rust::Notification::new()
        .appname(&fl!("app-title"))
        .summary(&fl!("app-title"))
        .body(&fl!(
            "reminders-missed",
            count = i64::try_from(count).unwrap_or(i64::MAX)
        ))
        .icon(app_id)
        .hint(notify_rust::Hint::Category(
            "appointment.reminded".to_owned(),
        ))
        .show_async()
        .await;

    if let Err(why) = result {
        tracing::warn!(%why, "could not deliver the missed-reminder summary");
    }
}

/// Where the last sleep began, for the missed-reminder digest.
///
/// The digest covers the triggers between the machine going down and coming
/// back. The going-down instant is taken from login1's own
/// `PrepareForSleep(true)`, not from "when the last sweep ran": a sweep that
/// runs after resume but before the resume signal is handled would otherwise
/// move the window's start past the sleep and empty it, silently absorbing
/// every miss.
///
/// Where login1 never says it is going down (the signal was missed, or the
/// process started asleep), the last sweep is the best available bound.
#[derive(Clone, Copy, Debug)]
pub struct SleepWindow {
    last_sweep: NaiveDateTime,
    asleep_since: Option<NaiveDateTime>,
}

impl SleepWindow {
    #[must_use]
    pub fn new(now: NaiveDateTime) -> Self {
        Self {
            last_sweep: now,
            asleep_since: None,
        }
    }

    /// A reminder sweep ran at `now`.
    pub fn swept(&mut self, now: NaiveDateTime) {
        self.last_sweep = now;
    }

    /// login1 announced the machine is going to sleep.
    pub fn going_down(&mut self, now: NaiveDateTime) {
        self.asleep_since = Some(now);
    }

    /// The machine is back; returns where the window it slept through began.
    pub fn woke(&mut self) -> NaiveDateTime {
        self.asleep_since.take().unwrap_or(self.last_sweep)
    }
}

/// Calls `on_change` each time login1 announces a sleep (`true`) or a resume
/// (`false`).
///
/// `org.freedesktop.login1`'s `PrepareForSleep` carries `true` on the way down
/// and `false` on the way back up; both matter — see [`SleepWindow`]. The
/// signal is on the *system* bus, unlike everything else this module talks
/// to.
///
/// Never returns while the connection holds; a failure to reach login1 (a
/// container, a non-systemd host) is logged once and then simply means no
/// digests, which is the pre-existing behaviour rather than an error.
pub async fn on_sleep(mut on_change: impl FnMut(bool)) {
    use cosmic::iced::futures::StreamExt;
    use zbus::MatchRule;

    let connection = match zbus::Connection::system().await {
        Ok(connection) => connection,
        Err(why) => {
            tracing::debug!(%why, "no system bus; missed-reminder digests are off");
            return;
        }
    };

    let rule = match MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .interface("org.freedesktop.login1.Manager")
        .and_then(|builder| builder.member("PrepareForSleep"))
    {
        Ok(builder) => builder.build(),
        Err(why) => {
            tracing::debug!(%why, "could not build the sleep match rule");
            return;
        }
    };

    let mut stream = match zbus::MessageStream::for_match_rule(rule, &connection, None).await {
        Ok(stream) => stream,
        Err(why) => {
            tracing::debug!(%why, "cannot watch login1 for sleep; digests are off");
            return;
        }
    };

    while let Some(Ok(message)) = stream.next().await {
        if let Ok(going_down) = message.body().deserialize::<bool>() {
            on_change(going_down);
        }
    }
}

/// Sends one reminder to the desktop's notification service, and — when it
/// carries a Join button — waits for that button.
///
/// Async deliberately. Showing a notification is a D-Bus round trip, and
/// `notify-rust`'s blocking variant spins up its own runtime to do it — which
/// panics outright when called from inside one, as both the daemon and the app's
/// executor are. The wait for the button is async too, and bounded by the
/// event's end: after that there is nothing left to join, and an undismissed
/// notification must not hold anything for the life of the process.
///
/// Callers that must not wait — the daemon's sweep — spawn this.
///
/// Failure is logged, not surfaced: a missing notification daemon should not
/// interrupt whatever the user is doing in the calendar.
pub async fn notify(reminder: &Reminder, app_id: &str, body: String) {
    let mut notification = notify_rust::Notification::new();
    notification
        .appname(&fl!("app-title"))
        .summary(&reminder.summary)
        .body(&body)
        .icon(app_id)
        // Calendar reminders should persist until dismissed rather than sliding
        // away after a few seconds.
        .hint(notify_rust::Hint::Category(
            "appointment.reminded".to_owned(),
        ))
        .timeout(notify_rust::Timeout::Never);
    // A video-call link earns the notification a Join button — the reminder
    // for a call should be one press from being in it.
    if reminder.join.is_some() {
        notification.action("join", &fl!("join-call"));
    }

    let handle = match notification.show_async().await {
        Ok(handle) => handle,
        Err(why) => {
            tracing::warn!(%why, "could not deliver a reminder notification");
            return;
        }
    };
    tracing::debug!(summary = %reminder.summary, "reminder delivered");

    let Some(url) = reminder.join.clone() else {
        return;
    };
    let pressed = tokio::time::timeout(
        reminder.joinable_for(),
        handle.wait_for_action_async(|response| {
            if let notify_rust::NotificationResponse::Action(action) = response
                && action == "join"
                && let Err(why) = open::that_detached(&url)
            {
                tracing::warn!(%why, "could not open the meeting link");
            }
        }),
    )
    .await;
    if pressed.is_err() {
        tracing::debug!(summary = %reminder.summary, "the meeting is over; no longer offering to join");
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
    fn a_sleep_counts_the_alarms_it_slept_through() {
        let mut scheduler = Scheduler::new();
        // Asleep from 09:00 to 12:00; two alarms fell in between.
        let events = vec![
            occurrence("standup", at(9, 30)),
            occurrence("review", at(11, 0)),
        ];

        let missed = sweep_missed(
            &mut scheduler,
            &events,
            ten_minutes_before,
            at(9, 0),
            at(12, 0),
            |_| None,
        );
        assert_eq!(missed, 2);
    }

    #[test]
    fn the_digest_counts_each_alarm_once() {
        // A second sweep over the same window must report nothing, or every
        // tick after a resume would re-announce the same misses.
        let mut scheduler = Scheduler::new();
        let events = vec![occurrence("standup", at(9, 30))];

        assert_eq!(
            sweep_missed(
                &mut scheduler,
                &events,
                ten_minutes_before,
                at(9, 0),
                at(12, 0),
                |_| None
            ),
            1
        );
        assert_eq!(
            sweep_missed(
                &mut scheduler,
                &events,
                ten_minutes_before,
                at(9, 0),
                at(12, 0),
                |_| None
            ),
            0
        );
    }

    #[test]
    fn an_alarm_still_inside_its_grace_is_left_to_fire() {
        // It is about to be shown properly; counting it as missed would both
        // suppress it and lie about it. Starts at 12:07, so its ten-minute
        // alarm fired at 11:57 — three minutes ago, inside the grace window.
        let mut scheduler = Scheduler::new();
        let events = vec![occurrence("standup", at(12, 7))];

        let missed = sweep_missed(
            &mut scheduler,
            &events,
            ten_minutes_before,
            at(9, 0),
            at(12, 0),
            |_| None,
        );
        assert_eq!(missed, 0);

        let due = scheduler.due(&events, ten_minutes_before, at(12, 0), |_| None);
        assert_eq!(due.len(), 1, "the reminder was swallowed by the digest");
    }

    #[test]
    fn alarms_from_before_the_sleep_are_not_counted() {
        // They were already missed, or already shown, before the machine slept.
        let mut scheduler = Scheduler::new();
        let events = vec![occurrence("early", at(7, 0))];

        assert_eq!(
            sweep_missed(
                &mut scheduler,
                &events,
                ten_minutes_before,
                at(9, 0),
                at(12, 0),
                |_| None
            ),
            0
        );
    }

    #[test]
    fn events_without_alarms_only_count_under_a_default() {
        let mut scheduler = Scheduler::new();
        let events = vec![occurrence("quiet", at(9, 30))];

        assert_eq!(
            sweep_missed(
                &mut scheduler,
                &events,
                no_alarms,
                at(9, 0),
                at(12, 0),
                |_| None
            ),
            0
        );
        assert_eq!(
            sweep_missed(
                &mut scheduler,
                &events,
                no_alarms,
                at(9, 0),
                at(12, 0),
                |_| Some(Duration::minutes(10))
            ),
            1
        );
    }

    #[test]
    fn fires_once_the_trigger_has_passed() {
        let mut scheduler = Scheduler::new();
        let events = [occurrence("Standup", at(9, 0))];

        assert!(
            scheduler
                .due(&events, ten_minutes_before, at(8, 45), |_| None)
                .is_empty(),
            "fired before the trigger"
        );

        let got = scheduler.due(&events, ten_minutes_before, at(8, 50), |_| None);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].summary, "Standup");
    }

    #[test]
    fn never_fires_the_same_reminder_twice() {
        let mut scheduler = Scheduler::new();
        let events = [occurrence("Standup", at(9, 0))];

        assert_eq!(
            scheduler
                .due(&events, ten_minutes_before, at(8, 50), |_| None)
                .len(),
            1
        );
        for minute in 51..55 {
            assert!(
                scheduler
                    .due(&events, ten_minutes_before, at(8, minute), |_| None)
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
                .due(&events, ten_minutes_before, at(17, 0), |_| None)
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
                .due(&events, ten_minutes_before, at(8, 52), |_| None)
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
                .due(&events, no_alarms, at(8, 50), |_| None)
                .is_empty(),
            "notified without any alarm configured"
        );
        assert_eq!(
            scheduler
                .due(&events, no_alarms, at(8, 50), |_| Some(Duration::minutes(
                    10
                )))
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
        let got = scheduler.due(&events, ten_minutes_before, at(8, 50), |_| {
            Some(Duration::minutes(60))
        });
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].id.offset_secs, -600);
    }

    #[test]
    fn multiple_alarms_on_one_event_each_fire() {
        let mut scheduler = Scheduler::new();
        let events = [occurrence("Flight", at(9, 0))];
        let two = |_: &Occurrence| vec![Duration::minutes(-60), Duration::minutes(-10)];

        assert_eq!(scheduler.due(&events, two, at(8, 5), |_| None).len(), 1);
        assert_eq!(scheduler.due(&events, two, at(8, 51), |_| None).len(), 1);
    }

    #[test]
    fn two_instances_of_a_series_are_separate_reminders() {
        let mut scheduler = Scheduler::new();
        let monday = occurrence("Standup", at(9, 0));
        let tuesday = occurrence("Standup", at(9, 0) + Duration::days(1));

        assert_eq!(
            scheduler
                .due(&[monday], ten_minutes_before, at(8, 50), |_| None)
                .len(),
            1
        );
        assert_eq!(
            scheduler
                .due(
                    &[tuesday],
                    ten_minutes_before,
                    at(8, 50) + Duration::days(1),
                    |_| None
                )
                .len(),
            1,
            "the next instance of the series was suppressed"
        );
    }

    #[test]
    fn the_join_button_is_waited_on_only_until_the_meeting_ends() {
        let mut scheduler = Scheduler::new();
        let events = [occurrence("Standup", at(9, 0))];

        // Fired at 08:50 for a 09:00-10:00 meeting.
        let reminder = scheduler
            .due(&events, ten_minutes_before, at(8, 50), |_| None)
            .remove(0);
        assert_eq!(reminder.joinable_for(), std::time::Duration::from_mins(70));
    }

    #[test]
    fn forgetting_prunes_old_reminders() {
        let mut scheduler = Scheduler::new();
        scheduler.due(
            &[occurrence("Standup", at(9, 0))],
            ten_minutes_before,
            at(8, 50),
            |_| None,
        );
        assert_eq!(scheduler.tracked(), 1);

        scheduler.forget_before(at(9, 0) + Duration::days(1));
        assert_eq!(scheduler.tracked(), 0);
    }

    /// A store in a temporary directory holding one calendar, and the zone its
    /// occurrences are expressed in.
    fn store_with(
        events: impl FnOnce(&str, chrono_tz::Tz) -> Vec<crate::model::Event>,
    ) -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(
            &dir.path().join("calendars"),
            &dir.path().join("index.sqlite"),
        )
        .unwrap();
        let calendar = store
            .create_calendar("Personal", crate::model::PALETTE[0])
            .unwrap();
        for event in events(&calendar.id, store.local_timezone()) {
            store.save(&event).unwrap();
        }
        (dir, store)
    }

    fn event_at(
        calendar_id: &str,
        local: chrono_tz::Tz,
        summary: &str,
        start: NaiveDateTime,
    ) -> crate::model::Event {
        let mut event = crate::model::Event::draft(calendar_id, start, local);
        event.summary = summary.into();
        event
    }

    #[test]
    fn a_restart_does_not_repeat_what_already_fired() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("slate").join("fired-reminders.json");
        let events = [occurrence("Standup", at(9, 0))];

        let mut before = Scheduler::persistent(path.clone());
        assert_eq!(
            before
                .due(&events, ten_minutes_before, at(8, 50), |_| None)
                .len(),
            1
        );
        before.persist().unwrap();

        // The daemon restarts a minute later, still inside the grace window.
        let mut after = Scheduler::persistent(path);
        assert!(
            after
                .due(&events, ten_minutes_before, at(8, 51), |_| None)
                .is_empty(),
            "the restarted process fired the same reminder again"
        );
    }

    #[test]
    fn a_hand_over_sees_what_the_other_process_fired() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("fired-reminders.json");
        let config = Config::default();
        let (_store_dir, store) = store_with(|calendar, local| {
            let mut standup = event_at(calendar, local, "Standup", at(9, 0));
            standup.alarms = vec![Duration::minutes(-10)];
            vec![standup]
        });

        // The daemon fires, then goes away; the app, which started earlier,
        // takes over inside the grace window.
        let mut app = Scheduler::persistent(path.clone());
        let mut daemon = Scheduler::persistent(path);
        assert_eq!(
            due_reminders(&store, &mut daemon, &config, at(8, 50))
                .unwrap()
                .len(),
            1
        );
        assert!(
            due_reminders(&store, &mut app, &config, at(8, 51))
                .unwrap()
                .is_empty(),
            "the reminder fired in both processes"
        );
    }

    #[test]
    fn an_alarm_days_ahead_of_its_event_fires() {
        // The flight is on Friday; its alarm is two days before.
        let config = Config::default();
        let friday = at(9, 0) + Duration::days(3);
        let (_dir, store) = store_with(|calendar, local| {
            let mut flight = event_at(calendar, local, "Flight", friday);
            flight.alarms = vec![Duration::days(-2)];
            vec![flight]
        });

        let due = due_reminders(
            &store,
            &mut Scheduler::new(),
            &config,
            friday - Duration::days(2),
        )
        .unwrap();
        assert_eq!(due.len(), 1, "an alarm beyond two days out never fired");
        assert!(
            due[0]
                .body(&config)
                .contains(&crate::ui::format_date_short(friday.date())),
            "a reminder days ahead must say which day: {}",
            due[0].body(&config)
        );
    }

    #[test]
    fn the_join_button_finds_a_link_in_the_description() {
        let config = Config::default();
        let (_dir, store) = store_with(|calendar, local| {
            let mut review = event_at(calendar, local, "Review", at(9, 0));
            review.alarms = vec![Duration::minutes(-10)];
            review.description = Some("Join: https://meet.google.com/abc-defg-hij".into());
            vec![review]
        });

        let due = due_reminders(&store, &mut Scheduler::new(), &config, at(8, 50)).unwrap();
        assert_eq!(
            due[0].join.as_deref(),
            Some("https://meet.google.com/abc-defg-hij")
        );
    }

    #[test]
    fn the_join_button_takes_a_conference_uri() {
        let config = Config::default();
        let (_dir, store) = store_with(|calendar, local| {
            let mut review = event_at(calendar, local, "Review", at(9, 0));
            review.alarms = vec![Duration::minutes(-10)];
            review.other = vec![
                "CONFERENCE;VALUE=URI;FEATURE=VIDEO:https://video.example.org/r/42".to_owned(),
            ];
            vec![review]
        });

        let due = due_reminders(&store, &mut Scheduler::new(), &config, at(8, 50)).unwrap();
        assert_eq!(
            due[0].join.as_deref(),
            Some("https://video.example.org/r/42")
        );
    }

    #[test]
    fn a_sweep_after_resume_does_not_empty_the_digest_window() {
        let mut window = SleepWindow::new(at(8, 0));
        window.swept(at(8, 30));
        window.going_down(at(9, 0));
        // Resumed at 12:00, and a tick ran before the resume signal arrived.
        window.swept(at(12, 0));
        assert_eq!(window.woke(), at(9, 0));
    }

    #[test]
    fn without_a_going_down_signal_the_last_sweep_bounds_the_window() {
        let mut window = SleepWindow::new(at(8, 0));
        window.swept(at(8, 30));
        assert_eq!(window.woke(), at(8, 30));
    }

    #[tokio::test]
    async fn the_app_follows_the_daemon_coming_and_going() {
        async fn next(rx: &mut tokio::sync::mpsc::UnboundedReceiver<bool>) -> bool {
            tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv())
                .await
                .expect("an ownership change within five seconds")
                .expect("the watch is still running")
        }

        let bus = crate::testbus::PrivateBus::start();
        let app = bus.connect().await;

        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let watch = tokio::spawn(async move {
            let _ = watch_owner(&app, |owned| {
                let _ = tx.send(owned);
            })
            .await;
        });

        assert!(!next(&mut rx).await, "nobody owns reminders yet");

        let daemon = claim_ownership(bus.connect().await)
            .await
            .unwrap()
            .expect("the name was free");
        assert!(
            next(&mut rx).await,
            "the daemon started; the app must stand down"
        );

        // A second daemon bows out rather than queueing for the name.
        assert!(
            claim_ownership(bus.connect().await)
                .await
                .unwrap()
                .is_none()
        );

        drop(daemon);
        assert!(
            !next(&mut rx).await,
            "the daemon exited; the app must take over"
        );

        watch.abort();
    }
}
