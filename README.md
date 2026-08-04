# Calendar for COSMIC

A calendar application for the [COSMIC desktop](https://github.com/pop-os/cosmic-epoch), built with
[libcosmic](https://github.com/pop-os/libcosmic).

Events are stored as plain iCalendar files in a [vdir](https://vdirsyncer.pimutils.org/en/stable/vdir.html)
layout, so they stay readable by `khal`, Thunderbird, and anything else that speaks `.ics` — and
CalDAV sync is a matter of pointing `vdirsyncer` at the same directory.

## Features

- **Month, week, and day views**, with a mini month for jumping around.
- **Multiple calendars** with per-calendar colours and visibility toggles.
- **Recurring events** — daily/weekly/monthly/yearly with an interval and an end condition.
  Rules the editor cannot express (`BYDAY`, `BYSETPOS`, …) are preserved verbatim rather than
  being flattened on save.
- **Picks up external changes.** A debounced filesystem watch means a `vdirsyncer` run or an edit
  in another app shows up without a restart.
- **Frosted glass**, following the COSMIC 1.3+ blur styling automatically.
- Localised through Fluent; settings persisted through `cosmic-config`.

## Building

Requires Rust 1.93+ and the usual libcosmic build dependencies. On Arch/CachyOS:

```sh
sudo pacman -S --needed rust cmake just pkgconf expat fontconfig freetype2 libxkbcommon
```

Then:

```sh
just build-release
sudo just install
```

Or with plain cargo:

```sh
cargo build --release
cargo run
```

Run the tests with `just test` (or `cargo test`) — the model and storage layers are covered
without needing a display.

## Where things live

| Path | What |
| --- | --- |
| `~/.local/share/calendars/` | Your calendars. One directory per calendar, one `.ics` per event. |
| `~/.cache/cosmic-calendar/index.sqlite` | Query index. Pure cache — safe to delete, rebuilds itself. |
| `~/.config/cosmic/io.github.entro314labs.Calendar/` | Settings, via `cosmic-config`. |

Set `COSMIC_CALENDAR_DIR` to point the app at a different calendar directory — handy for testing
against sample data without touching your real one.

A calendar directory looks like this, which is exactly what `vdirsyncer` expects:

```
~/.local/share/calendars/
├── personal/
│   ├── displayname        "Personal"
│   ├── color              "#2d7dd2"
│   └── 9f3ca1….ics
└── work/
    └── …
```

## Architecture

```
model/     Events, calendars, recurrence expansion. No toolkit types — testable headless.
store/
  vdir     Reads and writes the .ics files. Source of truth.
  index    SQLite range-query cache in front of them. Rebuildable.
  watcher  Debounced inotify, so external edits land in the UI.
ui/        Month grid, time grid, sidebar, event editor.
app.rs     State, messages, update loop.
```

Two decisions worth knowing about:

**Files are the source of truth; SQLite is only a cache.** The index stores one row per event, and a
range query returns *candidates* which are then expanded in memory. Expanded occurrences are
deliberately not cached — expansion is cheap, and caching it would mean invalidating on timezone and
DST changes as well as on edits. The cache is keyed on schema version and timezone, and rebuilds
itself when either changes.

**Time is chrono; jiff appears only at one boundary.** `icalendar` and `rrule` are chrono-based, so
chrono is the domain type. libcosmic's `calendar` widget speaks `jiff::civil::Date`, so conversion
happens there and nowhere else. Recurrence iterates in `DTSTART`'s own timezone, so a weekly 09:00
meeting stays at 09:00 across a DST boundary instead of drifting an hour.

## Frosted glass

libcosmic already opts applications in: `Settings::transparent` defaults to `true` and
`Core::auto_blur` covers windows and popups, so the compositor blurs whatever is behind the window.
The application's job is simply not to paint over it. Every custom surface here reads
`cosmic::Theme::transparent` — which the runtime sets from the compositor's blur state — and picks
the matching container from the theme, so the app tracks the desktop's blur setting rather than
hard-coding an opaque background.

## Known limitations

- The week and day grids place events in the row of the hour they start in, rather than drawing them
  at a height proportional to duration with overlapping events side by side. Each chip states its
  own end time so nothing is misleading, but a proper time-grid layout widget is still to come.
- No CalDAV sync built in — use `vdirsyncer` against the same directory.
- Editing a single occurrence of a recurring series is not yet supported; edits apply to the whole
  series. `EXDATE` is read and written, so exclusions made by other tools survive a round-trip.
- Timezone is read from the system; there is no per-event timezone picker yet.

## Licence

GPL-3.0-only.
