// SPDX-License-Identifier: GPL-3.0-only

//! The application model: state, messages, and the update loop.

use crate::config::{Config, ViewKind};
use crate::fl;
use crate::model::{CalendarMeta, Occurrence, PALETTE};
use crate::store::Store;
use crate::ui::editor::{DateField, Editor, TzField};
use crate::ui::{month::MonthView, sidebar::Sidebar, timegrid::TimeGrid};
use chrono::{Datelike, Duration, NaiveDate, NaiveDateTime, NaiveTime, Timelike};
use cosmic::app::context_drawer;
use cosmic::cosmic_config::{self, CosmicConfigEntry};
use cosmic::iced::{Alignment, Length, Subscription, futures};
use cosmic::prelude::*;
use cosmic::widget::{self, about::About, menu, nav_bar, segmented_button};
use cosmic_ext_widgets::{Mode as SidebarMode, SidebarState};
use futures::SinkExt;
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;

const REPOSITORY: &str = env!("CARGO_PKG_REPOSITORY");
/// Hour the week/day grid scrolls to when the day holds no timed events.
const DEFAULT_SCROLL_HOUR: u32 = 8;

/// Lead times offered for the default reminder. `0` disables it.
const REMINDER_CHOICES: &[u32] = &[0, 5, 10, 15, 30, 60, 120, 1440];

/// Snap steps offered for grid drags, in minutes.
const SNAP_CHOICES: &[u8] = &[5, 10, 15];

/// Per-calendar reminder lead times. `0` defers to the app-wide setting.
const CALENDAR_REMINDER_CHOICES: &[u32] = &[0, 5, 10, 15, 30, 60, 1440];

/// Per-calendar event lengths. `0` means the usual hour.
const CALENDAR_DURATION_CHOICES: &[u32] = &[0, 15, 30, 45, 60, 90, 120];

thread_local! {
    /// Built once per thread: the labels are localised, so they cannot be a const.
    static REMINDER_LABELS: Vec<String> = REMINDER_CHOICES
        .iter()
        .map(|minutes| match minutes {
            0 => fl!("reminder-none"),
            1440 => fl!("reminder-day-before"),
            m if *m < 60 => fl!("reminder-minutes", minutes = i64::from(*m)),
            m => fl!("reminder-hours", hours = i64::from(m / 60)),
        })
        .collect();
}
thread_local! {
    /// Labels for the per-calendar dropdowns. Localised, so not consts.
    static CALENDAR_REMINDER_LABELS: Vec<String> = CALENDAR_REMINDER_CHOICES
        .iter()
        .map(|minutes| match minutes {
            0 => fl!("calendar-default-inherit"),
            1440 => fl!("reminder-day-before"),
            m if *m < 60 => fl!("reminder-minutes", minutes = i64::from(*m)),
            m => fl!("reminder-hours", hours = i64::from(m / 60)),
        })
        .collect();

    static CALENDAR_DURATION_LABELS: Vec<String> = CALENDAR_DURATION_CHOICES
        .iter()
        .map(|minutes| match minutes {
            0 => fl!("calendar-duration-default"),
            m if *m < 60 => fl!("duration-minutes", minutes = i64::from(*m)),
            m if *m % 60 == 0 => fl!("duration-hours", hours = i64::from(m / 60)),
            m => fl!("duration-minutes", minutes = i64::from(*m)),
        })
        .collect();
}
const APP_ICON: &[u8] =
    include_bytes!("../resources/icons/hicolor/scalable/apps/com.magnetaros.Slate.svg");

/// The `calendar_id` birthday entries carry. Not a real collection: a
/// birthday is a fact about a contact, synthesised into the visible days on
/// demand and never written anywhere. See `cosmic_pim_core::birthdays`.
pub const BIRTHDAYS_CALENDAR_ID: &str = "suite-birthdays";

pub struct AppModel {
    core: cosmic::Core,
    context_page: ContextPage,
    about: About,
    key_binds: HashMap<menu::KeyBind, MenuAction>,

    config: Config,
    config_handler: Option<cosmic_config::Config>,

    /// `None` when the calendar directory could not be opened at all.
    store: Option<Store>,
    /// Surfaced in the main area when there is nothing else to show.
    fatal: Option<String>,

    /// The date the current view is centred on.
    anchor: NaiveDate,
    today: NaiveDate,
    /// Local wall-clock time, refreshed each minute to drive the "now" marker.
    now: NaiveDateTime,
    /// Occurrences for the visible range, grouped by day.
    days: BTreeMap<NaiveDate, Vec<Occurrence>>,
    /// Contacts from the suite's address books, held for birthday synthesis.
    /// Loaded at startup and after each sync pass; empty when birthdays are
    /// off or there is no address book.
    contact_cards: Vec<crate::model::Contact>,

    views: segmented_button::SingleSelectModel,
    mini: widget::calendar::CalendarModel,
    /// Expanded, rail, or hidden, and whether the element is still in the tree
    /// while it slides out. libcosmic's own nav-bar state stays the master
    /// switch for shown-at-all — it owns the narrow-window breakpoint — and
    /// `sync_sidebar` folds it into this.
    sidebar: SidebarState,
    /// The width the user last chose while the sidebar was showing, so a
    /// narrow window hiding it and a wide one bringing it back does not also
    /// change its shape.
    sidebar_rail: bool,

    editor: Option<Editor>,
    new_calendar_name: Option<String>,
    /// Accounts and credentials. `None` when the store could not be opened —
    /// the calendar still works, it just cannot sync.
    accounts: Option<cosmic_pim_accounts::AccountStore>,
    account_form: Option<AccountForm>,
    syncing: bool,
    /// Loaded on demand for the Tasks view.
    todos: Vec<crate::model::Todo>,
    show_done_tasks: bool,
    new_task: String,
    task_editor: Option<crate::ui::task_editor::TaskEditor>,
    sync_status: Option<String>,

    /// The in-progress "add a subscription" form, when open.
    sub_form: Option<SubscriptionForm>,
    /// A subscription the user asked to remove, awaiting the confirm dialog.
    pending_feed_removal: Option<String>,
    /// An account the user asked to remove, awaiting the confirm dialog.
    pending_account_removal: Option<String>,
    /// A local calendar the user asked to delete, awaiting the confirm dialog.
    pending_calendar_removal: Option<String>,
    /// Names being typed for local calendars, by id, until committed.
    calendar_name_drafts: HashMap<String, String>,
    /// The export dialog's chosen calendar, by index into `calendars()`;
    /// `Some` shows the dialog.
    export_choice: Option<usize>,
    /// True while a background feed-refresh pass is running.
    refreshing_feeds: bool,
    /// When feeds were last checked for due refreshes, to pace the checks.
    last_feed_check: Option<std::time::Instant>,
    /// Unresolved sync conflicts, shaped for the Accounts page.
    conflicts: Vec<ConflictRow>,
    /// The secondary-timezone settings field as typed, which may be a prefix
    /// of a zone name that does not parse yet.
    secondary_tz_input: String,

    /// A pointer interaction in progress on the time grid.
    grid_drag: Option<GridDrag>,
    /// The pointer's last known position on the grid, as a wall-clock time.
    grid_cursor: Option<NaiveDateTime>,

    /// The search overlay's query; `Some` replaces the grid with results.
    search_query: Option<String>,
    search_results: Vec<Occurrence>,

    /// The undo/redo journal: file snapshots around each mutation.
    history: crate::undo::History,

    /// The quick-add dialog's input; `Some` shows the dialog.
    quick_add: Option<String>,
    /// Where the current (or last) sleep began, so a resume knows which
    /// window it slept through.
    sleep: crate::reminders::SleepWindow,

    /// A pending "this event / all events" question. The editor stays open
    /// underneath until the user answers or cancels.
    scope_prompt: Option<ScopePrompt>,

    toasts: widget::Toasts<Message>,
    reminders: crate::reminders::Scheduler,
    /// True while the background daemon holds the owner name: it fires the
    /// reminders, so we stay quiet, and it runs every sync pass, so we ask it
    /// rather than syncing ourselves.
    daemon_running: bool,

    /// The single-instance bus connection, once libcosmic hands it over. The
    /// scheduling interface is served on it and invitation replies go out on
    /// it.
    dbus: Option<zbus::Connection>,
    /// Invitations delivered over the bus, oldest first; the dialog shows the
    /// front of the queue.
    invitations: Vec<PendingInvitation>,
}

#[derive(Debug, Clone)]
pub enum Message {
    // Shell
    LaunchUrl(String),
    ToggleContextPage(ContextPage),
    ToggleSidebar,
    /// The sidebar's closing slide finished; the element can leave the tree.
    SidebarClosed,
    UpdateConfig(Config),
    CloseToast(widget::ToastId),

    // Navigation
    ViewSelected(segmented_button::Entity),
    SetView(ViewKind),
    Today,
    Previous,
    Next,
    OpenDay(NaiveDate),
    MiniDateSelected(jiff::civil::Date),
    MiniPrevMonth,
    MiniNextMonth,

    // Calendars
    ToggleCalendar(String),
    // Tasks
    TaskToggleDone(String, String, bool),
    NewTaskChanged(String),
    NewTaskSubmit,
    TaskEdit(String, String),
    TaskEditorSummary(String),
    TaskEditorDescription(String),
    TaskEditorToggleDue,
    TaskEditorDueTime(String),
    TaskEditorPickDate,
    TaskEditorDatePicked(jiff::civil::Date),
    TaskEditorPickerPrev,
    TaskEditorPickerNext,
    TaskEditorPriority(usize),
    TaskEditorStatus(usize),
    TaskEditorCalendar(usize),
    TaskEditorSave,
    TaskEditorDelete,
    ToggleShowCompleted,

    // Accounts
    AccountAddStart,
    AccountAddCancel,
    AccountAddConfirm,
    AccountNameChanged(String),
    AccountUrlChanged(String),
    AccountUsernameChanged(String),
    AccountPasswordChanged(String),
    // Local calendars: (id, typed name), (id), (id, palette index), (id)
    CalendarNameInput(String, String),
    CalendarNameCommit(String),
    CalendarRecolor(String, usize),
    CalendarDeleteRequest(String),
    CalendarDeleteConfirm,
    CalendarDeleteCancel,

    /// Asks to remove the account with this id; confirmed through a dialog
    /// before anything is deleted.
    AccountRemoveRequest(String),
    AccountRemoveConfirm,
    AccountRemoveCancel,
    SyncNow,
    SyncFinished(Vec<String>, bool),

    // ICS feed subscriptions
    SubAddStart,
    SubAddCancel,
    SubNameChanged(String),
    SubUrlChanged(String),
    SubAddConfirm,
    /// Asks to remove the subscription with this collection id; confirmed
    /// through a dialog before anything is deleted.
    SubRemoveRequest(String),
    SubRemoveConfirm,
    SubRemoveCancel,
    /// A background feed-refresh pass finished; `true` if anything changed.
    FeedsRefreshed(bool),

    // Sync conflicts, by index into the conflicts list
    ConflictKeepLocal(usize),
    ConflictTakeRemote(usize),
    /// Pick which side survives for one disputed unit:
    /// (conflict index, unit index, side).
    ConflictChooseSide(usize, usize, cosmic_pim_core::merge::Side),
    /// Build the merged document from the per-unit choices and keep it.
    ConflictApplyMerge(usize),

    NewCalendarStart,
    NewCalendarNameChanged(String),
    NewCalendarConfirm,
    NewCalendarCancel,

    // Events
    NewEvent,
    NewEventOn(NaiveDate),
    /// Calendar id, UID, and — for a series member — the instance's identity,
    /// so the editor opens the override component when one exists.
    OpenEvent(String, String, Option<chrono::DateTime<chrono::Utc>>),

    // Editor
    EditorSummary(String),
    EditorLocation(String),
    EditorDescription(String),
    EditorCalendar(usize),
    EditorAllDay(bool),
    EditorStartTime(String),
    EditorEndTime(String),
    EditorFreq(usize),
    /// Adds the reminder preset at this index in `ALARM_PRESETS`.
    EditorAlarmAdd(usize),
    /// Removes the event's reminder at this index.
    EditorAlarmRemove(usize),
    EditorInterval(String),
    EditorRepeatEnd(usize),
    EditorCount(String),
    /// Opens or closes the timezone picker for one of the two zone fields.
    EditorTzToggle(TzField),
    EditorTzQuery(String),
    /// An IANA zone name chosen from the picker's matches.
    EditorTzChosen(String),
    /// Back to the default for whichever field the picker is open on:
    /// the system zone for the start, "same as start" for the end.
    EditorTzClear,
    EditorPickDate(DateField),
    EditorDatePicked(jiff::civil::Date),
    EditorPickerPrev,
    EditorPickerNext,
    EditorAttendeeDraft(String),
    EditorAttendeeAdd,
    EditorAttendeeRemove(usize),
    /// Ask the calendar's server when the attendees are busy.
    EditorCheckAvailability,
    AvailabilityAnswer(crate::ui::editor::AvailabilityView),
    EditorSave,
    EditorCancel,
    EditorDelete,
    /// The user answered the scope question for the pending operation.
    ScopeChosen(EditScope),
    ScopeCancelled,

    // Settings
    SetFirstDayOfWeek(usize),
    ToggleWeekNumbers(bool),
    ToggleShowBirthdays(bool),
    Toggle24Hour(bool),
    SetDefaultReminder(usize),
    /// The secondary-timezone field as typed; persisted once it is empty or
    /// names a real IANA zone.
    SetSecondaryTz(String),
    /// Index into [`SNAP_CHOICES`].
    SetSnapMinutes(usize),
    /// A calendar's own reminder default: its id, and an index into
    /// [`CALENDAR_REMINDER_CHOICES`].
    SetCalendarReminder(String, usize),
    /// A calendar's own event length: its id, and an index into
    /// [`CALENDAR_DURATION_CHOICES`].
    SetCalendarDuration(String, usize),

    // iMIP invitations, delivered over the bus by Envelope
    InvitationDelivered(crate::scheduling::Delivery),
    /// The user answered the invitation at the front of the queue.
    InvitationAnswer(InviteAnswer),
    /// The user put the decision off; the mail keeps the payload.
    InvitationDismiss,
    /// The reply handed to Envelope came back: `true` = queued in its outbox.
    InvitationReplySent(bool),

    /// login1 announced a sleep (`true`) or the resume from one (`false`).
    Sleep(bool),

    // History
    Undo,
    Redo,

    // Quick add
    QuickAddOpen,
    QuickAddInput(String),
    QuickAddConfirm,
    QuickAddCancel,

    // Search
    SearchOpen,
    SearchInput(String),
    SearchClose,
    /// A result was chosen: jump the anchor to its date and open it.
    SearchHit(
        NaiveDate,
        String,
        String,
        Option<chrono::DateTime<chrono::Utc>>,
    ),

    // Time-grid pointer interactions
    /// The pointer moved over a day column: the date, and the y offset in px.
    GridHover(NaiveDate, f32),
    /// The pointer went down on a block (or its resize strip).
    GridBlockPress(GridBlockRef),
    /// The pointer went down on empty grid space at this hour.
    GridEmptyPress(NaiveDateTime),
    /// The pointer was released over the grid.
    GridRelease,

    /// Deferred one frame so the grid's scrollable exists before we scroll it.
    ScrollTimeGrid,

    /// A key press no widget consumed, matched against [`crate::key_bind`].
    /// The physical key travels too: matching falls back to it, which is the
    /// only reason Ctrl+N works on a Greek or Cyrillic layout, where the
    /// logical key is "ν", not "n".
    Key(
        cosmic::iced::keyboard::Modifiers,
        cosmic::iced::keyboard::Key,
        cosmic::iced::keyboard::key::Physical,
    ),

    /// Minute tick, moving the "now" marker.
    Tick,

    /// Something on disk changed under us.
    FilesChanged,

    // Import / export
    ImportRequested,
    ExportRequested,
    /// The export dialog's calendar picker, by index into the calendar list.
    ExportCalendarChosen(usize),
    ExportConfirm,
    ExportCancel,
    ImportPath(PathBuf),
    ExportTo(PathBuf, String),
    DialogCancelled,
    DialogFailed(String),

    /// The user pressed Snooze on this reminder's notification.
    ReminderSnoozed(crate::reminders::ReminderId),

    /// Whether another process (the daemon) is firing reminders for us —
    /// reported at start-up and again whenever that changes.
    ReminderOwnership(bool),

    /// A background task finished and has nothing to report.
    Ignore,
}

/// Start-up options, from the command line or a D-Bus activation.
///
/// Build one with [`Flags::new`] rather than by filling the fields in: the
/// hand-over request a second launch sends over the bus is derived from them
/// once, at construction, because [`CosmicFlags::action`] can only return a
/// reference to something the struct already owns.
#[derive(Clone, Debug, Default)]
pub struct Flags {
    /// Open on this date rather than today.
    pub initial_date: Option<NaiveDate>,
    /// `.ics` files to import on start-up.
    pub import: Vec<PathBuf>,
    /// Open the editor on a blank event once the window is up.
    pub new_event: bool,
    /// What to ask an already-running instance to do, or `None` when there is
    /// nothing to say and raising its window is the whole request.
    task: Option<SlateTask>,
}

impl Flags {
    #[must_use]
    pub fn new(initial_date: Option<NaiveDate>, import: Vec<PathBuf>, new_event: bool) -> Self {
        // A bare `slate` with nothing to say should raise the existing window
        // and stop there, which is what a `None` action means to libcosmic.
        let task =
            (initial_date.is_some() || new_event || !import.is_empty()).then_some(SlateTask {
                date: initial_date,
                new_event,
            });

        Self {
            initial_date,
            import,
            new_event,
            task,
        }
    }
}

/// What a second launch asks the running instance to do.
///
/// libcosmic's single-instance entry point hands the whole request across the
/// bus as one action string plus a list of arguments. Serialising the struct
/// rather than inventing a bespoke encoding keeps the two ends impossible to
/// drift apart, and is the shape `cosmic-app-library` uses for the same job.
/// The `.ics` paths travel separately, in [`CosmicFlags::args`], because that is
/// the only part of the request that is a list.
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct SlateTask {
    pub date: Option<NaiveDate>,
    pub new_event: bool,
}

impl std::fmt::Display for SlateTask {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Infallible for this shape; the fallback keeps the impl total.
        f.write_str(&serde_json::to_string(self).unwrap_or_else(|_| "{}".to_owned()))
    }
}

impl std::str::FromStr for SlateTask {
    type Err = serde_json::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        serde_json::from_str(s)
    }
}

impl cosmic::app::CosmicFlags for Flags {
    type SubCommand = SlateTask;
    type Args = Vec<String>;

    fn action(&self) -> Option<&Self::SubCommand> {
        self.task.as_ref()
    }

    fn args(&self) -> Vec<&str> {
        self.import
            .iter()
            .filter_map(|path| path.to_str())
            .collect()
    }
}

#[derive(Copy, Clone, Debug, Default, Eq, PartialEq)]
pub enum ContextPage {
    #[default]
    About,
    Editor,
    Settings,
    Accounts,
    TaskEditor,
}

/// Which operation a scope answer applies to.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScopePrompt {
    Save,
    Delete,
}

/// How far an edit or delete of a series instance reaches.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EditScope {
    /// Only the clicked instance: an override on save, an `EXDATE` on delete.
    This,
    /// The clicked instance and everything after it: the series is split just
    /// before it — on save the successor series carries the edit, on delete
    /// the master is truncated and nothing succeeds it.
    Following,
    /// The whole series.
    All,
}

/// The in-progress "add an account" form.
#[derive(Clone, Debug, Default)]
pub struct AccountForm {
    pub display_name: String,
    pub url: String,
    pub username: String,
    pub password: String,
    pub error: Option<String>,
}

/// The in-progress "add a subscription" form.
#[derive(Default)]
pub struct SubscriptionForm {
    pub name: String,
    pub url: String,
    pub error: Option<String>,
}

/// One unresolved sync conflict, shaped for display: what each side's
/// component says it is, plus the identity needed to resolve it.
#[derive(Clone, Debug)]
pub struct ConflictRow {
    pub collection: String,
    pub href: String,
    /// The local, unsent edit — "yours".
    pub yours: String,
    /// The server's current version — "theirs".
    pub theirs: String,
    /// Per-unit resolution material, when the sync pass recorded the revision
    /// both sides diverged from and the texts parse as the same kind of
    /// document. `None` leaves only the wholesale answers.
    pub disputes: Option<Disputes>,
}

/// The three-way merge material for one conflict.
#[derive(Clone, Debug)]
pub struct Disputes {
    /// The last-synced text both sides diverged from.
    base: String,
    /// The full local text (`ConflictRow::yours` is only its summary).
    local: String,
    /// The full remote text.
    remote: String,
    /// The units both sides changed incompatibly. Empty means the two edits
    /// touch different units and merge without asking anything.
    pub units: Vec<cosmic_pim_core::merge::Overlap>,
    /// The side chosen so far for each disputed unit, keyed by
    /// [`cosmic_pim_core::merge::Overlap::unit`].
    pub choices: std::collections::BTreeMap<String, cosmic_pim_core::merge::Side>,
}

impl Disputes {
    /// Whether every disputed unit has an answer.
    #[must_use]
    pub fn decided(&self) -> bool {
        self.choices.len() == self.units.len()
    }
}

/// How the user answered an invitation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InviteAnswer {
    Accepted,
    Tentative,
    Declined,
}

impl InviteAnswer {
    /// The RFC 5545 `PARTSTAT` value.
    #[must_use]
    pub fn partstat(self) -> &'static str {
        match self {
            Self::Accepted => "ACCEPTED",
            Self::Tentative => "TENTATIVE",
            Self::Declined => "DECLINED",
        }
    }
}

/// One invitation awaiting an answer, as delivered by Envelope.
#[derive(Clone, Debug)]
pub struct PendingInvitation {
    pub account_id: String,
    /// The verbatim `text/calendar` payload.
    pub ics: String,
    pub parsed: cosmic_pim_caldav::itip::Itip,
    /// The invitation's first VEVENT, for showing the slot.
    pub event: Option<crate::model::Event>,
}

/// Removes the copy an earlier accept stored of the invitation `ics`, now
/// that the user declines it.
///
/// Applied as the organizer's own CANCEL of the same revision, so the tested
/// iTIP paths decide what goes: the whole file for a series or one-off, one
/// cancelled instance when the invitation names a RECURRENCE-ID. `NoMatch`
/// means nothing was stored.
fn withdraw_declined(
    collection: &std::path::Path,
    ics: &str,
    me: &str,
) -> cosmic_pim_caldav::Result<cosmic_pim_caldav::itip::Outcome> {
    use cosmic_pim_caldav::itip;
    // The user's own decision, not a mail: there is no sender to check.
    itip::apply(collection, &itip::with_method(ics, "CANCEL"), me, None)
}

/// The calendar the export dialog starts on: the first one shown in the
/// sidebar that the user writes to — their own, not a subscribed feed — or
/// failing that the first shown, or the first at all.
fn default_export(calendars: &[CalendarMeta], config: &Config) -> usize {
    let shown = |c: &&CalendarMeta| !config.is_hidden(&c.id);
    calendars
        .iter()
        .position(|c| shown(&c) && !c.read_only)
        .or_else(|| calendars.iter().position(|c| shown(&c)))
        .unwrap_or(0)
}

/// Whether an occurrence answers a search for `needle` (already lowercased):
/// in its summary, its location, or its event's `description`.
fn matches_search(needle: &str, occurrence: &Occurrence, description: Option<&str>) -> bool {
    let contains = |text: &str| text.to_lowercase().contains(needle);
    contains(&occurrence.summary)
        || occurrence.location.as_deref().is_some_and(contains)
        || description.is_some_and(contains)
}

/// What the user is told about a CANCEL or REPLY applied without asking:
/// `Ok` for news, `Err` for a payload refused, `None` for nothing to say.
fn quiet_notice(outcome: &cosmic_pim_caldav::itip::Outcome) -> Option<Result<String, String>> {
    use cosmic_pim_caldav::itip::Outcome;
    match outcome {
        Outcome::Cancelled { .. } | Outcome::InstanceCancelled { .. } => {
            Some(Ok(fl!("invitation-cancelled")))
        }
        // Someone other than the organizer tried to cancel a meeting on
        // this calendar. Ignored by the substrate; worth knowing about.
        Outcome::NotFromOrganizer => Some(Err(fl!("invitation-cancel-not-from-organizer"))),
        _ => None,
    }
}

/// Adds a delivered invitation to the queue, once.
///
/// Envelope delivers a REQUEST every time it sees the mail — a re-sync, a
/// second account holding the same message, the organizer re-sending. One
/// question per revision: a copy of what is already queued is dropped, and a
/// newer revision (higher SEQUENCE) of a queued invitation replaces it in
/// place, since answering the older one would be answering something the
/// organizer has already changed.
fn enqueue_invitation(queue: &mut Vec<PendingInvitation>, invitation: PendingInvitation) {
    let same = |queued: &PendingInvitation| {
        queued.parsed.uid == invitation.parsed.uid
            && queued.parsed.recurrence_id == invitation.parsed.recurrence_id
    };
    match queue.iter_mut().find(|queued| same(queued)) {
        Some(queued) if invitation.parsed.sequence > queued.parsed.sequence => *queued = invitation,
        Some(_) => {}
        None => queue.push(invitation),
    }
}

