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

use chrono::{Duration, NaiveDate};
use serde_json::{Value, json};
use slate::config::Config;
use slate::fl;
use slate::model::Occurrence;
use slate::store::Store;
use std::io::{BufRead, Write};

const APP_ID: &str = "com.magnetaros.Slate";

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
        match incoming(request) {
            Incoming::Search(query) => self.search(query),
            Incoming::Activate(id) => self.activate(id),
            Incoming::Exit => return false,
            Incoming::Other => {}
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

        if let Err(why) = slate::clock::follow_timezone(store) {
            tracing::warn!(%why, "could not follow the timezone change");
        }
        let today = slate::clock::now_in(store.local_timezone()).date();
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
            send(&append(
                id,
                &display_name(occurrence),
                &describe(occurrence, today, &self.config),
            ));
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

/// What the launcher asked for, from one request line.
#[derive(Debug, PartialEq, Eq)]
enum Incoming<'a> {
    Search(&'a str),
    Activate(usize),
    Exit,
    /// Anything this plugin has nothing to do for: `Interrupt` (our searches
    /// are synchronous), `Close`, `Quit`, context requests.
    Other,
}

fn incoming(request: &Value) -> Incoming<'_> {
    // Unit variants arrive as bare strings.
    if request.as_str() == Some("Exit") {
        return Incoming::Exit;
    }
    if let Some(query) = request.get("Search").and_then(Value::as_str) {
        return Incoming::Search(query);
    }
    request
        .get("Activate")
        .and_then(Value::as_u64)
        .and_then(|id| usize::try_from(id).ok())
        .map_or(Incoming::Other, Incoming::Activate)
}

/// One result row, as pop-launcher's `PluginResponse::Append`.
fn append(id: usize, name: &str, description: &str) -> Value {
    json!({
        "Append": {
            "id": id,
            "name": name,
            "description": description,
            "keywords": Value::Null,
            "icon": { "Name": "x-office-calendar" },
            "exec": Value::Null,
            "window": Value::Null,
        }
    })
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
        _ => slate::ui::format_day(date),
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

/// The protocol is built by hand (see the module docs); these hold it to
/// pop-launcher's own types, so a drift in either shows up here rather than
/// as a plugin that silently shows nothing.
#[cfg(test)]
mod tests {
    use super::*;
    use pop_launcher::{PluginResponse, Request};

    /// The manifest, without its comments.
    fn manifest() -> String {
        include_str!("../../resources/launcher/plugin.ron")
            .lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// pop-launcher reads the manifest with RON's `implicit_some`, where
    /// `Some(...)` is a parse error: it logs "malformed config" to its own
    /// log and skips the plugin. Every optional field was written that way
    /// once, and the plugin was never loaded.
    #[test]
    fn the_manifest_writes_optional_fields_the_way_pop_launcher_reads_them() {
        assert!(!manifest().contains("Some("), "{}", manifest());
    }

    /// `bin.path` is resolved in the plugin's own directory, where the
    /// justfile links this binary in under its own name.
    #[test]
    fn the_manifest_runs_this_binary() {
        let path = format!("path: \"{}\"", env!("CARGO_BIN_NAME"));
        assert!(manifest().contains(&path), "{}", manifest());
    }

    /// Without a `regex` pop-launcher sends a plugin every query typed, and
    /// its results sit among the applications.
    #[test]
    fn the_manifest_asks_only_for_queries_with_the_prefix() {
        assert!(manifest().contains("regex: \"^cal \""), "{}", manifest());
        assert!(manifest().contains("isolate: true"), "{}", manifest());
    }

    #[test]
    fn the_launchers_requests_are_understood() {
        let search = serde_json::to_value(Request::Search("cal standup".into())).unwrap();
        assert_eq!(incoming(&search), Incoming::Search("cal standup"));

        let activate = serde_json::to_value(Request::Activate(3)).unwrap();
        assert_eq!(incoming(&activate), Incoming::Activate(3));

        let exit = serde_json::to_value(Request::Exit).unwrap();
        assert_eq!(incoming(&exit), Incoming::Exit);

        for other in [Request::Interrupt, Request::Close, Request::Quit(0)] {
            let value = serde_json::to_value(other).unwrap();
            assert_eq!(incoming(&value), Incoming::Other);
        }
    }

    #[test]
    fn our_responses_are_what_the_launcher_reads() {
        let row = serde_json::from_value::<PluginResponse>(append(2, "Standup", "Today · 09:00"))
            .unwrap();
        let PluginResponse::Append(result) = row else {
            panic!("not an Append: {row:?}");
        };
        assert_eq!(result.id, 2);
        assert_eq!(result.name, "Standup");
        assert_eq!(result.description, "Today · 09:00");

        assert!(matches!(
            serde_json::from_value::<PluginResponse>(json!("Finished")).unwrap(),
            PluginResponse::Finished
        ));
        assert!(matches!(
            serde_json::from_value::<PluginResponse>(json!("Close")).unwrap(),
            PluginResponse::Close
        ));
    }
}
