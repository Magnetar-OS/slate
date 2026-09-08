// SPDX-License-Identifier: GPL-3.0-only

//! A COSMIC panel applet listing what is coming up.
//!
//! Reads the same vdir the app does, so it needs no IPC with it — and picks up
//! edits through the same filesystem watch.

use chrono::{Datelike, Duration, NaiveDate, NaiveDateTime};
use cosmic::Element;
use cosmic::app::{Core, Task};
use cosmic::applet::token::subscription::{
    TokenRequest, TokenUpdate, activation_token_subscription,
};
use cosmic::cctk::sctk::reexports::calloop;
use cosmic::iced::core::text::{Ellipsize, EllipsizeHeightLimit};
use cosmic::iced::window::Id;
use cosmic::iced::{Length, Rectangle, Subscription};
use cosmic::surface::action::{app_popup, destroy_popup};
use cosmic::widget;
use slate::config::Config;
use slate::fl;
use slate::model::{CalendarMeta, Occurrence, Todo};
use slate::store::Store;
use slate::ui;

const ID: &str = "io.github.entro314labs.SlateApplet";
const APP_ID: &str = "io.github.entro314labs.Slate";

/// How far ahead the popup looks.
const HORIZON_DAYS: i64 = 7;

/// Most events listed before the popup stops growing.
const MAX_LISTED: usize = 12;

fn main() -> cosmic::iced::Result {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "slate=warn".into()),
        )
        .init();

    // The applet shares the app's Fluent catalogue, but not its process — so it
    // has to select the language itself or every string falls back to English.
    slate::i18n::init(&i18n_embed::DesktopLanguageRequester::requested_languages());

    cosmic::applet::run::<Applet>(())
}

struct Applet {
    core: Core,
    popup: Option<Id>,
    store: Option<Store>,
    config: Config,
    today: NaiveDate,
    now: NaiveDateTime,
    upcoming: Vec<Occurrence>,
    /// Tasks due soon or already overdue.
    due_tasks: Vec<Todo>,
    /// Handle on the Wayland thread that mints XDG activation tokens. `None`
    /// until the subscription has started, and on non-Wayland sessions.
    token_tx: Option<calloop::channel::Sender<TokenRequest>>,
}

#[derive(Clone, Debug)]
enum Message {
    PopupClosed(Id),
    Surface(cosmic::surface::Action),
    Tick,
    FilesChanged,
    UpdateConfig(Config),
    OpenApp,
    OpenDate(NaiveDate),
    Token(TokenUpdate),
    /// A background task finished and has nothing to report.
    Ignore,
}

impl cosmic::Application for Applet {
    type Executor = cosmic::SingleThreadExecutor;
    type Flags = ();
    type Message = Message;
    const APP_ID: &'static str = ID;

    fn core(&self) -> &Core {
        &self.core
    }

    fn core_mut(&mut self) -> &mut Core {
        &mut self.core
    }

    fn init(core: Core, _flags: Self::Flags) -> (Self, Task<Message>) {
        let now = chrono::Local::now().naive_local();

        let store = match Store::open_default() {
            Ok(store) => Some(store),
            Err(why) => {
                tracing::error!(%why, "applet cannot open the calendar store");
                None
            }
        };

        let mut applet = Applet {
            core,
            popup: None,
            store,
            config: load_config(),
            today: now.date(),
            now,
            upcoming: Vec::new(),
            due_tasks: Vec::new(),
            token_tx: None,
        };
        applet.reload();

        (applet, Task::none())
    }

    fn on_close_requested(&self, id: Id) -> Option<Message> {
        Some(Message::PopupClosed(id))
    }