/// Identity and geometry of a time-grid block under the pointer.
#[derive(Clone, Debug)]
pub struct GridBlockRef {
    pub calendar_id: String,
    pub uid: String,
    /// Series-instance identity when the block is one occurrence of a series.
    pub rid: Option<chrono::DateTime<chrono::Utc>>,
    /// The occurrence's span, in the grid's own wall clock.
    pub start: NaiveDateTime,
    pub end: NaiveDateTime,
    /// True when the press landed on the bottom resize strip.
    pub resize: bool,
}

/// A pointer interaction in progress on the time grid, resolved on release.
#[derive(Clone, Debug)]
enum GridDrag {
    /// A block was grabbed. Without movement it is a click and opens the
    /// event; with movement it shifts the event, or its end.
    Block {
        block: GridBlockRef,
        /// Where the pointer was when it went down, so the grab point stays
        /// under the finger rather than snapping the start to the pointer.
        pressed_at: NaiveDateTime,
        moved: bool,
    },
    /// Empty grid space went down; movement sweeps out a new event's span.
    Create {
        anchor: NaiveDateTime,
        current: NaiveDateTime,
        moved: bool,
    },
}

/// Whether any of `busy` overlaps the half-open slot `[from_ms, to_ms)`.
///
/// Half-open on purpose: a meeting that ends exactly when this one starts does
/// not conflict with it, and neither does one that starts exactly when this one
/// ends. Treating a touch as a clash would mark every back-to-back day busy.
fn overlaps_slot(busy: &[cosmic_pim_caldav::itip::BusyPeriod], from_ms: i64, to_ms: i64) -> bool {
    busy.iter()
        .any(|period| period.start_ms < to_ms && period.end_ms > from_ms)
}

/// Identifies the quick-add input so opening the dialog can focus it.
fn quick_add_input_id() -> cosmic::widget::Id {
    cosmic::widget::Id::new("slate-quick-add")
}

/// The span a drag would commit if released now — the ghost's rectangle, and
/// the value the commit itself uses, so what is previewed is what is written.
///
/// `None` while the pointer has not moved far enough to count as a drag: a
/// click must not promise a change it will not make.
fn ghost_span(
    drag: &GridDrag,
    cursor: NaiveDateTime,
    snap: i64,
) -> Option<(NaiveDateTime, NaiveDateTime)> {
    match drag {
        GridDrag::Block {
            block,
            pressed_at,
            moved: true,
        } => {
            if block.resize {
                // Clamped to one step, so dragging the bottom edge above the
                // start shortens the event rather than inverting it.
                let end = snap_to(cursor, snap).max(block.start + Duration::minutes(snap));
                Some((block.start, end))
            } else {
                // Anchored on the grab point rather than the pointer, so the
                // event does not jump under the hand when the drag begins.
                let start = snap_to(cursor - (*pressed_at - block.start), snap);
                Some((start, start + (block.end - block.start)))
            }
        }
        GridDrag::Create {
            anchor,
            current,
            moved: true,
        } => {
            let swept = snap_to(*current, snap);
            let (from, to) = if swept >= *anchor {
                (*anchor, swept)
            } else {
                // Swept upwards: the anchor is the end.
                (swept, *anchor)
            };
            let to = if to <= from {
                from + Duration::minutes(snap)
            } else {
                to
            };
            Some((from, to))
        }
        _ => None,
    }
}

