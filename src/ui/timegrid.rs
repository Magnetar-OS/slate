// SPDX-License-Identifier: GPL-3.0-only

//! The week and day views: an hour-ruled grid with proportionally sized events.
//!
//! Each day column is a stack of three layers — the hour rules and click targets
//! underneath, the event blocks over them, and the "now" marker on top. Blocks are
//! placed and sized from their real start and duration, and events that overlap in
//! time are dealt columns side by side, the way every other calendar draws them.

use super::{
    HOUR_HEIGHT, MIN_BLOCK_HEIGHT, TODAY_BADGE, color_bar, dim_text, hour_rule, now_dot,
    now_marker, panel_surface, today_badge,
};
use crate::app::Message;
use crate::config::Config;
use crate::fl;
use crate::model::{CalendarMeta, Occurrence, Rgb};
use chrono::{Datelike, Duration, NaiveDate, NaiveDateTime, NaiveTime, Timelike};
use cosmic::iced::{Alignment, Length};
use cosmic::widget;
use cosmic::{Apply, Element};
use std::collections::BTreeMap;

/// Width of the hour-label gutter.
const GUTTER: f32 = 60.0;

/// Full height of the 24-hour ruler.
const GRID_HEIGHT: f32 = HOUR_HEIGHT * 24.0;

/// Identifies the hour-grid scrollable so the app can scroll it programmatically.
#[must_use]
pub fn scroll_id() -> cosmic::widget::Id {
    cosmic::widget::Id::new("cosmic-calendar-timegrid")
}

/// Vertical offset that puts `hour` at the top of the viewport.
#[must_use]
pub fn offset_for_hour(hour: u32) -> f32 {
    hour as f32 * HOUR_HEIGHT
}

/// One event, resolved to a rectangle within a day column.
struct Block<'a> {
    occurrence: &'a Occurrence,
    /// Distance from midnight, in pixels.
    top: f32,
    height: f32,
    /// Which of `columns` side-by-side slots this block occupies.
    column: usize,
    columns: usize,
    /// True when the event began before this day, so the label can say so.
    continues_from_earlier: bool,
}

pub struct TimeGrid<'a> {
    pub start: NaiveDate,
    /// 7 for the week view, 1 for the day view.
    pub days: i64,
    pub today: NaiveDate,
    /// Local wall-clock time, used to place the "now" marker.
    pub now: NaiveDateTime,
    pub occurrences: &'a BTreeMap<NaiveDate, Vec<Occurrence>>,
    pub calendars: &'a [CalendarMeta],
    pub config: &'a Config,
}

impl<'a> TimeGrid<'a> {
    #[must_use]
    pub fn view(&self) -> Element<'a, Message> {
        let spacing = cosmic::theme::spacing();