    fn subscription(&self) -> Subscription<Message> {
        Subscription::batch(vec![
            cosmic::iced::time::every(std::time::Duration::from_secs(60)).map(|_| Message::Tick),
            file_watch(),
            // Pushed by cosmic-settings-daemon rather than re-read on a timer:
            // a setting changed in the app should reach the panel at once, and
            // polling a file sixty times an hour to notice is the wrong trade.
            self.core()
                .watch_config::<Config>(APP_ID)
                .map(|update| Message::UpdateConfig(update.config)),
            // Mints the XDG activation token that lets the window we open take
            // focus instead of merely asking for attention.
            activation_token_subscription(0).map(Message::Token),
        ])
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Ignore => {}

            Message::PopupClosed(id) => {
                if self.popup.as_ref() == Some(&id) {
                    self.popup = None;
                }
            }

            Message::Surface(action) => {
                return cosmic::task::message(cosmic::Action::Cosmic(
                    cosmic::app::Action::Surface(action),
                ));
            }

            Message::Tick => {
                self.now = chrono::Local::now().naive_local();
                if self.now.date() != self.today {
                    self.today = self.now.date();
                }
                self.reload();
            }

            Message::UpdateConfig(config) => {
                self.config = config;
                self.reload();
            }

            Message::FilesChanged => {
                if let Some(store) = self.store.as_mut()
                    && let Err(why) = store.refresh()
                {
                    tracing::warn!(%why, "applet refresh failed");
                }
                self.reload();
            }

            Message::OpenApp => return self.launch(None),
            Message::OpenDate(date) => return self.launch(Some(date)),

            Message::Token(update) => match update {
                TokenUpdate::Init(tx) => self.token_tx = Some(tx),
                TokenUpdate::Finished => self.token_tx = None,
                // The compositor answered; `exec` is the command line we asked
                // about, handed back so concurrent requests cannot cross.
                TokenUpdate::ActivationToken { token, exec } => {
                    return cosmic::task::future(async move {
                        spawn(&exec, token).await;
                        Message::Ignore
                    });
                }
            },
        }

        Task::none()
    }

    fn view(&self) -> Element<'_, Message> {
        let have_popup = self.popup;

        let button = self
            .core
            .applet
            .icon_button_from_handle(widget::icon::from_name("x-office-calendar-symbolic").into())
            .on_press_with_rectangle(move |offset, bounds| {
                if let Some(id) = have_popup {
                    Message::Surface(destroy_popup(id))
                } else {
                    Message::Surface(app_popup::<Applet>(
                        |_| Default::default(),
                        move |state: &mut Applet| {
                            let new_id = Id::unique();
                            state.popup = Some(new_id);

                            let mut settings = state.core.applet.get_popup_settings(
                                state.core.main_window_id().unwrap_or(Id::NONE),
                                new_id,
                                None,
                                None,
                                None,
                            );
                            settings.positioner.anchor_rect = Rectangle {
                                x: (bounds.x - offset.x) as i32,
                                y: (bounds.y - offset.y) as i32,
                                width: bounds.width as i32,
                                height: bounds.height as i32,
                            };
                            settings
                        },
                        Some(Box::new(move |state: &Applet| {
                            Element::from(state.core.applet.popup_container(state.popup_content()))
                                .map(cosmic::Action::App)
                        })),
                    ))
                }
            });

        Element::from(self.core.applet.applet_tooltip::<Message>(
            button,
            self.tooltip(),
            self.popup.is_some(),
            Message::Surface,
            None,
        ))
    }

    fn view_window(&self, _id: Id) -> Element<'_, Message> {
        // Popup content is supplied through the surface action above.
        widget::Space::new().into()
    }

    fn style(&self) -> Option<cosmic::iced::theme::Style> {
        Some(cosmic::applet::style())
    }
}

impl Applet {
    /// Opens the full application, optionally on a given date.
    ///
    /// Two paths, because the token is only obtainable on Wayland and only
    /// asynchronously. With a token channel we ask for one and spawn when it
    /// comes back; without, we spawn straight away rather than not at all.
    fn launch(&self, date: Option<NaiveDate>) -> Task<Message> {
        let exec = exec_line(date);

        if let Some(tx) = self.token_tx.as_ref() {
            let request = TokenRequest {
                app_id: APP_ID.to_owned(),
                exec: exec.clone(),
            };
            match tx.send(request) {
                // The reply arrives as `TokenUpdate::ActivationToken`, which is
                // where the process is actually started.
                Ok(()) => return Task::none(),
                Err(why) => tracing::warn!(%why, "activation token request failed"),
            }
        }

        cosmic::task::future(async move {
            spawn(&exec, None).await;
            Message::Ignore
        })
    }

