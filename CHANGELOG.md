# Changelog

All notable user-facing changes to Slate. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

### Added

- A year view: twelve mini months with each day tinted by how busy it is, for
  the "when am I free in March" question the other views answer badly. `Ctrl+5`;
  the task list moves to `Ctrl+6`.
- Per-calendar defaults. A calendar can set its own reminder lead time and its
  own length for new events, overriding the app-wide settings; both are in
  Settings, under the calendar's name.
- A missed-reminder summary. Reminders whose moment passed while the machine
  slept are still not replayed one by one, but on resume you now get a single
  "N reminders passed while this machine was asleep" instead of silence.

- A collapsible sidebar. The header now carries the standard COSMIC nav-bar
  toggle, mirrored by View → Sidebar and `Ctrl+B`. It was previously always
  open, and vanished with no way back when the window was narrowed.
- The sidebar has three widths, and slides between them. Each press of the
  toggle takes it a step narrower — full, then a rail of calendar swatches
  with names in tooltips and a press to show or hide each, then gone — and
  from gone back to full. The slide is 200 ms and reverses mid-way if you
  change your mind; libcosmic's own nav bar snaps. Narrowing the window still
  hides the sidebar and widening it brings it back at the width you left it.
- Editing "this and following" occurrences of a repeating event. Saving a
  change now offers all three scopes the delete side already had: this
  occurrence, this and everything after it, or the whole series. Choosing "this
  and following" splits the series — the earlier occurrences keep the original,
  a successor carries the change — preserving the total number of occurrences
  and re-homing any already-modified instances past the split.
- Per-event time zones. An event keeps the `TZID` it was written with and is
  shown in it rather than flattened to yours, and its start and end can sit in
  different zones — which is what a flight is. Settings adds an optional second
  hour column in the week and day views showing another zone beside your own.
- Drag to move, resize, and create events in the week and day views, with a
  preview of where the event will land and snapping to 5, 10, or 15 minutes
  (Settings). Dragging one occurrence of a repeating event asks the same scope
  question saving does. Working hours are now the unshaded band of the day, and
  the week number appears in the grid's corner when week numbers are on.
- An agenda view: the next month of events as one flat, scannable list, grouped
  by day. `Ctrl+4`; the task list moves to `Ctrl+5`.
- Search across every calendar with `Ctrl+F` — titles and locations, six months
  back and a year ahead, nearest first, one row per event. Choosing a result
  jumps to its date and opens it.
- Undo and redo (`Ctrl+Z`, `Ctrl+Shift+Z`) for changes made in Slate, including
  a series split as a single step. Imports, sync, and conflict resolutions are
  deliberately not undoable.
- Quick add (`Ctrl+Shift+N`): type "lunch with Maria Thu 13:00 at Kolonaki" and
  see exactly how it was understood before it is added. English and Greek,
  weekday names, `today`/`tomorrow`, numeric dates, time ranges, and a location
  after "at".
- Calendar subscriptions. Add a feed by URL (`https:` or `webcal:`) under
  Accounts; it refreshes on its own schedule, in the app and in the background
  daemon, and is marked read-only everywhere so it cannot be edited by mistake.
- Meeting links (Meet, Zoom, Teams, Jitsi, BigBlueButton) found in an event's
  location or description get a Join button in the editor, and a Join action on
  the reminder notification.
- Sync conflicts are listed on the Accounts page with what each side calls the
  event, and can be resolved without leaving Slate.

- Invitations. An invitation arriving in Envelope now appears in Slate: a
  dialog shows who invites you, to what, when, and whether the slot is free,
  with Accept / Maybe / Decline. Accepting stores the event and your answer;
  the reply is queued in Envelope's outbox, or — when Envelope is not
  running — you're asked to reply from your mail client. Cancellations from
  the organizer update the calendar on their own, scoped to one occurrence
  or the whole series exactly as sent.
- Per-field conflict resolution. When a sync conflict carries enough history,
  the Accounts page now lists exactly which fields both sides changed and
  lets you pick a side per field — everything undisputed keeps both edits.
  Conflicts whose edits don't overlap offer a one-click "Merge both".
  Keep-mine / take-server's remain for everything else.
- Birthdays from the suite's address book appear as all-day entries in every
  view, with the age when the card states a year. On by default; toggle in
  Settings. Nothing is written to any calendar — they follow the address
  book live.

- Keyboard shortcuts, shown beside their menu entries: `Ctrl+N` new event,
  `Ctrl+1`–`Ctrl+4` views, `Ctrl+T` today, `Ctrl+←`/`Ctrl+→` paging,
  `Ctrl+I`/`Ctrl+E` import/export, `Ctrl+R` sync, `Ctrl+,` settings. Escape
  closes the editor and other side panels.
- Import, Export, Sync now, and the Tasks view are reachable from the menu bar,
  which is now split into File and View.
- `--today` and `--new-event` command-line flags, backing the desktop entry's
  actions.
- Recurring events modified by another calendar client are now honoured: a
  `RECURRENCE-ID` override displays in place of the instance it replaces,
  opens in the editor as itself, and can be deleted individually to restore
  the series' own instance. Reminders follow the override's own alarms.
- Occurrence-scoped editing. Saving a change to a repeating event asks whether
  it applies to this occurrence (an override) or the whole series; deleting
  asks for this occurrence, this and following, or the whole series. Opening
  an occurrence now shows that occurrence's own dates rather than the series'
  first ones.

### Fixed

- Dragging an event in the week or day view did nothing: the release that ends
  a drag was being swallowed before it reached the code that commits the move.
- Events in the week and day views could not be reached with the keyboard, and
  neither could agenda rows, search results, or days in the year view. All are
  focusable again, and Enter opens them.
- The new-event, previous, next, and cancel buttons in the header and sidebar
  are icon-only and had no accessible name; each now carries one.

### Changed

- The app, applet, daemon and launcher plugin present as **Slate**.
- A second launch now hands its arguments to the running window — a `.ics`
  from a file manager, a date from the applet or launcher — instead of opening
  a second copy.
- The panel applet and launcher plugin are translated, follow settings changes
  instantly, and open the app detached with focus handed over properly.
- Reminder notifications use the same localised wording from the app and the
  background daemon.

### Fixed

- Editing a recurring event no longer risks deleting modified instances that
  another client had saved into the same file.
- The reminder daemon's systemd sandbox no longer blocks the sync engine's
  own writes (contacts, accounts, cache index).
- Keyboard shortcuts work on non-Latin keyboard layouts.
