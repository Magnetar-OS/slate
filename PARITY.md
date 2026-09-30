# Slate — feature parity audit

Audited 2026-08-27, per the method in the suite roadmap
([cosmic-pim/ROADMAP.md](https://github.com/Magnetar-OS/cosmic-pim/blob/main/ROADMAP.md)):
walk the benchmark app's feature surface row by row, and mark each row
**have / partial / gap / rejected** — a rejection with a reason is an answer,
an unlisted feature is a hole. Rows I could not confirm against Slate's code
are marked **verify** rather than guessed.

The benchmarks, from the same roadmap: **GNOME Calendar** is the baseline
(must fully cover), **Thunderbird's calendar** (Lightning) is the ceiling
(the parity target). Fantastical is the polish reference — how Slate should
feel, not what it does — and is deliberately not audited here.

The benchmark surfaces were compiled from GNOME Calendar as of GNOME 49
(2025–26: adaptive window, event export, join-meeting buttons) and
Thunderbird's built-in calendar as of the 2026 releases (a calendar UI
refresh is in progress upstream; the audited surface is the shipped one).
Slate's side was verified against `src/` where the docs were uncertain —
and the code is ahead of the docs in two places worth naming: three-scope
recurrence edits (*this event* / *this and following* / *all events*, on
both save and delete) and the per-event timezone picker (separate start and
end zones) have both landed, while 02-slate.md still lists them as gaps.
The rows below reflect the code.

**Re-checked against the code 2026-09-29** (after the 2026-09-28 audit): the
rows for features that shipped since August — agenda and year views, in-app
search, drag, undo, subscriptions, join buttons, conflicts, attendees,
birthdays, per-calendar defaults, invitations, free/busy, reminders in the
editor — now say so.

---

## Baseline: GNOME Calendar

Every row here must end at *have* (or *rejected*, with the reason holding)
before the baseline claim is true.

### Views

| Feature | Status | Notes |
|---|---|---|
| Month view | have | Infinite paging by month; GNOME's is infinite-scroll — a presentation difference, not a gap. |
| Week view | have | |
| Day view | have | Slate-only; GNOME Calendar has no day view. |
| Agenda / schedule list | have | In-app agenda view (next 30 days), plus the applet's next week. |
| Mini month / jump to date | have | |
| Adaptive window, hideable sidebar | verify | Sidebar exists; behaviour at small widths not audited. |
| Week numbers | have | `show_week_numbers` setting, off by default. |
| First day of week | have | Configurable Monday…Sunday. |

### Event editing

| Feature | Status | Notes |
|---|---|---|
| Title, location, description, all-day, start/end | have | |
| Per-event timezone | have | Start and end zones chosen separately (the flight case); all-day stays a DATE. Exceeds GNOME Calendar, whose editor has no TZ picker. |
| Reminders set in the editor | have | The editor lists an event's reminders and adds or removes them from presets (at the start up to a week before); it also lists, read-only, those another app set from the end or at a fixed time. Without any reminder of any kind, the calendar's default applies. |
| Join button for meeting links | have | Meet/Zoom/Teams/Jitsi/BBB from `LOCATION`/`DESCRIPTION`, any `https` URI from `CONFERENCE`; on the event and on its reminder. |
| Weather in the grid | gap | GNOME Calendar shows a forecast in month view. The one allowed exception to the no-frills rule: optional, off by default, open-meteo (keyless) — not scheduled; nothing user-critical waits on it. |

### Recurrence

| Feature | Status | Notes |
|---|---|---|
| Repeat presets with end condition | have | Daily/weekly/monthly/yearly, interval, count/until. The interval exceeds GNOME Calendar's fixed presets. |
| Rules the editor can't express | have | `BYDAY`, `BYSETPOS`, … preserved verbatim, never flattened on save. |
| Scoped edit of a series instance | have | *This event* (`RECURRENCE-ID` override), *this and following* (series split), *all events*. Landed recently; correctness (override re-homing on split) still awaits the golden-suite gate — see the data-loss list. |
| Scoped delete | have | `EXDATE` / `UNTIL` truncation / remove file. |
| Overrides written by other clients | have | Displayed in place, editable as themselves, removable to restore the generated instance. Verification against Thunderbird/khal/Evolution-written files is the M0 item — see the data-loss list. |
| `EXDATE` read/write | have | |

### Reminders

| Feature | Status | Notes |
|---|---|---|
| Desktop notifications | have | Via `org.freedesktop.Notifications`; lands in COSMIC's notification centre. |
| Reminders with the window closed | have | `slate-daemon` with D-Bus name arbitration; GNOME leans on evolution-data-server for the same. |
| `VALARM` round-trip | have | Alarms set in another client survive; app-wide default lead time for alarm-less events, off by default. |
| Once-per-occurrence, staleness drop | have | Slate-only design details; no GNOME equivalent. |

### Invitations

| Feature | Status | Notes |
|---|---|---|
| Display an invitation's attendees | have | The editor lists and edits attendees; invitations arrive over iMIP from Envelope. |

### Calendars and accounts

| Feature | Status | Notes |
|---|---|---|
| Multiple calendars, colours, visibility | have | |
| Local calendars | have | vdir on disk; readable by khal, Thunderbird, vdirsyncer — GNOME Calendar's store is not. |
| CalDAV accounts | have | Built in, no GOA dependency; accounts shared suite-wide, passwords in the keychain. |
| Google / Nextcloud onboarding | verify | Nextcloud is plain CalDAV and should work; Google needs OAuth — whether the generic account form covers it is unverified. |
| ICS feed subscription (URL) | have | Subscribe by URL in Accounts; read-only, refreshed on each feed's own interval. |
| Birthdays from contacts | have | From the suite's address books (Circle's), toggled in Settings. |

