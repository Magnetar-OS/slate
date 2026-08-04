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
- **Reminders** as desktop notifications, honouring each event's own `VALARM`,
  with an optional app-wide default for events that carry none.
- **Frosted glass**, following the COSMIC 1.3+ blur styling automatically.
- **Import and export** `.ics` files through the desktop's file portal, and open a `.ics` from a file
  manager or mail client — re-importing updates rather than duplicating.
- Localised through Fluent; settings persisted through `cosmic-config`.

Four binaries share one library:

| Binary | What it is |
| --- | --- |
| `cosmic-calendar` | The application. |
| `cosmic-calendar-applet` | Panel applet listing the next week. Add it in Settings → Desktop → Panel. |
| `cosmic-calendar-daemon` | Fires reminders with the window closed. `systemctl --user enable --now cosmic-calendar-daemon` |
| `cosmic-calendar-launcher` | pop-launcher plugin — type `cal ` in the launcher to search events. |

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
| `/usr/share/pop-launcher/plugins/calendar/` | Launcher plugin registration. |
| `/usr/lib/systemd/user/cosmic-calendar-daemon.service` | Reminder daemon unit. |

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
reminders/ Trigger scheduling (pure, tested), delivery, and ownership arbitration.
app.rs     State, messages, update loop.
bin/       applet, daemon, launcher — thin shells over the library above.
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
hard-coding an opaque background. Day cells in the month grid are deliberately unfilled for the same
reason: 42 opaque rectangles would hide the very effect the window is asking for.

Whether it actually appears is the desktop's call, from `frosted_windows` in the active
`com.system76.CosmicTheme.*` config. `RUST_LOG=cosmic_calendar=debug` logs the resolved blur state at
startup, which is the quickest way to tell "the app is painting over the blur" apart from "the
desktop has blur switched off".

## Reminders

Each event's own `VALARM` triggers are honoured — parsed on read, written back on save, so alarms set
in another client survive a round-trip. Settings offers a default lead time for events that carry no
alarm of their own; it is off by default, because an app that starts notifying about everything
without being asked is one people uninstall.

Delivery goes through `org.freedesktop.Notifications`, which `cosmic-notifications` implements, so
reminders land in COSMIC's own notification centre with no COSMIC-specific code.

Two deliberate details: a reminder fires at most once per occurrence (two instances of a weekly
series are distinct reminders, but re-reading the same file is not), and a trigger more than five
minutes stale is dropped — otherwise opening the app in the evening would replay the whole day.

## Desktop integration

**Reminders with the window closed.** `cosmic-calendar-daemon` does nothing but watch the vdir and
notify. Both it and the app can see the same events, so they arbitrate over a D-Bus name: the daemon
claims `io.github.entro314labs.Calendar.Reminders` at startup, and the app checks for it and stays
quiet while it is held. A second daemon bows out with a success exit code, because failing would put
systemd's `Restart=on-failure` into a loop.

**Opening a `.ics`.** The desktop entry is `DBusActivatable`, so a file manager or mail client hands
the file to the already-running instance through `dbus_activation` rather than starting a second copy.
Import is keyed on UID: opening the same file twice updates the events instead of duplicating them.

**Launcher.** `cal <query>` searches summaries and locations across the previous month and the next
six, nearest-first, one row per event rather than per occurrence — otherwise a daily standup fills the
result list with itself.

## Known limitations

- No CalDAV sync built in — use `vdirsyncer` against the same directory.
- Editing a single occurrence of a recurring series is not yet supported; edits apply to the whole
  series. `EXDATE` is read and written, so exclusions made by other tools survive a round-trip.
- Timezone is read from the system; there is no per-event timezone picker yet.

## Licence

GPL-3.0-only.
