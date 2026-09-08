// SPDX-License-Identifier: GPL-3.0-only

//! A `pop-launcher` plugin, so calendar events are searchable from the COSMIC
//! launcher.
//!
//! The protocol is newline-delimited JSON on stdin and stdout. Rather than depend
//! on `pop-launcher` — which would pull its whole dependency tree in for four
//! message shapes — the handful of messages used here are built directly.
//!
//! Requests arrive as `{"Search":"..."}`, `{"Activate":0}`, or the bare strings
//! `"Exit"` / `"Interrupt"`. Responses are `{"Append":{...}}` followed by
//! `"Finished"`, or `"Close"` after activating a result.

use chrono::{Datelike, Duration, NaiveDate};
use serde_json::{Value, json};
use slate::config::Config;
use slate::fl;
use slate::model::Occurrence;
use slate::store::Store;
use std::io::{BufRead, Write};

const APP_ID: &str = "io.github.entro314labs.Slate";

/// How far either side of today to search.
///
/// Wide enough to find "that thing next month", bounded so a one-letter query
/// does not expand a year of recurrences.
const PAST_DAYS: i64 = 30;
const FUTURE_DAYS: i64 = 180;

/// Results returned for one query.
const MAX_RESULTS: usize = 12;

fn main() {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "slate=warn".into()),
        )
        .init();

    // Results are shown to a person, so they follow the desktop's language.
    slate::i18n::init(&i18n_embed::DesktopLanguageRequester::requested_languages());

    let mut plugin = Plugin::new();
    let stdin = std::io::stdin();

    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        let Ok(request) = serde_json::from_str::<Value>(line) else {
            tracing::warn!(line, "unparseable request");
            continue;
        };

        if !plugin.handle(&request) {
            break;
        }
    }
}

struct Plugin {
    store: Option<Store>,
    config: Config,
    /// Results from the last search, indexed by the id sent to the launcher.
    results: Vec<Occurrence>,
    /// Drives the one async call this plugin makes: detaching the process it
    /// spawns. Built once because the plugin outlives many queries.
    runtime: Option<tokio::runtime::Runtime>,
}

impl Plugin {
    fn new() -> Self {
        let store = match Store::open_default() {
            Ok(store) => Some(store),
            Err(why) => {
                tracing::error!(%why, "launcher plugin cannot open the calendar store");
                None
            }
        };

        let runtime = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(runtime) => Some(runtime),
            Err(why) => {
                tracing::error!(%why, "no async runtime; results will not open");
                None
            }
        };

        Self {
            store,
            config: load_config(),
            results: Vec::new(),
            runtime,
        }
    }

    /// Handles one request. Returns `false` when the launcher wants us to exit.
    fn handle(&mut self, request: &Value) -> bool {
        // Unit variants arrive as bare strings.
        if let Some(name) = request.as_str() {
            match name {
                "Exit" => return false,
                // Interrupt cancels an in-flight search; ours are synchronous.
                "Interrupt" | "Quit" => return true,
                _ => return true,
            }
        }

        if let Some(query) = request.get("Search").and_then(Value::as_str) {
            self.search(query);
        } else if let Some(id) = request.get("Activate").and_then(Value::as_u64) {
            self.activate(id as usize);
        }

        true
    }

    fn search(&mut self, query: &str) {
        self.results.clear();

        // The launcher strips the plugin prefix, but be tolerant of both forms.
        let needle = query
            .trim()
            .trim_start_matches("cal ")
            .trim()
            .to_lowercase();

        if needle.is_empty() {
            send(&json!("Finished"));
            return;
        }

        // Settings may have changed since the plugin was started; it is long-lived.
        self.config = load_config();

        let Some(store) = self.store.as_mut() else {
            send(&json!("Finished"));
            return;
        };

        // Pick up anything added since the last query.
        if let Err(why) = store.refresh() {
            tracing::warn!(%why, "refresh failed");
        }

        let today = chrono::Local::now().date_naive();
        let occurrences = store
            .occurrences(
                today - Duration::days(PAST_DAYS),
                today + Duration::days(FUTURE_DAYS),
                &self.config.hidden_set(),
            )
            .unwrap_or_default();

        let mut matches: Vec<Occurrence> = occurrences
            .into_iter()
            .filter(|o| {
                o.summary.to_lowercase().contains(&needle)
                    || o.location
                        .as_deref()
                        .is_some_and(|l| l.to_lowercase().contains(&needle))
            })
            .collect();

        // Nearest to today first: a search for "standup" wants the next one, not
        // the one from three weeks ago.
        matches.sort_by_key(|o| (o.start.date() - today).num_days().abs());

        // One row per event, not per occurrence. Without this a daily standup
        // fills the whole result list with itself.
        let mut seen = std::collections::HashSet::new();
        matches.retain(|o| seen.insert((o.calendar_id.clone(), o.uid.clone())));

        matches.truncate(MAX_RESULTS);

        for (id, occurrence) in matches.iter().enumerate() {
            send(&json!({
                "Append": {
                    "id": id,
                    "name": display_name(occurrence),
                    "description": describe(occurrence, today, &self.config),
                    "keywords": Value::Null,
                    "icon": { "Name": "x-office-calendar" },
                    "exec": Value::Null,
                    "window": Value::Null,
                }
            }));
        }

        self.results = matches;
        send(&json!("Finished"));
    }

    fn activate(&mut self, id: usize) {
        if let Some(occurrence) = self.results.get(id) {
            let date = occurrence.start.date();

            // The canonical launch path: double fork + setsid underneath, plus
            // a systemd transient scope. A plain `Command::spawn` would leave
            // the calendar a child of this plugin, which pop-launcher owns and
            // reaps — closing the launcher would take the window with it. No
            // activation token here: a pop-launcher plugin owns no surface to
            // bind one to, and the app raises its existing window itself.
            match self.runtime.as_ref() {
                Some(runtime) => runtime.block_on(cosmic::desktop::spawn_desktop_exec(
                    format!("slate --date={date}"),
                    std::iter::empty::<(&str, &str)>(),
                    Some(APP_ID),
                    false,
                )),
                None => tracing::warn!("no runtime; cannot launch the calendar"),
            }
        }

        send(&json!("Close"));
    }
}

fn display_name(occurrence: &Occurrence) -> String {
    if occurrence.summary.trim().is_empty() {
        fl!("untitled-event")
    } else {
        occurrence.summary.clone()
    }
}

fn describe(occurrence: &Occurrence, today: NaiveDate, config: &Config) -> String {
    let date = occurrence.start.date();
    let day = match (date - today).num_days() {
        0 => fl!("today"),
        1 => fl!("tomorrow"),
        -1 => fl!("yesterday"),
        _ => format!(
            "{} {} {}",
            slate::ui::weekday_short(date.weekday()),
            date.day(),
            slate::ui::month_name(date.month())
        ),
    };

    let when = if occurrence.all_day {
        day
    } else {
        format!(
            "{day} · {}",
            slate::ui::format_time(occurrence.start.time(), config)
        )
    };

    match occurrence.location.as_deref().filter(|l| !l.is_empty()) {
        Some(location) => format!("{when} · {location}"),
        None => when,
    }
}

fn send(value: &Value) {
    let mut stdout = std::io::stdout().lock();
    if serde_json::to_writer(&mut stdout, value).is_ok() {
        let _ = stdout.write_all(b"\n");
        let _ = stdout.flush();
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