### Import / export

| Feature | Status | Notes |
|---|---|---|
| Import `.ics`, open from file manager | have | Portal-based; UID-keyed, so re-import updates instead of duplicating. |
| Export `.ics` | have | GNOME Calendar gained event export in 49. |

### Search

| Feature | Status | Notes |
|---|---|---|
| System-level search | have | pop-launcher plugin (`cal <query>`), one row per event; GNOME's equivalent is its Shell search provider. |
| In-app search | have | `Ctrl+F`: summary, location and description across visible calendars, six months back to a year ahead. |

### Direct manipulation and undo

| Feature | Status | Notes |
|---|---|---|
| Drag to move an event | have | Move, resize and sweep-to-create on the time grid, with the scope prompt for a series. |
| Undo a deletion | have | Undo/redo of every edit and delete this session, splits included; refused, not forced, when a file changed since. |

### Accessibility and keyboard

| Feature | Status | Notes |
|---|---|---|
| Keyboard shortcuts with menu accelerators | have | One key map read by both the menu bar and the handler, so they cannot disagree. |
| Full keyboard operation of every action | partial | Editor and views are keyboard-reachable; grid-level gestures (move/resize/create) have no keyboard path until M4 builds them alongside the pointer path. |
| Screen reader state | verify | Bounded by libcosmic's AccessKit support; not audited. |
| Printing | — | Not a baseline row: GNOME Calendar cannot print either. Audited under the ceiling. |

**Baseline summary.** One open baseline gap: weather, the optional
exception. The remaining *verify* rows (adaptive window, Google onboarding,
screen reader) are audits, not known gaps.

---

## Ceiling: Thunderbird calendar (Lightning)

The target is the roadmap's suite-level claim: a Thunderbird user moves and
loses nothing they use.

### Views

| Feature | Status | Notes |
|---|---|---|
| Day / week / month | have | |
| Multiweek view | rejected | The agenda and year views, both shipped, cover the same "wider than a week" need (audit 2026-09-28, O-14). |
| Year view | have | Twelve mini months with a density heatmap; Thunderbird lacks it. |
| Today Pane (docked agenda) | partial | The agenda view and the panel applet cover it; neither is docked beside the grid. |
| Task view with filters | partial | Slate's task list has due/priority/percent and urgency-first ordering; Thunderbird adds filter tabs (today, overdue, next 7 days) and category filtering. |

