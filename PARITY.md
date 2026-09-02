# Slate — feature parity audit

Audited 2026-08-27, per the method in the suite roadmap
([cosmic-pim/ROADMAP.md](https://github.com/entro314-labs/cosmic-pim/blob/main/ROADMAP.md)):
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
| Agenda / schedule list | partial | Exists in the applet (next week) only; no in-app agenda. Slate M5. |
| Mini month / jump to date | have | |
| Adaptive window, hideable sidebar | verify | Sidebar exists; behaviour at small widths not audited. |
| Week numbers | have | `show_week_numbers` setting, off by default. |
| First day of week | have | Configurable Monday…Sunday. |

### Event editing

| Feature | Status | Notes |
|---|---|---|
| Title, location, description, all-day, start/end | have | |
| Per-event timezone | have | Start and end zones chosen separately (the flight case); all-day stays a DATE. Exceeds GNOME Calendar, whose editor has no TZ picker. |
| Reminders set in the editor | verify | Slate honours and round-trips `VALARM`, but no editor UI for adding one was found in `src/ui/editor.rs`; GNOME Calendar offers preset reminders in its editor. If absent, this is a baseline gap. |
| Join button for meeting links | gap | GNOME Calendar detects video-call URLs and offers Join. Slate M6 (Meet/Zoom/Teams/Jitsi/BBB from `LOCATION`/`DESCRIPTION`/`CONFERENCE`). |
| Weather in the grid | gap | GNOME Calendar shows a forecast in month view. The one allowed exception to the no-frills rule: planned as optional, off by default, open-meteo (keyless). |

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
| Display an invitation's attendees | gap | GNOME Calendar shows attendees read-only. Slate has no attendee surface at all; arrives with iMIP receive (M7). |

### Calendars and accounts

| Feature | Status | Notes |
|---|---|---|
| Multiple calendars, colours, visibility | have | |
| Local calendars | have | vdir on disk; readable by khal, Thunderbird, vdirsyncer — GNOME Calendar's store is not. |
| CalDAV accounts | have | Built in, no GOA dependency; accounts shared suite-wide, passwords in the keychain. |
| Google / Nextcloud onboarding | verify | Nextcloud is plain CalDAV and should work; Google needs OAuth — whether the generic account form covers it is unverified. |
| ICS feed subscription (URL) | gap | GNOME Calendar adds calendars from file or URL. The engine shipped in the substrate; Slate's UI (add-URL, refresh interval, read-only badge) is M3. |
| Birthdays from contacts | gap | GNOME shows GOA-contact birthdays. Substrate `BDAY` synthesis is planned (cosmic-pim M1); Slate consumes it when it lands. |

### Import / export

| Feature | Status | Notes |
|---|---|---|
| Import `.ics`, open from file manager | have | Portal-based; UID-keyed, so re-import updates instead of duplicating. |
| Export `.ics` | have | GNOME Calendar gained event export in 49. |

### Search

| Feature | Status | Notes |
|---|---|---|
| System-level search | have | pop-launcher plugin (`cal <query>`), one row per event; GNOME's equivalent is its Shell search provider. |
| In-app search | gap | Launcher only, and windowed to −1/+6 months. In-app search over summary/location/description across all calendars is M5. |

### Direct manipulation and undo

| Feature | Status | Notes |
|---|---|---|
| Drag to move an event | gap | GNOME Calendar supports it (without a scope prompt); Slate's drag arrives in M4 with the three-scope prompt. |
| Undo a deletion | gap | GNOME Calendar shows an undo toast on delete. Slate's undo (M5) is broader — journal of the patcher's inverse, covering splits — but nothing exists yet. |

### Accessibility and keyboard

| Feature | Status | Notes |
|---|---|---|
| Keyboard shortcuts with menu accelerators | have | One key map read by both the menu bar and the handler, so they cannot disagree. |
| Full keyboard operation of every action | partial | Editor and views are keyboard-reachable; grid-level gestures (move/resize/create) have no keyboard path until M4 builds them alongside the pointer path. |
| Screen reader state | verify | Bounded by libcosmic's AccessKit support; not audited. |
| Printing | — | Not a baseline row: GNOME Calendar cannot print either. Audited under the ceiling. |

**Baseline summary.** Open baseline gaps: in-app agenda, ICS-subscription
UI, in-app search, join buttons, drag-to-move, delete-undo, attendee
display, contact birthdays, weather — plus one *verify* that could become a
gap (reminders in the editor). None is data-loss-shaped by itself; the
loss-shaped items are all verification debts, listed at the end.

---

## Ceiling: Thunderbird calendar (Lightning)

The target is the roadmap's suite-level claim: a Thunderbird user moves and
loses nothing they use.

### Views

| Feature | Status | Notes |
|---|---|---|
| Day / week / month | have | |
| Multiweek view | gap | No plan names it; the M5 agenda + year views cover the same "wider than a week" need differently. Candidate for an argued rejection once those exist. |
| Year view | gap | Thunderbird lacks it too; Slate plans one with a density heatmap (M5). |
| Today Pane (docked agenda) | partial | The applet is the equivalent surface, but lives in the panel, not the app; in-app agenda is M5. |
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
| Multiple reminders, custom offsets, before/after end | partial | Multiple `VALARM`s are honoured on read and preserved on write; authoring them in the editor is the open half (see the baseline *verify*). |
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
| Alarm dialog with snooze / dismiss | gap | Slate fires a notification and deliberately drops >5-min-stale triggers; there is no snooze. The missed-alarm digest on resume (M6) covers the suspend case, not snooze itself. |
| Per-calendar default reminder | gap | M6, alongside per-calendar default duration. |

### Invitations and scheduling

| Feature | Status | Notes |
|---|---|---|
| Receive invitations, Accept/Tentative/Decline, reply to organizer | gap | The headline ceiling gap. Planned as receive-side iMIP over a minimal D-Bus contract with Envelope (M7); degrades to "import this .ics" without it. |
| Organizer side: invite attendees, track PARTSTAT | gap | After receive-side (M7+). |
| Updates and cancellations hitting the right occurrence | gap | Depends on the landed scoped-edit machinery; part of M7. |
| Free/busy lookup (RFC 6638) | gap | Explicitly last (M7+), substrate scheduling module. |
| Counter-proposals | gap | Unplanned; small once iTIP reply exists. Candidate rejection if it never earns its UI. |

### Calendars and accounts

| Feature | Status | Notes |
|---|---|---|
| CalDAV | have | Built in; suite-shared accounts. |
| ICS network calendar | partial | Thunderbird's is read/write; Slate's substrate feature is subscriptions (read-only), UI pending (M3). Read-only is the defensible scope — a feed you can write to is a sync engine wearing a costume. |
| Offline use and cache | have | Arguably exceeds: local files *are* the truth; sync is the add-on, not the substrate. |
| Per-calendar settings (colour, read-only, reminders toggle) | partial | Colour and visibility exist; a read-only flag exists in the model; per-calendar reminder defaults are M6. |
| Per-account sync status and error surfacing | partial | Sync exists (`Ctrl+R`, background daemon); last-sync/failure/retry surfacing is M3. |
| Conflict resolution UI | gap | Blocked purely on the substrate's conflict-record API (cosmic-pim M1). Thunderbird handles this weakly too, but the row stays a gap until it exists. |
| Exchange | verify | Thunderbird is finalizing native Exchange support. The substrate speaks DAV only, so Slate reaches Exchange exactly where a CalDAV gateway exists. If native EAS/EWS is ever wanted, it is a substrate decision, not Slate's. |

### Import / export

| Feature | Status | Notes |
|---|---|---|
| Import / export `.ics` | have | |
| Export a whole calendar | verify | The export path exists; whether it covers a full calendar vs selected events was not confirmed. |
| Printing (day/week/month/list layouts) | gap | Nothing planned in any Slate milestone. The honest options are a print row in a future milestone or a recorded rejection; today it is an unanswered hole against the ceiling. |

### Search

| Feature | Status | Notes |
|---|---|---|
| In-calendar search box and filter pane | gap | Launcher-only today; in-app search with a real results UI is M5. |

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
3. **No conflict UI.** Until the substrate exposes conflict records and
   Slate renders them, a genuine both-sides edit has no user-visible
   resolution path. Blocked on cosmic-pim M1; the app half is Slate M3.
4. **Windows-timezone mapping is absent.** An Outlook-authored event whose
   `TZID` fails IANA lookup falls back to floating and displays at the
   wrong wall-clock time. Substrate work (cosmic-pim M1); trust-loss if
   not byte-loss.

Explicitly *not* on this list: every field Slate cannot edit (categories,
privacy, status, attachments, attendees, complex RRULEs). The patcher
preserves them verbatim through every save — the editing gaps above are
feature gaps, not loss risks. That property is the audit's best result.

## Ceiling gaps, ranked by how much they are missed

1. **Invitations (iMIP receive, then reply).** The feature that separates
   a work calendar from a personal one; nothing else on this list changes
   what the app *is*. Planned, last, cross-repo (M7).
2. **In-app search.** Daily-use friction for anyone with more than a
   screenful of events; the launcher path is a workaround, not an answer.
3. **Custom recurrence authoring.** "Last Friday of the month" is a normal
   meeting; today Slate can keep it but not create it.
4. **Snooze.** The single most-pressed button on a reminder dialog;
   Slate's staleness-drop philosophy needs an answer for "not now" as well
   as "too late".
5. **ICS subscriptions UI.** Holiday and team feeds; the engine already
   exists, which makes the missing UI the cheapest big win here.
6. **The interop field set** (categories, status, privacy, show-as,
   attachments). Individually small; together they are what a Thunderbird
   power user notices first in the editor.
7. **Printing.** Unplanned and unrejected — the one row on this audit with
   no recorded answer at all.
8. **Conflict UI and sync-status surfacing.** Ranked below the daily
   features only because conflicts are rare; when one happens, nothing
   matters more.
9. **Free/busy and organizer-side scheduling.** Completes the invitation
   story; useless before item 1 exists.
10. **Multiweek view, Today Pane, task filters.** Presentation breadth;
    partially covered by the planned agenda and year views.