/// Rounds to the nearest `step`-minute mark within the day.
fn snap_to(t: NaiveDateTime, step: i64) -> NaiveDateTime {
    let minutes = i64::from(t.time().hour()) * 60 + i64::from(t.time().minute());
    let snapped = ((minutes + step / 2) / step * step).clamp(0, 24 * 60 - step);
    t.date().and_time(NaiveTime::MIN) + Duration::minutes(snapped)
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum MenuAction {
    About,
    Settings,
    Accounts,
    NewEvent,
    Today,
    Previous,
    Next,
    Month,
    Week,
    Day,
    Agenda,
    Year,
    Tasks,
    Find,
    Undo,
    Redo,
    QuickAdd,
    Import,
    Export,
    SyncNow,
    ToggleSidebar,
}

impl menu::action::MenuAction for MenuAction {
    type Message = Message;

    fn message(&self) -> Self::Message {
        match self {
            MenuAction::About => Message::ToggleContextPage(ContextPage::About),
            MenuAction::Settings => Message::ToggleContextPage(ContextPage::Settings),
            MenuAction::Accounts => Message::ToggleContextPage(ContextPage::Accounts),
            MenuAction::NewEvent => Message::NewEvent,
            MenuAction::Today => Message::Today,
            MenuAction::Previous => Message::Previous,
            MenuAction::Next => Message::Next,
            MenuAction::Month => Message::SetView(ViewKind::Month),
            MenuAction::Week => Message::SetView(ViewKind::Week),
            MenuAction::Day => Message::SetView(ViewKind::Day),
            MenuAction::Agenda => Message::SetView(ViewKind::Agenda),
            MenuAction::Year => Message::SetView(ViewKind::Year),
            MenuAction::Tasks => Message::SetView(ViewKind::Tasks),
            MenuAction::Find => Message::SearchOpen,
            MenuAction::Undo => Message::Undo,
            MenuAction::Redo => Message::Redo,
            MenuAction::QuickAdd => Message::QuickAddOpen,
            MenuAction::Import => Message::ImportRequested,
            MenuAction::Export => Message::ExportRequested,
            MenuAction::SyncNow => Message::SyncNow,
            MenuAction::ToggleSidebar => Message::ToggleSidebar,
        }
    }
}

impl cosmic::Application for AppModel {
    type Executor = cosmic::executor::Default;
    type Flags = Flags;
    type Message = Message;

    const APP_ID: &'static str = "com.magnetaros.Slate";

    fn core(&self) -> &cosmic::Core {
        &self.core
    }

    fn core_mut(&mut self) -> &mut cosmic::Core {
        &mut self.core
    }

    fn init(core: cosmic::Core, flags: Self::Flags) -> (Self, Task<cosmic::Action<Self::Message>>) {
        let config_handler = cosmic_config::Config::new(Self::APP_ID, Config::VERSION).ok();
        let config = config_handler
            .as_ref()
            .map(|handler| match Config::get_entry(handler) {
                Ok(config) => config,
                Err((errors, config)) => {
                    for why in errors {
                        tracing::warn!(%why, "error loading config; falling back to defaults");
                    }
                    config
                }
            })
            .unwrap_or_default();

        let (store, fatal) = match Store::open_default() {
            Ok(store) => (Some(store), None),
            Err(why) => {
                tracing::error!(%why, "cannot open the calendar store");
                (None, Some(why.to_string()))
            }
        };

        let now =
            crate::clock::now_in(store.as_ref().map_or(chrono_tz::UTC, Store::local_timezone));
        let today = now.date();
        // `--date` lets the applet and the launcher plugin open us on a specific day.
        let anchor = flags.initial_date.unwrap_or(today);

        let mut views = segmented_button::SingleSelectModel::default();
        for kind in ViewKind::ALL {
            let entity = views
                .insert()
                .text(view_label(*kind))
                .data::<ViewKind>(*kind)
                .id();
            if *kind == config.view {
                views.activate(entity);
            }
        }

        let about = About::default()
            .name(fl!("app-title"))
            .icon(widget::icon::from_svg_bytes(APP_ICON))
            .version(env!("CARGO_PKG_VERSION"))
            .links([(fl!("repository"), REPOSITORY)])
            .license(env!("CARGO_PKG_LICENSE"));

        let contact_cards = if config.show_birthdays {
            load_contact_cards()
        } else {
            Vec::new()
        };

        let mut app = AppModel {
            core,
            context_page: ContextPage::About,
            about,
            key_binds: crate::key_bind::key_binds(),
            config,
            config_handler,
            store,
            fatal,
            anchor,
            today,
            now,
            days: BTreeMap::new(),
            contact_cards,
            views,
            mini: widget::calendar::CalendarModel::now(),
            sidebar: SidebarState::default(),
            sidebar_rail: false,
            editor: None,
            new_calendar_name: None,
            accounts: match cosmic_pim_accounts::AccountStore::open_default() {
                Ok(accounts) => Some(accounts),
                Err(why) => {
                    tracing::warn!(%why, "cannot open the account store; sync is unavailable");
                    None
                }
            },
            account_form: None,
            syncing: false,
            todos: Vec::new(),
            show_done_tasks: false,
            new_task: String::new(),
            task_editor: None,
            sync_status: None,
            sub_form: None,
            pending_feed_removal: None,
            pending_account_removal: None,
            pending_calendar_removal: None,
            calendar_name_drafts: HashMap::new(),
            export_choice: None,
            refreshing_feeds: false,
            last_feed_check: None,
            conflicts: Vec::new(),
            secondary_tz_input: String::new(),
            grid_drag: None,
            grid_cursor: None,
            search_query: None,
            search_results: Vec::new(),
            history: crate::undo::History::default(),
            quick_add: None,
            sleep: crate::reminders::SleepWindow::new(now),
            scope_prompt: None,
            toasts: widget::Toasts::new(Message::CloseToast),
            // Shared with the daemon, so a hand-over in either direction does
            // not repeat what the other already showed.
            reminders: crate::reminders::Scheduler::persistent(crate::reminders::fired_path()),
            // Assumed until the owner watch first reports, so a fast-starting
            // daemon never races us into a duplicate notification.
            daemon_running: true,
            dbus: None,
            invitations: Vec::new(),
        };

        {
            // One-shot diagnostic: whether the compositor/theme want us frosted,
            // and whether the runtime is opted in.
            let theme = cosmic::theme::active();
            let cosmic_theme = theme.cosmic();
            tracing::debug!(
                frosted_windows = cosmic_theme.frosted_windows,
                frosted_maximized = cosmic_theme.frosted_maximized_apps,
                auto_blur = ?app.core.auto_blur(),
                app_type = ?app.core.app_type(),
                core_frosted = app.core.frosted(cosmic_theme),
                theme_transparent = theme.transparent,
                "blur state at startup"
            );
        }

        app.secondary_tz_input = app.config.secondary_timezone.clone();
        app.reload();
        app.reload_conflicts();
        let mut startup = vec![app.update_title(), Self::request_scroll()];

        for path in flags.import {
            startup.push(cosmic::task::message(cosmic::Action::App(
                Message::ImportPath(path),
            )));
        }

        // `--new-event`, which is what the desktop entry's "New Event" action
        // runs when there is no instance to hand it to over the bus.
        if flags.new_event {
            startup.push(cosmic::task::message(cosmic::Action::App(
                Message::NewEvent,
            )));
        }

        (app, Task::batch(startup))
    }

    fn header_start(&self) -> Vec<Element<'_, Self::Message>> {
        let menu_bar = menu::bar(vec![
            menu::Tree::with_children(
                menu::root(fl!("file")).apply(Element::from),
                menu::items(
                    &self.key_binds,
                    vec![
                        menu::Item::Button(fl!("new-event"), None, MenuAction::NewEvent),
                        menu::Item::Button(fl!("quick-add"), None, MenuAction::QuickAdd),
                        menu::Item::Button(fl!("undo"), None, MenuAction::Undo),
                        menu::Item::Button(fl!("redo"), None, MenuAction::Redo),
                        menu::Item::Divider,
                        menu::Item::Button(fl!("import"), None, MenuAction::Import),
                        menu::Item::Button(fl!("export"), None, MenuAction::Export),
                        menu::Item::Divider,
                        menu::Item::Button(fl!("sync-now"), None, MenuAction::SyncNow),
                        menu::Item::Button(fl!("accounts"), None, MenuAction::Accounts),
                        menu::Item::Divider,
                        menu::Item::Button(fl!("settings"), None, MenuAction::Settings),
                        menu::Item::Button(fl!("about"), None, MenuAction::About),
                    ],
                ),
            ),
            menu::Tree::with_children(
                menu::root(fl!("view")).apply(Element::from),
                menu::items(
                    &self.key_binds,
                    vec![
                        menu::Item::Button(fl!("month"), None, MenuAction::Month),
                        menu::Item::Button(fl!("week"), None, MenuAction::Week),
                        menu::Item::Button(fl!("day"), None, MenuAction::Day),
                        menu::Item::Button(fl!("agenda"), None, MenuAction::Agenda),
                        menu::Item::Button(fl!("year"), None, MenuAction::Year),
                        menu::Item::Button(fl!("tasks"), None, MenuAction::Tasks),
                        menu::Item::Divider,
                        menu::Item::Button(fl!("sidebar"), None, MenuAction::ToggleSidebar),
                        menu::Item::Button(fl!("find"), None, MenuAction::Find),
                        menu::Item::Divider,
                        menu::Item::Button(fl!("previous"), None, MenuAction::Previous),
                        menu::Item::Button(fl!("today"), None, MenuAction::Today),
                        menu::Item::Button(fl!("next"), None, MenuAction::Next),
                    ],
                ),
            ),
        ])
        // Envelope's menu geometry, for the same reasons: the default
        // `Uniform(30)` height gives every divider a full row, and the default
        // 150 width ellipsizes labels and leaves the shortcut column no room.
        .item_height(menu::ItemHeight::Dynamic(36))
        .item_width(menu::ItemWidth::Uniform(260));

        let spacing = cosmic::theme::spacing();

        // libcosmic only draws this itself for apps with a `nav_model`, and ours
        // is `None` (see `nav_bar` below), so we place it. First in the row is
        // where the stock one sits, and it drives libcosmic's own nav-bar state
        // rather than a parallel flag of ours.
        let focused = self
            .core
            .focus_chain()
            .iter()
            .any(|id| Some(*id) == self.core.main_window_id());
        let nav_toggle = widget::nav_bar_toggle()
            .active(self.sidebar.mode() != SidebarMode::Hidden)
            .selected(focused)
            .on_toggle(Message::ToggleSidebar);

        vec![
            widget::row::with_capacity(3)
                .align_y(Alignment::Center)
                .spacing(spacing.space_xxs)
                .push(nav_toggle)
                .push(menu_bar)
                // "New event" lives here rather than beside the view switcher.
                // The right-hand side has to fit four view tabs AND the window
                // controls, and it does not: the button was being clipped in
                // half. There is room on the left, and the action is a sibling
                // of the View menu anyway.
                .push(header_icon(
                    "list-add-symbolic",
                    fl!("new-event"),
                    Message::NewEvent,
                ))
                .into(),
        ]
    }

    fn header_center(&self) -> Vec<Element<'_, Self::Message>> {
        let spacing = cosmic::theme::spacing();

        let mut row = widget::row::with_capacity(4)
            .align_y(Alignment::Center)
            .spacing(spacing.space_xxs);

        // The task list is not a date range: paging it forward would move an
        // anchor nothing on screen reads, and "Today" would appear to do
        // nothing at all.
        if self.view().is_dated() {
            row = row
                .push(header_icon(
                    "go-previous-symbolic",
                    fl!("previous"),
                    Message::Previous,
                ))
                .push(widget::button::standard(fl!("today")).on_press(Message::Today))
                .push(header_icon("go-next-symbolic", fl!("next"), Message::Next));
        }

        vec![
            row.push(widget::text::heading(crate::ui::range_title(
                self.view(),
                self.anchor,
                &self.config,
            )))
            .into(),
        ]
    }

    fn header_end(&self) -> Vec<Element<'_, Self::Message>> {
        let spacing = cosmic::theme::spacing();

        vec![
            widget::row::with_capacity(1)
                .align_y(Alignment::Center)
                .spacing(spacing.space_xxs)
                .push(
                    // A fixed width is unavoidable: the control is `Fill`
                    // internally, so `Shrink` on the container does not
                    // constrain it and it swallows the whole header bar.
                    // Sized per tab so that adding a view widens the strip
                    // instead of silently truncating every label.
                    widget::segmented_control::horizontal(&self.views)
                        .on_activate(Message::ViewSelected)
                        .apply(widget::container)
                        .width(Length::Fixed(VIEW_TAB_WIDTH * ViewKind::ALL.len() as f32)),
                )
                .into(),
        ]
    }

    fn nav_model(&self) -> Option<&nav_bar::Model> {
        // Calendars are a filter, not a set of pages, so the stock nav-bar model
        // is the wrong shape for them. `nav_bar` below fills the slot instead,
        // which is what gives us the toggle button and the correct chrome.
        None
    }

    /// Puts our sidebar in libcosmic's nav-bar slot.
    ///
    /// Overriding this rather than `nav_model` keeps the desktop's nav-bar
    /// styling and padding while letting the content be a mini month and a set
    /// of visibility toggles — neither of which the single-select nav model can
    /// express. The one thing it costs us is the header toggle button, which
    /// libcosmic draws only for apps with a `nav_model`; `header_start` above
    /// puts it back. Shown-at-all is still libcosmic's own state, so the
    /// responsive behaviour comes with it; the width — full, rail, or on its
    /// way to nothing — is ours, through `cosmic-ext-widgets`.
    fn nav_bar(&self) -> Option<Element<'_, cosmic::Action<Self::Message>>> {
        if self.fatal.is_some() {
            return None;
        }

        let sidebar = Sidebar {
            mini: &self.mini,
            calendars: self.calendars(),
            config: &self.config,
            new_calendar_name: self.new_calendar_name.as_ref(),
        };

        self.sidebar
            .view(sidebar.view(), sidebar.rail(), Message::SidebarClosed)
            .map(|element| element.map(cosmic::Action::App))
    }

    fn on_window_resize(&mut self, _id: cosmic::iced::window::Id, _width: f32, _height: f32) {
        // Crossing libcosmic's condensed breakpoint flips its nav-bar state
        // without a message of ours; this is where it tells us.
        self.sync_sidebar();
    }

    fn dialog(&self) -> Option<Element<'_, Self::Message>> {
        // Removing a subscription deletes its collection directory; that gets
        // a confirm, like any destruction would.
        if let Some(id) = &self.pending_feed_removal {
            let name = self
                .store
                .as_ref()
                .and_then(|s| s.calendar(id))
                .map_or_else(|| id.clone(), |meta| meta.name.clone());
            return Some(
                widget::dialog()
                    .title(fl!("remove-subscription-title"))
                    .body(fl!("remove-subscription-body", name = name))
                    .primary_action(
                        widget::button::destructive(fl!("remove"))
                            .on_press(Message::SubRemoveConfirm),
                    )
                    .secondary_action(
                        widget::button::text(fl!("cancel")).on_press(Message::SubRemoveCancel),
                    )
                    .into(),
            );
        }

        // Deleting a calendar deletes its events and tasks with it.
        if let Some(id) = &self.pending_calendar_removal {
            let name = self
                .store
                .as_ref()
                .and_then(|s| s.calendar(id))
                .map_or_else(|| id.clone(), |meta| meta.name.clone());
            return Some(
                widget::dialog()
                    .title(fl!("delete-calendar-title"))
                    .body(fl!("delete-calendar-body", name = name))
                    .primary_action(
                        widget::button::destructive(fl!("delete"))
                            .on_press(Message::CalendarDeleteConfirm),
                    )
                    .secondary_action(
                        widget::button::text(fl!("cancel")).on_press(Message::CalendarDeleteCancel),
                    )
                    .into(),
            );
        }

        // The account list is the whole suite's: removing one here removes it
        // from Envelope and Circle too, and forgets its password. One click
        // must not be able to do that.
        if let Some(id) = &self.pending_account_removal {
            let name = self
                .accounts
                .as_ref()
                .and_then(|accounts| accounts.get(id))
                .map_or_else(|| id.clone(), |account| account.display_name.clone());
            return Some(
                widget::dialog()
                    .title(fl!("remove-account-title"))
                    .body(fl!("remove-account-body", name = name))
                    .primary_action(
                        widget::button::destructive(fl!("remove"))
                            .on_press(Message::AccountRemoveConfirm),
                    )
                    .secondary_action(
                        widget::button::text(fl!("cancel")).on_press(Message::AccountRemoveCancel),
                    )
                    .into(),
            );
        }

        if let Some(chosen) = self.export_choice {
            let names: Vec<String> = self.calendars().iter().map(|c| c.name.clone()).collect();
            return Some(
                widget::dialog()
                    .title(fl!("export"))
                    .body(fl!("export-choose"))
                    .control(widget::dropdown(
                        names,
                        Some(chosen),
                        Message::ExportCalendarChosen,
                    ))
                    .primary_action(
                        widget::button::suggested(fl!("export-action"))
                            .on_press(Message::ExportConfirm),
                    )
                    .secondary_action(
                        widget::button::text(fl!("cancel")).on_press(Message::ExportCancel),
                    )
                    .into(),
            );
        }

        if let Some(input) = &self.quick_add {
            return Some(self.quick_add_dialog(input));
        }

        // An invitation waits behind any scope question: the scope dialog
        // blocks an edit already in the user's hands.
        if self.scope_prompt.is_none()
            && let Some(invitation) = self.invitations.first()
        {
            return Some(self.invitation_dialog(invitation));
        }

        let prompt = self.scope_prompt?;

        let choice = |label: String, scope: EditScope| {
            widget::button::standard(label).on_press(Message::ScopeChosen(scope))
        };

        let dialog = match prompt {
            // Primary is the narrow scope, because "change every future
            // standup" should never be the reflex-click.
            ScopePrompt::Save => widget::dialog()
                .title(fl!("scope-save-title"))
                .body(fl!("scope-save-body"))
                .primary_action(
                    widget::button::suggested(fl!("scope-this"))
                        .on_press(Message::ScopeChosen(EditScope::This)),
                )
                .secondary_action(choice(fl!("scope-following"), EditScope::Following))
                .tertiary_action(choice(fl!("scope-all"), EditScope::All))
                // The fourth choice: a dialog has three action slots, so
                // cancel rides in the body as a control (Escape works too).
                .control(
                    widget::button::text(fl!("cancel"))
                        .on_press(Message::ScopeCancelled)
                        .apply(widget::container)
                        .align_x(cosmic::iced::alignment::Horizontal::Right)
                        .width(Length::Fill),
                ),
            ScopePrompt::Delete => widget::dialog()
                .title(fl!("scope-delete-title"))
                .body(fl!("scope-delete-body"))
                .primary_action(
                    widget::button::destructive(fl!("scope-this"))
                        .on_press(Message::ScopeChosen(EditScope::This)),
                )
                .secondary_action(choice(fl!("scope-following"), EditScope::Following))
                .tertiary_action(choice(fl!("scope-all"), EditScope::All))
                // The fourth choice: a dialog has three action slots, so
                // cancel rides in the body as a control (Escape works too).
                .control(
                    widget::button::text(fl!("cancel"))
                        .on_press(Message::ScopeCancelled)
                        .apply(widget::container)
                        .align_x(cosmic::iced::alignment::Horizontal::Right)
                        .width(Length::Fill),
                ),
        };

        Some(dialog.into())
    }

    fn context_drawer(&self) -> Option<context_drawer::ContextDrawer<'_, Self::Message>> {
        if !self.core.window.show_context {
            return None;
        }

        Some(match self.context_page {
            ContextPage::About => context_drawer::about(
                &self.about,
                |url| Message::LaunchUrl(url.to_string()),
                Message::ToggleContextPage(ContextPage::About),
            ),
            ContextPage::Settings => context_drawer::context_drawer(
                self.settings_view(),
                Message::ToggleContextPage(ContextPage::Settings),
            )
            .title(fl!("settings")),
            ContextPage::TaskEditor => context_drawer::context_drawer(
                self.task_editor.as_ref().map_or_else(
                    || widget::text::body(String::new()).into(),
                    |editor| editor.view(self.store.as_ref().map_or(&[], Store::calendars)),
                ),
                Message::ToggleContextPage(ContextPage::TaskEditor),
            )
            .title(match &self.task_editor {
                Some(editor) if editor.is_new() => fl!("new-task"),
                _ => fl!("edit-task"),
            }),
            ContextPage::Accounts => context_drawer::context_drawer(
                crate::ui::accounts::view(
                    self.accounts.as_ref().map_or(&[], |a| a.accounts()),
                    self.account_form.as_ref(),
                    self.syncing,
                    self.sync_status.as_deref(),
                    self.store.as_ref().map_or_else(Vec::new, |s| {
                        s.calendars()
                            .iter()
                            .filter(|c| cosmic_pim_caldav::feed::is_feed(&c.path))
                            .cloned()
                            .collect()
                    }),
                    self.sub_form.as_ref(),
                    &self.conflicts,
                ),
                Message::ToggleContextPage(ContextPage::Accounts),
            )
            .title(fl!("accounts")),
            ContextPage::Editor => {
                let title = match &self.editor {
                    Some(editor) if editor.is_new() => fl!("new-event"),
                    _ => fl!("edit-event"),
                };
                context_drawer::context_drawer(
                    self.editor_view(),
                    Message::ToggleContextPage(ContextPage::Editor),
                )
                .title(title)
            }
        })
    }

    fn view(&self) -> Element<'_, Self::Message> {
        if let Some(fatal) = &self.fatal {
            return widget::text::body(format!("{}\n\n{fatal}", fl!("error-load-calendars")))
                .apply(widget::container)
                .center(Length::Fill)
                .into();
        }

        let content: Element<'_, Message> = match &self.search_query {
            Some(query) => crate::ui::search::SearchView {
                query,
                results: &self.search_results,
                calendars: self.calendars(),
                config: &self.config,
                today: self.today,
            }
            .view(),
            None => self.grid(self.calendars()),
        };
        let content = content
            .apply(widget::container)
            .width(Length::Fill)
            .height(Length::Fill);

        // The toaster overlays transient errors without stealing focus.
        widget::toaster(&self.toasts, content)
    }

    /// Handles being activated over D-Bus.
    ///
    /// This is what makes the `MimeType=text/calendar` line in the desktop entry
    /// mean something: opening a `.ics` from a file manager or a mail client
    /// reaches the already-running instance here instead of starting a second
    /// copy. `single-instance` in the libcosmic feature list is what routes it.
    /// Serves the suite's scheduling interface on the single-instance name.
    ///
    /// Same connection, same name, one more interface on the same path — the
    /// hand-off contract's whole surface. Delivered payloads flow into the
    /// update loop as [`Message::InvitationDelivered`].
    fn dbus_connection(&mut self, conn: zbus::Connection) -> Task<cosmic::Action<Self::Message>> {
        use cosmic::iced::futures::{StreamExt, channel::mpsc, stream};

        self.dbus = Some(conn.clone());
        let (tx, rx) = mpsc::unbounded();
        let serve = stream::once(async move {
            match conn
                .object_server()
                .at(
                    crate::scheduling::OBJECT_PATH,
                    crate::scheduling::Scheduling::new(tx),
                )
                .await
            {
                Ok(_) => tracing::debug!("scheduling interface exported"),
                Err(why) => tracing::warn!(%why, "could not export the scheduling interface"),
            }
            None
        });
        let deliveries = rx.map(|delivery| Some(Message::InvitationDelivered(delivery)));
        cosmic::task::stream(serve.chain(deliveries).filter_map(std::future::ready))
    }

    fn dbus_activation(
        &mut self,
        msg: cosmic::dbus_activation::Message,
    ) -> Task<cosmic::Action<Self::Message>> {
        use cosmic::dbus_activation::Details;

        match msg.msg {
            // Plain launch: just raise the window, which the runtime has already done.
            Details::Activate => Task::none(),

            Details::Open { url } => {
                let paths: Vec<PathBuf> = url
                    .iter()
                    .filter_map(|url| url.to_file_path().ok())
                    .collect();

                if paths.is_empty() {
                    tracing::warn!(?url, "activation carried no local files");
                    return Task::none();
                }

                Task::batch(paths.into_iter().map(|path| {
                    cosmic::task::message(cosmic::Action::App(Message::ImportPath(path)))
                }))
            }

            // Two different callers land here. A launcher or dock invoking one
            // of the desktop entry's `Actions=` sends the bare action name; a
            // second `slate` process handing over its command line sends a
            // serialised `SlateTask` with the `.ics` paths in `args`.
            Details::ActivateAction { action, args } => match action.as_str() {
                "new-event" => cosmic::task::message(cosmic::Action::App(Message::NewEvent)),
                "today" => cosmic::task::message(cosmic::Action::App(Message::Today)),
                encoded => {
                    let Ok(task) = encoded.parse::<SlateTask>() else {
                        tracing::warn!(action = encoded, ?args, "unknown activation action");
                        return Task::none();
                    };
                    self.apply_task(&task, &args)
                }
            },
        }
    }

    fn on_escape(&mut self) -> Task<cosmic::Action<Self::Message>> {
        // Outermost surface first: a pending scope question, then the context
        // drawer — which is where every transient surface lives (the event
        // editor, the task editor, settings, accounts), so closing it is the
        // whole "cancel" story. Unsaved editor state is dropped, matching what
        // the drawer's own close button does.
        if self.grid_drag.is_some() {
            self.grid_drag = None;
        } else if self.quick_add.is_some() {
            self.quick_add = None;
        } else if self.pending_feed_removal.is_some() {
            self.pending_feed_removal = None;
        } else if self.pending_account_removal.is_some() {
            self.pending_account_removal = None;
        } else if self.pending_calendar_removal.is_some() {
            self.pending_calendar_removal = None;
        } else if self.export_choice.is_some() {
            self.export_choice = None;
        } else if self.scope_prompt.is_some() {
            self.scope_prompt = None;
        } else if self.core.window.show_context {
            self.core.window.show_context = false;
        } else if self.search_query.is_some() {
            self.search_query = None;
            self.search_results.clear();
        }
        Task::none()
    }

    fn subscription(&self) -> Subscription<Self::Message> {
        Subscription::batch(vec![
            self.core()
                .watch_config::<Config>(Self::APP_ID)
                .map(|update| {
                    for why in update.errors {
                        tracing::debug!(?why, "config watch error");
                    }
                    Message::UpdateConfig(update.config)
                }),
            file_watch_subscription(),
            sleep_subscription(),
            owner_subscription(),
            // Moves the "now" marker and rolls the highlight over at midnight.
            cosmic::iced::time::every(std::time::Duration::from_secs(30)).map(|_| Message::Tick),
            // Only `Ignored` presses: a focused text input has already claimed
            // anything it wants, so the editor keeps its arrow keys.
            cosmic::iced::event::listen_with(|event, status, _window| match (event, status) {
                (
                    cosmic::iced::Event::Keyboard(cosmic::iced::keyboard::Event::KeyPressed {
                        modifiers,
                        key,
                        physical_key,
                        ..
                    }),
                    cosmic::iced::event::Status::Ignored,
                ) => Some(Message::Key(modifiers, key, physical_key)),
                _ => None,
            }),
        ])
    }

    fn update(&mut self, message: Self::Message) -> Task<cosmic::Action<Self::Message>> {
        match message {
            Message::LaunchUrl(url) => {
                if let Err(why) = open::that_detached(&url) {
                    tracing::warn!(url, %why, "failed to open url");
                }
            }

            Message::ToggleContextPage(page) => {
                if self.context_page == page {
                    self.core.window.show_context = !self.core.window.show_context;
                } else {
                    self.context_page = page;
                    self.core.window.show_context = true;
                }
            }

            // libcosmic tracks the wide-window and narrow-window states
            // separately, so that collapsing the sidebar to read a month on a
            // small window does not lose the preference when the window grows
            // again. Which one this press moves is therefore the same question
            // the stock header toggle asks.
            Message::ToggleSidebar => {
                if self.core.is_condensed() {
                    self.core.nav_bar_toggle_condensed();
                } else {
                    // One button, three widths: each press takes the sidebar
                    // a step narrower, and from nothing back to full.
                    match self.sidebar.mode() {
                        SidebarMode::Expanded => self.sidebar_rail = true,
                        SidebarMode::Rail => {
                            self.sidebar_rail = false;
                            self.core.nav_bar_set_toggled(false);
                        }
                        SidebarMode::Hidden => self.core.nav_bar_set_toggled(true),
                    }
                }
                self.sync_sidebar();
            }

            Message::SidebarClosed => self.sidebar.closed(),

            Message::UpdateConfig(config) => {
                let reload_needed = config.first_day_of_week != self.config.first_day_of_week
                    || config.hidden_calendars != self.config.hidden_calendars
                    || config.view != self.config.view;
                self.config = config;
                if reload_needed {
                    self.reload();
                }
            }

            Message::CloseToast(id) => self.toasts.remove(id),

            Message::Key(modifiers, key, physical) => {
                use cosmic::widget::menu::action::MenuAction as _;

                for (bind, action) in &self.key_binds {
                    if bind.matches(modifiers, &key, Some(&physical)) {
                        return self.update(action.message());
                    }
                }
            }

            Message::ViewSelected(entity) => {
                self.views.activate(entity);
                if let Some(kind) = self.views.data::<ViewKind>(entity).copied() {
                    self.set_view(kind);
                    return Self::request_scroll();
                }
            }

            Message::SetView(kind) => {
                if let Some(entity) = self.entity_for(kind) {
                    self.views.activate(entity);
                }
                self.set_view(kind);
            }

            Message::Today => {
                self.today = self.clock_now().date();
                self.anchor = self.today;
                self.sync_mini();
                self.reload();
                return Self::request_scroll();
            }

            Message::Previous => {
                self.anchor = self.step(-1);
                self.sync_mini();
                self.reload();
                return Self::request_scroll();
            }

            Message::Next => {
                self.anchor = self.step(1);
                self.sync_mini();
                self.reload();
                return Self::request_scroll();
            }

            Message::OpenDay(date) => {
                self.anchor = date;
                if let Some(entity) = self.entity_for(ViewKind::Day) {
                    self.views.activate(entity);
                }
                self.set_view(ViewKind::Day);
                return Self::request_scroll();
            }

            Message::MiniDateSelected(date) => {
                if let Some(date) = crate::ui::editor::from_jiff(date) {
                    self.anchor = date;
                    self.mini
                        .set_selected_visible(crate::ui::editor::to_jiff(date));
                    self.reload();
                }
            }

            Message::MiniPrevMonth => self.mini.show_prev_month(),
            Message::MiniNextMonth => self.mini.show_next_month(),

            Message::ToggleCalendar(id) => {
                self.config.toggle_calendar(&id);
                self.persist_config();
                self.reload();
            }

            Message::NewTaskChanged(text) => self.new_task = text,

            Message::TaskEdit(calendar_id, uid) => {
                let Some(todo) = self.store.as_ref().and_then(|s| s.todo(&calendar_id, &uid))
                else {
                    return Task::none();
                };
                let local = self.local_timezone();
                self.task_editor = Some(crate::ui::task_editor::TaskEditor::existing(
                    todo,
                    local,
                    &self.config,
                ));
                self.context_page = ContextPage::TaskEditor;
                self.core.window.show_context = true;
            }

            Message::TaskEditorSummary(v) => self.with_task_editor(|e| e.summary = v),
            Message::TaskEditorDescription(v) => self.with_task_editor(|e| e.description = v),
            Message::TaskEditorDueTime(v) => self.with_task_editor(|e| e.due_time = v),
            Message::TaskEditorToggleDue => self.with_task_editor(|e| e.has_due = !e.has_due),
            Message::TaskEditorPickDate => self.with_task_editor(|e| e.picking = !e.picking),
            Message::TaskEditorPickerPrev => {
                self.with_task_editor(|e| e.picker.show_prev_month());
            }
            Message::TaskEditorPickerNext => {
                self.with_task_editor(|e| e.picker.show_next_month());
            }
            Message::TaskEditorDatePicked(date) => {
                if let Some(picked) = crate::ui::editor::from_jiff(date) {
                    self.with_task_editor(|e| {
                        e.due_date = picked;
                        e.has_due = true;
                        e.picking = false;
                        e.picker.set_selected_visible(date);
                    });
                }
            }
            Message::TaskEditorPriority(index) => {
                if let Some(p) = crate::ui::task_editor::PRIORITY_CHOICES.get(index).copied() {
                    self.with_task_editor(|e| e.priority = p);
                }
            }
            Message::TaskEditorStatus(index) => {
                self.with_task_editor(|e| {
                    if let Some(status) = [
                        crate::model::TodoStatus::NeedsAction,
                        crate::model::TodoStatus::InProcess,
                        crate::model::TodoStatus::Completed,
                        crate::model::TodoStatus::Cancelled,
                    ]
                    .get(index)
                    .copied()
                    {
                        e.status = status;
                    }
                });
            }
            Message::TaskEditorCalendar(index) => {
                let writable: Vec<String> = self
                    .store
                    .as_ref()
                    .map(|s| {
                        s.calendars()
                            .iter()
                            .filter(|c| !c.read_only)
                            .map(|c| c.id.clone())
                            .collect()
                    })
                    .unwrap_or_default();
                if let Some(id) = writable.get(index).cloned() {
                    self.with_task_editor(|e| e.calendar_id = id);
                }
            }

            Message::TaskEditorSave => return self.save_task_editor(),
            Message::TaskEditorDelete => return self.delete_task_editor(),

            Message::NewTaskSubmit => {
                let summary = self.new_task.trim().to_owned();
                if summary.is_empty() {
                    return Task::none();
                }
                let Some(calendar_id) = self
                    .store
                    .as_ref()
                    .and_then(Store::default_calendar)
                    .map(|c| c.id.clone())
                else {
                    return self.toast_error(&fl!("no-writable-calendar"));
                };
                let Some(store) = self.store.as_mut() else {
                    return Task::none();
                };

                let mut todo = crate::model::Todo::draft(&calendar_id);
                todo.summary = summary;
                match store.save_todo(&todo) {
                    Ok(()) => {
                        // A fresh draft has no previous revision to merge against.
                        let queued = queue_writeback_file(&todo.calendar_id, &todo.file_name, None);
                        self.journal_finish(vec![crate::undo::Entry {
                            calendar_id: todo.calendar_id.clone(),
                            file_name: todo.file_name.clone(),
                            before: None,
                            after: None,
                        }]);
                        self.new_task.clear();
                        self.reload_tasks();
                        return self.report_unqueued(queued);
                    }
                    Err(why) => return self.toast_error(&why.to_string()),
                }
            }

            Message::ToggleShowCompleted => {
                self.show_done_tasks = !self.show_done_tasks;
            }

            Message::TaskToggleDone(calendar_id, uid, done) => {
                let Some(store) = self.store.as_mut() else {
                    return Task::none();
                };
                let Some(mut todo) = store.todo(&calendar_id, &uid) else {
                    return Task::none();
                };
                todo.set_done(done);
                let base = writeback_base(store, &todo.calendar_id, &todo.file_name);
                match store.save_todo(&todo) {
                    Ok(()) => {
                        let queued = queue_writeback_file(
                            &todo.calendar_id,
                            &todo.file_name,
                            base.as_deref(),
                        );
                        self.journal_finish(vec![crate::undo::Entry {
                            calendar_id: todo.calendar_id.clone(),
                            file_name: todo.file_name.clone(),
                            before: base,
                            after: None,
                        }]);
                        self.reload_tasks();
                        return self.report_unqueued(queued);
                    }
                    Err(why) => return self.toast_error(&why.to_string()),
                }
            }

            Message::AccountAddStart => self.account_form = Some(AccountForm::default()),
            Message::AccountAddCancel => self.account_form = None,
            Message::AccountNameChanged(v) => self.with_account_form(|f| f.display_name = v),
            Message::AccountUrlChanged(v) => self.with_account_form(|f| f.url = v),
            Message::AccountUsernameChanged(v) => self.with_account_form(|f| f.username = v),
            Message::AccountPasswordChanged(v) => self.with_account_form(|f| f.password = v),

            Message::AccountAddConfirm => return self.confirm_account(),

            Message::SubAddStart => self.sub_form = Some(SubscriptionForm::default()),
            Message::SubAddCancel => self.sub_form = None,
            Message::SubNameChanged(v) => {
                if let Some(form) = self.sub_form.as_mut() {
                    form.name = v;
                }
            }
            Message::SubUrlChanged(v) => {
                if let Some(form) = self.sub_form.as_mut() {
                    form.url = v;
                }
            }
            Message::SubAddConfirm => return self.confirm_subscription(),
            Message::SubRemoveRequest(id) => self.pending_feed_removal = Some(id),
            Message::SubRemoveCancel => self.pending_feed_removal = None,
            Message::SubRemoveConfirm => return self.remove_subscription(),
            Message::FeedsRefreshed(changed) => {
                self.refreshing_feeds = false;
                if changed {
                    if let Some(store) = self.store.as_mut() {
                        let _ = store.refresh();
                    }
                    self.reload();
                }
            }

            Message::ConflictTakeRemote(index) => return self.resolve_conflict(index, false),
            Message::ConflictKeepLocal(index) => return self.resolve_conflict(index, true),
            Message::ConflictChooseSide(row, unit, side) => {
                if let Some(disputes) = self
                    .conflicts
                    .get_mut(row)
                    .and_then(|r| r.disputes.as_mut())
                    && let Some(overlap) = disputes.units.get(unit)
                {
                    disputes.choices.insert(overlap.unit.clone(), side);
                }
            }
            Message::ConflictApplyMerge(index) => return self.apply_conflict_merge(index),

            Message::InvitationDelivered(delivery) => {
                use cosmic_pim_caldav::itip::Method;
                let Some(parsed) = cosmic_pim_caldav::itip::parse(&delivery.ics) else {
                    tracing::debug!("a delivered payload was not an iTIP message; ignoring");
                    return Task::none();
                };
                match parsed.method {
                    Method::Request => {
                        // Only the organizer may invite: a REQUEST without one,
                        // or one this account organizes itself, is not an
                        // invitation to answer. Refused here rather than after
                        // the user has answered it.
                        self.reopen_accounts();
                        if let Some(me) = self.account_address(&delivery.account_id)
                            && !parsed.is_from_organizer(&me, None)
                        {
                            return self.toast_error(&fl!("invitation-not-from-organizer"));
                        }
                        let event = crate::store::vdir::parse_ics(&delivery.ics, "", "")
                            .into_iter()
                            .next();
                        enqueue_invitation(
                            &mut self.invitations,
                            PendingInvitation {
                                account_id: delivery.account_id,
                                ics: delivery.ics,
                                parsed,
                                event,
                            },
                        );
                    }
                    // A CANCEL or REPLY carries no decision for this user;
                    // the gates inside `itip::apply` decide what it may do.
                    Method::Cancel | Method::Reply => return self.apply_itip_quietly(&delivery),
                    Method::Publish | Method::Other => {}
                }
            }
            Message::InvitationAnswer(answer) => return self.answer_invitation(answer),
            Message::InvitationDismiss => {
                if !self.invitations.is_empty() {
                    self.invitations.remove(0);
                }
            }
            Message::InvitationReplySent(queued) => {
                let text = if queued {
                    fl!("invitation-reply-queued")
                } else {
                    fl!("invitation-reply-by-mail")
                };
                return self.toast_info(&text);
            }

            Message::CalendarNameInput(id, name) => {
                self.calendar_name_drafts.insert(id, name);
            }
            Message::CalendarNameCommit(id) => {
                let Some(name) = self.calendar_name_drafts.remove(&id) else {
                    return Task::none();
                };
                let name = name.trim().to_owned();
                let Some(color) = self
                    .store
                    .as_ref()
                    .and_then(|s| s.calendar(&id))
                    .map(|c| c.color)
                else {
                    return Task::none();
                };
                if name.is_empty() {
                    return Task::none();
                }
                return self.update_local_calendar(&id, &name, color);
            }
            Message::CalendarRecolor(id, index) => {
                let (Some(name), Some(color)) = (
                    self.store
                        .as_ref()
                        .and_then(|s| s.calendar(&id))
                        .map(|c| c.name.clone()),
                    PALETTE.get(index).copied(),
                ) else {
                    return Task::none();
                };
                return self.update_local_calendar(&id, &name, color);
            }
            Message::CalendarDeleteRequest(id) => self.pending_calendar_removal = Some(id),
            Message::CalendarDeleteCancel => self.pending_calendar_removal = None,
            Message::CalendarDeleteConfirm => return self.delete_local_calendar(),

            Message::AccountRemoveRequest(id) => self.pending_account_removal = Some(id),
            Message::AccountRemoveCancel => self.pending_account_removal = None,
            Message::AccountRemoveConfirm => {
                let Some(id) = self.pending_account_removal.take() else {
                    return Task::none();
                };
                self.reopen_accounts();
                if let Some(accounts) = self.accounts.as_mut()
                    && let Err(why) = accounts.remove(&id)
                {
                    return self.toast_error(&format!("{}: {why}", fl!("accounts")));
                }
            }

            Message::SyncNow => return self.sync_now(),

            Message::SyncFinished(lines, changed) => {
                self.syncing = false;
                // The pass may have bound new collections to accounts.
                self.reopen_accounts();
                self.sync_status = Some(lines.join("\n"));
                if changed {
                    // Sync wrote `.ics` files directly; the index has to catch up.
                    if let Some(store) = self.store.as_mut()
                        && let Err(why) = store.refresh()
                    {
                        tracing::warn!(%why, "refresh after a sync failed");
                    }
                    // The same pass syncs the address books; birthdays follow.
                    if self.config.show_birthdays {
                        self.contact_cards = load_contact_cards();
                    }
                    self.reload();
                }
                // A pull can have recorded new conflicts, or resolved old ones.
                self.reload_conflicts();
            }

            Message::NewCalendarStart => self.new_calendar_name = Some(String::new()),
            Message::NewCalendarNameChanged(name) => self.new_calendar_name = Some(name),
            Message::NewCalendarCancel => self.new_calendar_name = None,

            Message::NewCalendarConfirm => {
                if let Some(name) = self.new_calendar_name.take() {
                    let name = name.trim().to_owned();
                    if !name.is_empty() {
                        // Cycle the palette so consecutive calendars look distinct.
                        let index = self.calendars().len() % PALETTE.len();
                        let color = PALETTE[index];
                        if let Some(store) = self.store.as_mut() {
                            match store.create_calendar(&name, color) {
                                Ok(_) => self.reload(),
                                Err(why) => return self.toast_error(&why.to_string()),
                            }
                        }
                    }
                }
            }

            Message::NewEvent => {
                let start = self.default_new_event_time();
                return self.open_editor_new(start);
            }

            Message::NewEventOn(date) => {
                return self.open_editor_new(
                    date.and_hms_opt(9, 0, 0)
                        .unwrap_or(date.and_time(NaiveTime::MIN)),
                );
            }

            Message::OpenEvent(calendar_id, uid, instant) => {
                // A birthday chip is not an event file; there is nothing to
                // open here — Circle is its editor.
                if calendar_id == BIRTHDAYS_CALENDAR_ID {
                    return Task::none();
                }
                let Some(store) = self.store.as_ref() else {
                    return Task::none();
                };
                // Instance-aware: if this occurrence has been overridden — an
                // 11:00 moved to 14:00 by another client, say — the override
                // component is what the user sees and must be what they edit.
                match store.event_instance(&calendar_id, &uid, instant) {
                    Ok(Some(event)) => {
                        let local = store.local_timezone();
                        // A generated instance of a series opens on its own
                        // dates, carrying its identity for the scope prompt.
                        // An override or a one-off opens as itself.
                        self.editor = Some(match instant {
                            Some(instant)
                                if event.rrule.is_some() && event.recurrence_id.is_none() =>
                            {
                                Editor::from_series_occurrence(&event, instant, local)
                            }
                            _ => Editor::from_event(&event, local),
                        });
                        self.context_page = ContextPage::Editor;
                        self.core.window.show_context = true;
                    }
                    Ok(None) => tracing::warn!(uid, "event vanished before it could be opened"),
                    Err(why) => return self.toast_error(&why.to_string()),
                }
            }

            Message::EditorTzToggle(field) => self.with_editor(|e| {
                e.tz_picking = if e.tz_picking == Some(field) {
                    None
                } else {
                    Some(field)
                };
                e.tz_query.clear();
            }),
            Message::EditorTzQuery(v) => self.with_editor(|e| e.tz_query = v),
            Message::EditorTzChosen(name) => self.with_editor(|e| {
                if let (Some(field), Ok(tz)) = (e.tz_picking, name.parse::<chrono_tz::Tz>()) {
                    match field {
                        TzField::Start => e.start_tz = Some(tz),
                        TzField::End => e.end_tz = Some(tz),
                    }
                }
                e.tz_picking = None;
                e.tz_query.clear();
            }),
            Message::EditorTzClear => self.with_editor(|e| {
                match e.tz_picking {
                    Some(TzField::Start) => e.start_tz = None,
                    Some(TzField::End) => e.end_tz = None,
                    None => {}
                }
                e.tz_picking = None;
                e.tz_query.clear();
            }),
            Message::EditorAttendeeDraft(v) => self.with_editor(|e| e.attendee_draft = v),
            Message::EditorAttendeeAdd => self.with_editor(|e| {
                let email = crate::model::normalise_address(&e.attendee_draft);
                // An address needs an @ to be worth sending to a server; the
                // rest of the validation is the server's job, not ours.
                if email.contains('@') && !e.attendees.iter().any(|a| a.email == email) {
                    e.attendees.push(crate::model::Attendee::new(&email, None));
                    // The answer on screen no longer covers everyone.
                    e.availability = None;
                }
                e.attendee_draft.clear();
            }),
            Message::EditorAttendeeRemove(index) => self.with_editor(|e| {
                if index < e.attendees.len() {
                    e.attendees.remove(index);
                    e.availability = None;
                }
            }),
            Message::EditorCheckAvailability => return self.check_availability(),
            Message::AvailabilityAnswer(view) => self.with_editor(|e| {
                e.checking_availability = false;
                e.availability = Some(view);
            }),

            Message::EditorSummary(v) => self.with_editor(|e| e.summary = v),
            Message::EditorLocation(v) => self.with_editor(|e| e.location = v),
            Message::EditorDescription(v) => self.with_editor(|e| e.description = v),
            Message::EditorStartTime(v) => self.with_editor(|e| e.start_time = v),
            Message::EditorEndTime(v) => self.with_editor(|e| e.end_time = v),
            Message::EditorInterval(v) => self.with_editor(|e| e.interval = v),
            Message::EditorCount(v) => self.with_editor(|e| e.count = v),
            Message::EditorAllDay(v) => self.with_editor(|e| e.all_day = v),

            Message::EditorCalendar(index) => {
                // Indexes into the writable subset — the same filter the
                // editor's dropdown was built from.
                let id = self
                    .calendars()
                    .iter()
                    .filter(|c| !c.read_only)
                    .nth(index)
                    .map(|c| c.id.clone());
                if let Some(id) = id {
                    self.with_editor(|e| e.calendar_id = id);
                }
            }

            Message::EditorAlarmAdd(index) => self.with_editor(|e| e.add_alarm(index)),
            Message::EditorAlarmRemove(index) => self.with_editor(|e| e.remove_alarm(index)),
            Message::EditorFreq(index) => {
                let freq = crate::model::Freq::ALL.get(index).copied();
                if let Some(freq) = freq {
                    self.with_editor(|e| e.freq = freq);
                }
            }

            Message::EditorRepeatEnd(index) => {
                self.with_editor(|e| {
                    e.repeat_end = match index {
                        1 => crate::model::RepeatEnd::After(e.count.parse().unwrap_or(10)),
                        2 => crate::model::RepeatEnd::On(e.until),
                        _ => crate::model::RepeatEnd::Never,
                    };
                });
            }

            Message::EditorPickDate(field) => {
                self.with_editor(|e| {
                    // Clicking the same field again closes the picker.
                    e.picking = if e.picking == Some(field) {
                        None
                    } else {
                        let current = match field {
                            DateField::Start => e.start_date,
                            DateField::End => e.end_date,
                            DateField::Until => e.until,
                        };
                        e.picker = widget::calendar::CalendarModel::new(
                            crate::ui::editor::to_jiff(current),
                            crate::ui::editor::to_jiff(current),
                        );
                        Some(field)
                    };
                });
            }

            Message::EditorDatePicked(date) => {
                let Some(date) = crate::ui::editor::from_jiff(date) else {
                    return Task::none();
                };
                self.with_editor(|e| {
                    match e.picking {
                        Some(DateField::Start) => {
                            // Keep the event's length when its start moves.
                            let span = (e.end_date - e.start_date).num_days();
                            e.start_date = date;
                            e.end_date = date + Duration::days(span.max(0));
                        }
                        Some(DateField::End) => e.end_date = date,
                        Some(DateField::Until) => {
                            e.until = date;
                            e.repeat_end = crate::model::RepeatEnd::On(date);
                        }
                        None => {}
                    }
                    e.picker
                        .set_selected_visible(crate::ui::editor::to_jiff(date));
                    e.picking = None;
                });
            }

            Message::EditorPickerPrev => self.with_editor(|e| e.picker.show_prev_month()),
            Message::EditorPickerNext => self.with_editor(|e| e.picker.show_next_month()),

            Message::EditorSave => return self.save_editor(),

            Message::ScopeChosen(scope) => {
                let Some(prompt) = self.scope_prompt.take() else {
                    return Task::none();
                };
                return match prompt {
                    ScopePrompt::Save => self.save_with_scope(scope),
                    ScopePrompt::Delete => self.delete_with_scope(scope),
                };
            }

            Message::ScopeCancelled => {
                self.scope_prompt = None;
            }

            Message::EditorCancel => {
                self.editor = None;
                self.core.window.show_context = false;
            }

            Message::EditorDelete => return self.delete_editor(),

            Message::SetFirstDayOfWeek(index) => {
                self.config.first_day_of_week = u8::try_from(index).unwrap_or(0).min(6);
                self.persist_config();
                self.reload();
            }

            Message::ToggleWeekNumbers(v) => {
                self.config.show_week_numbers = v;
                self.persist_config();
            }

            Message::ToggleShowBirthdays(v) => {
                self.config.show_birthdays = v;
                self.persist_config();
                if v && self.contact_cards.is_empty() {
                    self.contact_cards = load_contact_cards();
                }
                self.reload();
            }

            Message::Toggle24Hour(v) => {
                self.config.time_24h = v;
                self.persist_config();
            }

            Message::SetDefaultReminder(index) => {
                if let Some(minutes) = REMINDER_CHOICES.get(index).copied() {
                    self.config.default_reminder_minutes = minutes;
                    self.persist_config();
                }
            }

            Message::SetSnapMinutes(index) => {
                if let Some(minutes) = SNAP_CHOICES.get(index).copied() {
                    self.config.snap_minutes = minutes;
                    self.persist_config();
                }
            }

            Message::SetCalendarReminder(calendar_id, index) => {
                if let Some(minutes) = CALENDAR_REMINDER_CHOICES.get(index).copied() {
                    let duration = self
                        .config
                        .defaults_for(&calendar_id)
                        .map_or(0, |d| d.duration_minutes);
                    self.config
                        .set_defaults_for(&calendar_id, minutes, duration);
                    self.persist_config();
                }
            }
            Message::SetCalendarDuration(calendar_id, index) => {
                if let Some(minutes) = CALENDAR_DURATION_CHOICES.get(index).copied() {
                    let reminder = self
                        .config
                        .defaults_for(&calendar_id)
                        .map_or(0, |d| d.reminder_minutes);
                    self.config
                        .set_defaults_for(&calendar_id, reminder, minutes);
                    self.persist_config();
                }
            }

            Message::SetSecondaryTz(text) => {
                // The field accepts any prefix while typing; only an empty
                // value (off) or a real IANA name reaches the config.
                let cleared = text.trim().is_empty();
                let valid = text.trim().parse::<chrono_tz::Tz>().is_ok();
                self.secondary_tz_input = text;
                if cleared {
                    self.config.secondary_timezone = String::new();
                    self.persist_config();
                } else if valid {
                    self.config.secondary_timezone = self.secondary_tz_input.trim().to_owned();
                    self.persist_config();
                }
            }

            Message::Sleep(going_down) => {
                self.now = self.clock_now();
                if going_down {
                    self.sleep.going_down(self.now);
                    return Task::none();
                }
                let slept_at = self.sleep.woke();
                self.today = self.now.date();
                // The vdir may have moved on underneath a suspended machine.
                if let Some(store) = self.store.as_mut() {
                    let _ = store.refresh();
                }
                self.reload();
                return Task::batch([
                    self.report_missed_reminders(slept_at),
                    self.fire_due_reminders(),
                ]);
            }

            Message::Undo => return self.apply_history(true),
            Message::Redo => return self.apply_history(false),

            Message::QuickAddOpen => {
                self.quick_add = Some(String::new());
                return widget::text_input::focus(quick_add_input_id());
            }
            Message::QuickAddInput(v) => self.quick_add = Some(v),
            Message::QuickAddCancel => self.quick_add = None,
            Message::QuickAddConfirm => return self.confirm_quick_add(),

            Message::SearchOpen => {
                self.search_query = Some(String::new());
                self.search_results.clear();
                return widget::text_input::focus(crate::ui::search::input_id());
            }
            Message::SearchInput(query) => {
                self.run_search(&query);
                self.search_query = Some(query);
            }
            Message::SearchClose => {
                self.search_query = None;
                self.search_results.clear();
            }
            Message::SearchHit(date, calendar_id, uid, rid) => {
                self.search_query = None;
                self.search_results.clear();
                self.anchor = date;
                self.sync_mini();
                self.reload();
                return <Self as cosmic::Application>::update(
                    self,
                    Message::OpenEvent(calendar_id, uid, rid),
                );
            }

            Message::GridHover(date, y) => {
                let max_y = crate::ui::HOUR_HEIGHT * 24.0 - 1.0;
                let minutes =
                    f64::from(y.clamp(0.0, max_y)) / f64::from(crate::ui::HOUR_HEIGHT) * 60.0;
                let cursor = date.and_time(NaiveTime::MIN) + Duration::minutes(minutes as i64);
                self.grid_cursor = Some(cursor);
                // A tiny wobble between press and release must stay a click.
                match self.grid_drag.as_mut() {
                    Some(GridDrag::Block {
                        pressed_at, moved, ..
                    }) => {
                        *moved |= (cursor - *pressed_at).num_minutes().abs() >= 5;
                    }
                    Some(GridDrag::Create {
                        anchor,
                        current,
                        moved,
                    }) => {
                        *current = cursor;
                        *moved |= (cursor - *anchor).num_minutes().abs() >= 5;
                    }
                    None => {}
                }
            }
            Message::GridBlockPress(block) => {
                let pressed_at = self.grid_cursor.unwrap_or(block.start);
                self.grid_drag = Some(GridDrag::Block {
                    block,
                    pressed_at,
                    moved: false,
                });
            }
            Message::GridEmptyPress(at) => {
                let snap = self.config.snap();
                let anchor = self.grid_cursor.map_or(at, |cursor| snap_to(cursor, snap));
                self.grid_drag = Some(GridDrag::Create {
                    anchor,
                    current: anchor,
                    moved: false,
                });
            }
            Message::GridRelease => return self.finish_grid_drag(),

            Message::ScrollTimeGrid => return self.scroll_time_grid(),

            Message::Tick => {
                // A timezone change (travel, automatic zone updates) moves
                // every occurrence's wall clock; follow it rather than fire
                // reminders offset by the difference until a restart.
                let rezoned = match self.store.as_mut().map(crate::clock::follow_timezone) {
                    Some(Ok(rezoned)) => rezoned,
                    Some(Err(why)) => {
                        tracing::warn!(%why, "could not follow the timezone change");
                        false
                    }
                    None => false,
                };
                self.now = self.clock_now();
                let today = self.now.date();
                if rezoned || today != self.today {
                    // Past midnight: today moved, so the highlight and any
                    // relative view must follow it.
                    self.today = today;
                    self.reload();
                }
                // Feed refreshes ride the same tick, paced to one check every
                // five minutes; each feed's own interval gates the fetch. The
                // daemon, when it runs, refreshes them on its own sync tick.
                let feeds_due = !self.daemon_running
                    && self
                        .last_feed_check
                        .is_none_or(|at| at.elapsed() >= std::time::Duration::from_secs(300));
                if feeds_due {
                    return Task::batch([self.refresh_feeds(false), self.fire_due_reminders()]);
                }
                return self.fire_due_reminders();
            }

            Message::ReminderSnoozed(id) => {
                let until = self.clock_now() + crate::reminders::SNOOZE;
                // Recorded in the shared memory, so whichever process owns
                // reminders when the snooze is up shows it again.
                if let Err(why) = self.reminders.snooze(id, until) {
                    tracing::warn!(%why, "could not record the snooze");
                }
            }

            Message::ReminderOwnership(delegated) => {
                let taking_over = self.daemon_running && !delegated;
                self.daemon_running = delegated;
                if delegated {
                    tracing::info!("the reminder daemon is running; leaving reminders to it");
                } else if taking_over {
                    // The daemon went away (or was never there): from now on
                    // reminders are ours, starting with any already due.
                    tracing::info!("no reminder daemon; firing reminders here");
                    return self.fire_due_reminders();
                }
            }

            Message::ImportRequested => {
                return cosmic::task::future(async {
                    use cosmic::dialog::file_chooser::{self, FileFilter};

                    let dialog = file_chooser::open::Dialog::new()
                        .title(fl!("import"))
                        .filter(FileFilter::new("iCalendar").glob("*.ics"));

                    match dialog.open_file().await {
                        Ok(response) => match response.url().to_file_path() {
                            Ok(path) => Message::ImportPath(path),
                            Err(()) => Message::DialogFailed(fl!("error-remote-file")),
                        },
                        Err(file_chooser::Error::Cancelled) => Message::DialogCancelled,
                        Err(why) => Message::DialogFailed(why.to_string()),
                    }
                });
            }

            // Which calendar to export is the user's to say; the first one on
            // disk could be a subscribed feed.
            Message::ExportRequested => {
                if self.calendars().is_empty() {
                    return self.toast_error(&fl!("no-calendars"));
                }
                self.export_choice = Some(default_export(self.calendars(), &self.config));
            }
            Message::ExportCalendarChosen(index) => self.export_choice = Some(index),
            Message::ExportCancel => self.export_choice = None,
            Message::ExportConfirm => {
                let Some(calendar) = self
                    .export_choice
                    .take()
                    .and_then(|index| self.calendars().get(index))
                    .map(|c| (c.id.clone(), c.name.clone()))
                else {
                    return Task::none();
                };

                let (id, name) = calendar;
                return cosmic::task::future(async move {
                    use cosmic::dialog::file_chooser::{self, FileFilter};

                    let dialog = file_chooser::save::Dialog::new()
                        .title(fl!("export"))
                        .file_name(format!("{name}.ics"))
                        .filter(FileFilter::new("iCalendar").glob("*.ics"));

                    match dialog.save_file().await {
                        Ok(response) => match response.url().and_then(|u| u.to_file_path().ok()) {
                            Some(path) => Message::ExportTo(path, id),
                            None => Message::DialogFailed(fl!("error-remote-file")),
                        },
                        Err(file_chooser::Error::Cancelled) => Message::DialogCancelled,
                        Err(why) => Message::DialogFailed(why.to_string()),
                    }
                });
            }

            Message::ImportPath(path) => return self.import(&path),

            Message::ExportTo(path, calendar_id) => {
                let Some(store) = self.store.as_ref() else {
                    return Task::none();
                };
                match store
                    .export_calendar(&calendar_id)
                    .and_then(|text| std::fs::write(&path, text).map_err(Into::into))
                {
                    Ok(()) => {
                        return self.toast(&fl!("export-done", path = file_label(&path)));
                    }
                    Err(why) => return self.toast_error(&why.to_string()),
                }
            }

            Message::DialogCancelled | Message::Ignore => {}

            Message::DialogFailed(why) => return self.toast_error(&why),

            Message::FilesChanged => {
                if let Some(store) = self.store.as_mut() {
                    match store.refresh() {
                        Ok(true) => {
                            tracing::debug!("picked up an external change");
                            self.reload();
                        }
                        Ok(false) => {}
                        Err(why) => tracing::warn!(%why, "refresh after a file change failed"),
                    }
                }
            }
        }

        Task::none()
    }
}

