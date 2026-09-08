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
const APP_ICON: &[u8] = include_bytes!("../resources/icons/hicolor/scalable/apps/icon.svg");

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
    /// When the reminder sweep last ran, so a resume knows which window it
    /// slept through.
    last_reminder_sweep: NaiveDateTime,

    /// A pending "this event / all events" question. The editor stays open
    /// underneath until the user answers or cancels.
    scope_prompt: Option<ScopePrompt>,

    toasts: widget::Toasts<Message>,
    reminders: crate::reminders::Scheduler,
    /// True while the background daemon owns reminders, so we stay quiet.
    reminders_delegated: bool,

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
    AccountRemove(String),
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

    /// The machine came back from sleep.
    Resumed,

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
    ImportPath(PathBuf),
    ExportTo(PathBuf, String),
    DialogCancelled,
    DialogFailed(String),

    /// Whether another process (the daemon) is firing reminders for us.
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

        let today = chrono::Local::now().date_naive();
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
            now: chrono::Local::now().naive_local(),
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
            last_reminder_sweep: chrono::Local::now().naive_local(),
            scope_prompt: None,
            toasts: widget::Toasts::new(Message::CloseToast),
            reminders: crate::reminders::Scheduler::new(),
            // Assumed until the check comes back, so a fast-starting daemon never
            // races us into a duplicate notification.
            reminders_delegated: true,
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
        let mut startup = vec![
            app.update_title(),
            Self::request_scroll(),
            // Ask the bus whether the daemon is already handling reminders.
            cosmic::task::future(async {
                Message::ReminderOwnership(crate::reminders::someone_else_owns_reminders().await)
            }),
        ];

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
        ]);

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
            wake_subscription(),
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
                self.today = chrono::Local::now().date_naive();
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
                        queue_writeback_task(&todo, None);
                        self.journal_finish(vec![crate::undo::Entry {
                            calendar_id: todo.calendar_id.clone(),
                            file_name: todo.file_name.clone(),
                            before: None,
                            after: None,
                        }]);
                        self.new_task.clear();
                        self.reload_tasks();
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
                        queue_writeback_task(&todo, base.as_deref());
                        self.journal_finish(vec![crate::undo::Entry {
                            calendar_id: todo.calendar_id.clone(),
                            file_name: todo.file_name.clone(),
                            before: base,
                            after: None,
                        }]);
                        self.reload_tasks();
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
                        let event = crate::store::vdir::parse_ics(&delivery.ics, "", "")
                            .into_iter()
                            .next();
                        self.invitations.push(PendingInvitation {
                            account_id: delivery.account_id,
                            ics: delivery.ics,
                            parsed,
                            event,
                        });
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

            Message::AccountRemove(id) => {
                if let Some(accounts) = self.accounts.as_mut()
                    && let Err(why) = accounts.remove(&id)
                {
                    return self.toast_error(&format!("{}: {why}", fl!("accounts")));
                }
            }

            Message::SyncNow => return self.sync_now(),

            Message::SyncFinished(lines, changed) => {
                self.syncing = false;
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

            Message::Resumed => {
                let slept_at = self.last_reminder_sweep;
                self.now = chrono::Local::now().naive_local();
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
                self.now = chrono::Local::now().naive_local();
                let today = self.now.date();
                if today != self.today {
                    // Past midnight: today moved, so the highlight and any
                    // relative view must follow it.
                    self.today = today;
                    self.reload();
                }
                // Feed refreshes ride the same tick, paced to one check every
                // five minutes; each feed's own interval gates the fetch.
                let feeds_due = self
                    .last_feed_check
                    .is_none_or(|at| at.elapsed() >= std::time::Duration::from_secs(300));
                if feeds_due {
                    return Task::batch([self.refresh_feeds(false), self.fire_due_reminders()]);
                }
                return self.fire_due_reminders();
            }

            Message::ReminderOwnership(delegated) => {
                if delegated {
                    tracing::info!("the reminder daemon is running; leaving reminders to it");
                }
                self.reminders_delegated = delegated;
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

            Message::ExportRequested => {
                let Some(calendar) = self
                    .store
                    .as_ref()
                    .and_then(|store| store.calendars().first())
                    .map(|c| (c.id.clone(), c.name.clone()))
                else {
                    return self.toast_error(&fl!("no-calendars"));
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
        if let Some(original) = &moved_from {
            queue_writeback_task_delete(original);
            if let Err(why) = store.delete_todo(&original.calendar_id, &original.uid) {
                return self.toast_error(&why.to_string());
            }
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
                queue_writeback_task(&todo, base.as_deref());
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
                Task::none()
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
        queue_writeback_task_delete(&original);
        match store.delete_todo(&original.calendar_id, &original.uid) {
            Ok(()) => {
                self.journal_finish(vec![crate::undo::Entry {
                    calendar_id: original.calendar_id.clone(),
                    file_name: original.file_name.clone(),
                    before,
                    after: None,
                }]);
                self.task_editor = None;
                self.core.window.show_context = false;
                self.reload_tasks();
                Task::none()
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

    /// Shows any reminder whose trigger has just passed.
    ///
    /// Reminders are checked against the store rather than the currently visible
    /// range, so they still fire while you are looking at a different month.
    /// One notification for the reminders that passed while asleep.
    ///
    /// Same ownership rule as [`Self::fire_due_reminders`]: if the daemon is
    /// running, the digest is its job, not ours.
    fn report_missed_reminders(
        &mut self,
        slept_at: NaiveDateTime,
    ) -> Task<cosmic::Action<Message>> {
        if self.reminders_delegated {
            return Task::none();
        }
        let Some(store) = self.store.as_ref() else {
            return Task::none();
        };

        // A day either side of the window covers any plausible suspend.
        let from = slept_at.date() - Duration::days(1);
        let Ok(occurrences) = store.occurrences(
            from,
            self.now.date() + Duration::days(1),
            &self.config.hidden_set(),
        ) else {
            return Task::none();
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

        let missed = crate::reminders::sweep_missed(
            &mut self.reminders,
            &occurrences,
            alarms_for,
            slept_at,
            self.now,
            |occurrence| self.config.reminder_for(&occurrence.calendar_id),
        );

        if missed == 0 {
            return Task::none();
        }
        cosmic::task::future(async move {
            crate::reminders::notify_missed(missed, <AppModel as cosmic::Application>::APP_ID)
                .await;
            Message::Ignore
        })
    }

    fn fire_due_reminders(&mut self) -> Task<cosmic::Action<Message>> {
        // The daemon has it covered; firing here too would double every reminder.
        if self.reminders_delegated {
            return Task::none();
        }

        let Some(store) = self.store.as_ref() else {
            return Task::none();
        };

        let today = self.now.date();
        let occurrences =
            match store.occurrences(today, today + Duration::days(2), &self.config.hidden_set()) {
                Ok(occurrences) => occurrences,
                Err(why) => {
                    tracing::warn!(%why, "could not load occurrences to check reminders");
                    return Task::none();
                }
            };

        // Map an occurrence back to its alarms. Instance-aware: an overridden
        // occurrence carries its own VALARMs (a moved meeting reminds relative
        // to its new time), and only falls back to the series master's.
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

        self.last_reminder_sweep = self.now;
        let due = self
            .reminders
            .due(&occurrences, alarms_for, self.now, |occurrence| {
                self.config.reminder_for(&occurrence.calendar_id)
            });

        if !due.is_empty() {
            tracing::info!(count = due.len(), "firing reminders");
        }

        // Delivery is a D-Bus round trip, so it happens off the update loop.
        let tasks: Vec<_> = due
            .into_iter()
            .map(|reminder| {
                let body = reminder.body(&self.config);
                cosmic::task::future(async move {
                    crate::reminders::notify(
                        &reminder,
                        <Self as cosmic::Application>::APP_ID,
                        body,
                    )
                    .await;
                    Message::Ignore
                })
            })
            .collect();

        // Keep the fired set from growing for the lifetime of the process.
        self.reminders.forget_before(self.now - Duration::days(1));

        Task::batch(tasks)
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
            .push(self.calendar_defaults_view())
            .apply(widget::scrollable)
            .into()
    }

    /// Per-calendar overrides of the two defaults above.
    ///
    /// Only writable calendars appear: a read-only feed cannot hold a new
    /// event, so a default duration for one would be an offer that cannot be
    /// taken. Its reminders still follow the app-wide setting.
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
    fn confirm_account(&mut self) -> Task<cosmic::Action<Message>> {
        let Some(form) = self.account_form.clone() else {
            return Task::none();
        };
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

    /// Runs a sync pass off the UI thread.
    fn sync_now(&mut self) -> Task<cosmic::Action<Message>> {
        if self.syncing || self.accounts.is_none() {
            return Task::none();
        }
        self.syncing = true;
        self.sync_status = None;

        let root = crate::store::vdir::default_root();
        // The sync engine walks CalDAV and CardDAV in one pass: an account can
        // offer both, and the address books land in the suite's contacts root
        // for Circle to read.
        let contacts_root = crate::store::contacts::default_root();
        // Provider manifests: how an account that names a provider rather than
        // a raw URL resolves its endpoints and OAuth client.
        let registry = cosmic_pim_accounts::Registry::load();
        cosmic::task::future(async move {
            let outcome = tokio::task::spawn_blocking(move || {
                // Reopened inside the task: `AccountStore` is not `Send`-shared
                // with the UI, and re-reading also picks up any change made
                // since the button was pressed.
                let mut accounts = match cosmic_pim_accounts::AccountStore::open_default() {
                    Ok(accounts) => accounts,
                    Err(why) => return (vec![why.to_string()], false),
                };
                let reports =
                    cosmic_pim_sync::sync_all(&mut accounts, &registry, &root, &contacts_root);
                let changed = reports.iter().any(cosmic_pim_sync::AccountReport::changed);
                (
                    reports
                        .iter()
                        .map(cosmic_pim_sync::AccountReport::summary)
                        .collect(),
                    changed,
                )
            })
            .await
            .unwrap_or_else(|why| (vec![why.to_string()], false));

            Message::SyncFinished(outcome.0, outcome.1)
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

    /// Refreshes due ICS feeds off the UI thread; `force` fetches them all,
    /// due or not, which is what a just-added subscription wants.
    fn refresh_feeds(&mut self, force: bool) -> Task<cosmic::Action<Message>> {
        if self.refreshing_feeds {
            return Task::none();
        }
        self.refreshing_feeds = true;
        self.last_feed_check = Some(std::time::Instant::now());

        let root = crate::store::vdir::default_root();
        cosmic::task::future(async move {
            let changed = tokio::task::spawn_blocking(move || {
                let now_ms = chrono::Utc::now().timestamp_millis();
                let mut changed = false;
                for meta in crate::store::vdir::collections(&root) {
                    if !cosmic_pim_caldav::feed::is_feed(&meta.path) {
                        continue;
                    }
                    let due = cosmic_pim_caldav::feed::FeedState::load(&meta.path)
                        .is_some_and(|state| state.due(now_ms));
                    if !(force || due) {
                        continue;
                    }
                    match cosmic_pim_caldav::feed::refresh(&meta.path, now_ms) {
                        Ok(outcome) => changed |= outcome.changed(),
                        // Logged, not toasted: a feed that is down will be
                        // retried on its own schedule, and there is nothing
                        // for the user to do about it right now.
                        Err(why) => {
                            tracing::warn!(feed = meta.id, %why, "feed refresh failed");
                        }
                    }
                }
                changed
            })
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
            let when = if event.start.is_all_day() {
                start.date().to_string()
            } else {
                format!(
                    "{} {}",
                    start.date(),
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

    /// Queues writeback for a file iTIP changed.
    fn queue_invitation_writeback(&self, collection_id: &str, file: &str) {
        let root = crate::store::vdir::default_root();
        if let Err(why) = cosmic_pim_sync::queue_save(&root, collection_id, file) {
            tracing::warn!(%why, file, "could not queue the invitation for upload");
        }
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
    /// Decline stores nothing — a declined meeting on the grid is clutter,
    /// and the organizer's next update re-delivers it if things change.
    /// Either way the reply goes to Envelope's outbox when Envelope is
    /// running, and degrades to "reply from your mail client" when not.
    fn answer_invitation(&mut self, answer: InviteAnswer) -> Task<cosmic::Action<Message>> {
        use cosmic_pim_caldav::itip;

        if self.invitations.is_empty() {
            return Task::none();
        }
        let invitation = self.invitations.remove(0);

        let Some(meta) = self.invitation_collection(&invitation.account_id) else {
            return self.toast_error(&fl!("no-writable-calendar"));
        };
        let Some(me) = self.account_address(&invitation.account_id) else {
            return self.toast_error(&fl!("invitation-no-address"));
        };

        if answer != InviteAnswer::Declined {
            match itip::apply(&meta.path, &invitation.ics, &me) {
                Ok(itip::Outcome::Stale) => return self.toast_error(&fl!("invitation-stale")),
                Ok(itip::Outcome::NotForMe) => {
                    return self.toast_error(&fl!("invitation-not-for-me"));
                }
                Ok(outcome) => {
                    if let Some(file) = outcome.file() {
                        self.queue_invitation_writeback(&meta.id, file);
                    }
                }
                Err(why) => return self.toast_error(&why.to_string()),
            }
        }

        let Some(organizer) = invitation
            .parsed
            .organizer
            .as_ref()
            .map(|o| o.email.clone())
        else {
            // No organizer, nothing to answer; whatever was stored, stands.
            self.refresh_after_invitation();
            return Task::none();
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
            match itip::apply(&meta.path, &reply_ics, &me) {
                Ok(outcome) => {
                    if let Some(file) = outcome.file() {
                        self.queue_invitation_writeback(&meta.id, file);
                    }
                }
                Err(why) => tracing::warn!(%why, "could not record the PARTSTAT locally"),
            }
        }

        self.refresh_after_invitation();

        let Some(conn) = self.dbus.clone() else {
            return self.toast_info(&fl!("invitation-reply-by-mail"));
        };
        let account_id = invitation.account_id.clone();
        cosmic::task::future(async move {
            let queued = crate::scheduling::send_reply(&conn, &reply_ics, &account_id, &organizer)
                .await
                .unwrap_or_else(|why| {
                    tracing::warn!(%why, "the scheduling reply could not reach Envelope");
                    false
                });
            Message::InvitationReplySent(queued)
        })
    }

    /// Applies a CANCEL or REPLY without asking: neither carries a decision
    /// for this user to make. The gates live inside `itip::apply`.
    fn apply_itip_quietly(
        &mut self,
        delivery: &crate::scheduling::Delivery,
    ) -> Task<cosmic::Action<Message>> {
        use cosmic_pim_caldav::itip::Outcome;

        let Some(meta) = self.invitation_collection(&delivery.account_id) else {
            return Task::none();
        };
        let me = self
            .account_address(&delivery.account_id)
            .unwrap_or_default();
        match cosmic_pim_caldav::itip::apply(&meta.path, &delivery.ics, &me) {
            Ok(outcome) => {
                if let Some(file) = outcome.file() {
                    self.queue_invitation_writeback(&meta.id, file);
                }
                // A whole-series CANCEL removed the file; the deletion must
                // reach the server too.
                if let Outcome::Cancelled { file } = &outcome {
                    let root = crate::store::vdir::default_root();
                    if let Err(why) = cosmic_pim_sync::queue_delete(&root, &meta.id, file) {
                        tracing::warn!(%why, file, "could not queue the cancellation for upload");
                    }
                }
                let cancelled = matches!(
                    outcome,
                    Outcome::Cancelled { .. } | Outcome::InstanceCancelled { .. }
                );
                self.refresh_after_invitation();
                if cancelled {
                    return self.toast_info(&fl!("invitation-cancelled"));
                }
                Task::none()
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

        let preview = match &parsed {
            Some(parsed) => {
                let (start, end) = parsed.span();
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

        let (start, end) = parsed.span();
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
                queue_writeback_save(&event, None);
                self.journal_finish(vec![crate::undo::Entry {
                    calendar_id: event.calendar_id.clone(),
                    file_name: event.file_name.clone(),
                    before: None,
                    after: None,
                }]);
                self.anchor = parsed.date;
                self.sync_mini();
                self.reload();
                self.toast(&fl!("quick-add-done", summary = parsed.summary))
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

        for entry in &group.entries {
            let Some(meta) = self
                .store
                .as_ref()
                .and_then(|s| s.calendar(&entry.calendar_id))
                .cloned()
            else {
                tracing::warn!(
                    calendar = entry.calendar_id,
                    "cannot restore into a calendar that no longer exists"
                );
                continue;
            };
            let path = meta.path.join(&entry.file_name);
            let current = std::fs::read_to_string(&path).ok();
            let desired = if undo { &entry.before } else { &entry.after };
            let root = crate::store::vdir::default_root();

            match desired {
                Some(bytes) => {
                    if let Err(why) = cosmic_pim_core::atomic::write(&path, bytes, None) {
                        tracing::warn!(%why, file = entry.file_name, "history restore failed");
                        continue;
                    }
                    if let Err(why) = cosmic_pim_sync::queue_save_with_base(
                        &root,
                        &entry.calendar_id,
                        &entry.file_name,
                        current.as_deref(),
                    ) {
                        tracing::warn!(%why, "could not queue the restored file for upload");
                    }
                }
                None => {
                    if let Err(why) = std::fs::remove_file(&path)
                        && why.kind() != std::io::ErrorKind::NotFound
                    {
                        tracing::warn!(%why, file = entry.file_name, "history removal failed");
                        continue;
                    }
                    if let Err(why) =
                        cosmic_pim_sync::queue_delete(&root, &entry.calendar_id, &entry.file_name)
                    {
                        tracing::warn!(%why, "could not queue the removal for upload");
                    }
                }
            }
        }

        self.history.shelve(group, undo);
        if let Some(store) = self.store.as_mut() {
            let _ = store.refresh();
        }
        self.reload();
        self.reload_tasks();
        self.toast(&if undo {
            fl!("undo-done")
        } else {
            fl!("redo-done")
        })
    }

    /// Recomputes search results for `query`: summaries and locations across
    /// every visible calendar, six months back and a year forward — wider
    /// than the launcher plugin's window, nearest-first, one row per event.
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

        let mut hits: Vec<Occurrence> = occurrences
            .into_iter()
            .filter(|o| {
                o.summary.to_lowercase().contains(&needle)
                    || o.location
                        .as_deref()
                        .is_some_and(|l| l.to_lowercase().contains(&needle))
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
            let now = chrono::Local::now().naive_local();
            let hour = (now.hour() + 1).min(23);
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
        let mut journal = vec![self.journal_before(&event.calendar_id, &event.file_name)];
        if let Some(original) = &moved_from {
            journal.push(self.journal_before(&original.calendar_id, &original.file_name));
        }

        let Some(store) = self.store.as_mut() else {
            return Task::none();
        };
        let result = match moved_from {
            Some(original) => store
                .move_to_calendar(&original, &event.calendar_id)
                .and_then(|_| store.save(&event)),
            None => store.save(&event),
        };

        match result {
            Ok(()) => {
                queue_writeback_save(&event, base.as_deref());
                self.journal_finish(journal);
                self.editor = None;
                self.core.window.show_context = false;
                self.reload();
                Task::none()
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

            // The whole series. The editor's dates showed the *clicked
            // instance*, so what the user expressed is a shift of that
            // instance — apply the same shift to the series' start, and its
            // new length directly.
            EditScope::All => {
                let occurrence_start = instant.with_timezone(&local).naive_local();
                let edited_start = edited.start.naive_local(local);
                let shift = edited_start - occurrence_start;
                let length = edited.end.naive_local(local) - edited_start;

                let mut series = edited;
                series.start = shift_time(master.start, shift);
                series.end = shift_time(series.start, length);
                series
            }

            EditScope::Following => unreachable!("handled above"),
        };

        let Some(store) = self.store.as_mut() else {
            return Task::none();
        };

        // A whole-series edit may also have moved the event to another
        // calendar, which means removing the old file as well.
        let base = writeback_base(store, &event.calendar_id, &event.file_name);
        let mut journal = vec![self.journal_before(&event.calendar_id, &event.file_name)];
        if event.calendar_id != master.calendar_id {
            journal.push(self.journal_before(&master.calendar_id, &master.file_name));
        }

        let Some(store) = self.store.as_mut() else {
            return Task::none();
        };
        let result = if event.calendar_id == master.calendar_id {
            store.save(&event)
        } else {
            store
                .move_to_calendar(&master, &event.calendar_id)
                .and_then(|_| store.save(&event))
        };

        match result {
            Ok(()) => {
                queue_writeback_save(&event, base.as_deref());
                self.journal_finish(journal);
                self.editor = None;
                self.core.window.show_context = false;
                self.reload();
                Task::none()
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

        match result {
            Ok(()) => {
                // Two files changed: the truncated master and the successor.
                if let Ok(Some(truncated)) = self
                    .store
                    .as_ref()
                    .map_or(Ok(None), |s| s.event(&master.calendar_id, &master.uid))
                {
                    queue_writeback_save(&truncated, master_base.as_deref());
                }
                queue_writeback_save(&series, None);

                // One undo step for the whole split: the master's pre-split
                // bytes come back, the successor file (wherever it landed)
                // goes away.
                let mut journal = vec![
                    crate::undo::Entry {
                        calendar_id: master.calendar_id.clone(),
                        file_name: master.file_name.clone(),
                        before: master_base,
                        after: None,
                    },
                    crate::undo::Entry {
                        calendar_id: series.calendar_id.clone(),
                        file_name: series.file_name.clone(),
                        before: None,
                        after: None,
                    },
                ];
                if series.calendar_id != master.calendar_id {
                    journal.push(crate::undo::Entry {
                        calendar_id: master.calendar_id.clone(),
                        file_name: series.file_name.clone(),
                        before: None,
                        after: None,
                    });
                }
                self.journal_finish(journal);

                self.editor = None;
                self.core.window.show_context = false;
                self.reload();
                Task::none()
            }
            Err(why) => self.toast_error(&format!("{}: {why}", fl!("error-save-event"))),
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
                if deleted_whole {
                    // The file is gone; tell the server so too.
                    queue_writeback_delete(&master);
                } else if let Ok(Some(after)) = self
                    .store
                    .as_ref()
                    .map_or(Ok(None), |s| s.event(&master.calendar_id, &master.uid))
                {
                    // The master changed (EXDATE or UNTIL); push the new revision.
                    queue_writeback_save(&after, base.as_deref());
                }
                self.journal_finish(journal);
                self.editor = None;
                self.core.window.show_context = false;
                self.reload();
                Task::none()
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

        // Queued BEFORE the local delete only for clarity — the coordinates
        // live in the CalDAV sidecar, which the store never touches, so either
        // order works.
        queue_writeback_delete(&original);

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
                self.journal_finish(journal);
                self.editor = None;
                self.core.window.show_context = false;
                self.reload();
                Task::none()
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
                self.reload();
                self.toast(&fl!(
                    "import-done",
                    added = summary.added.to_string(),
                    updated = summary.updated.to_string()
                ))
            }
            Err(why) => self.toast_error(&why.to_string()),
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
            self.today = chrono::Local::now().date_naive();
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

/// Fires once each time the machine resumes from sleep.
///
/// Modelled on [`file_watch_subscription`], including the park-on-failure: a
/// stream that ended would make iced restart the subscription in a tight loop.
fn wake_subscription() -> Subscription<Message> {
    Subscription::run(|| {
        cosmic::iced::stream::channel(
            1,
            |mut output: futures::channel::mpsc::Sender<_>| async move {
                let (tx, mut rx) = futures::channel::mpsc::unbounded();
                let watch = crate::reminders::on_wake(move || {
                    let _ = tx.unbounded_send(());
                });

                let pump = async move {
                    use futures::StreamExt;
                    while rx.next().await.is_some() {
                        if output.send(Message::Resumed).await.is_err() {
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

/// Queues a saved event for upload to its CalDAV server, if it has one.
///
/// Failures are logged rather than surfaced: the *save* succeeded, the user's
/// data is safe on disk, and the push queue retries on its own schedule. A
/// toast here would report a problem the user can do nothing about and that
/// will very likely resolve itself.
/// `base` is the file's bytes from *before* this session's edit; with it, the
/// push queue can three-way-merge a concurrent server-side change instead of
/// recording a conflict. `None` (new file, or the read failed) degrades to
/// the plain no-merge queue.
fn queue_writeback_save(event: &crate::model::Event, base: Option<&str>) {
    let root = crate::store::vdir::default_root();
    if let Err(why) =
        cosmic_pim_sync::queue_save_with_base(&root, &event.calendar_id, &event.file_name, base)
    {
        tracing::warn!(
            calendar = event.calendar_id, file = event.file_name, %why,
            "could not queue the edit for upload"
        );
    }
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

/// Queues a deleted event for removal on its CalDAV server, if it had one.
fn queue_writeback_delete(event: &crate::model::Event) {
    let root = crate::store::vdir::default_root();
    if let Err(why) = cosmic_pim_sync::queue_delete(&root, &event.calendar_id, &event.file_name) {
        tracing::warn!(
            calendar = event.calendar_id, file = event.file_name, %why,
            "could not queue the deletion for upload"
        );
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

/// Queues a saved task for upload, the same way a saved event is.
fn queue_writeback_task(todo: &crate::model::Todo, base: Option<&str>) {
    let root = crate::store::vdir::default_root();
    if let Err(why) =
        cosmic_pim_sync::queue_save_with_base(&root, &todo.calendar_id, &todo.file_name, base)
    {
        tracing::warn!(
            calendar = todo.calendar_id, file = todo.file_name, %why,
            "could not queue the task for upload"
        );
    }
}

/// Queues a deleted task for removal on its CalDAV server, if it had one.
fn queue_writeback_task_delete(todo: &crate::model::Todo) {
    let root = crate::store::vdir::default_root();
    if let Err(why) = cosmic_pim_sync::queue_delete(&root, &todo.calendar_id, &todo.file_name) {
        tracing::warn!(
            calendar = todo.calendar_id, file = todo.file_name, %why,
            "could not queue the task deletion for upload"
        );
    }
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

    #[test]
    fn days_in_month_knows_leap_years() {
        assert_eq!(days_in_month(2026, 2), 28);
        assert_eq!(days_in_month(2028, 2), 29);
        assert_eq!(days_in_month(2026, 12), 31);
        assert_eq!(days_in_month(2026, 4), 30);
    }
}
