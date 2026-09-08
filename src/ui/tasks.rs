// SPDX-License-Identifier: GPL-3.0-only

//! The task list.
//!
//! One flat, sorted list rather than a grid: a task's defining property is that
//! it may have no date at all, so there is no cell to put it in. The sort
//! (unfinished first, then by due date with undated last, then priority) lives
//! in [`cosmic_pim_core::model::Todo::sort_key`] so the daemon and any future
//! applet order things identically.

use chrono::{NaiveDate, NaiveDateTime};
use chrono_tz::Tz;
use cosmic::Element;
use cosmic::iced::{Alignment, Length};
use cosmic::prelude::*;
use cosmic::widget;

use crate::app::Message;
use crate::config::Config;
use crate::fl;
use crate::model::{CalendarMeta, EventTime, Todo};

pub struct TaskList<'a> {
    pub todos: &'a [Todo],
    pub calendars: &'a [CalendarMeta],
    pub config: &'a Config,
    pub now: NaiveDateTime,
    pub local: Tz,
    /// Whether finished tasks are shown.
    pub show_done: bool,
    /// The in-progress "add a task" text.
    pub draft: &'a str,
    /// The calendar a new task would land in, if there is a writable one.
    pub can_add: bool,
}

impl<'a> TaskList<'a> {
    #[must_use]
    pub fn view(&self) -> Element<'a, Message> {
        let spacing = cosmic::theme::spacing();

        let visible: Vec<&Todo> = self
            .todos
            .iter()
            .filter(|t| self.show_done || !t.is_done())
            .collect();

        let header = widget::row::with_capacity(4)
            .align_y(Alignment::Center)
            .spacing(spacing.space_xs)
            .push(widget::text::title4(fl!("tasks")))
            .push(widget::Space::new().width(Length::Fill))
            .push(widget::checkbox(self.show_done).on_toggle(|_| Message::ToggleShowCompleted))
            .push(
                widget::button::text(fl!("show-completed"))
                    .class(cosmic::theme::Button::Text)
                    .on_press(Message::ToggleShowCompleted),
            );

