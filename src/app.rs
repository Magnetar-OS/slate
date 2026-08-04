// SPDX-License-Identifier: GPL-3.0-only

//! The application model: state, messages, and the update loop.

use crate::config::{Config, ViewKind};
use crate::fl;
use crate::model::{CalendarMeta, Occurrence, PALETTE};
use crate::store::Store;
use crate::ui::editor::{DateField, Editor};
use crate::ui::{month::MonthView, sidebar::Sidebar, timegrid::TimeGrid};
use chrono::{Datelike, Duration, NaiveDate, NaiveDateTime, NaiveTime, Timelike};
use cosmic::app::context_drawer;
use cosmic::cosmic_config::{self, CosmicConfigEntry};
use cosmic::iced::{Alignment, Length, Subscription, futures};
use cosmic::prelude::*;
use cosmic::widget::{self, about::About, menu, nav_bar, segmented_button};
use futures::SinkExt;
use std::collections::{BTreeMap, HashMap};

const REPOSITORY: &str = env!("CARGO_PKG_REPOSITORY");
/// Hour the week/day grid scrolls to when the day holds no timed events.
const DEFAULT_SCROLL_HOUR: u32 = 8;

/// Lead times offered for the default reminder. `0` disables it.
const REMINDER_CHOICES: &[u32] = &[0, 5, 10, 15, 30, 60, 120, 1440];

thread_local! {
    /// Built once per thread: the labels are localised, so they cannot be a const.
    static REMINDER_LABELS: Vec<String> = REMINDER_CHOICES
        .iter()
        .map(|minutes| match minutes {
            0 => fl!("reminder-none"),
            1440 => fl!("reminder-day-before"),
            m if *m < 60 => fl!("reminder-minutes", minutes = m.to_string()),
            m => fl!("reminder-hours", hours = (m / 60).to_string()),
        })
        .collect();
}
const APP_ICON: &[u8] = include_bytes!("../resources/icons/hicolor/scalable/apps/icon.svg");

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

    views: segmented_button::SingleSelectModel,
    mini: widget::calendar::CalendarModel,

    editor: Option<Editor>,
    new_calendar_name: Option<String>,

    toasts: widget::Toasts<Message>,
    reminders: crate::reminders::Scheduler,
}

#[derive(Debug, Clone)]
pub enum Message {
    // Shell
    LaunchUrl(String),
    ToggleContextPage(ContextPage),
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
    NewCalendarStart,
    NewCalendarNameChanged(String),
    NewCalendarConfirm,
    NewCalendarCancel,

    // Events
    NewEvent,
    NewEventOn(NaiveDate),
    NewEventAt(NaiveDateTime),
    OpenEvent(String, String),

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
    EditorPickDate(DateField),
    EditorDatePicked(jiff::civil::Date),
    EditorPickerPrev,
    EditorPickerNext,
    EditorSave,
    EditorCancel,
    EditorDelete,

    // Settings
    SetFirstDayOfWeek(usize),
    ToggleWeekNumbers(bool),
    Toggle24Hour(bool),
    SetDefaultReminder(usize),

    /// Deferred one frame so the grid's scrollable exists before we scroll it.
    ScrollTimeGrid,

    /// Minute tick, moving the "now" marker.
    Tick,

    /// Something on disk changed under us.
    FilesChanged,
}

#[derive(Copy, Clone, Debug, Default, Eq, PartialEq)]
pub enum ContextPage {
    #[default]
    About,
    Editor,
    Settings,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MenuAction {
    About,
    Settings,
    NewEvent,
    Today,
    Month,
    Week,
    Day,
}

impl menu::action::MenuAction for MenuAction {
    type Message = Message;

    fn message(&self) -> Self::Message {
        match self {
            MenuAction::About => Message::ToggleContextPage(ContextPage::About),
            MenuAction::Settings => Message::ToggleContextPage(ContextPage::Settings),
            MenuAction::NewEvent => Message::NewEvent,
            MenuAction::Today => Message::Today,
            MenuAction::Month => Message::SetView(ViewKind::Month),
            MenuAction::Week => Message::SetView(ViewKind::Week),
            MenuAction::Day => Message::SetView(ViewKind::Day),
        }
    }
}

impl cosmic::Application for AppModel {
    type Executor = cosmic::executor::Default;
    type Flags = ();
    type Message = Message;

    const APP_ID: &'static str = "io.github.entro314labs.Calendar";

    fn core(&self) -> &cosmic::Core {
        &self.core
    }

    fn core_mut(&mut self) -> &mut cosmic::Core {
        &mut self.core
    }