impl AppModel {
    fn view(&self) -> ViewKind {
        self.views
            .active_data::<ViewKind>()
            .copied()
            .unwrap_or(self.config.view)
    }

    fn calendars(&self) -> &[CalendarMeta] {
        self.store.as_ref().map_or(&[], Store::calendars)
    }

    fn entity_for(&self, kind: ViewKind) -> Option<segmented_button::Entity> {
        self.views
            .iter()
            .find(|entity| self.views.data::<ViewKind>(*entity) == Some(&kind))
    }

    fn set_view(&mut self, kind: ViewKind) {
        if self.config.view != kind {
            self.config.view = kind;
            self.persist_config();
        }
        self.reload();
    }

    /// Steps the anchor one page in `direction`.
    fn step(&self, direction: i64) -> NaiveDate {
        match self.view() {
            ViewKind::Month => add_months(self.anchor, direction),
            ViewKind::Year => add_months(self.anchor, direction * 12),
            other => self.anchor + Duration::days(other.page_days() * direction),
        }
    }

    fn sync_mini(&mut self) {
        self.mini
            .set_selected_visible(crate::ui::editor::to_jiff(self.anchor));
    }

    /// Recomputes the occurrences for the visible range.
    ///
    /// Done synchronously: the SQLite index makes a month's worth of events a
    /// sub-millisecond query, and keeping the store off the async executor
    /// avoids sharing a `rusqlite::Connection` across threads.
    /// Reloads the task list from disk.
    ///
    /// Separate from [`Self::reload`] because tasks are not indexed and are
    /// only needed by one view; loading them on every grid redraw would read
    /// every `.ics` in every collection for nothing.
    /// Wall-clock now in the zone occurrences are expressed in.
    fn clock_now(&self) -> NaiveDateTime {
        crate::clock::now_in(self.local_timezone())
    }

    /// The store's timezone, or UTC before the store has opened.
    fn local_timezone(&self) -> chrono_tz::Tz {
        self.store.as_ref().map_or(
            chrono_tz::UTC,
            cosmic_pim_core::store::Store::local_timezone,
        )
    }

    fn with_task_editor(&mut self, f: impl FnOnce(&mut crate::ui::task_editor::TaskEditor)) {
        if let Some(editor) = self.task_editor.as_mut() {
            f(editor);
            editor.error = None;
        }
    }

    fn save_task_editor(&mut self) -> Task<cosmic::Action<Message>> {
        let local = self.local_timezone();
        let Some(editor) = self.task_editor.as_ref() else {
            return Task::none();
        };

        let todo = match editor.to_todo(local) {
            Ok(todo) => todo,
            Err(why) => {
                self.with_task_editor(|e| e.error = Some(why));
                return Task::none();
            }
        };

        // Moving a task between calendars is a delete plus a write, exactly as
        // it is for an event; without the delete the old copy is orphaned.
        let moved_from = editor
            .original
            .as_ref()
            .filter(|original| original.calendar_id != todo.calendar_id)
            .cloned();

        let Some(store) = self.store.as_mut() else {
            return Task::none();
        };

        // Snapshotted before the delete below, or undo could not restore it.
        let moved_before = moved_from
            .as_ref()
            .and_then(|original| writeback_base(store, &original.calendar_id, &original.file_name));
        let mut queued = Ok(());
        if let Some(original) = &moved_from {
            if let Err(why) = store.delete_todo(&original.calendar_id, &original.uid) {
                return self.toast_error(&why.to_string());
            }
            queued = queue_writeback_removal(
                store,
                &original.calendar_id,
                &original.file_name,
                moved_before.as_deref(),
            );
        }

        let base = writeback_base(store, &todo.calendar_id, &todo.file_name);
        let moved_entry = moved_from.as_ref().map(|original| crate::undo::Entry {
            calendar_id: original.calendar_id.clone(),
            file_name: original.file_name.clone(),
            before: moved_before,
            after: None,
        });
        match store.save_todo(&todo) {
            Ok(()) => {
                let queued = queued.and(queue_writeback_file(
                    &todo.calendar_id,
                    &todo.file_name,
                    base.as_deref(),
                ));
                let mut journal = vec![crate::undo::Entry {
                    calendar_id: todo.calendar_id.clone(),
                    file_name: todo.file_name.clone(),
                    before: base,
                    after: None,
                }];
                journal.extend(moved_entry);
                self.journal_finish(journal);
                self.task_editor = None;
                self.core.window.show_context = false;
                self.reload_tasks();
                self.report_unqueued(queued)
            }
            Err(why) => self.toast_error(&why.to_string()),
        }
    }

    fn delete_task_editor(&mut self) -> Task<cosmic::Action<Message>> {
        let Some(original) = self
            .task_editor
            .as_ref()
            .and_then(|e| e.original.as_ref())
            .cloned()
        else {
            return Task::none();
        };
        let Some(store) = self.store.as_mut() else {
            return Task::none();
        };

        let before = writeback_base(store, &original.calendar_id, &original.file_name);
        match store.delete_todo(&original.calendar_id, &original.uid) {
            Ok(()) => {
                let queued = queue_writeback_removal(
                    store,
                    &original.calendar_id,
                    &original.file_name,
                    before.as_deref(),
                );
                self.journal_finish(vec![crate::undo::Entry {
                    calendar_id: original.calendar_id.clone(),
                    file_name: original.file_name.clone(),
                    before,
                    after: None,
                }]);
                self.task_editor = None;
                self.core.window.show_context = false;
                self.reload_tasks();
                self.report_unqueued(queued)
            }
            Err(why) => self.toast_error(&why.to_string()),
        }
    }

    fn reload_tasks(&mut self) {
        let hidden = self.config.hidden_set();
        if let Some(store) = self.store.as_ref() {
            self.todos = store.todos(&hidden);
        }
    }

    fn reload(&mut self) {
        if self.view() == ViewKind::Tasks {
            self.reload_tasks();
        }

        let Some(store) = self.store.as_ref() else {
            return;
        };

        let (from, to) = crate::ui::visible_range(self.view(), self.anchor, &self.config);
        match store.occurrences_by_day(from, to, &self.config.hidden_set()) {
            Ok(days) => self.days = days,
            Err(why) => {
                tracing::error!(%why, "failed to load occurrences");
                self.days.clear();
            }
        }
        if self.config.show_birthdays {
            self.inject_birthdays(from, to);
        }
    }

    /// Adds birthday entries from the address book to the visible days.
    ///
    /// Synthesised on the fly from `BDAY` — never written anywhere — so they
    /// track every card edit with nothing to keep in step.
    fn inject_birthdays(&mut self, from: NaiveDate, to: NaiveDate) {
        if self.contact_cards.is_empty() {
            return;
        }
        let mut touched = false;
        for birthday in cosmic_pim_core::birthdays::in_range(&self.contact_cards, from, to) {
            let summary = match birthday.turns {
                Some(age) => fl!("birthday-turns", name = birthday.name, age = age),
                None => fl!("birthday-name", name = birthday.name),
            };
            let start = birthday.date.and_time(chrono::NaiveTime::MIN);
            self.days
                .entry(birthday.date)
                .or_default()
                .push(Occurrence {
                    uid: birthday.uid,
                    calendar_id: BIRTHDAYS_CALENDAR_ID.to_owned(),
                    summary,
                    location: None,
                    all_day: true,
                    start,
                    end: start + chrono::Duration::days(1),
                    recurrence_id: None,
                });
            touched = true;
        }
        if touched {
            for day in self.days.values_mut() {
                day.sort_by_key(Occurrence::sort_key);
            }
        }
    }

    fn grid<'a>(&'a self, calendars: &'a [CalendarMeta]) -> Element<'a, Message> {
        let (start, _) = crate::ui::visible_range(self.view(), self.anchor, &self.config);

        match self.view() {
            ViewKind::Month => MonthView {
                anchor: self.anchor,
                today: self.today,
                start,
                days: &self.days,
                calendars,
                config: &self.config,
            }
            .view(),
            ViewKind::Week => TimeGrid {
                start,
                days: 7,
                today: self.today,
                now: self.now,
                occurrences: &self.days,
                calendars,
                config: &self.config,
                local: self.local_timezone(),
                ghost: self.grid_ghost(),
            }
            .view(),
            ViewKind::Agenda => crate::ui::agenda::Agenda {
                occurrences: &self.days,
                calendars,
                config: &self.config,
                today: self.today,
            }
            .view(),
            ViewKind::Year => crate::ui::year::YearView {
                anchor: self.anchor,
                today: self.today,
                days: &self.days,
                config: &self.config,
            }
            .view(),
            ViewKind::Tasks => crate::ui::tasks::TaskList {
                todos: &self.todos,
                calendars,
                config: &self.config,
                now: self.now,
                local: self.local_timezone(),
                show_done: self.show_done_tasks,
                draft: &self.new_task,
                can_add: self
                    .store
                    .as_ref()
                    .and_then(Store::default_calendar)
                    .is_some(),
            }
            .view(),
            ViewKind::Day => TimeGrid {
                start,
                days: 1,
                today: self.today,
                now: self.now,
                occurrences: &self.days,
                calendars,
                config: &self.config,
                local: self.local_timezone(),
                ghost: self.grid_ghost(),
            }
            .view(),
        }
    }

    /// Asks for a scroll on the next frame.
    ///
    /// Scrolling immediately from `update` is a no-op: the widget tree is rebuilt
    /// *after* update returns, so the grid's scrollable does not exist yet and the
    /// operation finds nothing to act on.
    fn request_scroll() -> Task<cosmic::Action<Message>> {
        cosmic::task::future(async {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            Message::ScrollTimeGrid
        })
    }

    /// Scrolls the week/day grid so the day's first event is in view, rather
    /// than leaving the user staring at midnight.
    fn scroll_time_grid(&self) -> Task<cosmic::Action<Message>> {
        if !matches!(self.view(), ViewKind::Week | ViewKind::Day) {
            return Task::none();
        }

        // An hour of lead-in above the first event reads better than pinning it
        // to the very top.
        let hour = self
            .days
            .values()
            .flatten()
            .filter(|o| !o.all_day)
            .map(|o| o.start.hour())
            .min()
            .unwrap_or(DEFAULT_SCROLL_HOUR)
            .saturating_sub(1);

        cosmic::iced::widget::scrollable::scroll_to(
            crate::ui::timegrid::scroll_id(),
            cosmic::iced::widget::scrollable::AbsoluteOffset {
                x: Some(0.0),
                y: Some(crate::ui::timegrid::offset_for_hour(hour)),
            },
        )
    }

    /// One notification for the reminders that passed while asleep.
    ///
    /// Same ownership rule as [`Self::fire_due_reminders`]: if the daemon is
    /// running, the digest is its job, not ours.
    fn report_missed_reminders(
        &mut self,
        slept_at: NaiveDateTime,
    ) -> Task<cosmic::Action<Message>> {
        if self.daemon_running {
            return Task::none();
        }
        let Some(store) = self.store.as_ref() else {
            return Task::none();
        };

        let missed = match crate::reminders::missed_reminders(
            store,
            &mut self.reminders,
            &self.config,
            slept_at,
            self.now,
        ) {
            Ok(missed) => missed,
            Err(why) => {
                tracing::warn!(%why, "could not load occurrences to count missed reminders");
                return Task::none();
            }
        };

        if missed == 0 {
            return Task::none();
        }
        cosmic::task::future(async move {
            crate::reminders::notify_missed(missed, <AppModel as cosmic::Application>::APP_ID)
                .await;
            Message::Ignore
        })
    }