        widget::column::with_capacity(3)
            .spacing(spacing.space_xxs)
            .push(self.header_row())
            .push(self.all_day_band())
            .push(
                widget::scrollable(self.hour_grid())
                    .id(scroll_id())
                    .height(Length::Fill)
                    .width(Length::Fill),
            )
            .padding(spacing.space_xs)
            .into()
    }

    /// Copies the bounds out so the iterator borrows nothing from `self`.
    fn dates(&self) -> impl Iterator<Item = NaiveDate> + use<> {
        let start = self.start;
        (0..self.days).map(move |i| start + Duration::days(i))
    }

    fn header_row(&self) -> Element<'a, Message> {
        let spacing = cosmic::theme::spacing();
        let mut row = widget::row::with_capacity(self.days as usize + 1)
            .spacing(spacing.space_xxxs)
            .push(widget::Space::new().width(Length::Fixed(GUTTER)));

        for date in self.dates() {
            let is_today = date == self.today;

            let weekday =
                widget::text::caption(super::weekday_short(date.weekday()).to_uppercase())
                    .class(cosmic::theme::Text::Custom(dim_text));

            // The same accent badge the month grid uses, so "today" reads
            // identically wherever you are in the app.
            let number: Element<'a, Message> = if is_today {
                widget::text::title4(date.day().to_string())
                    .apply(widget::container)
                    .class(today_badge())
                    .width(Length::Fixed(TODAY_BADGE + 6.0))
                    .height(Length::Fixed(TODAY_BADGE + 6.0))
                    .align_x(Alignment::Center)
                    .align_y(Alignment::Center)
                    .into()
            } else {
                widget::text::title4(date.day().to_string()).into()
            };

            row = row.push(
                widget::column::with_capacity(2)
                    .spacing(2)
                    .push(weekday)
                    .push(number)
                    .align_x(Alignment::Center)
                    .apply(widget::container)
                    .width(Length::Fill)
                    .align_x(Alignment::Center),
            );
        }

        row.into()
    }

    /// All-day events get their own band above the ruler; they have no place on
    /// an hour axis.
    fn all_day_band(&self) -> Element<'a, Message> {
        let spacing = cosmic::theme::spacing();

        let has_any = self
            .dates()
            .any(|d| self.day_occurrences(d).iter().any(|o| o.all_day));

        if !has_any {
            return widget::Space::new().height(Length::Fixed(0.0)).into();
        }

        let mut row = widget::row::with_capacity(self.days as usize + 1)
            .spacing(spacing.space_xxxs)
            .push(
                widget::text::caption(fl!("all-day-events"))
                    .class(cosmic::theme::Text::Custom(dim_text))
                    .apply(widget::container)
                    .width(Length::Fixed(GUTTER))
                    .align_y(Alignment::Center),
            );

        for date in self.dates() {
            let mut column = widget::column::with_capacity(2).spacing(2);
            for occurrence in self.day_occurrences(date).iter().filter(|o| o.all_day) {
                column = column.push(super::month::chip_button(
                    occurrence,
                    self.color_of(occurrence),
                    self.config,
                    false,
                    Some(date),
                ));
            }
            row = row.push(column.width(Length::Fill));
        }

        row.apply(widget::container)
            .padding(spacing.space_xxs)
            .class(panel_surface())
            .width(Length::Fill)
            .into()
    }

    fn hour_grid(&self) -> Element<'a, Message> {
        let spacing = cosmic::theme::spacing();

        let mut row = widget::row::with_capacity(self.days as usize + 1)
            .spacing(spacing.space_xxxs)
            .push(self.hour_labels());

        for date in self.dates() {
            row = row.push(self.day_column(date));
        }

        row.height(Length::Fixed(GRID_HEIGHT)).into()
    }

    /// The gutter of hour labels, each sitting on its rule.
    fn hour_labels(&self) -> Element<'a, Message> {
        let mut column = widget::column::with_capacity(24);

        for hour in 0..24u32 {
            let time = NaiveTime::from_hms_opt(hour, 0, 0).unwrap_or(NaiveTime::MIN);
            column = column.push(
                widget::text::caption(super::format_time(time, self.config))
                    .class(cosmic::theme::Text::Custom(dim_text))
                    .apply(widget::container)
                    .width(Length::Fixed(GUTTER))
                    .height(Length::Fixed(HOUR_HEIGHT))
                    .align_x(Alignment::End)
                    .align_y(Alignment::Start)
                    .padding([0, 6, 0, 0]),
            );
        }

        column.into()
    }

    /// One day: rules and click targets, then event blocks, then the now marker.
    fn day_column(&self, date: NaiveDate) -> Element<'a, Message> {
        let mut layers: Vec<Element<'a, Message>> = vec![self.hour_cells(date)];

        for block in self.layout_day(date) {
            layers.push(self.block_element(block));
        }

        if date == self.today {
            layers.push(self.now_line());
        }

        cosmic::iced::widget::stack(layers)
            .width(Length::Fill)
            .height(Length::Fixed(GRID_HEIGHT))
            .into()
    }

    /// The background layer: an hour rule per row, each row a click target that
    /// creates an event at that hour.
    fn hour_cells(&self, date: NaiveDate) -> Element<'a, Message> {
        let mut column = widget::column::with_capacity(24);

        for hour in 0..24u32 {
            let cell = widget::column::with_capacity(2)
                .push(
                    widget::container(widget::Space::new().height(Length::Fixed(1.0)))
                        // Every third rule is stronger, giving the eye something to
                        // count by without ruling all 24 lines heavily.
                        .class(hour_rule(hour % 3 == 0))
                        .width(Length::Fill),
                )
                .push(widget::Space::new().height(Length::Fill));

            column = column.push(
                widget::mouse_area(cell.width(Length::Fill).height(Length::Fixed(HOUR_HEIGHT)))
                    .on_press(Message::NewEventAt(
                        date.and_hms_opt(hour, 0, 0)
                            .unwrap_or_else(|| date.and_time(NaiveTime::MIN)),
                    )),
            );
        }

        column.into()
    }

    /// Positions a block by spacing it away from the top of the column, and by
    /// giving it one of `columns` equal-width slots.
    fn block_element(&self, block: Block<'a>) -> Element<'a, Message> {
        let occurrence = block.occurrence;
        let color = self.color_of(occurrence);
        let spacing = cosmic::theme::spacing();

        let summary = if occurrence.summary.trim().is_empty() {
            fl!("untitled-event")
        } else {
            occurrence.summary.clone()
        };

        // Two independent constraints. A short block has room for one line only,
        // and a block sharing its slot with two or more others is too narrow for a
        // time range — in both cases the title is the part worth keeping.
        let tall_enough = block.height >= HOUR_HEIGHT * 0.75;
        let wide_enough = block.columns < 3;
        let roomy = tall_enough && wide_enough;

        let mut label = widget::column::with_capacity(3).push(
            widget::text::caption(summary)
                .wrapping(cosmic::iced::core::text::Wrapping::None)
                .width(Length::Fill),
        );

        if roomy {
            let mut time = super::format_range(occurrence.start, occurrence.end, self.config);
            if block.continues_from_earlier {
                time.insert_str(0, "↳ ");
            }
            label = label.push(
                widget::text::caption(time)
                    .wrapping(cosmic::iced::core::text::Wrapping::None)
                    .class(cosmic::theme::Text::Custom(dim_text)),
            );

            if let Some(location) = occurrence.location.as_deref().filter(|s| !s.is_empty())
                && block.height >= HOUR_HEIGHT * 1.4
                && block.columns == 1
            {
                label = label.push(
                    widget::text::caption(location.to_owned())
                        .wrapping(cosmic::iced::core::text::Wrapping::None)
                        .class(cosmic::theme::Text::Custom(dim_text)),
                );
            }
        }

        let mut body = widget::row::with_capacity(2).spacing(spacing.space_xxxs);
        if wide_enough {
            body = body.push(
                widget::container(widget::Space::new().width(Length::Fixed(3.0)))
                    .height(Length::Fill)
                    .class(color_bar(color)),
            );
        }
        let body = body.push(label);

        let button = widget::button::custom(body)
            .class(super::event_chip(color))
            .padding(2)
            .width(Length::Fill)
            .height(Length::Fixed(block.height))
            .on_press(Message::OpenEvent(
                occurrence.calendar_id.clone(),
                occurrence.uid.clone(),
            ));

        // When the label had to be abbreviated, hovering should still tell the
        // whole story.
        let block_button: Element<'a, Message> = if roomy {
            button.into()
        } else {
            widget::tooltip(
                button,
                widget::text::caption(self.full_label(occurrence)),
                widget::tooltip::Position::Top,
            )
            .into()
        };

        // Horizontal placement: an equal-width slot per overlapping column. The
        // empty `Space` slots let clicks through to the hour cells underneath.
        let mut slots = widget::row::with_capacity(block.columns).spacing(2);
        let mut placed = Some(block_button);
        for index in 0..block.columns {
            if index == block.column {
                slots = slots.push(
                    widget::container(placed.take().unwrap_or_else(|| widget::Space::new().into()))
                        .width(Length::FillPortion(1)),
                );
            } else {
                slots = slots.push(widget::Space::new().width(Length::FillPortion(1)));
            }
        }

        widget::column::with_capacity(2)
            .push(widget::Space::new().height(Length::Fixed(block.top)))
            .push(slots.height(Length::Fixed(block.height)))
            .width(Length::Fill)
            .into()
    }

    /// Everything about an occurrence, for the tooltip on an abbreviated block.
    fn full_label(&self, occurrence: &Occurrence) -> String {
        let mut out = format!(
            "{}\n{}",
            occurrence.summary,
            super::format_range(occurrence.start, occurrence.end, self.config)
        );
        if let Some(location) = occurrence.location.as_deref().filter(|s| !s.is_empty()) {
            out.push('\n');
            out.push_str(location);
        }
        out
    }

    /// The line across today's column at the current time.
    fn now_line(&self) -> Element<'a, Message> {
        let minutes = self.now.hour() as f32 * 60.0 + self.now.minute() as f32;
        let top = (minutes / 60.0) * HOUR_HEIGHT;

        let marker = widget::row::with_capacity(2)
            .align_y(Alignment::Center)
            .push(
                widget::container(widget::Space::new())
                    .width(Length::Fixed(8.0))
                    .height(Length::Fixed(8.0))
                    .class(now_dot()),
            )
            .push(
                widget::container(widget::Space::new().height(Length::Fixed(2.0)))
                    .width(Length::Fill)
                    .class(now_marker()),
            );

        widget::column::with_capacity(2)
            // Centre the 8px dot on the exact minute.
            .push(widget::Space::new().height(Length::Fixed((top - 4.0).max(0.0))))
            .push(marker)
            .width(Length::Fill)
            .into()
    }

    /// Resolves a day's timed events into positioned blocks that never overlap in x.
    fn layout_day(&self, date: NaiveDate) -> Vec<Block<'a>> {
        let day_start = date.and_time(NaiveTime::MIN);
        let day_end = day_start + Duration::days(1);

        // Clip each event to this day, so a run-over from yesterday starts at the
        // top of the column rather than off-screen above it.
        let mut spans: Vec<(&'a Occurrence, NaiveDateTime, NaiveDateTime)> = self
            .day_occurrences(date)
            .iter()
            .filter(|o| !o.all_day)
            .filter(|o| o.start < day_end && o.end > day_start)
            .map(|o| (o, o.start.max(day_start), o.end.min(day_end)))
            .collect();

        spans.sort_by_key(|(_, start, end)| (*start, *end));

        let mut blocks = Vec::with_capacity(spans.len());
        let mut cluster: Vec<usize> = Vec::new();
        let mut cluster_end: Option<NaiveDateTime> = None;

        // Walk clusters of mutually overlapping events; each is laid out
        // independently, so one busy hour does not narrow the whole day.
        for (i, (_, start, end)) in spans.iter().enumerate() {
            if let Some(current_end) = cluster_end
                && *start >= current_end
            {
                flush_cluster(&mut cluster, &spans, &mut blocks);
                cluster_end = None;
            }
            cluster.push(i);
            cluster_end = Some(cluster_end.map_or(*end, |e: NaiveDateTime| e.max(*end)));
        }
        flush_cluster(&mut cluster, &spans, &mut blocks);

        blocks
    }

    fn day_occurrences(&self, date: NaiveDate) -> &'a [Occurrence] {
        self.occurrences.get(&date).map_or(&[], Vec::as_slice)
    }

    fn color_of(&self, occurrence: &Occurrence) -> Rgb {
        self.calendars
            .iter()
            .find(|c| c.id == occurrence.calendar_id)
            .map_or(crate::model::DEFAULT_CALENDAR_COLOR, |c| c.color)
    }
}