    fn init(
        core: cosmic::Core,
        _flags: Self::Flags,
    ) -> (Self, Task<cosmic::Action<Self::Message>>) {
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

        let mut app = AppModel {
            core,
            context_page: ContextPage::About,
            about,
            key_binds: HashMap::new(),
            config,
            config_handler,
            store,
            fatal,
            anchor: today,
            today,
            now: chrono::Local::now().naive_local(),
            days: BTreeMap::new(),
            views,
            mini: widget::calendar::CalendarModel::now(),
            editor: None,
            new_calendar_name: None,
            toasts: widget::Toasts::new(Message::CloseToast),
            reminders: crate::reminders::Scheduler::new(),
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

        app.reload();
        let command = Task::batch([app.update_title(), Self::request_scroll()]);
        (app, command)
    }

    fn header_start(&self) -> Vec<Element<'_, Self::Message>> {
        let menu_bar = menu::bar(vec![menu::Tree::with_children(
            menu::root(fl!("view")).apply(Element::from),
            menu::items(
                &self.key_binds,
                vec![
                    menu::Item::Button(fl!("new-event"), None, MenuAction::NewEvent),
                    menu::Item::Divider,
                    menu::Item::Button(fl!("month"), None, MenuAction::Month),
                    menu::Item::Button(fl!("week"), None, MenuAction::Week),
                    menu::Item::Button(fl!("day"), None, MenuAction::Day),
                    menu::Item::Divider,
                    menu::Item::Button(fl!("today"), None, MenuAction::Today),
                    menu::Item::Divider,
                    menu::Item::Button(fl!("settings"), None, MenuAction::Settings),
                    menu::Item::Button(fl!("about"), None, MenuAction::About),
                ],
            ),
        )]);

        vec![menu_bar.into()]
    }

    fn header_center(&self) -> Vec<Element<'_, Self::Message>> {
        let spacing = cosmic::theme::spacing();