    /// Shows any reminder whose trigger has just passed.
    ///
    /// Reminders are checked against the store rather than the currently visible
    /// range, so they still fire while you are looking at a different month.
    fn fire_due_reminders(&mut self) -> Task<cosmic::Action<Message>> {
        // The daemon has it covered; firing here too would double every reminder.
        if self.daemon_running {
            return Task::none();
        }

        let Some(store) = self.store.as_ref() else {
            return Task::none();
        };

        self.sleep.swept(self.now);
        let due = match crate::reminders::due_reminders(
            store,
            &mut self.reminders,
            &self.config,
            self.now,
        ) {
            Ok(due) => due,
            Err(why) => {
                tracing::warn!(%why, "could not load occurrences to check reminders");
                return Task::none();
            }
        };

        if !due.is_empty() {
            tracing::info!(count = due.len(), "firing reminders");
        }

        // Delivery is a D-Bus round trip, so it happens off the update loop.
        Task::batch(due.into_iter().map(|reminder| {
            let body = reminder.body(&self.config);
            cosmic::task::future(async move {
                let snoozed = crate::reminders::notify(
                    &reminder,
                    <Self as cosmic::Application>::APP_ID,
                    body,
                )
                .await;
                if snoozed {
                    Message::ReminderSnoozed(reminder.id)
                } else {
                    Message::Ignore
                }
            })
        }))
    }

