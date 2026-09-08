// SPDX-License-Identifier: GPL-3.0-only

//! Editing one task.
//!
//! Much smaller than the event editor, and deliberately so. An event has to
//! express a span, a repeat rule, and exceptions to it; a task has a title, a
//! note, an optional due date, and an importance. Building a task editor by
//! copying the event one would have inherited a repeat section that almost no
//! task uses and a start/end pair that a task does not have.
//!
//! The one shared idea is the string-typed fields: a half-typed time must not
//! have to survive a round trip through a parser on every keystroke.

use chrono::{NaiveDate, NaiveTime};
use chrono_tz::Tz;
use cosmic::iced::{Alignment, Length};
use cosmic::widget;
use cosmic::{Apply as _, Element};

use crate::app::Message;
use crate::config::Config;
use crate::fl;
use crate::model::{CalendarMeta, EventTime, Todo, TodoStatus};

/// The priority buckets the editor offers.
///
/// RFC 5545 priority is a 0–9 scale, which is more precision than anyone wants
/// from a dropdown. These map onto the RFC's own recommended bands, and a value
/// the user never touches is preserved exactly rather than being snapped to a
/// bucket — so a task another client set to 3 stays at 3.
pub const PRIORITY_CHOICES: [u8; 4] = [0, 1, 5, 9];

#[must_use]
pub fn priority_label(priority: u8) -> String {
    match priority {
        0 => fl!("priority-none"),
        1..=4 => fl!("priority-high"),
        5 => fl!("priority-medium"),
        _ => fl!("priority-low"),
    }
}

pub struct TaskEditor {
    /// `None` when creating; `Some` carries the uid and file name to keep.
    pub original: Option<Todo>,
    pub calendar_id: String,
    pub summary: String,
    pub description: String,
    pub has_due: bool,
    pub due_date: NaiveDate,
    /// Empty means the due date is date-valued (no particular time of day).
    pub due_time: String,
    pub priority: u8,
    pub status: TodoStatus,
    pub picker: widget::calendar::CalendarModel,
    pub picking: bool,
    pub error: Option<String>,
}

impl TaskEditor {
    #[must_use]
    pub fn existing(todo: Todo, local: Tz, config: &Config) -> Self {
        let (has_due, due_date, due_time) = match todo.due {
            Some(EventTime::Date(d)) => (true, d, String::new()),
            Some(other) => {
                let naive = other.naive_local(local);
                (
                    true,
                    naive.date(),
                    crate::ui::format_time(naive.time(), config),
                )
            }
            None => (false, chrono::Local::now().date_naive(), String::new()),
        };

        Self {
            calendar_id: todo.calendar_id.clone(),
            summary: todo.summary.clone(),
            description: todo.description.clone().unwrap_or_default(),
            priority: todo.priority,
            status: todo.status,
            picker: to_picker(due_date),
            picking: false,
            error: None,
            has_due,
            due_date,
            due_time,
            original: Some(todo),
        }
    }

    #[must_use]
    pub fn is_new(&self) -> bool {
        self.original.is_none()
    }

    /// Builds the task to save, or the reason it cannot be built.
    ///
    /// Preserves everything the editor does not surface — categories,
    /// `RELATED-TO`, alarms, the recurrence rule, the completion timestamp — by
    /// starting from the original rather than from a blank task. Rebuilding
    /// from the visible fields is how a subtask silently loses its parent.
    pub fn to_todo(&self, local: Tz) -> Result<Todo, String> {
        if self.summary.trim().is_empty() {
            return Err(fl!("error-summary-required"));
        }

        let mut todo = self
            .original
            .clone()
            .unwrap_or_else(|| Todo::draft(&self.calendar_id));

        todo.calendar_id = self.calendar_id.clone();
        todo.summary = self.summary.trim().to_owned();
        todo.description = Some(self.description.trim().to_owned()).filter(|d| !d.is_empty());
        todo.priority = self.priority;
        todo.status = self.status;

        todo.due = if self.has_due {
            if self.due_time.trim().is_empty() {
                Some(EventTime::Date(self.due_date))
            } else {
                let time = crate::ui::editor::parse_time(&self.due_time)
                    .ok_or_else(|| fl!("error-bad-time"))?;
                Some(EventTime::Zoned(self.due_date.and_time(time), local))
            }
        } else {
            None
        };

        // Keep the completion fields consistent with the status the dropdown
        // shows — the same invariant `Todo::set_done` maintains.
        if todo.status.is_closed() {
            if todo.completed.is_none() {
                todo.completed = Some(chrono::Utc::now());
            }
            if todo.status == TodoStatus::Completed {
                todo.percent_complete = 100;
            }
        } else {
            todo.completed = None;
            if todo.percent_complete == 100 {
                todo.percent_complete = 0;
            }
        }

        todo.last_modified = Some(chrono::Utc::now());
        todo.sequence = todo.sequence.saturating_add(1);
        Ok(todo)
    }