        vec![
            widget::row::with_capacity(4)
                .align_y(Alignment::Center)
                .spacing(spacing.space_xxs)
                .push(
                    widget::button::icon(widget::icon::from_name("go-previous-symbolic"))
                        .on_press(Message::Previous),
                )
                .push(widget::button::standard(fl!("today")).on_press(Message::Today))
                .push(
                    widget::button::icon(widget::icon::from_name("go-next-symbolic"))
                        .on_press(Message::Next),
                )
                .push(widget::text::heading(crate::ui::range_title(
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
            widget::row::with_capacity(2)
                .align_y(Alignment::Center)
                .spacing(spacing.space_xxs)
                .push(
                    // Without a fixed width the segmented control fills the whole
                    // header bar and crowds out the menu and the nav controls.
                    widget::segmented_control::horizontal(&self.views)
                        .on_activate(Message::ViewSelected)
                        .apply(widget::container)
                        .width(Length::Fixed(288.0)),
                )
                .push(
                    widget::button::icon(widget::icon::from_name("list-add-symbolic"))
                        .on_press(Message::NewEvent),
                )
                .into(),
        ]
    }

    fn nav_model(&self) -> Option<&nav_bar::Model> {
        // Calendars are a filter, not a set of pages, so they live in our own
        // sidebar rather than the nav bar.
        None
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
        let spacing = cosmic::theme::spacing();

        if let Some(fatal) = &self.fatal {
            return widget::text::body(format!("{}\n\n{fatal}", fl!("error-load-calendars")))
                .apply(widget::container)
                .center(Length::Fill)
                .into();
        }

        let calendars = self.calendars();

        let sidebar = Sidebar {
            mini: &self.mini,
            calendars,
            config: &self.config,
            new_calendar_name: self.new_calendar_name.as_ref(),
        }
        .view();

        let main = self.grid(calendars);

        let content = widget::row::with_capacity(2)
            .spacing(spacing.space_xxs)
            .push(sidebar)
            .push(
                main.apply(widget::container)
                    .width(Length::Fill)
                    .height(Length::Fill),
            );

        // The toaster overlays transient errors without stealing focus.
        widget::toaster(&self.toasts, content)
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
            // Moves the "now" marker and rolls the highlight over at midnight.
            cosmic::iced::time::every(std::time::Duration::from_secs(30)).map(|_| Message::Tick),
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

            Message::NewEventAt(at) => return self.open_editor_new(at),

            Message::OpenEvent(calendar_id, uid) => {
                let Some(store) = self.store.as_ref() else {
                    return Task::none();
                };
                match store.event(&calendar_id, &uid) {
                    Ok(Some(event)) => {
                        self.editor = Some(Editor::from_event(&event, store.local_timezone()));
                        self.context_page = ContextPage::Editor;
                        self.core.window.show_context = true;
                    }
                    Ok(None) => tracing::warn!(uid, "event vanished before it could be opened"),
                    Err(why) => return self.toast_error(&why.to_string()),
                }
            }

            Message::EditorSummary(v) => self.with_editor(|e| e.summary = v),
            Message::EditorLocation(v) => self.with_editor(|e| e.location = v),
            Message::EditorDescription(v) => self.with_editor(|e| e.description = v),
            Message::EditorStartTime(v) => self.with_editor(|e| e.start_time = v),
            Message::EditorEndTime(v) => self.with_editor(|e| e.end_time = v),
            Message::EditorInterval(v) => self.with_editor(|e| e.interval = v),
            Message::EditorCount(v) => self.with_editor(|e| e.count = v),
            Message::EditorAllDay(v) => self.with_editor(|e| e.all_day = v),

            Message::EditorCalendar(index) => {
                let id = self.calendars().get(index).map(|c| c.id.clone());
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
                self.fire_due_reminders();
            }

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
    fn reload(&mut self) {
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
    fn fire_due_reminders(&mut self) {
        let Some(store) = self.store.as_ref() else {
            return;
        };

        let today = self.now.date();
        let occurrences =
            match store.occurrences(today, today + Duration::days(2), &self.config.hidden_set()) {
                Ok(occurrences) => occurrences,
                Err(why) => {
                    tracing::warn!(%why, "could not load occurrences to check reminders");
                    return;
                }
            };

        // Map an occurrence back to its event's alarms. The index lookup is by
        // UID, so every instance of a series inherits the series' alarms.
        let alarms_for = |occurrence: &Occurrence| {
            store
                .event(&occurrence.calendar_id, &occurrence.uid)
                .ok()
                .flatten()
                .map(|event| event.alarms)
                .unwrap_or_default()
        };

        let due = self.reminders.due(
            &occurrences,
            alarms_for,
            self.now,
            self.config.default_reminder(),
        );

        if !due.is_empty() {
            tracing::info!(count = due.len(), "firing reminders");
        }

        for reminder in &due {
            crate::reminders::notify(
                reminder,
                <Self as cosmic::Application>::APP_ID,
                self.reminder_body(reminder),
            );
        }

        // Keep the fired set from growing for the lifetime of the process.
        self.reminders.forget_before(self.now - Duration::days(1));
    }

    fn reminder_body(&self, reminder: &crate::reminders::Reminder) -> String {
        let when = if reminder.all_day {
            fl!("all-day")
        } else {
            let minutes = reminder.lead.num_minutes();
            if minutes <= 0 {
                fl!("reminder-now")
            } else {
                fl!("reminder-in", minutes = minutes.to_string())
            }
        };

        match &reminder.location {
            Some(location) if !location.is_empty() => format!("{when} · {location}"),
            _ => when,
        }
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

        widget::settings::section()
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
            .into()
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

        self.editor = Some(Editor::new(calendar, start, false));
        self.context_page = ContextPage::Editor;
        self.core.window.show_context = true;
        Task::none()
    }

    fn save_editor(&mut self) -> Task<cosmic::Action<Message>> {
        let Some(editor) = self.editor.as_ref() else {
            return Task::none();
        };
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

        let result = match moved_from {
            Some(original) => store
                .move_to_calendar(&original, &event.calendar_id)
                .and_then(|_| store.save(&event)),
            None => store.save(&event),
        };

        match result {
            Ok(()) => {
                self.editor = None;
                self.core.window.show_context = false;
                self.reload();
                Task::none()
            }
            Err(why) => self.toast_error(&format!("{}: {why}", fl!("error-save-event"))),
        }
    }

    fn delete_editor(&mut self) -> Task<cosmic::Action<Message>> {
        let Some(original) = self
            .editor
            .as_ref()
            .and_then(|e| e.original.as_ref())
            .cloned()
        else {
            return Task::none();
        };

        let Some(store) = self.store.as_mut() else {
            return Task::none();
        };

        match store.delete(&original.calendar_id, &original.uid) {
            Ok(()) => {
                self.editor = None;
                self.core.window.show_context = false;
                self.reload();
                Task::none()
            }
            Err(why) => self.toast_error(&format!("{}: {why}", fl!("error-delete-event"))),
        }
    }

    fn toast_error(&mut self, message: &str) -> Task<cosmic::Action<Message>> {
        tracing::error!(message);
        self.toasts
            .push(widget::Toast::new(message.to_owned()))
            .map(cosmic::Action::App)
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

fn view_label(kind: ViewKind) -> String {
    match kind {
        ViewKind::Month => fl!("month"),
        ViewKind::Week => fl!("week"),
        ViewKind::Day => fl!("day"),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn day(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
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