    fn editor_view(&self) -> Element<'_, Message> {
        match &self.editor {
            Some(editor) => editor.view(self.calendars(), &self.config),
            None => widget::text::body(fl!("no-events-range")).into(),
        }
    }

    fn settings_view(&self) -> Element<'_, Message> {
        use chrono::Weekday;

        let weekdays: Vec<String> = [
            Weekday::Mon,
            Weekday::Tue,
            Weekday::Wed,
            Weekday::Thu,
            Weekday::Fri,
            Weekday::Sat,
            Weekday::Sun,
        ]
        .into_iter()
        .map(crate::ui::weekday_short)
        .collect();

        let general = widget::settings::section()
            .add(
                widget::settings::item::builder(fl!("first-day-of-week")).control(
                    widget::dropdown(
                        weekdays,
                        Some(usize::from(self.config.first_day_of_week.min(6))),
                        Message::SetFirstDayOfWeek,
                    )
                    .width(Length::Fixed(160.0)),
                ),
            )
            .add(
                widget::settings::item::builder(fl!("show-week-numbers"))
                    .toggler(self.config.show_week_numbers, Message::ToggleWeekNumbers),
            )
            .add(
                widget::settings::item::builder(fl!("show-birthdays"))
                    .description(fl!("show-birthdays-description"))
                    .toggler(self.config.show_birthdays, Message::ToggleShowBirthdays),
            )
            .add(
                widget::settings::item::builder(fl!("time-format-24h"))
                    .toggler(self.config.time_24h, Message::Toggle24Hour),
            )
            .add(
                widget::settings::item::builder(fl!("default-reminder"))
                    .description(fl!("default-reminder-description"))
                    .control(
                        widget::dropdown(
                            REMINDER_LABELS.with(Clone::clone),
                            REMINDER_CHOICES
                                .iter()
                                .position(|m| *m == self.config.default_reminder_minutes),
                            Message::SetDefaultReminder,
                        )
                        .width(Length::Fixed(160.0)),
                    ),
            )
            .add(
                widget::settings::item::builder(fl!("secondary-timezone"))
                    .description(fl!("secondary-timezone-description"))
                    .control(
                        widget::text_input(fl!("time-zone-search-hint"), &self.secondary_tz_input)
                            .on_input(Message::SetSecondaryTz)
                            .width(Length::Fixed(220.0)),
                    ),
            )
            .add(
                widget::settings::item::builder(fl!("snap-minutes"))
                    .description(fl!("snap-minutes-description"))
                    .control(
                        widget::dropdown(
                            SNAP_CHOICES
                                .iter()
                                .map(|m| fl!("snap-minutes-value", minutes = i64::from(*m)))
                                .collect::<Vec<_>>(),
                            SNAP_CHOICES
                                .iter()
                                .position(|m| i64::from(*m) == self.config.snap()),
                            Message::SetSnapMinutes,
                        )
                        .width(Length::Fixed(160.0)),
                    ),
            );

        widget::column::with_capacity(2)
            .spacing(cosmic::theme::spacing().space_m)
            .push(general)
            .push(self.local_calendars_view())
            .push(self.calendar_defaults_view())
            .apply(widget::scrollable)
            .into()
    }

    /// Per-calendar overrides of the two defaults above.
    ///
    /// Only writable calendars appear: a read-only feed cannot hold a new
    /// event, so a default duration for one would be an offer that cannot be
    /// taken. Its reminders still follow the app-wide setting.
    /// Whether `calendar` is this computer's own: writable, and bound to no
    /// account. Only those are renamed, recoloured and deleted here — a
    /// server calendar's name and colour are the server's, and deleting its
    /// folder would only have the next sync bring it back.
    fn is_local_calendar(&self, calendar: &CalendarMeta) -> bool {
        !calendar.read_only
            && self.accounts.as_ref().is_none_or(|accounts| {
                cosmic_pim_sync::account_for_collection(accounts, &calendar.id).is_none()
            })
    }

    /// Renames and/or recolours a local calendar.
    fn update_local_calendar(
        &mut self,
        id: &str,
        name: &str,
        color: crate::model::Rgb,
    ) -> Task<cosmic::Action<Message>> {
        if !self
            .store
            .as_ref()
            .and_then(|s| s.calendar(id))
            .is_some_and(|calendar| self.is_local_calendar(calendar))
        {
            return Task::none();
        }
        let Some(store) = self.store.as_mut() else {
            return Task::none();
        };
        match store.update_calendar(id, name, color) {
            Ok(()) => {
                self.sync_sidebar();
                self.reload();
                Task::none()
            }
            Err(why) => self.toast_error(&why.to_string()),
        }
    }

    /// Deletes the local calendar the confirm dialog asked about, events and
    /// tasks with it. Not journaled: undo restores files, not collections.
    fn delete_local_calendar(&mut self) -> Task<cosmic::Action<Message>> {
        let Some(id) = self.pending_calendar_removal.take() else {
            return Task::none();
        };
        let Some(meta) = self.store.as_ref().and_then(|s| s.calendar(&id)).cloned() else {
            return Task::none();
        };
        // Checked again at the point of no return: only ever this computer's
        // own calendar, never a feed or one a server holds.
        if !self.is_local_calendar(&meta) {
            return Task::none();
        }
        if let Err(why) = std::fs::remove_dir_all(&meta.path) {
            return self.toast_error(&why.to_string());
        }
        if let Some(store) = self.store.as_mut()
            && let Err(why) = store.refresh()
        {
            tracing::warn!(%why, "refresh after deleting a calendar failed");
        }
        self.sync_sidebar();
        self.reload();
        self.reload_tasks();
        Task::none()
    }

    fn local_calendars_view(&self) -> Element<'_, Message> {
        let local: Vec<&CalendarMeta> = self
            .calendars()
            .iter()
            .filter(|calendar| self.is_local_calendar(calendar))
            .collect();
        if local.is_empty() {
            return widget::Space::new().into();
        }

        let spacing = cosmic::theme::spacing();
        let mut section = widget::settings::section().title(fl!("local-calendars"));
        for calendar in local {
            let id = calendar.id.clone();
            let name = self
                .calendar_name_drafts
                .get(&calendar.id)
                .cloned()
                .unwrap_or_else(|| calendar.name.clone());

            let mut swatches =
                widget::row::with_capacity(PALETTE.len()).spacing(spacing.space_xxxs);
            for (index, rgb) in PALETTE.iter().enumerate() {
                swatches = swatches.push(
                    widget::button::custom(crate::ui::swatch(*rgb, *rgb == calendar.color, 14.0))
                        .class(cosmic::theme::Button::Icon)
                        .padding(spacing.space_xxxs)
                        .on_press(Message::CalendarRecolor(id.clone(), index)),
                );
            }

            let for_input = id.clone();
            let for_submit = id.clone();
            section = section.add(
                widget::column::with_capacity(2)
                    .spacing(spacing.space_xxs)
                    .push(
                        widget::row::with_capacity(2)
                            .spacing(spacing.space_xxs)
                            .align_y(Alignment::Center)
                            .push(
                                widget::text_input(fl!("calendar-name"), name)
                                    .on_input(move |text| {
                                        Message::CalendarNameInput(for_input.clone(), text)
                                    })
                                    .on_submit(move |_| {
                                        Message::CalendarNameCommit(for_submit.clone())
                                    })
                                    .width(Length::Fill),
                            )
                            .push(
                                widget::button::destructive(fl!("delete"))
                                    .on_press(Message::CalendarDeleteRequest(id.clone())),
                            ),
                    )
                    .push(swatches),
            );
        }
        section.into()
    }

    fn calendar_defaults_view(&self) -> Element<'_, Message> {
        let writable: Vec<&CalendarMeta> = self
            .calendars()
            .iter()
            .filter(|calendar| !calendar.read_only)
            .collect();

        if writable.is_empty() {
            return widget::Space::new().into();
        }

        let mut section = widget::settings::section().title(fl!("calendar-defaults"));
        for calendar in writable {
            let defaults = self.config.defaults_for(&calendar.id);
            let reminder = defaults.map_or(0, |d| d.reminder_minutes);
            let duration = defaults.map_or(0, |d| d.duration_minutes);
            let id = calendar.id.clone();
            let for_duration = calendar.id.clone();

            section = section.add(
                widget::settings::item::builder(calendar.name.clone()).control(
                    widget::row::with_capacity(2)
                        .spacing(cosmic::theme::spacing().space_xxs)
                        .push(
                            widget::dropdown(
                                CALENDAR_REMINDER_LABELS.with(Clone::clone),
                                CALENDAR_REMINDER_CHOICES
                                    .iter()
                                    .position(|m| *m == reminder),
                                move |index| Message::SetCalendarReminder(id.clone(), index),
                            )
                            .width(Length::Fixed(150.0)),
                        )
                        .push(
                            widget::dropdown(
                                CALENDAR_DURATION_LABELS.with(Clone::clone),
                                CALENDAR_DURATION_CHOICES
                                    .iter()
                                    .position(|m| *m == duration),
                                move |index| {
                                    Message::SetCalendarDuration(for_duration.clone(), index)
                                },
                            )
                            .width(Length::Fixed(150.0)),
                        ),
                ),
            );
        }
        section.into()
    }

    fn with_account_form(&mut self, f: impl FnOnce(&mut AccountForm)) {
        if let Some(form) = self.account_form.as_mut() {
            f(form);
            // Any edit invalidates the previous validation failure.
            form.error = None;
        }
    }

    /// Validates the add-account form and stores the account.
    /// Rereads the suite's shared account list.
    ///
    /// Other processes write it — the sync engine records which collections
    /// belong to which account, Envelope and Circle add and remove accounts —
    /// so a copy loaded at start-up goes stale. Saving from a stale copy puts
    /// its old bindings back over the ones sync wrote, so every change here
    /// starts from the file as it is now.
    fn reopen_accounts(&mut self) {
        match cosmic_pim_accounts::AccountStore::open_default() {
            Ok(accounts) => self.accounts = Some(accounts),
            // The copy we have is still better than none.
            Err(why) => tracing::warn!(%why, "cannot reread the account store"),
        }
    }

    fn confirm_account(&mut self) -> Task<cosmic::Action<Message>> {
        let Some(form) = self.account_form.clone() else {
            return Task::none();
        };
        self.reopen_accounts();
        let Some(accounts) = self.accounts.as_mut() else {
            return self.toast_error(&fl!("error-no-account-store"));
        };

        let url = form.url.trim();
        // Refuse plaintext up front rather than after the password has been
        // typed, stored, and sent: a CalDAV password over http is compromised
        // the first time it is used, and no later warning undoes that.
        if !url.starts_with("https://") && !url.starts_with("http://") {
            self.with_account_form(|f| f.error = Some(fl!("error-url-scheme")));
            return Task::none();
        }
        if url.starts_with("http://") && !is_loopback(url) {
            self.with_account_form(|f| f.error = Some(fl!("error-url-insecure")));
            return Task::none();
        }

        let display_name = if form.display_name.trim().is_empty() {
            form.username.trim().to_owned()
        } else {
            form.display_name.trim().to_owned()
        };

        let account = cosmic_pim_accounts::Account::new(&display_name, url, form.username.trim());

        match accounts.add(account, &form.password) {
            Ok(()) => {
                self.account_form = None;
                // Sync immediately: the user just told us where their calendar
                // is, and waiting five minutes to act on that feels broken.
                self.sync_now()
            }
            Err(why) => {
                self.with_account_form(|f| f.error = Some(why.to_string()));
                Task::none()
            }
        }
    }

    /// Runs a sync pass: the daemon's, when it is running, otherwise one of
    /// our own off the UI thread.
    ///
    /// Never both. Two passes at once each rewrite the collections' sync
    /// state whole, and one of them loses the other's queue entries and
    /// etags — see [`crate::background`].
    fn sync_now(&mut self) -> Task<cosmic::Action<Message>> {
        if self.syncing || self.accounts.is_none() {
            return Task::none();
        }
        self.syncing = true;
        self.sync_status = None;

        if self.daemon_running {
            let bus = self.dbus.clone();
            return cosmic::task::future(async move {
                let report = async {
                    let connection = match bus {
                        Some(connection) => connection,
                        None => zbus::Connection::session().await?,
                    };
                    crate::background::BackgroundProxy::new(&connection)
                        .await?
                        .sync()
                        .await
                }
                .await;
                let (lines, changed) = report.unwrap_or_else(|why| {
                    tracing::warn!(%why, "the reminder daemon did not run the sync");
                    (
                        vec![fl!("sync-daemon-failed", reason = why.to_string())],
                        false,
                    )
                });
                Message::SyncFinished(lines, changed)
            });
        }

        cosmic::task::future(async move {
            let (lines, changed) = tokio::task::spawn_blocking(crate::background::sync_accounts)
                .await
                .unwrap_or_else(|why| (vec![why.to_string()], false));
            Message::SyncFinished(lines, changed)
        })
    }

    /// Creates a feed subscription. Deliberately offline — `feed::subscribe`
    /// writes the collection and its state file without touching the network,
    /// so the dialog completes instantly; the first fetch happens in the
    /// refresh pass kicked off right after.
    fn confirm_subscription(&mut self) -> Task<cosmic::Action<Message>> {
        let Some(form) = self.sub_form.as_mut() else {
            return Task::none();
        };
        let url = form.url.trim().to_owned();

        let scheme_ok = ["https://", "webcal://", "webcals://", "http://"]
            .iter()
            .any(|s| url.starts_with(s));
        if !scheme_ok {
            form.error = Some(fl!("error-feed-scheme"));
            return Task::none();
        }
        // The same plaintext rule as accounts: http is for loopback testing only.
        if url.starts_with("http://") && !is_loopback(&url) {
            form.error = Some(fl!("error-url-insecure"));
            return Task::none();
        }

        let name = if form.name.trim().is_empty() {
            url.clone()
        } else {
            form.name.trim().to_owned()
        };
        let color = PALETTE[self.store.as_ref().map_or(0, |s| s.calendars().len()) % PALETTE.len()];

        let root = crate::store::vdir::default_root();
        match cosmic_pim_caldav::feed::subscribe(&root, &name, &url, color, None) {
            Ok(_) => {
                self.sub_form = None;
                if let Some(store) = self.store.as_mut() {
                    let _ = store.refresh();
                }
                self.reload();
                self.refresh_feeds(true)
            }
            Err(why) => {
                if let Some(form) = self.sub_form.as_mut() {
                    form.error = Some(why.to_string());
                }
                Task::none()
            }
        }
    }

    /// Removes the subscription the confirm dialog was shown for. The
    /// collection holds only derived data — a re-subscribe to the same URL
    /// rebuilds it — which is what makes deletion acceptable at all.
    fn remove_subscription(&mut self) -> Task<cosmic::Action<Message>> {
        let Some(id) = self.pending_feed_removal.take() else {
            return Task::none();
        };
        let Some(meta) = self.store.as_ref().and_then(|s| s.calendar(&id)).cloned() else {
            return Task::none();
        };
        // Only ever remove a directory that really is a feed: anything else
        // holds user data this button must not be able to reach.
        if !cosmic_pim_caldav::feed::is_feed(&meta.path) {
            return Task::none();
        }
        if let Err(why) = std::fs::remove_dir_all(&meta.path) {
            return self.toast_error(&why.to_string());
        }
        if let Some(store) = self.store.as_mut() {
            let _ = store.refresh();
        }
        self.reload();
        Task::none()
    }

    /// Refreshes due ICS feeds; `force` fetches them all, due or not, which
    /// is what a just-added subscription wants. Asks the daemon when it is
    /// running, for the same reason [`Self::sync_now`] does.
    fn refresh_feeds(&mut self, force: bool) -> Task<cosmic::Action<Message>> {
        if self.refreshing_feeds {
            return Task::none();
        }
        self.refreshing_feeds = true;
        self.last_feed_check = Some(std::time::Instant::now());

        if self.daemon_running {
            let bus = self.dbus.clone();
            return cosmic::task::future(async move {
                let changed = async {
                    let connection = match bus {
                        Some(connection) => connection,
                        None => zbus::Connection::session().await?,
                    };
                    crate::background::BackgroundProxy::new(&connection)
                        .await?
                        .refresh_feeds(force)
                        .await
                }
                .await
                .unwrap_or_else(|why| {
                    // A feed is retried on its own schedule; nothing for the
                    // user to act on now.
                    tracing::warn!(%why, "the reminder daemon did not refresh the feeds");
                    false
                });
                Message::FeedsRefreshed(changed)
            });
        }

        cosmic::task::future(async move {
            let changed =
                tokio::task::spawn_blocking(move || crate::background::refresh_feeds(force))
                    .await
                    .unwrap_or(false);
            Message::FeedsRefreshed(changed)
        })
    }

    /// Reloads the unresolved-conflict list from the sync sidecars.
    fn reload_conflicts(&mut self) {
        let root = crate::store::vdir::default_root();
        let describe = |text: &str| -> String {
            crate::store::vdir::parse_ics(text, "", "")
                .into_iter()
                .next()
                .map(|event| {
                    if event.summary.trim().is_empty() {
                        fl!("untitled-event")
                    } else {
                        event.summary
                    }
                })
                .unwrap_or_else(|| fl!("untitled-event"))
        };

        self.conflicts = cosmic_pim_sync::conflicts(&root)
            .into_iter()
            .map(|(collection, conflict)| {
                // With the base revision on record, the engine can say which
                // units are actually in dispute; without it (or when the texts
                // are not comparable) only the wholesale answers are honest.
                let disputes = conflict.base.as_deref().and_then(|base| {
                    cosmic_pim_core::merge::overlaps(base, &conflict.local, &conflict.remote).map(
                        |units| Disputes {
                            base: base.to_owned(),
                            local: conflict.local.clone(),
                            remote: conflict.remote.clone(),
                            units,
                            choices: std::collections::BTreeMap::new(),
                        },
                    )
                });
                ConflictRow {
                    yours: describe(&conflict.local),
                    theirs: describe(&conflict.remote),
                    collection,
                    href: conflict.href,
                    disputes,
                }
            })
            .collect();
    }

    /// Resolves one conflict: keep this device's version, or take the server's.
    fn resolve_conflict(
        &mut self,
        index: usize,
        keep_local: bool,
    ) -> Task<cosmic::Action<Message>> {
        let Some(row) = self.conflicts.get(index).cloned() else {
            return Task::none();
        };
        let root = crate::store::vdir::default_root();
        let result = if keep_local {
            cosmic_pim_sync::conflict::keep_local(&root, &row.collection, &row.href, None)
        } else {
            cosmic_pim_sync::conflict::take_remote(&root, &row.collection, &row.href)
        };
        match result {
            Ok(_) => {
                if let Some(store) = self.store.as_mut() {
                    let _ = store.refresh();
                }
                self.reload();
                self.reload_conflicts();
                Task::none()
            }
            Err(why) => self.toast_error(&why.to_string()),
        }
    }

    /// Resolves one conflict from its per-unit choices: builds the merged
    /// document and keeps it as the local version, re-queued for upload.
    ///
    /// Silently a no-op while any disputed unit is undecided — the Apply
    /// button is only pressable once every unit has an answer, so arriving
    /// here early means a stale index, not a user mistake.
    fn apply_conflict_merge(&mut self, index: usize) -> Task<cosmic::Action<Message>> {
        let Some(row) = self.conflicts.get(index) else {
            return Task::none();
        };
        let Some(disputes) = &row.disputes else {
            return Task::none();
        };
        if !disputes.decided() {
            return Task::none();
        }
        let merged = cosmic_pim_core::merge::resolve(
            &disputes.base,
            &disputes.local,
            &disputes.remote,
            &disputes.choices,
        );
        let (collection, href) = (row.collection.clone(), row.href.clone());
        let Some(merged) = merged else {
            // The engine refused the choices — the texts stopped being
            // comparable underneath us. Wholesale is still available.
            return self.toast_error(&fl!("conflict-merge-failed"));
        };
        let root = crate::store::vdir::default_root();
        match cosmic_pim_sync::conflict::keep_local(&root, &collection, &href, Some(&merged)) {
            Ok(_) => {
                if let Some(store) = self.store.as_mut() {
                    let _ = store.refresh();
                }
                self.reload();
                self.reload_conflicts();
                Task::none()
            }
            Err(why) => self.toast_error(&why.to_string()),
        }
    }

    /// The invitation dialog: who, what, when, whether the slot is free, and
    /// the three PARTSTAT answers.
    fn invitation_dialog<'a>(&'a self, invitation: &'a PendingInvitation) -> Element<'a, Message> {
        let organizer = invitation.parsed.organizer.as_ref().map_or_else(
            || fl!("invitation-unknown-organizer"),
            |o| o.name.clone().unwrap_or_else(|| o.email.clone()),
        );
        let summary = invitation
            .parsed
            .summary
            .clone()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| fl!("untitled-event"));

        let mut body = fl!("invitation-body", organizer = organizer, summary = summary);
        if let Some(event) = &invitation.event {
            let local = self.local_timezone();
            let start = event.start.naive_local(local);
            // The same "Tue 4 August 09:00" the rest of the app shows, not
            // an ISO date.
            let when = if event.start.is_all_day() {
                crate::ui::format_day(start.date())
            } else {
                format!(
                    "{} {}",
                    crate::ui::format_day(start.date()),
                    crate::ui::format_time(start.time(), &self.config)
                )
            };
            body.push('\n');
            body.push_str(&fl!("invitation-when", when = when));
            body.push('\n');
            let overlaps = self.slot_overlaps(event);
            body.push_str(&if overlaps == 0 {
                fl!("invitation-slot-free")
            } else {
                fl!("invitation-slot-busy", count = overlaps)
            });
        }

        widget::dialog()
            .title(fl!("invitation-title"))
            .body(body)
            .primary_action(
                widget::button::suggested(fl!("invitation-accept"))
                    .on_press(Message::InvitationAnswer(InviteAnswer::Accepted)),
            )
            .secondary_action(
                widget::button::standard(fl!("invitation-tentative"))
                    .on_press(Message::InvitationAnswer(InviteAnswer::Tentative)),
            )
            .tertiary_action(
                widget::button::destructive(fl!("invitation-decline"))
                    .on_press(Message::InvitationAnswer(InviteAnswer::Declined)),
            )
            .control(
                widget::button::text(fl!("invitation-later"))
                    .class(cosmic::theme::Button::Link)
                    .on_press(Message::InvitationDismiss),
            )
            .into()
    }

    /// How many stored occurrences overlap the invitation's slot.
    ///
    /// Deduplicated across days, and the invitation's own UID excluded so an
    /// update to an already-stored event never counts as its own conflict.
    fn slot_overlaps(&self, event: &crate::model::Event) -> usize {
        let Some(store) = self.store.as_ref() else {
            return 0;
        };
        let local = self.local_timezone();
        let start = event.start.naive_local(local);
        let end = event.end.naive_local(local);
        let from = start.date();
        let to = end.date() + chrono::Duration::days(1);
        let Ok(days) = store.occurrences_by_day(from, to, &self.config.hidden_set()) else {
            return 0;
        };
        let mut seen = std::collections::HashSet::new();
        days.values()
            .flatten()
            .filter(|o| o.uid != event.uid && o.start < end && o.end > start)
            .filter(|o| seen.insert((o.calendar_id.clone(), o.uid.clone(), o.recurrence_id)))
            .count()
    }

    /// The collection an invitation for `account_id` lands in: the account's
    /// first writable bound calendar, else the first writable one anywhere.
    fn invitation_collection(&self, account_id: &str) -> Option<crate::model::CalendarMeta> {
        let store = self.store.as_ref()?;
        if let Some(accounts) = self.accounts.as_ref()
            && let Some(account) = accounts.get(account_id)
        {
            for collection_id in account.collections.values() {
                if let Some(meta) = store.calendar(collection_id)
                    && !meta.read_only
                {
                    return Some(meta.clone());
                }
            }
        }
        self.calendars().iter().find(|c| !c.read_only).cloned()
    }

    /// The address `account_id` receives mail at — the identity the attendee
    /// gate matches and the REPLY answers as.
    fn account_address(&self, account_id: &str) -> Option<String> {
        let account = self.accounts.as_ref()?.get(account_id)?;
        if let Some(mail) = &account.mail {
            if !mail.from_address.trim().is_empty() {
                return Some(mail.from_address.trim().to_owned());
            }
            if let Some(login) = &mail.imap_username
                && login.contains('@')
            {
                return Some(login.trim().to_owned());
            }
        }
        account
            .username
            .contains('@')
            .then(|| account.username.trim().to_owned())
    }

    /// The display name to put on a REPLY. Empty is fine — `build_reply`
    /// omits the CN.
    fn account_display_name(&self, account_id: &str) -> String {
        self.accounts
            .as_ref()
            .and_then(|a| a.get(account_id))
            .and_then(|a| a.mail.as_ref())
            .map(|m| m.from_name.trim().to_owned())
            .unwrap_or_default()
    }

    fn refresh_after_invitation(&mut self) {
        if let Some(store) = self.store.as_mut() {
            let _ = store.refresh();
        }
        self.reload();
    }

    /// Answers the invitation at the front of the queue.
    ///
    /// Accept and Tentative store the event (via `itip::apply`, which owns
    /// the attendee gate and the SEQUENCE rule) and record the PARTSTAT on
    /// the stored copy through the same tested path a mailed REPLY takes.
    /// Decline keeps nothing — a declined meeting on the grid is clutter,
    /// and the organizer's next update re-delivers it if things change. A
    /// copy stored by an earlier accept is withdrawn ([`withdraw_declined`]),
    /// so the calendar ends up the same whether or not the user said yes
    /// first.
    /// Either way the reply goes to Envelope's outbox when Envelope is
    /// running, and degrades to "reply from your mail client" when not.
    fn answer_invitation(&mut self, answer: InviteAnswer) -> Task<cosmic::Action<Message>> {
        use cosmic_pim_caldav::itip;

        // Stays at the front of the queue until it has been answered: a
        // failure the user can fix (no writable calendar, no address, a disk
        // error) leaves the dialog up to try again or put off.
        let Some(invitation) = self.invitations.first().cloned() else {
            return Task::none();
        };
        // Routing reads the accounts' collection bindings, which sync keeps
        // current in the shared file, not in our copy.
        self.reopen_accounts();

        let Some(meta) = self.invitation_collection(&invitation.account_id) else {
            return self.toast_error(&fl!("no-writable-calendar"));
        };
        let Some(me) = self.account_address(&invitation.account_id) else {
            return self.toast_error(&fl!("invitation-no-address"));
        };
        let mut queued = Ok(());

        if answer == InviteAnswer::Declined {
            match withdraw_declined(&meta.path, &invitation.ics, &me) {
                Ok(itip::Outcome::Cancelled { file }) => {
                    queued = queue_writeback_delete_file(&meta.id, &file);
                }
                Ok(itip::Outcome::InstanceCancelled { file }) => {
                    queued = queue_writeback_file(&meta.id, &file, None);
                }
                Ok(itip::Outcome::Stale) => {
                    self.invitations.remove(0);
                    return self.toast_error(&fl!("invitation-stale"));
                }
                // Nothing was stored: nothing to withdraw.
                Ok(_) => {}
                Err(why) => return self.toast_error(&why.to_string()),
            }
        } else {
            match itip::apply(&meta.path, &invitation.ics, &me, None) {
                // Neither can succeed on a retry; the invitation is done with.
                Ok(itip::Outcome::Stale) => {
                    self.invitations.remove(0);
                    return self.toast_error(&fl!("invitation-stale"));
                }
                Ok(itip::Outcome::NotForMe) => {
                    self.invitations.remove(0);
                    return self.toast_error(&fl!("invitation-not-for-me"));
                }
                Ok(itip::Outcome::NotFromOrganizer) => {
                    self.invitations.remove(0);
                    return self.toast_error(&fl!("invitation-not-from-organizer"));
                }
                Ok(outcome) => {
                    if let Some(file) = outcome.file() {
                        queued = queue_writeback_file(&meta.id, file, None);
                    }
                }
                Err(why) => return self.toast_error(&why.to_string()),
            }
        }
        // Answered: what remains is telling the organizer.
        self.invitations.remove(0);

        let Some(organizer) = invitation
            .parsed
            .organizer
            .as_ref()
            .map(|o| o.email.clone())
        else {
            // No organizer, nothing to answer; whatever was stored, stands.
            self.refresh_after_invitation();
            return self.report_unqueued(queued);
        };

        let reply_ics = itip::build_reply(
            &itip::Reply {
                uid: &invitation.parsed.uid,
                recurrence_id: invitation.parsed.recurrence_id.as_deref(),
                sequence: invitation.parsed.sequence,
                organizer_email: &organizer,
                summary: invitation.parsed.summary.as_deref(),
                partstat: answer.partstat(),
            },
            &me,
            &self.account_display_name(&invitation.account_id),
        );

        if answer != InviteAnswer::Declined {
            match itip::apply(&meta.path, &reply_ics, &me, None) {
                Ok(outcome) => {
                    if let Some(file) = outcome.file() {
                        queued = queue_writeback_file(&meta.id, file, None);
                    }
                }
                Err(why) => tracing::warn!(%why, "could not record the PARTSTAT locally"),
            }
        }

        self.refresh_after_invitation();
        let unqueued = self.report_unqueued(queued);

        let Some(conn) = self.dbus.clone() else {
            return Task::batch([unqueued, self.toast_info(&fl!("invitation-reply-by-mail"))]);
        };
        let account_id = invitation.account_id.clone();
        let reply = cosmic::task::future(async move {
            let queued = crate::scheduling::send_reply(&conn, &reply_ics, &account_id, &organizer)
                .await
                .unwrap_or_else(|why| {
                    tracing::warn!(%why, "the scheduling reply could not reach Envelope");
                    false
                });
            Message::InvitationReplySent(queued)
        });
        Task::batch([unqueued, reply])
    }

    /// Applies a CANCEL or REPLY without asking: neither carries a decision
    /// for this user to make. The gates live inside `itip::apply`.
    fn apply_itip_quietly(
        &mut self,
        delivery: &crate::scheduling::Delivery,
    ) -> Task<cosmic::Action<Message>> {
        use cosmic_pim_caldav::itip::Outcome;

        self.reopen_accounts();
        let Some(meta) = self.invitation_collection(&delivery.account_id) else {
            return Task::none();
        };
        let me = self
            .account_address(&delivery.account_id)
            .unwrap_or_default();
        match cosmic_pim_caldav::itip::apply(&meta.path, &delivery.ics, &me, None) {
            Ok(outcome) => {
                let queued = match &outcome {
                    // A whole-series CANCEL removed the file; the deletion
                    // must reach the server too.
                    Outcome::Cancelled { file } => queue_writeback_delete_file(&meta.id, file),
                    other => other
                        .file()
                        .map_or(Ok(()), |file| queue_writeback_file(&meta.id, file, None)),
                };
                self.refresh_after_invitation();
                let unqueued = self.report_unqueued(queued);
                match quiet_notice(&outcome) {
                    Some(Ok(info)) => Task::batch([unqueued, self.toast_info(&info)]),
                    Some(Err(error)) => Task::batch([unqueued, self.toast_error(&error)]),
                    None => unqueued,
                }
            }
            Err(why) => {
                tracing::warn!(%why, "could not apply an iTIP payload");
                Task::none()
            }
        }
    }

    /// The quick-add dialog: the input, the live parse underneath it, and Add
    /// enabled exactly when the parse holds — a wrong guess is visible before
    /// anything is committed.
    fn quick_add_dialog<'a>(&'a self, input: &'a str) -> Element<'a, Message> {
        let parsed = crate::quickadd::parse(input, self.today);

        let length = self
            .store
            .as_ref()
            .and_then(Store::default_calendar)
            .map_or(Duration::hours(1), |calendar| {
                self.config.duration_for(&calendar.id)
            });
        let preview = match &parsed {
            Some(parsed) => {
                let (start, end) = parsed.span(length);
                let mut line = format!(
                    "{} · {} {} {}",
                    parsed.summary,
                    crate::ui::weekday_short(parsed.date.weekday()),
                    crate::ui::format_date_short(parsed.date),
                    parsed.date.year(),
                );
                if parsed.all_day() {
                    line.push_str(&format!(" · {}", fl!("all-day")));
                } else {
                    line.push_str(&format!(
                        " · {}–{}",
                        crate::ui::format_time(start.time(), &self.config),
                        crate::ui::format_time(end.time(), &self.config),
                    ));
                }
                if let Some(location) = &parsed.location {
                    line.push_str(&format!(" · {location}"));
                }
                line
            }
            None => fl!("quick-add-hint"),
        };

        let add = widget::button::suggested(fl!("add"));
        let add = if parsed.is_some() {
            add.on_press(Message::QuickAddConfirm)
        } else {
            add
        };

        widget::dialog()
            .title(fl!("quick-add-title"))
            .control(
                widget::column::with_capacity(2)
                    .spacing(cosmic::theme::spacing().space_xs)
                    .push(
                        widget::text_input(fl!("quick-add-placeholder"), input)
                            .id(quick_add_input_id())
                            .on_input(Message::QuickAddInput)
                            .on_submit(|_| Message::QuickAddConfirm),
                    )
                    .push(widget::text::caption(preview)),
            )
            .primary_action(add)
            .secondary_action(widget::button::text(fl!("cancel")).on_press(Message::QuickAddCancel))
            .into()
    }

    /// Commits the quick-add parse as a real event in the default calendar.
    fn confirm_quick_add(&mut self) -> Task<cosmic::Action<Message>> {
        let Some(input) = self.quick_add.take() else {
            return Task::none();
        };
        let Some(parsed) = crate::quickadd::parse(&input, self.today) else {
            self.quick_add = Some(input);
            return Task::none();
        };
        let Some(calendar_id) = self
            .store
            .as_ref()
            .and_then(Store::default_calendar)
            .map(|c| c.id.clone())
        else {
            return self.toast_error(&fl!("no-calendars"));
        };
        let Some(store) = self.store.as_mut() else {
            return Task::none();
        };
        let local = store.local_timezone();

        // The calendar's own default length, as the editor and the grid use.
        let (start, end) = parsed.span(self.config.duration_for(&calendar_id));
        let mut event = crate::model::Event::draft(&calendar_id, start, local);
        event.summary = parsed.summary.clone();
        event.location = parsed.location.clone();
        if parsed.all_day() {
            event.start = crate::model::EventTime::Date(start.date());
            event.end = crate::model::EventTime::Date(end.date());
        } else {
            event.end = crate::model::EventTime::Zoned(end, local);
        }

        match store.save(&event) {
            Ok(()) => {
                let queued = queue_writeback_file(&event.calendar_id, &event.file_name, None);
                self.journal_finish(vec![crate::undo::Entry {
                    calendar_id: event.calendar_id.clone(),
                    file_name: event.file_name.clone(),
                    before: None,
                    after: None,
                }]);
                self.anchor = parsed.date;
                self.sync_mini();
                self.reload();
                Task::batch([
                    self.toast(&fl!("quick-add-done", summary = parsed.summary)),
                    self.report_unqueued(queued),
                ])
            }
            Err(why) => self.toast_error(&format!("{}: {why}", fl!("error-save-event"))),
        }
    }

    /// A journal entry's "before" side, snapshotted ahead of a mutation.
    fn journal_before(&self, calendar_id: &str, file_name: &str) -> crate::undo::Entry {
        crate::undo::Entry {
            before: self
                .store
                .as_ref()
                .and_then(|store| writeback_base(store, calendar_id, file_name)),
            calendar_id: calendar_id.to_owned(),
            file_name: file_name.to_owned(),
            after: None,
        }
    }

    /// Completes journal entries after a successful mutation — each file's
    /// current bytes become the "after" side — and records the group.
    fn journal_finish(&mut self, mut entries: Vec<crate::undo::Entry>) {
        if let Some(store) = self.store.as_ref() {
            for entry in &mut entries {
                entry.after = writeback_base(store, &entry.calendar_id, &entry.file_name);
            }
        }
        self.history.record(entries);
    }

    /// Applies one history group: undo writes each file's "before" back,
    /// redo its "after". The files are the source of truth, so restoring
    /// their bytes *is* restoring the state — the index catches up on refresh
    /// and the server through the writeback queue, with the replaced bytes as
    /// the merge base.
    fn apply_history(&mut self, undo: bool) -> Task<cosmic::Action<Message>> {
        let group = if undo {
            self.history.pop_undo()
        } else {
            self.history.pop_redo()
        };
        let Some(group) = group else {
            return Task::none();
        };

        // Where each file lives now; a calendar that no longer exists has
        // nowhere to restore into.
        let paths: Option<Vec<std::path::PathBuf>> = group
            .entries
            .iter()
            .map(|entry| {
                self.store
                    .as_ref()
                    .and_then(|s| s.calendar(&entry.calendar_id))
                    .map(|meta| meta.path.join(&entry.file_name))
            })
            .collect();
        // A group whose files moved on since cannot be applied without
        // throwing that change away. It is dropped rather than kept: it would
        // refuse again on every attempt and block the steps behind it.
        let Some(paths) = paths else {
            return self.toast_error(&if undo {
                fl!("undo-stale")
            } else {
                fl!("redo-stale")
            });
        };
        let current: Vec<Option<String>> = paths
            .iter()
            .map(|path| std::fs::read_to_string(path).ok())
            .collect();
        if !group.applies_cleanly(undo, &current) {
            return self.toast_error(&if undo {
                fl!("undo-stale")
            } else {
                fl!("redo-stale")
            });
        }

        let mut queued = Ok(());
        let mut failed = Vec::new();
        for ((entry, path), current) in group.entries.iter().zip(&paths).zip(current) {
            let desired = if undo { &entry.before } else { &entry.after };

            match desired {
                Some(bytes) => {
                    if let Err(why) = cosmic_pim_core::atomic::write(path, bytes, None) {
                        tracing::warn!(%why, file = entry.file_name, "history restore failed");
                        failed.push(entry.file_name.clone());
                        continue;
                    }
                    queued = queued.and(queue_writeback_file(
                        &entry.calendar_id,
                        &entry.file_name,
                        current.as_deref(),
                    ));
                }
                None => {
                    if let Err(why) = std::fs::remove_file(path)
                        && why.kind() != std::io::ErrorKind::NotFound
                    {
                        tracing::warn!(%why, file = entry.file_name, "history removal failed");
                        failed.push(entry.file_name.clone());
                        continue;
                    }
                    queued = queued.and(queue_writeback_delete_file(
                        &entry.calendar_id,
                        &entry.file_name,
                    ));
                }
            }
        }

        self.history.shelve(group, undo);
        if let Some(store) = self.store.as_mut() {
            let _ = store.refresh();
        }
        self.reload();
        self.reload_tasks();
        let unqueued = self.report_unqueued(queued);
        // Said as it is: a partly applied step is not "undone".
        let outcome = if !failed.is_empty() {
            self.toast_error(&fl!("history-incomplete", files = failed.join(", ")))
        } else if undo {
            self.toast(&fl!("undo-done"))
        } else {
            self.toast(&fl!("redo-done"))
        };
        Task::batch([outcome, unqueued])
    }

    /// Recomputes search results for `query`: summaries, locations and
    /// descriptions across every visible calendar, six months back and a year
    /// forward — wider than the launcher plugin's window, nearest-first, one
    /// row per event.
    fn run_search(&mut self, query: &str) {
        self.search_results.clear();
        let needle = query.trim().to_lowercase();
        if needle.is_empty() {
            return;
        }
        let Some(store) = self.store.as_ref() else {
            return;
        };

        let from = self.today - Duration::days(180);
        let to = self.today + Duration::days(365);
        let Ok(occurrences) = store.occurrences(from, to, &self.config.hidden_set()) else {
            return;
        };

        // Occurrences carry no description, so it is read from the event —
        // once per event, not once per instance of a daily series. A series
        // is searched by its master's description.
        let mut descriptions: HashMap<(String, String), Option<String>> = HashMap::new();
        let mut hits: Vec<Occurrence> = occurrences
            .into_iter()
            .filter(|o| {
                let description = descriptions
                    .entry((o.calendar_id.clone(), o.uid.clone()))
                    .or_insert_with(|| {
                        store
                            .event(&o.calendar_id, &o.uid)
                            .ok()
                            .flatten()
                            .and_then(|event| event.description)
                    });
                matches_search(&needle, o, description.as_deref())
            })
            .collect();

        let today = self.today;
        hits.sort_by_key(|o| (o.start.date() - today).num_days().abs());

        // One row per event, nearest occurrence kept — a daily standup must
        // not fill the list with itself.
        let mut seen = std::collections::HashSet::new();
        hits.retain(|o| seen.insert((o.calendar_id.clone(), o.uid.clone())));
        hits.truncate(50);
        self.search_results = hits;
    }

    /// Resolves a grid press on release: a click opens, a drag commits.
    fn finish_grid_drag(&mut self) -> Task<cosmic::Action<Message>> {
        let Some(drag) = self.grid_drag.take() else {
            return Task::none();
        };
        let snap = self.config.snap();

        match drag {
            // No movement: the press was a click.
            GridDrag::Block {
                block,
                moved: false,
                ..
            } => <Self as cosmic::Application>::update(
                self,
                Message::OpenEvent(block.calendar_id, block.uid, block.rid),
            ),
            GridDrag::Block {
                block,
                pressed_at,
                moved: true,
            } => {
                let Some(cursor) = self.grid_cursor else {
                    return Task::none();
                };
                self.commit_block_drag(block, pressed_at, cursor)
            }

            // A plain press on empty space: an event at that hour, as before.
            GridDrag::Create {
                anchor,
                moved: false,
                ..
            } => {
                let start = anchor
                    .date()
                    .and_hms_opt(anchor.time().hour(), 0, 0)
                    .unwrap_or(anchor);
                self.open_editor_new(start)
            }
            GridDrag::Create {
                anchor,
                current,
                moved: true,
            } => {
                let swept = snap_to(current, snap);
                let (from, to) = if swept >= anchor {
                    (anchor, swept)
                } else {
                    (swept, anchor)
                };
                let to = if to <= from {
                    from + Duration::minutes(snap)
                } else {
                    to
                };
                let task = self.open_editor_new(from);
                if let Some(editor) = self.editor.as_mut() {
                    editor.end_date = to.date();
                    editor.end_time = format!("{:02}:{:02}", to.time().hour(), to.time().minute());
                }
                task
            }
        }
    }

    /// Commits a moved or resized block through the editor's own save path,
    /// which brings validation, override creation, and the scope prompt along
    /// for free. A straight drag commits silently; a series instance pops the
    /// scope question over the grid.
    fn commit_block_drag(
        &mut self,
        block: GridBlockRef,
        pressed_at: NaiveDateTime,
        cursor: NaiveDateTime,
    ) -> Task<cosmic::Action<Message>> {
        let snap = self.config.snap();
        let Some(store) = self.store.as_ref() else {
            return Task::none();
        };
        let local = store.local_timezone();
        let Ok(Some(event)) = store.event_instance(&block.calendar_id, &block.uid, block.rid)
        else {
            return Task::none();
        };
        if event.is_all_day() {
            // All-day events live in the band above the grid; a timed drag on
            // one would be a category change this gesture does not mean.
            return Task::none();
        }

        let (new_start, new_end) = if block.resize {
            let end = snap_to(cursor, snap).max(block.start + Duration::minutes(snap));
            (block.start, end)
        } else {
            let start = snap_to(cursor - (pressed_at - block.start), snap);
            (start, start + (block.end - block.start))
        };
        if new_start == block.start && new_end == block.end {
            return Task::none();
        }

        let mut editor = match block.rid {
            Some(instant) if event.rrule.is_some() && event.recurrence_id.is_none() => {
                Editor::from_series_occurrence(&event, instant, local)
            }
            _ => Editor::from_event(&event, local),
        };

        // Grid times are the viewer's wall clock; the editor's fields may be
        // in the event's own zone. Convert through the instant.
        use chrono::TimeZone;
        let to_field_zone = |t: NaiveDateTime, zone: Option<chrono_tz::Tz>| match zone {
            None => t,
            Some(zone) => local
                .from_local_datetime(&t)
                .earliest()
                .map_or(t, |instant| instant.with_timezone(&zone).naive_local()),
        };
        let start_shown = to_field_zone(new_start, editor.start_tz);
        let end_shown = to_field_zone(new_end, editor.end_tz.or(editor.start_tz));

        editor.start_date = start_shown.date();
        editor.start_time = format!(
            "{:02}:{:02}",
            start_shown.time().hour(),
            start_shown.time().minute()
        );
        editor.end_date = end_shown.date();
        editor.end_time = format!(
            "{:02}:{:02}",
            end_shown.time().hour(),
            end_shown.time().minute()
        );

        // An event the validator would reject silently (no title) opens the
        // drawer instead, so the failure has somewhere to be seen.
        let needs_drawer = editor.summary.trim().is_empty();
        self.editor = Some(editor);
        if needs_drawer {
            self.context_page = ContextPage::Editor;
            self.core.window.show_context = true;
            return Task::none();
        }
        self.save_editor()
    }

    /// The span a grid drag would commit if released now, for the ghost.
    fn grid_ghost(&self) -> Option<(NaiveDateTime, NaiveDateTime)> {
        ghost_span(
            self.grid_drag.as_ref()?,
            self.grid_cursor?,
            self.config.snap(),
        )
    }

    /// Asks the calendar's account when the attendees are busy during this
    /// event's slot.
    ///
    /// Off the update loop: it is a DAV round trip to somebody else's server.
    /// Everything that can go wrong — no account behind this calendar, a
    /// server with no scheduling engine, a refusal — comes back as a state the
    /// editor renders as "unknown", because the one answer that must never be
    /// invented is "free".
    fn check_availability(&mut self) -> Task<cosmic::Action<Message>> {
        let Some(editor) = self.editor.as_ref() else {
            return Task::none();
        };
        let Some(local) = self.store.as_ref().map(Store::local_timezone) else {
            return Task::none();
        };
        let attendees: Vec<String> = editor.attendees.iter().map(|a| a.email.clone()).collect();
        if attendees.is_empty() {
            return Task::none();
        }

        // The slot to ask about is the one the editor currently shows, not the
        // one on disk: the point is to check before committing.
        let Ok(event) = editor.to_event(local) else {
            return Task::none();
        };
        let from_ms = event.start.to_utc(local).timestamp_millis();
        let to_ms = event.end.to_utc(local).timestamp_millis();
        let calendar_id = editor.calendar_id.clone();

        let account_id = self
            .accounts
            .as_ref()
            .and_then(|accounts| cosmic_pim_sync::account_for_collection(accounts, &calendar_id));
        let Some(account_id) = account_id else {
            return <Self as cosmic::Application>::update(
                self,
                Message::AvailabilityAnswer(crate::ui::editor::AvailabilityView::NoAccount),
            );
        };

        self.with_editor(|e| {
            e.checking_availability = true;
            e.availability = None;
        });
        let registry = cosmic_pim_accounts::Registry::load();

        cosmic::task::future(async move {
            let view = tokio::task::spawn_blocking(move || {
                use crate::ui::editor::{AttendeeAvailability, AvailabilityView};

                // Reopened here rather than shared: the store is not `Send`,
                // and resolving may renew and persist an OAuth token.
                let mut accounts = match cosmic_pim_accounts::AccountStore::open_default() {
                    Ok(accounts) => accounts,
                    Err(why) => return AvailabilityView::Failed(why.to_string()),
                };

                match cosmic_pim_sync::availability(
                    &mut accounts,
                    &registry,
                    &account_id,
                    &attendees,
                    from_ms,
                    to_ms,
                ) {
                    Ok(cosmic_pim_sync::Answer::Unsupported) => AvailabilityView::Unsupported,
                    Ok(cosmic_pim_sync::Answer::Answers(answers)) => AvailabilityView::Answers(
                        answers
                            .into_iter()
                            .map(|answer| AttendeeAvailability {
                                busy: overlaps_slot(&answer.busy, from_ms, to_ms),
                                answered: answer.answered(),
                                email: answer.attendee,
                            })
                            .collect(),
                    ),
                    Err(why) => AvailabilityView::Failed(why.to_string()),
                }
            })
            .await
            .unwrap_or_else(|why| crate::ui::editor::AvailabilityView::Failed(why.to_string()));

            Message::AvailabilityAnswer(view)
        })
    }

    fn with_editor(&mut self, f: impl FnOnce(&mut Editor)) {
        if let Some(editor) = self.editor.as_mut() {
            f(editor);
            // Any edit invalidates the last validation failure.
            editor.error = None;
        }
    }

    fn default_new_event_time(&self) -> NaiveDateTime {
        // On today's date, start from the next whole hour; otherwise 09:00.
        if self.anchor == self.today {
            let hour = (self.clock_now().hour() + 1).min(23);
            self.anchor
                .and_hms_opt(hour, 0, 0)
                .unwrap_or_else(|| self.anchor.and_time(NaiveTime::MIN))
        } else {
            self.anchor
                .and_hms_opt(9, 0, 0)
                .unwrap_or_else(|| self.anchor.and_time(NaiveTime::MIN))
        }
    }

    fn open_editor_new(&mut self, start: NaiveDateTime) -> Task<cosmic::Action<Message>> {
        let Some(calendar) = self
            .store
            .as_ref()
            .and_then(Store::default_calendar)
            .map(|c| c.id.clone())
        else {
            return self.toast_error(&fl!("no-calendars"));
        };

        let duration = self.config.duration_for(&calendar);
        self.editor = Some(Editor::new(calendar, start, false, duration));
        self.context_page = ContextPage::Editor;
        self.core.window.show_context = true;
        Task::none()
    }

    fn save_editor(&mut self) -> Task<cosmic::Action<Message>> {
        let Some(editor) = self.editor.as_ref() else {
            return Task::none();
        };

        // Editing one instance of a series is ambiguous until the user says
        // how far the edit reaches — so ask, and come back through
        // `save_with_scope`. Overrides are unambiguous (they *are* one
        // instance), as is anything opened outside a series.
        if editor.occurrence.is_some() && !editor.is_override() {
            self.scope_prompt = Some(ScopePrompt::Save);
            return Task::none();
        }
        let Some(local) = self.store.as_ref().map(Store::local_timezone) else {
            return Task::none();
        };

        let event = match editor.to_event(local) {
            Ok(event) => event,
            Err(why) => {
                // A validation failure belongs next to the fields, not in a toast.
                self.with_editor(|e| e.error = Some(why.clone()));
                if let Some(e) = self.editor.as_mut() {
                    e.error = Some(why);
                }
                return Task::none();
            }
        };

        // Moving an event between calendars means deleting the old file, not
        // just writing a new one.
        let moved_from = editor
            .original
            .as_ref()
            .filter(|original| original.calendar_id != event.calendar_id)
            .cloned();

        let Some(store) = self.store.as_mut() else {
            return Task::none();
        };

        let base = writeback_base(store, &event.calendar_id, &event.file_name);
        let moved_base = moved_from
            .as_ref()
            .and_then(|original| writeback_base(store, &original.calendar_id, &original.file_name));
        let mut journal = vec![self.journal_before(&event.calendar_id, &event.file_name)];
        if let Some(original) = &moved_from {
            journal.push(self.journal_before(&original.calendar_id, &original.file_name));
        }

        let Some(store) = self.store.as_mut() else {
            return Task::none();
        };
        let result = match &moved_from {
            Some(original) => store
                .move_to_calendar(original, &event.calendar_id)
                .and_then(|_| store.save(&event)),
            None => store.save(&event),
        };

        match result {
            Ok(()) => {
                let mut queued =
                    queue_writeback_file(&event.calendar_id, &event.file_name, base.as_deref());
                // The old calendar's server still holds it; without this the
                // next sync brings it back as a duplicate.
                if let Some(original) = &moved_from {
                    queued = queued.and(queue_writeback_removal(
                        store,
                        &original.calendar_id,
                        &original.file_name,
                        moved_base.as_deref(),
                    ));
                }
                self.journal_finish(journal);
                self.editor = None;
                self.core.window.show_context = false;
                self.reload();
                self.report_unqueued(queued)
            }
            Err(why) => self.toast_error(&format!("{}: {why}", fl!("error-save-event"))),
        }
    }

    /// Applies the edited fields at the chosen scope, after the prompt.
    fn save_with_scope(&mut self, scope: EditScope) -> Task<cosmic::Action<Message>> {
        let Some(editor) = self.editor.as_ref() else {
            return Task::none();
        };
        let (Some(instant), Some(master)) = (editor.occurrence, editor.original.clone()) else {
            return Task::none();
        };
        let Some(local) = self.store.as_ref().map(Store::local_timezone) else {
            return Task::none();
        };

        let edited = match editor.to_event(local) {
            Ok(event) => event,
            Err(why) => {
                self.with_editor(|e| e.error = Some(why));
                return Task::none();
            }
        };

        // "This and following" does not fit the build-one-event-then-save
        // shape below: the substrate splits the series first, and the edit
        // lands on the successor it hands back.
        if matches!(scope, EditScope::Following) {
            return self.save_following(&master, instant, edited);
        }

        let event = match scope {
            // One instance: an override component in the master's file. It
            // has no rule of its own and no exclusions — those belong to the
            // series — and it names the instance it replaces.
            EditScope::This => {
                let mut over = edited;
                over.rrule = None;
                over.exdates = Vec::new();
                over.recurrence_id = Some(crate::model::rid_for(master.start, instant, local));
                // An override lives in its master's file; a calendar change in
                // the editor cannot apply to one instance.
                over.calendar_id = master.calendar_id.clone();
                over.file_name = master.file_name.clone();
                over
            }

            EditScope::All => whole_series_edit(&master, edited, instant, local),

            EditScope::Following => unreachable!("handled above"),
        };

        let Some(store) = self.store.as_mut() else {
            return Task::none();
        };

        // A whole-series edit may also have moved the event to another
        // calendar, which means removing the old file as well.
        let base = writeback_base(store, &event.calendar_id, &event.file_name);
        let moved = event.calendar_id != master.calendar_id;
        let master_base = writeback_base(store, &master.calendar_id, &master.file_name);
        let mut journal = vec![self.journal_before(&event.calendar_id, &event.file_name)];
        if moved {
            journal.push(self.journal_before(&master.calendar_id, &master.file_name));
        }

        let Some(store) = self.store.as_mut() else {
            return Task::none();
        };
        let result = write_series_edit(store, &master, &event, matches!(scope, EditScope::All));

        match result {
            Ok(()) => {
                let mut queued =
                    queue_writeback_file(&event.calendar_id, &event.file_name, base.as_deref());
                if moved {
                    queued = queued.and(queue_writeback_removal(
                        store,
                        &master.calendar_id,
                        &master.file_name,
                        master_base.as_deref(),
                    ));
                }
                self.journal_finish(journal);
                self.editor = None;
                self.core.window.show_context = false;
                self.reload();
                self.report_unqueued(queued)
            }
            Err(why) => self.toast_error(&format!("{}: {why}", fl!("error-save-event"))),
        }
    }

    /// "This and following": split the series at the clicked instance and
    /// apply the edited fields to the successor the split produced.
    fn save_following(
        &mut self,
        master: &crate::model::Event,
        instant: chrono::DateTime<chrono::Utc>,
        edited: crate::model::Event,
    ) -> Task<cosmic::Action<Message>> {
        let Some(store) = self.store.as_mut() else {
            return Task::none();
        };

        // The master's pre-split bytes: the base for the truncated master's
        // writeback merge. The successor is a new file and has no base.
        let master_base = writeback_base(store, &master.calendar_id, &master.file_name);

        let successor = match store.split_series(&master.calendar_id, &master.uid, instant) {
            Ok(crate::store::SplitOutcome::Split(successor)) => successor,
            // The cut fell on the first instance: nothing precedes it, so
            // "this and following" is the whole series.
            Ok(crate::store::SplitOutcome::WholeSeries) => {
                return self.save_with_scope(EditScope::All);
            }
            Err(why) => return self.toast_error(&format!("{}: {why}", fl!("error-save-event"))),
        };

        // The editor's dates showed the clicked instance, which is exactly
        // where the successor anchors — the edited fields apply directly.
        let mut series = edited;
        series.uid = successor.uid.clone();
        series.file_name = successor.file_name.clone();
        // A recurrence the user did not touch means the successor's own rule:
        // the split already reduced its COUNT by the instances the old master
        // keeps, and re-imposing the master's rule would undo that.
        if series.rrule == master.rrule
            || (series.recurrence().is_some() && series.recurrence() == master.recurrence())
        {
            series.rrule = successor.rrule.clone();
        }
        // Exclusions are not editable here; the split already kept the ones
        // that fall on the successor's side of the cut.
        series.exdates = successor.exdates.clone();
        series.sequence = successor.sequence;
        series.created = successor.created;
        series.last_modified = successor.last_modified;

        // A calendar change in the editor moves the successor only — the past
        // instances stay where the series lived.
        let result = if series.calendar_id == master.calendar_id {
            store.save(&series)
        } else {
            store
                .move_to_calendar(&successor, &series.calendar_id)
                .and_then(|_| store.save(&series))
        };

        // The split landed whatever happens to the edit on top of it: the
        // truncated master and the successor are both on disk, so both go to
        // the server and both undo as one step — a failed save of the edit
        // below must not leave a split the server never hears of and undo
        // cannot reach.
        let mut queued = queue_writeback_file(
            &master.calendar_id,
            &master.file_name,
            master_base.as_deref(),
        );
        // The successor, wherever it landed: the series' calendar, or the one
        // the editor moved it to.
        let mut journal = vec![
            crate::undo::Entry {
                calendar_id: master.calendar_id.clone(),
                file_name: master.file_name.clone(),
                before: master_base,
                after: None,
            },
            crate::undo::Entry {
                calendar_id: master.calendar_id.clone(),
                file_name: successor.file_name.clone(),
                before: None,
                after: None,
            },
        ];
        if series.calendar_id != master.calendar_id {
            journal.push(crate::undo::Entry {
                calendar_id: series.calendar_id.clone(),
                file_name: series.file_name.clone(),
                before: None,
                after: None,
            });
        }
        if let Some(store) = self.store.as_ref() {
            for entry in &journal[1..] {
                queued = queued.and(queue_writeback_removal(
                    store,
                    &entry.calendar_id,
                    &entry.file_name,
                    None,
                ));
            }
        }
        self.journal_finish(journal);
        self.reload();

        match result {
            Ok(()) => {
                self.editor = None;
                self.core.window.show_context = false;
                self.report_unqueued(queued)
            }
            Err(why) => {
                let failed = self.toast_error(&format!("{}: {why}", fl!("error-save-event")));
                Task::batch([failed, self.report_unqueued(queued)])
            }
        }
    }

    /// Deletes at the chosen scope, after the prompt.
    fn delete_with_scope(&mut self, scope: EditScope) -> Task<cosmic::Action<Message>> {
        let Some(editor) = self.editor.as_ref() else {
            return Task::none();
        };
        let (Some(instant), Some(master)) = (editor.occurrence, editor.original.clone()) else {
            return Task::none();
        };
        let Some(store) = self.store.as_mut() else {
            return Task::none();
        };

        let base = writeback_base(store, &master.calendar_id, &master.file_name);
        let journal = vec![self.journal_before(&master.calendar_id, &master.file_name)];
        let Some(store) = self.store.as_mut() else {
            return Task::none();
        };
        let result = match scope {
            EditScope::This => store
                .exclude_occurrence(&master.calendar_id, &master.uid, instant)
                .map(|()| false),
            EditScope::Following => {
                store.truncate_series(&master.calendar_id, &master.uid, instant)
            }
            EditScope::All => store
                .delete(&master.calendar_id, &master.uid)
                .map(|()| true),
        };

        match result {
            Ok(deleted_whole) => {
                let queued = if deleted_whole {
                    // Gone locally; tell the server too — or, if the file
                    // held other records, send what is left of it.
                    self.store.as_ref().map_or(Ok(()), |store| {
                        queue_writeback_removal(
                            store,
                            &master.calendar_id,
                            &master.file_name,
                            base.as_deref(),
                        )
                    })
                } else {
                    // The master changed (EXDATE or UNTIL); push the new revision.
                    queue_writeback_file(&master.calendar_id, &master.file_name, base.as_deref())
                };
                self.journal_finish(journal);
                self.editor = None;
                self.core.window.show_context = false;
                self.reload();
                self.report_unqueued(queued)
            }
            Err(why) => self.toast_error(&format!("{}: {why}", fl!("error-delete-event"))),
        }
    }

    fn delete_editor(&mut self) -> Task<cosmic::Action<Message>> {
        if self
            .editor
            .as_ref()
            .is_some_and(|e| e.occurrence.is_some() && !e.is_override())
        {
            self.scope_prompt = Some(ScopePrompt::Delete);
            return Task::none();
        }

        let Some(original) = self
            .editor
            .as_ref()
            .and_then(|e| e.original.as_ref())
            .cloned()
        else {
            return Task::none();
        };

        let journal = vec![self.journal_before(&original.calendar_id, &original.file_name)];
        let Some(store) = self.store.as_mut() else {
            return Task::none();
        };
        let base = writeback_base(store, &original.calendar_id, &original.file_name);

        // Deleting an override removes just that component — the series and
        // its master stay, and the master's generated instance returns.
        // Deleting anything else removes the file, series included.
        let result = if original.recurrence_id.is_some() {
            store.delete_override(&original)
        } else {
            store.delete(&original.calendar_id, &original.uid)
        };

        match result {
            Ok(()) => {
                // After the local change, and shaped by it: an override's
                // removal leaves its series in the file, which must be sent
                // as it now is — deleting the resource would delete the
                // whole series on the server.
                let queued = queue_writeback_removal(
                    store,
                    &original.calendar_id,
                    &original.file_name,
                    base.as_deref(),
                );
                self.journal_finish(journal);
                self.editor = None;
                self.core.window.show_context = false;
                self.reload();
                self.report_unqueued(queued)
            }
            Err(why) => self.toast_error(&format!("{}: {why}", fl!("error-delete-event"))),
        }
    }

    /// Imports a `.ics` file into the default calendar.
    fn import(&mut self, path: &std::path::Path) -> Task<cosmic::Action<Message>> {
        let Some(calendar_id) = self
            .store
            .as_ref()
            .and_then(Store::default_calendar)
            .map(|c| c.id.clone())
        else {
            return self.toast_error(&fl!("no-calendars"));
        };

        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(why) => return self.toast_error(&format!("{}: {why}", file_label(path))),
        };

        let Some(store) = self.store.as_mut() else {
            return Task::none();
        };

        match store.import_ics(&text, &calendar_id) {
            Ok(summary) if summary.total() == 0 => {
                self.toast_error(&fl!("import-empty", path = file_label(path)))
            }
            Ok(summary) => {
                // Imported events are local changes like any other: into a
                // CalDAV-bound calendar they must be queued, or the server
                // never sees them and the next sync treats them as strays.
                let queued = summary
                    .files
                    .iter()
                    .map(|file| queue_writeback_file(&calendar_id, file, None))
                    .fold(Ok(()), Result::and);
                self.reload();
                let done = self.toast(&fl!(
                    "import-done",
                    added = summary.added.to_string(),
                    updated = summary.updated.to_string()
                ));
                Task::batch([done, self.report_unqueued(queued)])
            }
            Err(why) => self.toast_error(&why.to_string()),
        }
    }

    /// Tells the user when a change saved on this computer will not reach
    /// its server. See [`Queued`].
    fn report_unqueued(&mut self, queued: Queued) -> Task<cosmic::Action<Message>> {
        match queued {
            Ok(()) => Task::none(),
            Err(why) => self.toast_error(&fl!("error-not-queued", reason = why)),
        }
    }

    fn toast(&mut self, message: &str) -> Task<cosmic::Action<Message>> {
        self.toasts
            .push(widget::Toast::new(message.to_owned()))
            .map(cosmic::Action::App)
    }

    fn toast_error(&mut self, message: &str) -> Task<cosmic::Action<Message>> {
        tracing::error!(message);
        self.toasts
            .push(widget::Toast::new(message.to_owned()))
            .map(cosmic::Action::App)
    }

    /// A toast for something that went right — same surface, no error log.
    fn toast_info(&mut self, message: &str) -> Task<cosmic::Action<Message>> {
        self.toasts
            .push(widget::Toast::new(message.to_owned()))
            .map(cosmic::Action::App)
    }

    /// Applies a hand-over request from a second launch or a desktop action.
    ///
    /// Deliberately additive: a request carrying a date *and* files to import
    /// does both, in the order a person would expect — move to the day, then
    /// pull the files in — rather than picking one and dropping the rest.
    fn apply_task(&mut self, task: &SlateTask, args: &[String]) -> Task<cosmic::Action<Message>> {
        let mut tasks = Vec::new();

        if let Some(date) = task.date {
            self.anchor = date;
            self.today = self.clock_now().date();
            self.sync_mini();
            self.reload();
            tasks.push(Self::request_scroll());
        }

        for arg in args {
            tasks.push(cosmic::task::message(cosmic::Action::App(
                Message::ImportPath(PathBuf::from(arg)),
            )));
        }

        if task.new_event {
            tasks.push(cosmic::task::message(cosmic::Action::App(
                Message::NewEvent,
            )));
        }

        Task::batch(tasks)
    }

    /// Folds libcosmic's shown/hidden nav-bar state and the user's width
    /// choice into the sidebar's mode. Idempotent; called wherever either
    /// input can change.
    fn sync_sidebar(&mut self) {
        let mode = if !self.core.nav_bar_active() {
            SidebarMode::Hidden
        } else if self.sidebar_rail {
            SidebarMode::Rail
        } else {
            SidebarMode::Expanded
        };
        self.sidebar.set_mode(mode);
    }

    fn persist_config(&mut self) {
        let Some(handler) = self.config_handler.as_ref() else {
            return;
        };
        if let Err(why) = self.config.write_entry(handler) {
            tracing::warn!(%why, "could not save configuration");
        }
    }

    fn update_title(&mut self) -> Task<cosmic::Action<Message>> {
        let title = fl!("app-title");
        if let Some(id) = self.core.main_window_id() {
            self.set_window_title(title, id)
        } else {
            Task::none()
        }
    }
}