        // The add field lives above the list and commits on Enter — the
        // standard task-list gesture, and the reason a task does not need the
        // full event editor to exist.
        let add: Element<'a, Message> = if self.can_add {
            widget::text_input(fl!("new-task"), self.draft.to_owned())
                .on_input(Message::NewTaskChanged)
                .on_submit(|_| Message::NewTaskSubmit)
                .width(Length::Fill)
                .into()
        } else {
            widget::text::caption(fl!("no-writable-calendar"))
                .class(cosmic::theme::Text::Custom(crate::ui::dim_text))
                .into()
        };

        let body: Element<'a, Message> = if visible.is_empty() {
            widget::text::body(fl!("no-tasks"))
                .class(cosmic::theme::Text::Custom(crate::ui::dim_text))
                .into()
        } else {
            let mut list =
                widget::column::with_capacity(visible.len() + 5).spacing(spacing.space_xxs);

            for (label, todos) in self.sections(&visible) {
                if todos.is_empty() {
                    continue;
                }
                // A heading per group, so "what is late" and "what is just
                // someday" stop reading as one undifferentiated pile. The
                // count spares scrolling to learn how deep the hole is.
                list = list.push(
                    widget::text::caption(format!("{label} · {}", todos.len()))
                        .class(cosmic::theme::Text::Custom(crate::ui::dim_text))
                        .apply(widget::container)
                        .padding([spacing.space_xs, spacing.space_xxs, 0, spacing.space_xxs]),
                );
                for todo in todos {
                    list = list.push(self.row(todo));
                }
            }

            widget::scrollable(list).height(Length::Fill).into()
        };

        widget::column::with_capacity(3)
            .spacing(spacing.space_m)
            .padding(spacing.space_s)
            .push(header)
            .push(add)
            .push(body)
            .into()
    }

    /// Splits the visible tasks into the groups the list is read in.
    ///
    /// Order matters more than category: late things first, then today's, then
    /// the dated future, then the undated pile, and — only when shown —
    /// what's already finished. Within a group the incoming order is kept;
    /// [`Todo::sort_key`] already sorted by due date and priority.
    fn sections(&self, visible: &[&'a Todo]) -> [(String, Vec<&'a Todo>); 5] {
        let today: NaiveDate = self.now.date();

        let mut overdue = Vec::new();
        let mut due_today = Vec::new();
        let mut upcoming = Vec::new();
        let mut undated = Vec::new();
        let mut completed = Vec::new();

        for todo in visible {
            if todo.is_done() {
                completed.push(*todo);
            } else if todo.is_overdue(self.now, self.local) {
                overdue.push(*todo);
            } else {
                match todo.due_date(self.local) {
                    // Due later today — not yet overdue, but not "upcoming"
                    // either: this is the group the day is planned around.
                    Some(date) if date <= today => due_today.push(*todo),
                    Some(_) => upcoming.push(*todo),
                    None => undated.push(*todo),
                }
            }
        }

        [
            (fl!("task-section-overdue"), overdue),
            (fl!("task-section-today"), due_today),
            (fl!("task-section-upcoming"), upcoming),
            (fl!("task-section-no-date"), undated),
            (fl!("task-section-completed"), completed),
        ]
    }

    fn row(&self, todo: &'a Todo) -> Element<'a, Message> {
        let spacing = cosmic::theme::spacing();
        let overdue = todo.is_overdue(self.now, self.local);

        let colour = self
            .calendars
            .iter()
            .find(|c| c.id == todo.calendar_id)
            .map(|c| c.color)
            .unwrap_or(crate::model::DEFAULT_CALENDAR_COLOR);

        let title = widget::text::body(todo.summary.clone()).class(if todo.is_done() {
            // Struck-through text is not available here, so a dimmed title is
            // what carries "finished" alongside the ticked box.
            cosmic::theme::Text::Custom(crate::ui::dim_text)
        } else {
            cosmic::theme::Text::Default
        });

        let mut details = widget::row::with_capacity(3)
            .spacing(spacing.space_xs)
            .align_y(Alignment::Center);

        if let Some(due) = todo.due {
            let label = match due {
                EventTime::Date(d) => crate::ui::format_date_short(d),
                other => {
                    let naive = other.naive_local(self.local);
                    format!(
                        "{} {}",
                        crate::ui::format_date_short(naive.date()),
                        crate::ui::format_time(naive.time(), self.config)
                    )
                }
            };
            details = details.push(widget::text::caption(label).class(if overdue {
                cosmic::theme::Text::Custom(|theme| cosmic::iced::widget::text::Style {
                    color: Some(theme.cosmic().destructive_color().into()),
                    ..Default::default()
                })
            } else {
                cosmic::theme::Text::Custom(crate::ui::dim_text)
            }));
        }

        if todo.priority > 0 && todo.priority <= 4 {
            details = details.push(widget::text::caption(fl!("high-priority")).class(
                cosmic::theme::Text::Custom(|theme| cosmic::iced::widget::text::Style {
                    color: Some(theme.cosmic().warning_color().into()),
                    ..Default::default()
                }),
            ));
        }

        if todo.rrule.is_some() {
            // Enough to say the task comes back; the rule's shape belongs to
            // the editor, not a one-line list entry.
            details = details.push(
                widget::text::caption(fl!("repeats"))
                    .class(cosmic::theme::Text::Custom(crate::ui::dim_text)),
            );
        }

        for category in todo.categories.iter().take(3) {
            details = details.push(
                widget::text::caption(category.clone())
                    .class(cosmic::theme::Text::Custom(crate::ui::dim_text)),
            );
        }

        let text = widget::column::with_capacity(2).push(title).push(details);

        widget::row::with_capacity(3)
            .align_y(Alignment::Center)
            .spacing(spacing.space_xs)
            .padding([spacing.space_xxs, spacing.space_xs])
            .push(widget::checkbox(todo.is_done()).on_toggle({
                let calendar_id = todo.calendar_id.clone();
                let uid = todo.uid.clone();
                move |done| Message::TaskToggleDone(calendar_id.clone(), uid.clone(), done)
            }))
            .push(
                widget::container(widget::Space::new().height(Length::Fixed(28.0)))
                    .class(crate::ui::color_bar(colour))
                    .width(Length::Fixed(3.0)),
            )
            // The text opens the editor; the checkbox beside it stays a
            // one-click complete, which is the gesture a task list is used for
            // ninety per cent of the time.
            //
            // `mouse_area` rather than a text button: a button applies its own
            // class to the label, so every task title rendered in the accent
            // colour — which made the whole list look like links and, worse,
            // swallowed the red that marks an overdue task.
            .push(
                widget::mouse_area(
                    widget::container(text)
                        .width(Length::Fill)
                        .padding([spacing.space_xxxs, spacing.space_xxs]),
                )
                .on_press(Message::TaskEdit(
                    todo.calendar_id.clone(),
                    todo.uid.clone(),
                )),
            )
            .into()
    }
}