    pub fn view<'a>(&'a self, calendars: &'a [CalendarMeta]) -> Element<'a, Message> {
        let spacing = cosmic::theme::spacing();
        let mut column = widget::column::with_capacity(4).spacing(spacing.space_s);

        if let Some(error) = &self.error {
            column = column.push(
                widget::text::body(error.clone())
                    .class(cosmic::theme::Text::Custom(|theme| {
                        cosmic::iced::widget::text::Style {
                            color: Some(theme.cosmic().destructive_color().into()),
                            ..Default::default()
                        }
                    }))
                    .wrapping(cosmic::iced::core::text::Wrapping::Word),
            );
        }

        let writable: Vec<&CalendarMeta> = calendars.iter().filter(|c| !c.read_only).collect();
        let names: Vec<String> = writable.iter().map(|c| c.name.clone()).collect();
        let selected = writable.iter().position(|c| c.id == self.calendar_id);

        let statuses: Vec<String> = STATUS_CHOICES.iter().map(|s| status_label(*s)).collect();
        let priorities: Vec<String> = PRIORITY_CHOICES
            .iter()
            .map(|p| priority_label(*p))
            .collect();

        let mut section = widget::settings::section()
            .add(
                widget::settings::item::builder(fl!("task-summary")).control(
                    widget::text_input(fl!("task-summary"), &self.summary)
                        .on_input(Message::TaskEditorSummary)
                        .width(Length::Fixed(220.0)),
                ),
            )
            .add(
                widget::settings::item::builder(fl!("event-description")).control(
                    widget::text_input(fl!("event-description"), &self.description)
                        .on_input(Message::TaskEditorDescription)
                        .width(Length::Fixed(220.0)),
                ),
            )
            .add(
                widget::settings::item::builder(fl!("due"))
                    .toggler(self.has_due, |_| Message::TaskEditorToggleDue),
            );

        if self.has_due {
            section = section.add(
                widget::settings::item::builder(fl!("due-date")).control(
                    widget::row::with_capacity(2)
                        .spacing(spacing.space_xxs)
                        .align_y(Alignment::Center)
                        .push(
                            widget::button::standard(crate::ui::format_date_short(self.due_date))
                                .on_press(Message::TaskEditorPickDate),
                        )
                        .push(
                            // Blank means "no particular time", which is the
                            // normal shape for a task due date.
                            widget::text_input(fl!("all-day"), &self.due_time)
                                .on_input(Message::TaskEditorDueTime)
                                .width(Length::Fixed(84.0)),
                        ),
                ),
            );
        }

        section = section
            .add(
                widget::settings::item::builder(fl!("priority")).control(
                    widget::dropdown(
                        priorities,
                        PRIORITY_CHOICES
                            .iter()
                            .position(|p| bucket(*p) == bucket(self.priority)),
                        Message::TaskEditorPriority,
                    )
                    .width(Length::Fixed(140.0)),
                ),
            )
            .add(
                widget::settings::item::builder(fl!("status")).control(
                    widget::dropdown(
                        statuses,
                        STATUS_CHOICES.iter().position(|s| *s == self.status),
                        Message::TaskEditorStatus,
                    )
                    .width(Length::Fixed(140.0)),
                ),
            );

        if names.len() > 1 {
            section = section.add(
                widget::settings::item::builder(fl!("event-calendar")).control(
                    widget::dropdown(names, selected, Message::TaskEditorCalendar)
                        .width(Length::Fixed(140.0)),
                ),
            );
        }

        column = column.push(section);

        if self.picking {
            column = column.push(
                widget::calendar(
                    &self.picker,
                    Message::TaskEditorDatePicked,
                    || Message::TaskEditorPickerPrev,
                    || Message::TaskEditorPickerNext,
                    jiff::civil::Weekday::Monday,
                )
                .apply(widget::container)
                .padding(spacing.space_xxs),
            );
        }

        let mut buttons = widget::row::with_capacity(3).spacing(spacing.space_xs);
        if !self.is_new() {
            buttons = buttons.push(
                widget::button::text(fl!("delete"))
                    .class(cosmic::theme::Button::Destructive)
                    .on_press(Message::TaskEditorDelete),
            );
        }
        buttons = buttons.push(widget::Space::new().width(Length::Fill)).push(
            widget::button::text(fl!("save"))
                .class(cosmic::theme::Button::Suggested)
                .on_press(Message::TaskEditorSave),
        );

        column.push(buttons).into()
    }
}

const STATUS_CHOICES: [TodoStatus; 4] = [
    TodoStatus::NeedsAction,
    TodoStatus::InProcess,
    TodoStatus::Completed,
    TodoStatus::Cancelled,
];

fn status_label(status: TodoStatus) -> String {
    match status {
        TodoStatus::NeedsAction => fl!("status-needs-action"),
        TodoStatus::InProcess => fl!("status-in-process"),
        TodoStatus::Completed => fl!("status-completed"),
        TodoStatus::Cancelled => fl!("status-cancelled"),
    }
}

/// Which dropdown bucket a raw 0–9 priority falls in.
fn bucket(priority: u8) -> u8 {
    match priority {
        0 => 0,
        1..=4 => 1,
        5 => 5,
        _ => 9,
    }
}

fn to_picker(date: NaiveDate) -> widget::calendar::CalendarModel {
    let d = crate::ui::editor::to_jiff(date);
    widget::calendar::CalendarModel::new(d, d)
}

/// A blank time is legitimate here — it means a date-valued due date.
#[must_use]
pub fn parse_optional_time(input: &str) -> Option<Option<NaiveTime>> {
    if input.trim().is_empty() {
        return Some(None);
    }
    crate::ui::editor::parse_time(input).map(Some)
}