/// Assigns side-by-side columns within one cluster of overlapping events.
///
/// Greedy interval partitioning: an event reuses the first column whose previous
/// event has already ended, so a gap in the middle of a busy stretch does not cost
/// an extra column.
fn flush_cluster<'a>(
    cluster: &mut Vec<usize>,
    spans: &[(&'a Occurrence, NaiveDateTime, NaiveDateTime)],
    blocks: &mut Vec<Block<'a>>,
) {
    if cluster.is_empty() {
        return;
    }

    let mut column_ends: Vec<NaiveDateTime> = Vec::new();
    let mut assigned: Vec<usize> = Vec::with_capacity(cluster.len());

    for &i in cluster.iter() {
        let (_, start, end) = spans[i];
        match column_ends.iter().position(|e| *e <= start) {
            Some(c) => {
                column_ends[c] = end;
                assigned.push(c);
            }
            None => {
                column_ends.push(end);
                assigned.push(column_ends.len() - 1);
            }
        }
    }

    let columns = column_ends.len();
    for (n, &i) in cluster.iter().enumerate() {
        let (occurrence, start, end) = spans[i];
        let top = minutes_from_midnight(start) / 60.0 * HOUR_HEIGHT;
        let height = ((minutes_from_midnight(end) - minutes_from_midnight(start)) / 60.0
            * HOUR_HEIGHT)
            .max(MIN_BLOCK_HEIGHT);

        blocks.push(Block {
            occurrence,
            top,
            height,
            column: assigned[n],
            columns,
            continues_from_earlier: occurrence.start < start,
        });
    }

    cluster.clear();
}

/// Minutes since midnight, as a float so partial hours place precisely.
fn minutes_from_midnight(dt: NaiveDateTime) -> f32 {
    dt.hour() as f32 * 60.0 + dt.minute() as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(h: u32, m: u32) -> NaiveDateTime {
        NaiveDate::from_ymd_opt(2026, 8, 4)
            .unwrap()
            .and_hms_opt(h, m, 0)
            .unwrap()
    }

    fn occurrence(summary: &str, start: NaiveDateTime, end: NaiveDateTime) -> Occurrence {
        Occurrence {
            uid: summary.into(),
            calendar_id: "personal".into(),
            summary: summary.into(),
            location: None,
            all_day: false,
            start,
            end,
            recurrence_id: None,
        }
    }

    /// Runs the layout and returns `(summary, top, height, column, columns)`.
    fn layout(occurrences: Vec<Occurrence>) -> Vec<(String, f32, f32, usize, usize)> {
        let date = NaiveDate::from_ymd_opt(2026, 8, 4).unwrap();
        let mut map = BTreeMap::new();
        map.insert(date, occurrences);
        let config = Config::default();
        let calendars: Vec<CalendarMeta> = Vec::new();

        let view = TimeGrid {
            start: date,
            days: 1,
            today: date,
            now: at(12, 0),
            occurrences: &map,
            calendars: &calendars,
            config: &config,
        };

        view.layout_day(date)
            .into_iter()
            .map(|b| {
                (
                    b.occurrence.summary.clone(),
                    b.top,
                    b.height,
                    b.column,
                    b.columns,
                )
            })
            .collect()
    }

    #[test]
    fn block_position_and_height_follow_the_clock() {
        let got = layout(vec![occurrence("A", at(9, 0), at(10, 30))]);
        assert_eq!(got.len(), 1);
        assert!((got[0].1 - 9.0 * HOUR_HEIGHT).abs() < 0.01, "wrong top");
        assert!((got[0].2 - 1.5 * HOUR_HEIGHT).abs() < 0.01, "wrong height");
    }

    #[test]
    fn a_very_short_event_still_gets_a_usable_height() {
        let got = layout(vec![occurrence("A", at(9, 0), at(9, 5))]);
        assert!(got[0].2 >= MIN_BLOCK_HEIGHT);
    }

    #[test]
    fn sequential_events_share_one_full_width_column() {
        let got = layout(vec![
            occurrence("A", at(9, 0), at(10, 0)),
            occurrence("B", at(10, 0), at(11, 0)),
        ]);
        // Touching but not overlapping — each should span the full width.
        assert!(
            got.iter().all(|b| b.4 == 1),
            "non-overlapping events were narrowed"
        );
    }

    #[test]
    fn overlapping_events_are_dealt_side_by_side() {
        let got = layout(vec![
            occurrence("A", at(9, 0), at(11, 0)),
            occurrence("B", at(10, 0), at(12, 0)),
        ]);
        assert!(
            got.iter().all(|b| b.4 == 2),
            "overlap did not split the column"
        );
        let columns: Vec<usize> = got.iter().map(|b| b.3).collect();
        assert_ne!(
            columns[0], columns[1],
            "overlapping events landed in one slot"
        );
    }

    #[test]
    fn three_way_overlap_uses_three_columns() {
        let got = layout(vec![
            occurrence("A", at(9, 0), at(12, 0)),
            occurrence("B", at(9, 30), at(11, 0)),
            occurrence("C", at(10, 0), at(10, 30)),
        ]);
        assert!(got.iter().all(|b| b.4 == 3));
    }

    #[test]
    fn a_freed_column_is_reused() {
        // C starts as A ends, so it should take A's slot rather than a third.
        let got = layout(vec![
            occurrence("A", at(9, 0), at(10, 0)),
            occurrence("B", at(9, 30), at(12, 0)),
            occurrence("C", at(10, 0), at(11, 0)),
        ]);
        assert!(got.iter().all(|b| b.4 == 2), "column was not reused");
    }

    #[test]
    fn separate_clusters_do_not_narrow_each_other() {
        let got = layout(vec![
            occurrence("A", at(9, 0), at(10, 0)),
            occurrence("B", at(9, 30), at(10, 30)),
            occurrence("Later", at(15, 0), at(16, 0)),
        ]);
        let later = got.iter().find(|b| b.0 == "Later").unwrap();
        assert_eq!(
            later.4, 1,
            "a busy morning narrowed an unrelated afternoon event"
        );
    }

    #[test]
    fn an_event_running_in_from_yesterday_is_clipped_to_the_top() {
        let yesterday_evening = NaiveDate::from_ymd_opt(2026, 8, 3)
            .unwrap()
            .and_hms_opt(22, 0, 0)
            .unwrap();
        let got = layout(vec![occurrence("Overnight", yesterday_evening, at(2, 0))]);
        assert_eq!(got.len(), 1);
        assert!((got[0].1 - 0.0).abs() < 0.01, "did not clip to midnight");
        assert!(
            (got[0].2 - 2.0 * HOUR_HEIGHT).abs() < 0.01,
            "wrong clipped height"
        );
    }

    #[test]
    fn all_day_events_stay_out_of_the_hour_grid() {
        let mut all_day = occurrence("Holiday", at(0, 0), at(23, 59));
        all_day.all_day = true;
        assert!(layout(vec![all_day]).is_empty());
    }
}