    /// Reloads the next week of events.
    fn reload(&mut self) {
        let Some(store) = self.store.as_ref() else {
            self.upcoming.clear();
            self.due_tasks.clear();
            return;
        };

        // Tasks worth interrupting someone for: overdue, or due inside the
        // horizon. An undated task is deliberately excluded — "someday" items
        // would fill the popup and push out everything time-sensitive, which
        // is the opposite of what a panel glance is for.
        let local = store.local_timezone();
        let horizon = self.today + Duration::days(HORIZON_DAYS);
        self.due_tasks = store
            .todos(&self.config.hidden_set())
            .into_iter()
            .filter(|todo| !todo.is_done())
            .filter(|todo| {
                todo.is_overdue(self.now, local)
                    || todo.due_date(local).is_some_and(|due| due < horizon)
            })
            .take(MAX_LISTED)
            .collect();

        let from = self.today;
        let to = from + Duration::days(HORIZON_DAYS);

        match store.occurrences(from, to, &self.config.hidden_set()) {
            Ok(occurrences) => {
                // Anything already finished is not "upcoming".
                self.upcoming = occurrences
                    .into_iter()
                    .filter(|o| o.all_day || o.end > self.now)
                    .take(MAX_LISTED)
                    .collect();
            }
            Err(why) => {
                tracing::warn!(%why, "applet could not load occurrences");
                self.upcoming.clear();
            }
        }
    }

    fn tooltip(&self) -> String {
        match self.upcoming.first() {
            Some(next) if next.all_day => next.summary.clone(),
            Some(next) => format!(
                "{} · {}",
                ui::format_time(next.start.time(), &self.config),
                next.summary
            ),
            None => fl!("no-upcoming-events"),
        }
    }

    fn popup_content(&self) -> Element<'_, Message> {
        let spacing = cosmic::theme::spacing();
        let calendars = self.store.as_ref().map_or(&[][..], Store::calendars);

        let mut column = widget::column::with_capacity(MAX_LISTED + 3)
            .spacing(spacing.space_xxs)
            .padding([spacing.space_xxs, 0]);

        column = column.push(cosmic::applet::padded_control(widget::text::title4(
            format!(
                "{} {}",
                ui::month_name(self.today.month()),
                self.today.day()
            ),
        )));

        if self.upcoming.is_empty() {
            column = column.push(cosmic::applet::padded_control(
                widget::text::body(fl!("nothing-scheduled"))
                    .class(cosmic::theme::Text::Custom(ui::dim_text)),
            ));
        }

        let mut last_date: Option<NaiveDate> = None;
        for occurrence in &self.upcoming {
            let date = occurrence.start.date().max(self.today);

            // A date heading whenever the day changes, so a week's worth of
            // events does not read as one flat list.
            if last_date != Some(date) {
                column = column.push(cosmic::applet::padded_control(
                    widget::text::caption(day_heading(date, self.today))
                        .class(cosmic::theme::Text::Custom(ui::dim_text)),
                ));
                last_date = Some(date);
            }

            column = column.push(self.row(occurrence, calendars));
        }

        if !self.due_tasks.is_empty() {
            column = column
                .push(cosmic::applet::padded_control(
                    widget::divider::horizontal::default(),
                ))
                .push(cosmic::applet::padded_control(
                    widget::text::caption(fl!("tasks"))
                        .class(cosmic::theme::Text::Custom(ui::dim_text)),
                ));

            let local = self
                .store
                .as_ref()
                .map_or(chrono_tz::UTC, slate::store::Store::local_timezone);

            for todo in &self.due_tasks {
                let overdue = todo.is_overdue(self.now, local);
                let due = todo
                    .due_date(local)
                    .map(ui::format_date_short)
                    .unwrap_or_default();

                column = column.push(cosmic::applet::padded_control(
                    widget::row::with_capacity(3)
                        .spacing(spacing.space_xxs)
                        .push(
                            widget::text::body(todo.summary.clone())
                                .ellipsize(Ellipsize::End(EllipsizeHeightLimit::Lines(1))),
                        )
                        .push(widget::Space::new().width(Length::Fill))
                        .push(widget::text::caption(due).class(if overdue {
                            cosmic::theme::Text::Custom(|theme| cosmic::iced::widget::text::Style {
                                color: Some(theme.cosmic().destructive_color().into()),
                                ..Default::default()
                            })
                        } else {
                            cosmic::theme::Text::Custom(ui::dim_text)
                        })),
                ));
            }
        }

        column = column
            .push(cosmic::applet::padded_control(
                widget::divider::horizontal::default(),
            ))
            .push(
                cosmic::applet::menu_button(widget::text::body(fl!("open-calendar")))
                    .on_press(Message::OpenApp),
            );

