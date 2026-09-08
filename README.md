# Slate

A calendar application for the [COSMIC desktop](https://github.com/pop-os/cosmic-epoch), built with
[libcosmic](https://github.com/pop-os/libcosmic).

Events and tasks are stored as plain iCalendar files in a
[vdir](https://vdirsyncer.pimutils.org/en/stable/vdir.html) layout, so they stay readable by `khal`,
Thunderbird, and anything else that speaks `.ics`.

**CalDAV sync is built in** — add an account under *Accounts* and Slate syncs it directly, in the app
and in the background daemon. You can instead point `vdirsyncer` at the same directory if you prefer;
what you must not do is both, on the same collection. See
[one sync engine per collection](https://github.com/entro314-labs/cosmic-pim/blob/main/ARCHITECTURE.md#one-sync-engine-per-collection).

## Part of a suite

Slate is one of three applications over a shared substrate,
[cosmic-pim](https://github.com/entro314-labs/cosmic-pim):

| App | Repository | What it is |
|---|---|---|
| **Slate** | you are here | Calendar and tasks |
| **Circle** | [circle](https://github.com/entro314-labs/circle) | Contacts |
| **Envelope** | [envelope](https://github.com/entro314-labs/envelope) | Mail (scaffold) |

The substrate owns everything below the user interface: the model, the
iCalendar/vCard layer, the vdir on disk, CalDAV and CardDAV, accounts and
credentials. This repository is the COSMIC front end and little else — which is
why it is small.

Two consequences worth knowing:

- **Accounts are shared.** A CalDAV account added here lives at
  `$XDG_CONFIG_HOME/cosmic-pim/accounts.toml` with its password in the OS
  keychain, and is already visible to Circle and Envelope. You enter it once.
- **Fixes are shared.** A bug in the sync reconciler is fixed in `cosmic-pim`
  and all three apps get it. Please do not work around substrate bugs here.

[cosmic-pim/ARCHITECTURE.md](https://github.com/entro314-labs/cosmic-pim/blob/main/ARCHITECTURE.md)
is the canonical description of how the layers fit and where new code belongs.

## Features

- **Month, week, day, agenda, and year views**, with a mini month for jumping around.
  The year view tints each day by how busy it is, so a free week is visible at a glance.
- **Multiple calendars** with per-calendar colours and visibility toggles.
- **Recurring events** — daily/weekly/monthly/yearly with an interval and an end condition.
  Rules the editor cannot express (`BYDAY`, `BYSETPOS`, …) are preserved verbatim rather than
  being flattened on save. Editing and deleting one instance offer all three scopes; see
  [Recurring events, in full](#recurring-events-in-full).
- **Direct manipulation.** Drag an event to move it, drag its lower edge to resize, drag empty
  space to create — with a live preview of where it will land, snapping to 5, 10, or 15 minutes.
  Working hours read as the bright band of the day.
- **Per-event time zones.** An event keeps whatever `TZID` it was written with and displays in
  it; start and end can differ, which is what a flight is. An optional second hour column shows
  another zone beside your own.
- **Quick add** with natural-language parsing — *"lunch with Maria Thu 13:00 at Kolonaki"* —
  in English and Greek, showing the parse before anything is committed.
- **Search** across every calendar (`Ctrl+F`), and **undo/redo** (`Ctrl+Z`) for the changes
  made here.
- **Meeting links** detected in an event's location or description (Meet, Zoom, Teams, Jitsi,
  BigBlueButton): a Join button on the event, and on its reminder notification.
- **Subscribe to calendar feeds** by URL (`webcal:`/`https:`), refreshed on their own schedule
  and marked read-only, so a public holiday calendar cannot be edited by accident.
- **Sync conflicts are resolvable in-app**, per field where the history allows it.
- **Picks up external changes.** A debounced filesystem watch means a `vdirsyncer` run or an edit
  in another app shows up without a restart.
- **Reminders** as desktop notifications, honouring each event's own `VALARM`,
  with an optional default per calendar or app-wide for events that carry none.
  Reminders missed while the machine slept are summarised once on resume rather
  than replayed or dropped silently.
- **Frosted glass**, following the COSMIC 1.3+ blur styling automatically.
- **Import and export** `.ics` files through the desktop's file portal, and open a `.ics` from a file
  manager or mail client — re-importing updates rather than duplicating.
- Localised through Fluent; settings persisted through `cosmic-config`.

Four binaries share one library:

| Binary | What it is |
| --- | --- |
| `slate` | The application. |
| `slate-applet` | Panel applet listing the next week. Add it in Settings → Desktop → Panel. |
| `slate-daemon` | Fires reminders with the window closed. `systemctl --user enable --now slate-daemon` |
| `slate-launcher` | pop-launcher plugin — type `cal ` in the launcher to search events. |

## Building

Requires Rust 1.98+ (pinned by `rust-toolchain.toml`) and the usual libcosmic build dependencies. On Arch/CachyOS:

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

`cosmic-pim` is a path dependency, so it needs to be checked out as a sibling
directory of this one.

Run the tests with `just test` (or `cargo test`) — the model and storage layers are covered
without needing a display. `just check-all` runs what CI runs: metadata
validation, formatting, clippy, and the test suite. The metadata pass needs
`appstream` and `desktop-file-utils`.

## Where things live

| Path | What |
| --- | --- |
| `~/.local/share/calendars/` | Your calendars. One directory per calendar, one `.ics` per event. |
| `~/.cache/cosmic-pim/index.sqlite` | Query index, shared with the suite. Pure cache — safe to delete, rebuilds itself. |
| `~/.config/cosmic/io.github.entro314labs.Slate/` | Settings, via `cosmic-config`. |
| `~/.config/cosmic-pim/accounts.toml` | Accounts, shared with Circle and Envelope. Passwords live in the OS keychain. |
| `/usr/share/pop-launcher/plugins/slate/` | Launcher plugin registration. |
| `/usr/lib/systemd/user/slate-daemon.service` | Reminder daemon unit. |

Set `COSMIC_PIM_CALENDAR_DIR` to point the app at a different calendar directory — handy for testing
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
                      ── this repository ──
ui/        Month grid, time grid, task list, editors, accounts page.
app.rs     State, messages, update loop.
key_bind.rs Keyboard shortcuts — read by both the menu bar and the key handler.
reminders/ Trigger scheduling (pure, tested), delivery, ownership arbitration.
bin/       applet, daemon, launcher — thin shells over the library above.
config.rs  Settings, via cosmic-config.

                      ── cosmic-pim ──
model      Events, tasks, contacts, recurrence expansion. No toolkit types.
ical       The one iCalendar parser the suite shares.
store      vdir files (source of truth) + SQLite index (cache) + watcher.
atomic     Crash-safe writes.
caldav     CalDAV/CardDAV protocol, reconciliation, writeback queue.
accounts   Accounts and credentials.
sync       One sync pass per account.
```

`model` and `store` are re-exported from `lib.rs`, so in-app paths stay
`crate::model::Event` and `crate::store::Store` — the substrate move is
invisible to the view code.

Two decisions worth knowing about:

**Files are the source of truth; SQLite is only a cache.** The index stores one row per event, and a
range query returns *candidates* which are then expanded in memory. Expanded occurrences are
deliberately not cached — expansion is cheap, and caching it would mean invalidating on timezone and
DST changes as well as on edits. The cache is keyed on schema version and timezone, and rebuilds
itself when either changes.

**Time is chrono; jiff appears only at one boundary.** The substrate's model and `rrule` are
chrono-based, so chrono is the domain type. libcosmic's `calendar` widget speaks `jiff::civil::Date`, so conversion
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
`com.system76.CosmicTheme.*` config. `RUST_LOG=slate=debug` logs the resolved blur state at
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

**Reminders with the window closed.** `slate-daemon` does nothing but watch the vdir and
notify. Both it and the app can see the same events, so they arbitrate over a D-Bus name: the daemon
claims `io.github.entro314labs.Slate.Reminders` at startup, and the app checks for it and stays
quiet while it is held. A second daemon bows out with a success exit code, because failing would put
systemd's `Restart=on-failure` into a loop.

**One window, whatever opens it.** The app runs through libcosmic's
`run_single_instance`, which serves `org.freedesktop.Application` on
`io.github.entro314labs.Slate`. That is what backs the `DBusActivatable=true` in
the desktop entry, and it means a second `slate` hands its command line to the
running window and exits rather than opening a second copy — whether that is a
file manager passing a `.ics`, the applet asking for a date, the launcher opening
a search result, or the desktop entry's *New Event* and *Today* actions. Import
is keyed on UID: opening the same file twice updates the events instead of
duplicating them. `COSMIC_SINGLE_INSTANCE=false` turns the hand-over off, which
is useful when running two builds side by side.

**Keyboard.** `Ctrl+N` new event, `Ctrl+Shift+N` quick add, `Ctrl+1`…`Ctrl+6`
switch view, `Ctrl+T` today, `Ctrl+Left`/`Ctrl+Right` page, `Ctrl+F` find,
`Ctrl+Z`/`Ctrl+Shift+Z` undo and redo, `Ctrl+I`/`Ctrl+E` import and export,
`Ctrl+R` sync, `Ctrl+,` settings. The bindings live in one map that the menu bar
reads to draw its accelerators, so a shortcut and its menu entry cannot
disagree.

**Launcher.** `cal <query>` searches summaries and locations across the previous month and the next
six, nearest-first, one row per event rather than per occurrence — otherwise a daily standup fills the
result list with itself.

## Recurring events, in full

Editing or deleting one instance of a repeating event asks how far the change
reaches, and all three answers are implemented on both sides:

| Scope | Edit | Delete |
| --- | --- | --- |
| *This event* | a `RECURRENCE-ID` override in the series' own file | an `EXDATE` on the master |
| *This and following* | the series is split: the master is truncated with `UNTIL`, a successor series carries the edit | `UNTIL` truncation, nothing succeeds it |
| *All events* | the master is edited | the file is removed |

A split re-homes any overrides past the cut onto the successor rather than
dropping them, and reduces a `COUNT` by the instances the master keeps — so the
total number of occurrences does not change. Overrides written by other clients
are honoured the same way: displayed in place of the instance they replace,
editable as themselves, and removable to restore the generated instance.

## Known limitations

- Undo covers the mutations this app makes (edits, deletes, splits, quick-add,
  task changes) for the current session only; it deliberately does not cover
  imports, sync, or conflict resolutions.
- The recurrence editor still expresses only daily/weekly/monthly/yearly with an
  interval and an end condition. Richer rules (`BYDAY`, `BYSETPOS`, …) are
  preserved verbatim and shown read-only rather than being editable.

## Licence

GPL-3.0-only for this application.

The [cosmic-pim](https://github.com/entro314-labs/cosmic-pim) substrate it links
is MPL-2.0 — deliberately, so that the engine can be shared with non-GPL
consumers while improvements to *it* stay public. MPL-2.0 is a Secondary Licence
under its own §3.3, so GPL-3 absorbs it and the distributed binary is GPL-3 as a
whole. See [LICENSING.md](LICENSING.md), and the fuller rationale in
[cosmic-pim/LICENSING.md](https://github.com/entro314-labs/cosmic-pim/blob/main/LICENSING.md).