/// A path's file name, for user-facing messages.
fn file_label(path: &std::path::Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |n| n.to_string_lossy().into(),
    )
}

/// A header-bar icon button, built the way libcosmic builds its own.
///
/// The three sizing pieces all matter, and getting them wrong is what made the "+"
/// render as half a glyph: `icon_size` sizes the icon *within* the button
/// (`icon::size()` on the handle does not, and leaves the natural size to
/// overflow and clip), `padding(8)` gives it the header's button box, and the
/// `HeaderBar` class matches the window controls beside it.
fn header_icon(name: &'static str, label: String, on_press: Message) -> Element<'static, Message> {
    let button = widget::icon::from_name(name)
        .apply(widget::button::icon)
        .icon_size(16)
        .padding(8)
        .class(cosmic::theme::Button::HeaderBar)
        .on_press(on_press);

    // The tooltip is the button's only name: a glyph on its own tells a
    // screen reader nothing, and tells a new user very little.
    widget::tooltip(
        button,
        widget::text::caption(label),
        widget::tooltip::Position::Bottom,
    )
    .into()
}

/// Width allotted to one tab of the view selector.
/// Width allotted to one tab of the view selector.
///
/// Wide enough for the longest label plus the checkmark the active tab
/// prefixes; the whole strip is this times the number of views.
const VIEW_TAB_WIDTH: f32 = 96.0;

fn view_label(kind: ViewKind) -> String {
    match kind {
        ViewKind::Month => fl!("month"),
        ViewKind::Week => fl!("week"),
        ViewKind::Day => fl!("day"),
        ViewKind::Agenda => fl!("agenda"),
        ViewKind::Year => fl!("year"),
        ViewKind::Tasks => fl!("tasks"),
    }
}

/// Adds `delta` calendar months, clamping the day to the target month's length
/// so 31 January + 1 lands on 28/29 February rather than failing.
fn add_months(date: NaiveDate, delta: i64) -> NaiveDate {
    let months = i64::from(date.year()) * 12 + i64::from(date.month0()) + delta;
    let year = months.div_euclid(12);
    let month0 = months.rem_euclid(12);

    let year = i32::try_from(year).unwrap_or(date.year());
    let month = u32::try_from(month0).unwrap_or(0) + 1;

    let last_day = days_in_month(year, month);
    NaiveDate::from_ymd_opt(year, month, date.day().min(last_day)).unwrap_or(date)
}

fn days_in_month(year: i32, month: u32) -> u32 {
    let (next_year, next_month) = if month == 12 {
        (year + 1, 1)
    } else {
        (year, month + 1)
    };
    NaiveDate::from_ymd_opt(next_year, next_month, 1)
        .and_then(|d| d.pred_opt())
        .map_or(28, |d| d.day())
}

/// Watches the calendar directory, forwarding one message per settled burst.
///
/// The watcher is created inside the stream so its lifetime is tied to the
/// subscription: iced drops the stream when the subscription goes away, which
/// tears the watch down with it.
fn file_watch_subscription() -> Subscription<Message> {
    Subscription::run(|| {
        cosmic::iced::stream::channel(
            1,
            |mut output: futures::channel::mpsc::Sender<_>| async move {
                let root = crate::store::vdir::default_root();

                match crate::store::watcher::watch(&root) {
                    Ok((_watch, mut rx)) => {
                        while rx.recv().await.is_some() {
                            if output.send(Message::FilesChanged).await.is_err() {
                                break;
                            }
                        }
                    }
                    Err(why) => {
                        tracing::warn!(%why, "cannot watch the calendar directory; external changes will need a restart");
                        // Park forever rather than returning: a finished stream would
                        // make iced restart the subscription in a tight loop.
                        std::future::pending::<()>().await;
                    }
                }
            },
        )
    })
}

/// Follows who owns reminders for as long as the app runs.
///
/// The daemon can start after the app (enabled, or restarted by systemd) or
/// stop while it is open; checking once at start-up left the app firing
/// alongside a new daemon, or silent after the daemon exited. When the bus is
/// unreachable the app fires reminders itself: a calendar with no reminders
/// is worse than one that occasionally shows a duplicate.
fn owner_subscription() -> Subscription<Message> {
    Subscription::run(|| {
        cosmic::iced::stream::channel(
            4,
            |mut output: futures::channel::mpsc::Sender<_>| async move {
                let (tx, mut rx) = futures::channel::mpsc::unbounded();
                let fallback = tx.clone();
                let watch = async move {
                    let followed = match zbus::Connection::session().await {
                        Ok(connection) => {
                            crate::reminders::watch_owner(&connection, move |owned| {
                                let _ = tx.unbounded_send(owned);
                            })
                            .await
                        }
                        Err(why) => Err(why),
                    };
                    if let Err(why) = followed {
                        tracing::warn!(%why, "cannot follow the reminder daemon; firing reminders here");
                    }
                    let _ = fallback.unbounded_send(false);
                };

                let pump = async move {
                    use futures::StreamExt;
                    while let Some(owned) = rx.next().await {
                        if output
                            .send(Message::ReminderOwnership(owned))
                            .await
                            .is_err()
                        {
                            break;
                        }
                    }
                };

                futures::future::join(watch, pump).await;
                // A stream that ended would make iced restart the subscription
                // in a tight loop.
                std::future::pending::<()>().await;
            },
        )
    })
}

/// Reports each time the machine goes to sleep and comes back.
///
/// Modelled on [`file_watch_subscription`], including the park-on-failure: a
/// stream that ended would make iced restart the subscription in a tight loop.
fn sleep_subscription() -> Subscription<Message> {
    Subscription::run(|| {
        cosmic::iced::stream::channel(
            1,
            |mut output: futures::channel::mpsc::Sender<_>| async move {
                let (tx, mut rx) = futures::channel::mpsc::unbounded();
                let watch = crate::reminders::on_sleep(move |going_down| {
                    let _ = tx.unbounded_send(going_down);
                });

                let pump = async move {
                    use futures::StreamExt;
                    while let Some(going_down) = rx.next().await {
                        if output.send(Message::Sleep(going_down)).await.is_err() {
                            break;
                        }
                    }
                };

                futures::future::join(watch, pump).await;
                // login1 is unreachable (a container, a non-systemd host): no
                // digests, which is the behaviour before this existed.
                std::future::pending::<()>().await;
            },
        )
    })
}

/// Whether a saved change made it into its server's upload queue; the error
/// is what the user is told.
///
/// The queue is the only road a local edit has to the server, and nothing
/// retries an entry that never got into it: a failure here means the change
/// stays on this computer, so it is reported rather than logged.
type Queued = Result<(), String>;

/// Queues a written file for upload to its CalDAV server, if it has one.
///
/// `base` is the file's bytes from *before* this session's edit; with it, the
/// push queue can three-way-merge a concurrent server-side change instead of
/// recording a conflict. `None` (new file, or the read failed) degrades to
/// the plain no-merge queue.
fn queue_writeback_file(calendar_id: &str, file_name: &str, base: Option<&str>) -> Queued {
    let root = crate::store::vdir::default_root();
    cosmic_pim_sync::queue_save_with_base(&root, calendar_id, file_name, base)
        .map(|_| ())
        .map_err(|why| {
            tracing::warn!(calendar = calendar_id, file = file_name, %why, "could not queue the change for upload");
            why.to_string()
        })
}

/// Queues a removed file's deletion on its CalDAV server, if it had one.
fn queue_writeback_delete_file(calendar_id: &str, file_name: &str) -> Queued {
    let root = crate::store::vdir::default_root();
    cosmic_pim_sync::queue_delete(&root, calendar_id, file_name)
        .map(|_| ())
        .map_err(|why| {
            tracing::warn!(calendar = calendar_id, file = file_name, %why, "could not queue the deletion for upload");
            why.to_string()
        })
}

/// Reads every contact from the suite's address books, for birthday display.
///
/// Missing or unreadable books mean no birthdays, not an error — the address
/// book is Circle's to manage; Slate only looks at it.
fn load_contact_cards() -> Vec<crate::model::Contact> {
    let root = crate::store::contacts::default_root();
    crate::store::contacts::books(&root)
        .iter()
        .flat_map(crate::store::contacts::read_book)
        .collect()
}

/// The event file's bytes as they are right now — captured *before* a save,
/// they are the base a three-way writeback merge needs.
fn writeback_base(store: &Store, calendar_id: &str, file_name: &str) -> Option<String> {
    let meta = store.calendar(calendar_id)?;
    std::fs::read_to_string(meta.path.join(file_name)).ok()
}

/// The same wall-clock value, moved by `delta`, keeping the time's kind.
fn shift_time(t: crate::model::EventTime, delta: chrono::Duration) -> crate::model::EventTime {
    use crate::model::EventTime;
    match t {
        EventTime::Date(d) => EventTime::Date(d + chrono::Duration::days(delta.num_days())),
        EventTime::Floating(dt) => EventTime::Floating(dt + delta),
        EventTime::Zoned(dt, tz) => EventTime::Zoned(dt + delta, tz),
    }
}

/// The series an "all events" edit writes, from the fields the editor showed
/// for one clicked instance of it.
///
/// The editor's dates showed the *clicked instance*, so what the user
/// expressed is a change to that instance; the series takes the same change
/// from its own first instance:
///
/// - **Same kind** (timed stays timed, all-day stays all-day): the wall-clock
///   shift between the instance as it was and as edited moves the series'
///   start in its own zone, and the edited length sets its end.
/// - **Kind changed** (the all-day switch was flipped): the edited instance's
///   own start and end — its kind, zone and time — move back to the series'
///   first date. Shifting the old start would keep the old kind and write a
///   24-hour timed block, or an all-day `DTEND` equal to its `DTSTART`.
///
/// `EXDATE`s live in the series' own value space, so they move by exactly the
/// amount the start moved — otherwise every exclusion stops matching the
/// instance it removed, and deleted occurrences come back.
fn whole_series_edit(
    master: &crate::model::Event,
    edited: crate::model::Event,
    instant: chrono::DateTime<chrono::Utc>,
    local: chrono_tz::Tz,
) -> crate::model::Event {
    let occurrence_start = instant.with_timezone(&local).naive_local();
    let edited_start = edited.start.naive_local(local);

    let mut series = edited;
    if series.start.is_all_day() == master.start.is_all_day() {
        let shift = edited_start - occurrence_start;
        let length = series.end.naive_local(local) - edited_start;
        series.start = shift_time(master.start, shift);
        series.end = shift_time(series.start, length);
    } else {
        let back = Duration::days(
            (occurrence_start.date() - master.start.naive_local(local).date()).num_days(),
        );
        series.start = shift_time(series.start, -back);
        series.end = shift_time(series.end, -back);
    }

    let moved = own_wall_clock(series.start) - own_wall_clock(master.start);
    series.exdates = master
        .exdates
        .iter()
        .map(|exdate| *exdate + moved)
        .collect();
    series
}

/// Writes a scoped edit of `master`'s series: `event` is the new master when
/// `whole`, one override otherwise. A calendar change moves the series first.
///
/// A whole-series save goes through `Store::save_series`, which moves each
/// override's `RECURRENCE-ID` with the series. Saving the master alone left
/// every override naming an instance the series no longer generates: it
/// showed beside the regenerated one and could not be reached as "this
/// event" (audit F-03).
fn write_series_edit(
    store: &mut Store,
    master: &crate::model::Event,
    event: &crate::model::Event,
    whole: bool,
) -> Result<(), crate::store::StoreError> {
    let previous = if event.calendar_id == master.calendar_id {
        master.clone()
    } else {
        store.move_to_calendar(master, &event.calendar_id)?
    };
    if whole {
        store.save_series(&previous, event)
    } else {
        store.save(event)
    }
}

/// A time's value in its own frame — the space `EXDATE`s are written in.
fn own_wall_clock(t: crate::model::EventTime) -> NaiveDateTime {
    use crate::model::EventTime;
    match t {
        EventTime::Date(d) => d.and_time(NaiveTime::MIN),
        EventTime::Floating(dt) | EventTime::Zoned(dt, _) => dt,
    }
}

/// What the server needs after a record left a file.
#[derive(Debug, PartialEq, Eq)]
enum Removal {
    /// The file is gone: delete the resource.
    Delete,
    /// Other components still live in the file — a removed override leaves
    /// its series, a removed task can leave its siblings — so the resource
    /// is rewritten, not deleted.
    Rewrite,
}

/// Which [`Removal`] a record's departure from `file` calls for, judged from
/// what the local change left on disk.
fn removal_after(file: &std::path::Path) -> Removal {
    if file.exists() {
        Removal::Rewrite
    } else {
        Removal::Delete
    }
}

/// Queues what the server needs after a record left `file_name` in
/// `calendar_id` — deleted, or moved to another calendar. `base` is the
/// file's bytes before the change, for the writeback merge.
fn queue_writeback_removal(
    store: &Store,
    calendar_id: &str,
    file_name: &str,
    base: Option<&str>,
) -> Queued {
    let Some(meta) = store.calendar(calendar_id) else {
        return Ok(());
    };
    match removal_after(&meta.path.join(file_name)) {
        Removal::Delete => queue_writeback_delete_file(calendar_id, file_name),
        Removal::Rewrite => queue_writeback_file(calendar_id, file_name, base),
    }
}

/// Whether an `http://` URL points at this machine, where plaintext is a
/// deliberate local-testing choice rather than a credential leak.
fn is_loopback(url: &str) -> bool {
    let host = url
        .trim_start_matches("http://")
        .split(['/', ':'])
        .next()
        .unwrap_or_default();
    host == "localhost" || host == "127.0.0.1" || host == "::1"
}

#[cfg(test)]
mod tests {
    use super::*;

    fn day(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    fn at(y: i32, m: u32, d: u32, h: u32, min: u32) -> NaiveDateTime {
        day(y, m, d).and_hms_opt(h, min, 0).unwrap()
    }

    #[test]
    fn snapping_rounds_to_the_nearest_step() {
        assert_eq!(snap_to(at(2026, 8, 4, 9, 7), 15), at(2026, 8, 4, 9, 0));
        assert_eq!(snap_to(at(2026, 8, 4, 9, 8), 15), at(2026, 8, 4, 9, 15));
        assert_eq!(snap_to(at(2026, 8, 4, 9, 7), 5), at(2026, 8, 4, 9, 5));
        assert_eq!(snap_to(at(2026, 8, 4, 9, 7), 10), at(2026, 8, 4, 9, 10));
    }

    #[test]
    fn snapping_stays_inside_the_day_it_started_in() {
        // Rounding up from 23:58 must not roll the date forward, or a drag at
        // the bottom of the column would silently move the event to tomorrow.
        let snapped = snap_to(at(2026, 8, 4, 23, 58), 15);
        assert_eq!(snapped.date(), day(2026, 8, 4));
        assert!(snapped.time() < chrono::NaiveTime::from_hms_opt(23, 59, 59).unwrap());

        assert_eq!(snap_to(at(2026, 8, 4, 0, 1), 15), at(2026, 8, 4, 0, 0));
    }

    #[test]
    fn a_drag_that_did_not_move_paints_no_ghost() {
        // The ghost is the promise that something will change on release; a
        // click that never moved must not make that promise.
        let drag = GridDrag::Create {
            anchor: at(2026, 8, 4, 9, 0),
            current: at(2026, 8, 4, 9, 0),
            moved: false,
        };
        assert!(ghost_span(&drag, at(2026, 8, 4, 10, 0), 15).is_none());
    }

    #[test]
    fn sweeping_upwards_still_produces_a_forward_span() {
        let drag = GridDrag::Create {
            anchor: at(2026, 8, 4, 11, 0),
            current: at(2026, 8, 4, 9, 0),
            moved: true,
        };
        let (from, to) = ghost_span(&drag, at(2026, 8, 4, 9, 0), 15).expect("a moved drag");
        assert_eq!((from, to), (at(2026, 8, 4, 9, 0), at(2026, 8, 4, 11, 0)));
    }

    #[test]
    fn a_resize_never_collapses_the_event() {
        // Dragging the bottom edge above the start would invert the event;
        // it is clamped to one snap step instead.
        let drag = GridDrag::Block {
            block: GridBlockRef {
                calendar_id: "personal".into(),
                uid: "u".into(),
                rid: None,
                start: at(2026, 8, 4, 10, 0),
                end: at(2026, 8, 4, 11, 0),
                resize: true,
            },
            pressed_at: at(2026, 8, 4, 11, 0),
            moved: true,
        };
        let (from, to) = ghost_span(&drag, at(2026, 8, 4, 8, 0), 15).expect("a moved drag");
        assert_eq!(from, at(2026, 8, 4, 10, 0), "resize moved the start");
        assert!(to > from, "the event collapsed or inverted");
        assert_eq!(to, at(2026, 8, 4, 10, 15));
    }

    #[test]
    fn moving_a_block_keeps_its_duration_and_the_grab_point() {
        // Grabbed 30 minutes into the event, dropped two hours later.
        let drag = GridDrag::Block {
            block: GridBlockRef {
                calendar_id: "personal".into(),
                uid: "u".into(),
                rid: None,
                start: at(2026, 8, 4, 10, 0),
                end: at(2026, 8, 4, 11, 0),
                resize: false,
            },
            pressed_at: at(2026, 8, 4, 10, 30),
            moved: true,
        };
        let (from, to) = ghost_span(&drag, at(2026, 8, 4, 12, 30), 15).expect("a moved drag");
        assert_eq!(from, at(2026, 8, 4, 12, 0), "the grab point was not held");
        assert_eq!(
            to - from,
            Duration::hours(1),
            "duration changed while moving"
        );
    }

    fn busy(start_ms: i64, end_ms: i64) -> cosmic_pim_caldav::itip::BusyPeriod {
        cosmic_pim_caldav::itip::BusyPeriod {
            start_ms,
            end_ms,
            kind: "BUSY".into(),
        }
    }

    #[test]
    fn a_meeting_that_merely_touches_the_slot_does_not_clash() {
        // 09:00–10:00 against a slot of 10:00–11:00.
        let slot = (10_000, 11_000);
        assert!(!overlaps_slot(&[busy(9_000, 10_000)], slot.0, slot.1));
        assert!(!overlaps_slot(&[busy(11_000, 12_000)], slot.0, slot.1));
    }

    #[test]
    fn a_real_overlap_clashes() {
        let slot = (10_000, 11_000);
        assert!(overlaps_slot(&[busy(10_500, 12_000)], slot.0, slot.1));
        assert!(overlaps_slot(&[busy(9_000, 10_500)], slot.0, slot.1));
        // Wholly containing the slot, and wholly inside it.
        assert!(overlaps_slot(&[busy(8_000, 13_000)], slot.0, slot.1));
        assert!(overlaps_slot(&[busy(10_200, 10_800)], slot.0, slot.1));
    }

    #[test]
    fn no_busy_periods_is_free() {
        assert!(!overlaps_slot(&[], 10_000, 11_000));
    }

    #[test]
    fn invite_answers_spell_partstat_as_rfc_5545_does() {
        // These strings go verbatim into a REPLY another client parses;
        // a typo here is an interop bug, not a cosmetic one.
        assert_eq!(InviteAnswer::Accepted.partstat(), "ACCEPTED");
        assert_eq!(InviteAnswer::Tentative.partstat(), "TENTATIVE");
        assert_eq!(InviteAnswer::Declined.partstat(), "DECLINED");
    }

    #[test]
    fn month_stepping_wraps_the_year() {
        assert_eq!(add_months(day(2026, 12, 15), 1), day(2027, 1, 15));
        assert_eq!(add_months(day(2026, 1, 15), -1), day(2025, 12, 15));
    }

    #[test]
    fn month_stepping_clamps_short_months() {
        // 31 Jan + 1 month has no 31st to land on.
        assert_eq!(add_months(day(2026, 1, 31), 1), day(2026, 2, 28));
        assert_eq!(add_months(day(2028, 1, 31), 1), day(2028, 2, 29));
        assert_eq!(add_months(day(2026, 3, 31), -1), day(2026, 2, 28));
    }

    #[test]
    fn month_stepping_is_reversible_for_safe_days() {
        let d = day(2026, 8, 15);
        for delta in 1..=24 {
            assert_eq!(add_months(add_months(d, delta), -delta), d);
        }
    }

    /// A weekly series from Tuesday 4 Aug 2026, with the 11 Aug instance
    /// deleted, in Athens — the viewer's own zone.
    fn weekly_series(
        start: crate::model::EventTime,
        end: crate::model::EventTime,
    ) -> crate::model::Event {
        let mut master =
            crate::model::Event::draft("personal", at(2026, 8, 4, 9, 0), chrono_tz::Europe::Athens);
        master.summary = "Standup".into();
        master.start = start;
        master.end = end;
        master.rrule = Some("FREQ=WEEKLY".into());
        master.exdates = vec![own_wall_clock(start) + Duration::days(7)];
        master
    }

    /// The instant of the 18 Aug instance, as a grid click hands it over.
    fn clicked(wall: NaiveDateTime) -> chrono::DateTime<chrono::Utc> {
        use chrono::TimeZone;
        chrono_tz::Europe::Athens
            .from_local_datetime(&wall)
            .unwrap()
            .with_timezone(&chrono::Utc)
    }

    #[test]
    fn moving_every_instance_keeps_the_deleted_one_deleted() {
        use crate::model::EventTime;
        let athens = chrono_tz::Europe::Athens;
        let master = weekly_series(
            EventTime::Zoned(at(2026, 8, 4, 9, 0), athens),
            EventTime::Zoned(at(2026, 8, 4, 10, 0), athens),
        );
        let mut edited = master.clone();
        edited.start = EventTime::Zoned(at(2026, 8, 18, 10, 0), athens);
        edited.end = EventTime::Zoned(at(2026, 8, 18, 11, 0), athens);

        let series = whole_series_edit(&master, edited, clicked(at(2026, 8, 18, 9, 0)), athens);

        assert_eq!(
            series.start,
            EventTime::Zoned(at(2026, 8, 4, 10, 0), athens)
        );
        assert_eq!(series.end, EventTime::Zoned(at(2026, 8, 4, 11, 0), athens));
        assert_eq!(
            series.exdates,
            vec![at(2026, 8, 11, 10, 0)],
            "the exclusion no longer matches the instance it deleted"
        );
    }

    #[test]
    fn moving_every_instance_takes_the_changed_ones_along() {
        use crate::model::EventTime;
        use chrono::TimeZone;
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(
            &dir.path().join("calendars"),
            &dir.path().join("index.sqlite"),
        )
        .unwrap();
        let calendar = store.create_calendar("Work", PALETTE[0]).unwrap();
        let local = store.local_timezone();

        // Weekly at 09:00 from Tue 4 Aug; the 11 Aug instance renamed.
        let mut master = crate::model::Event::draft(&calendar.id, at(2026, 8, 4, 9, 0), local);
        master.summary = "Standup".into();
        master.rrule = Some("FREQ=WEEKLY".into());
        store.save(&master).unwrap();
        let mut renamed = master.clone();
        renamed.rrule = None;
        renamed.summary = "Standup (demo)".into();
        renamed.recurrence_id = Some(EventTime::Zoned(at(2026, 8, 11, 9, 0), local));
        renamed.start = EventTime::Zoned(at(2026, 8, 11, 9, 0), local);
        renamed.end = EventTime::Zoned(at(2026, 8, 11, 10, 0), local);
        store.save(&renamed).unwrap();
        let master = store.event(&calendar.id, &master.uid).unwrap().unwrap();

        // Every instance moves to 10:00, edited from the 18 Aug one.
        let mut edited = master.clone();
        edited.start = EventTime::Zoned(at(2026, 8, 18, 10, 0), local);
        edited.end = EventTime::Zoned(at(2026, 8, 18, 11, 0), local);
        let clicked = local
            .from_local_datetime(&at(2026, 8, 18, 9, 0))
            .unwrap()
            .with_timezone(&chrono::Utc);
        let series = whole_series_edit(&master, edited, clicked, local);
        write_series_edit(&mut store, &master, &series, true).unwrap();

        let tuesday = store
            .occurrences(
                day(2026, 8, 11),
                day(2026, 8, 12),
                &std::collections::HashSet::new(),
            )
            .unwrap();
        let shown: Vec<(String, NaiveDateTime)> =
            tuesday.into_iter().map(|o| (o.summary, o.start)).collect();
        assert_eq!(
            shown,
            vec![("Standup (demo)".to_owned(), at(2026, 8, 11, 10, 0))],
            "the changed instance must move with the series, not show beside it"
        );
    }

    #[test]
    fn switching_a_timed_series_to_all_day_writes_dates() {
        use crate::model::EventTime;
        let athens = chrono_tz::Europe::Athens;
        let master = weekly_series(
            EventTime::Zoned(at(2026, 8, 4, 9, 0), athens),
            EventTime::Zoned(at(2026, 8, 4, 10, 0), athens),
        );
        let mut edited = master.clone();
        edited.start = EventTime::Date(day(2026, 8, 18));
        edited.end = EventTime::Date(day(2026, 8, 19));

        let series = whole_series_edit(&master, edited, clicked(at(2026, 8, 18, 9, 0)), athens);

        assert_eq!(series.start, EventTime::Date(day(2026, 8, 4)));
        assert_eq!(series.end, EventTime::Date(day(2026, 8, 5)));
        assert_eq!(series.exdates, vec![at(2026, 8, 11, 0, 0)]);
    }

    #[test]
    fn switching_an_all_day_series_to_timed_writes_times() {
        use crate::model::EventTime;
        let athens = chrono_tz::Europe::Athens;
        let master = weekly_series(
            EventTime::Date(day(2026, 8, 4)),
            EventTime::Date(day(2026, 8, 5)),
        );
        let mut edited = master.clone();
        edited.start = EventTime::Zoned(at(2026, 8, 18, 9, 0), athens);
        edited.end = EventTime::Zoned(at(2026, 8, 18, 10, 0), athens);

        let series = whole_series_edit(&master, edited, clicked(at(2026, 8, 18, 0, 0)), athens);

        assert_eq!(series.start, EventTime::Zoned(at(2026, 8, 4, 9, 0), athens));
        assert_eq!(
            series.end,
            EventTime::Zoned(at(2026, 8, 4, 10, 0), athens),
            "an all-day DTEND equal to its DTSTART is not an event"
        );
        assert_eq!(series.exdates, vec![at(2026, 8, 11, 9, 0)]);
    }

    #[test]
    fn deleting_one_changed_instance_rewrites_the_series_instead_of_deleting_it() {
        use crate::model::EventTime;
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(
            &dir.path().join("calendars"),
            &dir.path().join("index.sqlite"),
        )
        .unwrap();
        let calendar = store.create_calendar("Work", PALETTE[0]).unwrap();
        let local = store.local_timezone();

        let mut series = crate::model::Event::draft(&calendar.id, at(2026, 8, 4, 9, 0), local);
        series.summary = "Standup".into();
        series.rrule = Some("FREQ=WEEKLY".into());
        store.save(&series).unwrap();
        // The 11 Aug instance, moved to 10:00: an override in the same file.
        let mut moved = series.clone();
        moved.rrule = None;
        moved.recurrence_id = Some(EventTime::Zoned(at(2026, 8, 11, 9, 0), local));
        moved.start = EventTime::Zoned(at(2026, 8, 11, 10, 0), local);
        moved.end = EventTime::Zoned(at(2026, 8, 11, 11, 0), local);
        store.save(&moved).unwrap();

        store.delete_override(&moved).unwrap();
        assert_eq!(
            removal_after(&calendar.path.join(&series.file_name)),
            Removal::Rewrite,
            "a DELETE here would remove the whole series from the server"
        );

        store.delete(&calendar.id, &series.uid).unwrap();
        assert_eq!(
            removal_after(&calendar.path.join(&series.file_name)),
            Removal::Delete
        );
    }

    fn invitation(uid: &str, sequence: i64, summary: &str) -> PendingInvitation {
        let ics = format!(
            "BEGIN:VCALENDAR\r\nMETHOD:REQUEST\r\nBEGIN:VEVENT\r\nUID:{uid}\r\n\
             SEQUENCE:{sequence}\r\nSUMMARY:{summary}\r\nDTSTART:20260804T090000Z\r\n\
             ORGANIZER:mailto:boss@example.com\r\nATTENDEE:mailto:me@example.com\r\n\
             END:VEVENT\r\nEND:VCALENDAR\r\n"
        );
        PendingInvitation {
            account_id: "work".into(),
            parsed: cosmic_pim_caldav::itip::parse(&ics).unwrap(),
            event: None,
            ics,
        }
    }

    #[test]
    fn declining_after_accepting_takes_the_meeting_off_the_calendar() {
        use cosmic_pim_caldav::itip::{self, Outcome};
        let collection = tempfile::tempdir().unwrap();
        let review = invitation("review@x", 1, "Review");

        // Accepted earlier: the event is stored.
        let Outcome::Created { file } =
            itip::apply(collection.path(), &review.ics, "me@example.com", None).unwrap()
        else {
            panic!("the invitation was not stored");
        };

        assert_eq!(
            withdraw_declined(collection.path(), &review.ics, "me@example.com").unwrap(),
            Outcome::Cancelled { file: file.clone() }
        );
        assert!(!collection.path().join(file).exists());
    }

    #[test]
    fn declining_what_was_never_accepted_touches_nothing() {
        let collection = tempfile::tempdir().unwrap();
        let review = invitation("review@x", 1, "Review");
        assert_eq!(
            withdraw_declined(collection.path(), &review.ics, "me@example.com").unwrap(),
            cosmic_pim_caldav::itip::Outcome::NoMatch
        );
    }

    fn calendar(id: &str, read_only: bool) -> CalendarMeta {
        CalendarMeta {
            id: id.to_owned(),
            name: id.to_owned(),
            color: PALETTE[0],
            path: std::path::PathBuf::from(id),
            read_only,
        }
    }

    #[test]
    fn export_starts_on_the_users_own_calendar_not_a_feed() {
        let calendars = [
            calendar("holidays-feed", true),
            calendar("hidden", false),
            calendar("personal", false),
        ];
        let mut config = Config::default();
        config.toggle_calendar("hidden");
        assert_eq!(default_export(&calendars, &config), 2);
    }

    #[test]
    fn search_finds_a_word_that_is_only_in_the_description() {
        let occurrence = Occurrence {
            uid: "review".into(),
            calendar_id: "work".into(),
            summary: "Quarterly review".into(),
            location: Some("Room 4".into()),
            all_day: false,
            start: at(2026, 8, 4, 9, 0),
            end: at(2026, 8, 4, 10, 0),
            recurrence_id: None,
        };
        assert!(matches_search(
            "budget",
            &occurrence,
            Some("Bring the Budget numbers")
        ));
        assert!(!matches_search("budget", &occurrence, None));
        assert!(matches_search("room 4", &occurrence, None));
    }

    #[test]
    fn a_cancel_from_someone_else_is_refused_and_said_so() {
        use cosmic_pim_caldav::itip::{self, Outcome};
        let collection = tempfile::tempdir().unwrap();
        let review = invitation("review@x", 1, "Review");
        itip::apply(collection.path(), &review.ics, "me@example.com", None).unwrap();

        // The same meeting, cancelled by a mail from someone who is not boss@.
        let cancel = itip::with_method(&review.ics, "CANCEL");
        let outcome = itip::apply(
            collection.path(),
            &cancel,
            "me@example.com",
            Some("mallory@example.com"),
        )
        .unwrap();
        assert_eq!(outcome, Outcome::NotFromOrganizer);
        assert!(
            matches!(quiet_notice(&outcome), Some(Err(_))),
            "an ignored cancellation must be visible"
        );
        assert!(matches!(
            quiet_notice(&Outcome::Cancelled {
                file: "x.ics".into()
            }),
            Some(Ok(_))
        ));
    }

    #[test]
    fn an_invitation_without_an_organizer_is_not_one_to_answer() {
        let ics = "BEGIN:VCALENDAR\r\nMETHOD:REQUEST\r\nBEGIN:VEVENT\r\nUID:x@y\r\n\
                   DTSTART:20260804T090000Z\r\nATTENDEE:mailto:me@example.com\r\n\
                   END:VEVENT\r\nEND:VCALENDAR\r\n";
        let parsed = cosmic_pim_caldav::itip::parse(ics).unwrap();
        assert!(!parsed.is_from_organizer("me@example.com", None));
        assert!(
            invitation("review@x", 1, "Review")
                .parsed
                .is_from_organizer("me@example.com", None)
        );
    }

    #[test]
    fn the_same_invitation_is_asked_about_once() {
        let mut queue = Vec::new();
        enqueue_invitation(&mut queue, invitation("review@x", 0, "Review"));
        enqueue_invitation(&mut queue, invitation("review@x", 0, "Review"));
        enqueue_invitation(&mut queue, invitation("lunch@x", 0, "Lunch"));
        assert_eq!(queue.len(), 2);
    }

    #[test]
    fn a_newer_revision_replaces_the_queued_one() {
        let mut queue = Vec::new();
        enqueue_invitation(&mut queue, invitation("review@x", 1, "Review"));
        enqueue_invitation(&mut queue, invitation("review@x", 2, "Review, moved"));
        enqueue_invitation(&mut queue, invitation("review@x", 1, "Review"));
        assert_eq!(queue.len(), 1);
        assert_eq!(queue[0].parsed.sequence, 2);
    }

    #[test]
    fn days_in_month_knows_leap_years() {
        assert_eq!(days_in_month(2026, 2), 28);
        assert_eq!(days_in_month(2028, 2), 29);
        assert_eq!(days_in_month(2026, 12), 31);
        assert_eq!(days_in_month(2026, 4), 30);
    }
}