        widget::scrollable(column).height(Length::Shrink).into()
    }

    fn row<'a>(
        &'a self,
        occurrence: &'a Occurrence,
        calendars: &'a [CalendarMeta],
    ) -> Element<'a, Message> {
        let spacing = cosmic::theme::spacing();
        let color = calendars
            .iter()
            .find(|c| c.id == occurrence.calendar_id)
            .map_or(slate::model::DEFAULT_CALENDAR_COLOR, |c| c.color);

        let when = if occurrence.all_day {
            fl!("all-day")
        } else {
            ui::format_time(occurrence.start.time(), &self.config)
        };

        let row = widget::row::with_capacity(3)
            .align_y(cosmic::iced::Alignment::Center)
            .spacing(spacing.space_xs)
            .push(
                widget::container(widget::Space::new().width(Length::Fixed(3.0)))
                    .height(Length::Fixed(20.0))
                    .class(ui::color_bar(color)),
            )
            .push(
                widget::text::caption(when)
                    .class(cosmic::theme::Text::Custom(ui::dim_text))
                    .width(Length::Fixed(56.0)),
            )
            .push(
                widget::text::body(occurrence.summary.clone())
                    // Summaries are whoever-wrote-the-event's text at whatever
                    // length; an ellipsis reads better than a hard clip.
                    .ellipsize(Ellipsize::End(EllipsizeHeightLimit::Lines(1)))
                    .width(Length::Fill),
            );

        cosmic::applet::menu_button(row)
            .on_press(Message::OpenDate(occurrence.start.date()))
            .into()
    }
}

fn day_heading(date: NaiveDate, today: NaiveDate) -> String {
    let delta = (date - today).num_days();
    match delta {
        0 => fl!("today"),
        1 => fl!("tomorrow"),
        _ => format!(
            "{} {}",
            ui::weekday_short(date.weekday()),
            ui::format_date_short(date)
        ),
    }
}

/// The settings as they stand at start-up.
///
/// [`Core::watch_config`] reports changes from the moment it subscribes, so the
/// first value still has to be read directly.
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

/// The command line that opens the app, optionally on a given date.
///
/// Round-trips through the activation-token request as a string, so it is built
/// from values that never need quoting — an ISO date and a binary name.
fn exec_line(date: Option<NaiveDate>) -> String {
    match date {
        Some(date) => format!("slate --date={date}"),
        None => "slate".to_owned(),
    }
}

/// Detaches the app from the panel and hands it the activation token.
///
/// `spawn_desktop_exec` is the canonical launch path: a double fork with
/// `setsid` underneath — a plain `Command::spawn` would leave the window a
/// child of the panel, so restarting the panel would take the calendar with
/// it — plus a systemd transient scope, so the new process belongs to the
/// session. The token, passed under both names for Wayland and XWayland, is
/// what allows the window to raise itself; without it the compositor treats
/// it as focus-stealing and leaves it flashing in the dock.
async fn spawn(exec: &str, token: Option<String>) {
    let mut env = Vec::new();
    match token {
        Some(token) => {
            env.push(("XDG_ACTIVATION_TOKEN", token.clone()));
            env.push(("DESKTOP_STARTUP_ID", token));
        }
        // Not fatal. The window still opens; it just may not be given focus.
        None => tracing::debug!(exec, "no activation token; launching without one"),
    }

    cosmic::desktop::spawn_desktop_exec(exec, env, Some(APP_ID), false).await;
}

/// Same debounced vdir watch the app uses, so the applet follows external edits.
fn file_watch() -> Subscription<Message> {
    Subscription::run(|| {
        cosmic::iced::stream::channel(
            1,
            |mut output: cosmic::iced::futures::channel::mpsc::Sender<_>| async move {
                use cosmic::iced::futures::SinkExt;

                let root = slate::store::vdir::default_root();
                match slate::store::watcher::watch(&root) {
                    Ok((_watch, mut rx)) => {
                        while rx.recv().await.is_some() {
                            if output.send(Message::FilesChanged).await.is_err() {
                                break;
                            }
                        }
                    }
                    Err(why) => {
                        tracing::warn!(%why, "applet cannot watch the calendar directory");
                        std::future::pending::<()>().await;
                    }
                }
            },
        )
    })
}
