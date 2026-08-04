// SPDX-License-Identifier: GPL-3.0-only

//! A COSMIC panel applet listing what is coming up.
//!
//! Reads the same vdir the app does, so it needs no IPC with it — and picks up
//! edits through the same filesystem watch.

use chrono::{Datelike, Duration, NaiveDate, NaiveDateTime};
use cosmic::Element;
use cosmic::app::{Core, Task};
use cosmic::iced::window::Id;
use cosmic::iced::{Length, Rectangle, Subscription};
use cosmic::surface::action::{app_popup, destroy_popup};
use cosmic::widget;
use cosmic_calendar::config::Config;
use cosmic_calendar::model::{CalendarMeta, Occurrence};
use cosmic_calendar::store::Store;
use cosmic_calendar::ui;

const ID: &str = "io.github.entro314labs.CalendarApplet";
const APP_ID: &str = "io.github.entro314labs.Calendar";

/// How far ahead the popup looks.
const HORIZON_DAYS: i64 = 7;

/// Most events listed before the popup stops growing.
const MAX_LISTED: usize = 12;

fn main() -> cosmic::iced::Result {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "cosmic_calendar=warn".into()),
        )
        .init();

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
}

#[derive(Clone, Debug)]
enum Message {
    PopupClosed(Id),
    Surface(cosmic::surface::Action),
    Tick,
    FilesChanged,
    OpenApp,
    OpenDate(NaiveDate),
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
        ])
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
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
                self.config = load_config();
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

            Message::OpenApp => launch(None),
            Message::OpenDate(date) => launch(Some(date)),
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
    /// Reloads the next week of events.
    fn reload(&mut self) {
        let Some(store) = self.store.as_ref() else {
            self.upcoming.clear();
            return;
        };

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
            None => "No upcoming events".to_owned(),
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
                widget::text::body("Nothing scheduled")
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

        column = column
            .push(cosmic::applet::padded_control(
                widget::divider::horizontal::default(),
            ))
            .push(
                cosmic::applet::menu_button(widget::text::body("Open Calendar"))
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
            .map_or(cosmic_calendar::model::DEFAULT_CALENDAR_COLOR, |c| c.color);

        let when = if occurrence.all_day {
            "All day".to_owned()
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
                    .wrapping(cosmic::iced::core::text::Wrapping::None)
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
        0 => "Today".to_owned(),
        1 => "Tomorrow".to_owned(),
        _ => format!(
            "{} {}",
            ui::weekday_short(date.weekday()),
            ui::format_date_short(date)
        ),
    }
}

/// Opens the full application, optionally at a given date.
fn launch(date: Option<NaiveDate>) {
    let mut command = std::process::Command::new("cosmic-calendar");
    if let Some(date) = date {
        command.arg(format!("--date={date}"));
    }

    match command.spawn() {
        Ok(_) => {}
        Err(why) => tracing::warn!(%why, "could not launch cosmic-calendar"),
    }
}

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

/// Same debounced vdir watch the app uses, so the applet follows external edits.
fn file_watch() -> Subscription<Message> {
    Subscription::run(|| {
        cosmic::iced::stream::channel(
            1,
            |mut output: cosmic::iced::futures::channel::mpsc::Sender<_>| async move {
                use cosmic::iced::futures::SinkExt;

                let root = cosmic_calendar::store::vdir::default_root();
                match cosmic_calendar::store::watcher::watch(&root) {
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