### Event editing

| Feature | Status | Notes |
|---|---|---|
| Core fields | have | |
| Separate start/end timezones | have | At parity. |
| Categories (with colours) | gap | Task list *displays* `CATEGORIES` as chips; neither events nor tasks can edit them. Preserved verbatim on round-trip, so nothing is lost — just not editable. |
| Privacy (public / private / confidential) | gap | `CLASS` not surfaced. Preserved verbatim. |
| Status (tentative / confirmed / cancelled) | gap | `STATUS` not surfaced. Preserved verbatim. |
| Show as busy/free (`TRANSP`) | gap | Meaningful mostly alongside free/busy scheduling (below). Preserved verbatim. |
| Priority on events | gap | Tasks have priority; events don't surface it. |
| Attachments (link URLs) | gap | `ATTACH` not surfaced. Preserved verbatim. |
| Multiple reminders, custom offsets, before/after end | partial | Several reminders per event, authored from presets. Reminders another app set from the end or at a fixed time are fired, shown in the editor and kept through every save, but cannot be authored or removed here; nor can arbitrary offsets. |
| Event templates / duplicate | gap | Slate M6. |

### Recurrence

| Feature | Status | Notes |
|---|---|---|
| Custom recurrence dialog (weekday sets, "last Friday", monthly by day/weekday) | partial | Slate preserves what it cannot express and displays it correctly, but the editor can't author `BYDAY`/`BYSETPOS` rules. The verbatim model means adopting them later is UI work only. |
| Edit/delete one occurrence vs all | have | Slate additionally offers *this and following*, which Thunderbird does not. |
| Recurring tasks | partial | Preserved verbatim (with alarms, `RELATED-TO`, completion timestamps); not authorable in the task editor. |

### Reminders

| Feature | Status | Notes |
|---|---|---|
| Alarm dialog with snooze / dismiss | have | Snooze (10 minutes) and dismiss on the notification; a snooze survives a restart and a hand-over between the app and the daemon. Stale triggers are still dropped, and the missed-alarm digest covers a suspend. |
| Per-calendar default reminder | have | With per-calendar default duration, in Settings. |

### Invitations and scheduling

| Feature | Status | Notes |
|---|---|---|
| Receive invitations, Accept/Tentative/Decline, reply to organizer | have | iMIP from Envelope over D-Bus, with a slot-conflict check; the reply goes back through Envelope, or "reply from your mail client" without it. Declining after accepting takes the copy off the calendar. |
| Organizer side: invite attendees, track PARTSTAT | partial | Attendees are edited and their replies' PARTSTAT recorded; sending the invitations is not wired (M7+). |
| Updates and cancellations hitting the right occurrence | have | `SEQUENCE` and `RECURRENCE-ID` honoured; an instance CANCEL cancels one occurrence. |
| Free/busy lookup (RFC 6638) | have | Attendee availability from the calendar's own account. |
| Counter-proposals | gap | Unplanned; small once iTIP reply exists. Candidate rejection if it never earns its UI. |

### Calendars and accounts

| Feature | Status | Notes |
|---|---|---|
| CalDAV | have | Built in; suite-shared accounts. |
| ICS network calendar | partial | Thunderbird's is read/write; Slate's is subscriptions (read-only). Read-only is the defensible scope — a feed you can write to is a sync engine wearing a costume. |
| Offline use and cache | have | Arguably exceeds: local files *are* the truth; sync is the add-on, not the substrate. |
| Per-calendar settings (colour, read-only, reminders toggle) | have | Visibility, default reminder and duration per calendar; a local calendar is renamed, recoloured and deleted in Settings; feeds are read-only. |
| Per-account sync status and error surfacing | partial | Sync exists (`Ctrl+R`, background daemon); last-sync/failure/retry surfacing is M3. |
| Conflict resolution UI | have | Keep mine / take theirs, or choose per field when both sides changed different things. |
| Exchange | verify | Thunderbird is finalizing native Exchange support. The substrate speaks DAV only, so Slate reaches Exchange exactly where a CalDAV gateway exists. If native EAS/EWS is ever wanted, it is a substrate decision, not Slate's. |

