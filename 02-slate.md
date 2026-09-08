# Slate — Calendar and Tasks

## As built

Month/week/day + mini month; task list; multiple calendars with colors and
visibility; recurrence editor covering daily/weekly/monthly/yearly with
interval and end condition, inexpressible rules (BYDAY, BYSETPOS, …) preserved
verbatim rather than flattened; EXDATE read/written; debounced fs watch;
reminders honoring per-event VALARM with opt-in app default, once-per-
occurrence, >5-min-stale dropped; delivery via org.freedesktop.Notifications;
four binaries (app, applet, daemon with D-Bus name arbitration and
success-exit on second start, pop-launcher plugin with one-row-per-event
dedup); .ics import/export via portal, UID-keyed re-import; DBusActivatable;
frosted-glass compliance; Fluent i18n; cosmic-config. Recurrence iterates in
DTSTART's own timezone (09:00 stays 09:00 across DST).

The reminder details — arbitration, staleness drop, once-per-occurrence — are
the kind of design that never makes feature lists and is why the thing feels
solid. Keep that bar.

## Fixes

- README contradicts the substrate on CalDAV sync (00). Either wire the
  accounts/sync UI to `cosmic-pim-sync` and update the README, or correct the
  cosmic-pim table — the vdirsyncer pitch and the built-in engine also
  collide with the one-owner-per-collection rule; whichever engine the README
  recommends, say what happens with the other.
- Rust version claim (1.93 vs 1.97 toolchain).

## Gaps, dependency-ordered

### 1. Occurrence-scoped edits (the headline data-model gap)

Currently edits apply to the whole series. The verbatim-preservation pattern
already used for inexpressible RRULEs is the same pattern overrides need:

- *This event*: write an override VEVENT with RECURRENCE-ID into the same
  .ics; editor touches only properties it understands, patcher preserves the
  rest.
- *This and following*: truncate master with UNTIL, emit successor series;
  re-home overrides falling after the split; single atomic file write.
- *All events*: edit master; keep overrides valid, drop/remap orphaned ones
  with explicit user confirmation.
- Display side must already merge overrides on read (other clients write
  them today — verify; if read-side ignores RECURRENCE-ID, that's a
  correctness bug ahead of the edit feature).
- Delete gains the same three scopes (EXDATE / truncate / remove file).
- Gate on the libical golden suite (00).

### 2. Per-event timezone

TZID picker in the editor (default: system zone), including start-TZ ≠ end-TZ
(the flight case); all-day events remain DATE values and never shift. Then a
secondary-timezone column in week/day views. Windows-TZ mapping (01) makes
foreign events display correctly regardless.

### 3. Conflict UI

Consume the substrate's conflict records (01): both-versions view, per-property
choice via the patcher, wholesale accept-theirs/mine. Small screen; blocked
purely on the engine API.

### 4. ICS subscriptions

Substrate feature (01); Slate's UI is add-feed URL + refresh interval +
read-only badge on the calendar list.

### 5. Interaction depth in the time grid

Drag-to-move and resize with the three-scope recurrence prompt from (1);
5/10/15-min snap; working-hours shading; now-line; ISO week numbers and
Monday start as defaults with locale override (verify which of these already
exist — README doesn't say). Pointer-capture + ghost-preview state is a small
interaction library to write once. Profile with 200+ visible events, at 100%
scale and at 240 Hz.

### 6. Quality-of-life tier

- **Agenda view** (flat upcoming list) and year heatmap.
- **In-app search** over summary/location/description across all calendars —
  the launcher plugin's query path, surfaced in-app with a wider window than
  ±1/+6 months.
- **Undo**, including recurrence-splitting edits and deletes — journal the
  patcher's inverse; the verbatim model makes inverses well-defined.
- **Meeting-link detection** (Meet/Zoom/Teams/Jitsi/BBB in
  LOCATION/DESCRIPTION/CONFERENCE): join button on the event, join action on
  the reminder notification.
- **Missed-alarm digest**: the >5-min staleness drop is the right default;
  a single "N reminders missed while suspended" summary on resume
  (login1 PrepareForSleep) is the middle ground between silence and replay.
- **Quick-add** with NL parsing ("lunch with Maria Thu 13:00 at Kolonaki") —
  deterministic grammar, English + Greek, parse shown before commit; wrong
  guesses visibly wrong. Attendee names resolve through the shared contact
  model once Circle links land.
- Per-calendar default alarm and default event duration.

### 7. Scheduling (last; cross-repo)

Receive-side iMIP first — the feature separating a personal calendar from a
work one, and feasible only because Envelope is in the family:

- Envelope detects text/calendar + METHOD, hands payload over a minimal D-Bus
  contract (deliver-payload, send-reply); Slate renders the invitation with a
  conflict check for that slot; Accept/Tentative/Decline sets PARTSTAT, stores,
  asks Envelope to send the REPLY.
- SEQUENCE honored; RECURRENCE-ID-scoped updates hit the right occurrence
  (depends on gap 1); CANCEL of one occurrence vs series handled distinctly.
- Degrades to "import this .ics" without Envelope — keep the contract minimal
  so that stays true.
- Organizer send-side and RFC 6638 free/busy after; iTIP semantics live in the
  substrate scheduling module (01), not here.

## Non-goals (unchanged)

AI scheduling, social feeds, heavy theming. Weather overlay stays the allowed
exception — optional, off by default, open-meteo (keyless).

## Risks

- Read-side RECURRENCE-ID handling status is the unknown that reorders gap 1.
- Time-grid performance under iced is the biggest UI unknown; measure before
  building gap 5's interactions on top.
- Outlook interop long tail (Windows TZs, legacy names in invites) — quirks
  discipline, same as DAV.