### Import / export

| Feature | Status | Notes |
|---|---|---|
| Import / export `.ics` | have | |
| Export a whole calendar | have | One calendar at a time, chosen in the export dialog. |
| Printing (day/week/month/list layouts) | gap | Deferred, recorded (audit 2026-09-28, O-11): no milestone yet; the answer is no longer missing, only unscheduled. |

### Search

| Feature | Status | Notes |
|---|---|---|
| In-calendar search box and filter pane | partial | In-app search with a results list; no filter pane. |

### Extensibility and appearance

| Feature | Status | Notes |
|---|---|---|
| Add-ons / extension ecosystem | rejected | Out of scope for a focused, first-party-feeling COSMIC app; the suite's extension points are its files (vdir, open to any tool) and D-Bus surfaces, not an in-process add-on API. |
| Themes beyond the system theme | rejected | Heavy theming is a named non-goal (02-slate.md); Slate follows COSMIC's theme tokens, light/dark/accent/high-contrast, and nothing else. |
| AI scheduling assistants | rejected | Named non-goal: no AI scheduling, no inference budget anywhere. (Thunderbird ships none either; recorded because the category keeps growing it.) |

### Accessibility and keyboard

| Feature | Status | Notes |
|---|---|---|
| Keyboard-complete operation | partial | As in the baseline table; M4/M5 add keyboard paths for gestures and search. |
| Screen reader support | verify | Bounded by libcosmic/AccessKit; an audit is scheduled at Slate M8. |

---

## Data-loss-shaped gaps

The Milestone 2 exit criterion is that this list is empty against the
baseline. It is close to empty — the write-path features exist — but three
verification debts and one blocked feature are still loss-shaped:

1. **Scoped edits are landed but ungated.** The three-scope machinery
   (override write, `UNTIL` split with override re-homing, orphan handling
   on all-events edits) has no golden-suite verification yet. An incorrect
   split silently corrupts a series — the RRULE golden suite and the
   round-trip corpus (cosmic-pim M1) are the gate, and until they run
   green, this is the top loss-shaped item.
2. **Read-side override merging is claimed, not verified.** Files written
   by Thunderbird, khal, and Evolution must display correctly (Slate M0).
   Showing a moved occurrence at its original time is data loss in the
   user's eyes, whatever the bytes say.
3. **Windows-timezone mapping is absent.** An Outlook-authored event whose
   `TZID` fails IANA lookup falls back to floating and displays at the
   wrong wall-clock time. Substrate work (cosmic-pim M1); trust-loss if
   not byte-loss.

Explicitly *not* on this list: every field Slate cannot edit (categories,
privacy, status, attachments, attendees, complex RRULEs). The patcher
preserves them verbatim through every save — the editing gaps above are
feature gaps, not loss risks. That property is the audit's best result.

## Ceiling gaps, ranked by how much they are missed

1. **Organizer send-side.** Receive, reply and free/busy are in; sending
   invitations is the half that remains (M7+).
2. **Search filters.** In-app search is in; a filter pane is not.
3. **Custom recurrence authoring.** "Last Friday of the month" is a normal
   meeting; today Slate can keep it but not create it.
4. **Custom reminder offsets and end-relative alarms.** Presets cover the
   common cases; the rest round-trip and fire but cannot be authored.
5. **The interop field set** (categories, status, privacy, show-as,
   attachments). Individually small; together they are what a Thunderbird
   power user notices first in the editor.
6. **Printing.** Deferred, with no milestone.
7. **Sync-status surfacing.** Per-account last sync and failures; the
   conflict UI exists.
8. **Today Pane, task filters.** Presentation breadth; partly covered by
   the agenda view and the applet.
