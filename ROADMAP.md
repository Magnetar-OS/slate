# Slate roadmap

The destination: a calendar and task app that goes **1:1 against GNOME Calendar and beats it**,
measures itself against **Fantastical** for interaction quality, and is **indistinguishable from a
first-party COSMIC application** in look, behaviour, and integration. Wayland-native, wgpu-rendered,
files-as-truth, substrate-backed.

This document orders the work. [02-slate.md](02-slate.md) is the underlying gap analysis; the
substrate's own roadmap lives in
[cosmic-pim](https://github.com/Magnetar-OS/cosmic-pim). No dates — milestones are
dependency-ordered and each has an exit criterion, because "done" must be checkable.

## Benchmarks

| Role | App | Why |
|---|---|---|
| Parity floor | **GNOME Calendar** | The Linux desktop calendar standard. Every feature it has, Slate has. Slate already exceeds it on tasks, vdir interop, and built-in CalDAV without a GOA-style dependency. |
| Feature ceiling | **KOrganizer / Merkuro** | The completeness reference: scheduling (iTIP/iMIP), free/busy, subscriptions, per-event timezones. Slate targets the useful subset, not the kitchen sink. |
| Delight ceiling | **Fantastical** | The interaction bar: natural-language quick-add, direct manipulation, zero-jank rendering, an app that feels faster than thought. |
| Tasks reference | **Planify / Endeavour** | Task UX: sections, scheduled/today/upcoming groupings, drag between lists. |

A feature-parity matrix against these lives at the bottom; it is the scoreboard.

## Pillars

Every milestone item serves one of these; anything that serves none of them is scope creep.

1. **Feature-complete** — the parity matrix fully green.
2. **Pixel-perfect and delightful** — theme tokens only, no hard-coded metrics; 100% keyboard
   operable; animations that inform, never decorate; blur-compliant surfaces; zero dropped frames
   at 240 Hz.
3. **100% COSMIC integration** — conventions per [cosmic-conventions.md](cosmic-conventions.md):
   applet, launcher plugin, daemon, portals, single-instance, cosmic-config, Fluent. Already
   largely true; the remaining work is depth, not breadth.
4. **Modern architecture and quality** — substrate owns everything below the UI; the app stays a
   thin, honest front end. Tested recurrence math, golden-suite interop, benchmarked rendering.
5. **Wayland + wgpu, no fallbacks apologised for** — layer-shell for the applet, GPU rendering as
   the only path that is performance-tuned.

---

## Milestones

### M0 — Truth and baselines *(gate for everything else)*

Cheap, but everything later leans on it.

- [x] Resolve the README ↔ substrate contradiction on CalDAV sync: one sync engine per collection,
      documented behaviour for the other engine. Fix the Rust-version claim drift.
- [x] **Verify read-side `RECURRENCE-ID` handling** against files written by Thunderbird, khal,
      and Evolution. If the display layer ignores overrides, that is a correctness bug that
      reorders M1.
- [x] Wire the **libical golden test suite** into CI as a gate on the iCalendar layer
      (substrate work, gated here because M1 depends on it).
- [x] **Performance baseline for the time grid**: 200+ visible events, 100% scale, 240 Hz
      display. Record frame times under wgpu; publish the numbers in the repo. This is the
      before-picture for M4 — interaction work does not start until we know what the renderer
      can afford.

*Exit: docs are truthful, golden suite green in CI, a committed benchmark harness with baseline
numbers.*

### M1 — Recurrence completeness *(the headline data-model gap)*

Edits currently apply to a whole series. The verbatim-preservation pattern already used for
inexpressible RRULEs is exactly what overrides need.

- [x] *This event*: override `VEVENT` with `RECURRENCE-ID` in the same `.ics`; the editor touches
      only properties it understands, the patcher preserves the rest.
- [x] *This and following*: truncate master with `UNTIL`, emit successor series, re-home
      overrides after the split, single atomic write.
- [x] *All events*: edit master, keep overrides valid, drop/remap orphans only with explicit
      confirmation.
- [x] Delete gains the same three scopes (`EXDATE` / truncate / remove file).
- [x] Undo journal groundwork — landed as whole-file before/after snapshots rather than
      patcher inverses: every mutation is already an atomic rewrite of one or two files and
      foreign bytes are preserved verbatim, so the file *is* the inverse. Consumed in M5.

*Exit: golden suite green including override round-trips; a series edited in Slate re-opens
byte-faithfully in Thunderbird and khal.*

### M2 — Time zones

- [x] `TZID` picker in the editor, defaulting to the system zone; start-TZ ≠ end-TZ (the flight
      case); all-day events remain `DATE` values and never shift.
- [x] Secondary-timezone column in week/day views.
- [ ] Windows-TZ mapping (substrate) so Outlook-authored events display correctly.

*Exit: a flight EEST → PST round-trips through Google Calendar and displays correctly at both
ends; DST-boundary weekly events hold their wall-clock time.*

### M3 — Sync surface

The engine lives in the substrate; this is Slate's UI for it.

- [x] **Conflict UI**: both-versions view, per-property choice via the patcher, wholesale
      accept-theirs/mine. Blocked purely on the substrate's conflict-record API.
- [x] **ICS subscriptions** (webcal): add-feed URL, refresh interval, read-only badge on the
      calendar list.
- [~] Per-account sync status and error surfacing — the aggregate summary and a retry are in
      the Accounts page; per-account last-sync and failure reason still to break out.

*Exit: a deliberately induced conflict is resolvable entirely in-app; a public holiday feed
subscribes, refreshes, and is visibly read-only.*

### M4 — Direct manipulation *(the delight milestone)*

Built on M0's numbers and M1's scope prompt. This is where Fantastical is the bar.

- [x] Drag-to-move and drag-to-resize in the time grid, with the three-scope recurrence prompt.
- [x] Drag-to-create on empty grid space.
- [x] 5/10/15-minute snap; pointer capture + ghost preview as one small interaction library,
      written once and reused.
- [x] Now-line, working-hours shading, ISO week numbers, Monday start — locale-aware defaults
      with settings overrides (audit first: some may already exist).
- [x] Keyboard equivalents for every pointer gesture — every block, row, and
      day is focusable and opens with Enter, and the editor is the keyboard
      path to move and resize. (A grid selection model for arrow-key nudging
      is deliberately not built: the editor already reaches the same outcome.)
- [x] Re-run the M0 benchmark; interaction must not regress frame times.

*Exit: an event moves between days under the pointer at display refresh rate with 200+ events
visible; every gesture has a keyboard path.*

### M5 — Views and findability

- [x] **Agenda view** — flat upcoming list (also closes the applet/main-app gap: same model).
- [x] **Year view** with density heatmap.
- [x] **In-app search** over summary/location/description across all calendars — the launcher
      plugin's query path with a wider window and a real results UI.
- [x] **Undo/redo**, including recurrence-splitting edits and deletes, from M1's inverse journal.
- [x] `Ctrl+F` search as a first-class citizen in the key map; jump-to-date via the mini month
      and by choosing a search result.

*Exit: parity matrix rows for views and search go green; undo restores a split series
byte-faithfully.*

### M6 — Speed and small delights

- [x] **Quick-add with natural-language parsing** ("lunch with Maria Thu 13:00 at Kolonaki") —
      deterministic grammar, English + Greek, parse preview before commit, wrong guesses visibly
      wrong. Attendee names resolve through the shared contact model once Circle links land.
- [x] **Meeting-link detection** (Meet/Zoom/Teams/Jitsi/BBB in
      `LOCATION`/`DESCRIPTION`/`CONFERENCE`): join button on the event, join action on the
      reminder notification.
- [x] **Missed-alarm digest**: one "N reminders missed while suspended" summary on resume
      (login1 `PrepareForSleep`) — the middle ground between silence and replay.
- [x] Per-calendar default alarm and default event duration.
- [ ] Event templates / duplicate-event.

*Exit: quick-add round-trip under one second from shortcut to committed event; a Zoom invite
shows a join button with no configuration.*

### M7 — Scheduling *(cross-repo; last because it depends on M1 and Envelope)*

Receive-side iMIP first — the feature separating a personal calendar from a work one, feasible
because Envelope is in the family.

- [x] Envelope detects `text/calendar` + `METHOD`, hands payload over a minimal D-Bus contract
      (deliver-payload, send-reply).
- [x] Slate renders the invitation with a conflict check for that slot;
      Accept/Tentative/Decline sets `PARTSTAT`, stores, asks Envelope to send the `REPLY`.
- [x] `SEQUENCE` honoured; `RECURRENCE-ID`-scoped updates hit the right occurrence; `CANCEL` of
      one occurrence vs the series handled distinctly.
- [x] Degrades to "import this .ics" without Envelope — the contract stays minimal so that
      remains true.
- [x] After receive-side is solid: organizer send-side, then RFC 6638 free/busy.
      Free/busy is in: events carry attendees and an organizer, the editor edits
      them, and `cosmic_pim_sync::availability` asks the calendar's own account.
      Organizer send-side remains — the substrate half (`itip::with_method`,
      `send_invitation`, `send_cancellation`) is there, unused by the app.

*Exit: a Google-sent invite lands in Envelope, is accepted in Slate, and the organizer sees the
acceptance; a rescheduled single occurrence updates the right instance.*

### M8 — 1.0

- [ ] Flathub submission + distro packaging out of `packaging/`; release automation already
      scaffolded in `release.config.json`.
- [x] AppStream metadata at Flathub quality: release notes, OARS, branding,
      `<supports>`, and four screenshots, all validating. `packaging/screenshots.sh`
      regenerates the shots inside a nested compositor against a generated
      calendar, so a capture can never carry the machine it was taken on into a
      public listing.
- [x] Accessibility audit: keyboard traversal fixed everywhere it was broken —
      grid blocks, agenda rows, search results, and year days are focusable and
      open with Enter — and every icon-only button now carries a name. No
      animation exists to honour reduced-motion; colours come from theme tokens,
      so contrast follows the active theme. Remaining: the month view's
      background "new event here" target and the task rows are still
      pointer-only, both pre-existing.
- [ ] i18n beyond en/el — extract, document, invite translators.
- [ ] Parity matrix: every GNOME Calendar row green; no known data-loss bug open.

---

## Parallel tracks

These run across all milestones rather than belonging to one.

**Design & pixel-perfection.** All metrics from theme tokens (`cosmic::theme::spacing`), never
hard-coded; every surface blur-compliant (already policy — keep the bar); icons from the system
theme with correct symbolic fallbacks; empty states, loading states, and error states designed,
not defaulted; motion used only to explain state changes and disabled under reduced-motion.
Each milestone's UI ships with a screenshot pass against a first-party app (cosmic-edit,
cosmic-files) as the visual reference.

**COSMIC integration depth.** Conventions checklist from
[cosmic-conventions.md](cosmic-conventions.md) kept green as libcosmic moves: track the default
branch, `cargo update -p libcosmic` as the deliberate update, xdgen-generated XDG files, menu
bar/key map unity (exists — a shortcut and its menu entry cannot disagree). Applet grows with the
app: agenda parity in M5, join actions in M6.

**Engineering quality.** CI runs what `just check-all` runs; golden interop suite from M0
onward; property tests on recurrence math; the benchmark harness re-run at every milestone that
touches rendering, with regressions treated as failures. Substrate bugs are fixed in
`cosmic-pim`, never worked around here.

**Performance.** wgpu is the tuned path. Budget: full-frame render under 4 ms at 240 Hz with
200+ visible events; expansion stays uncached-by-design (cheap, and correct across TZ/DST
changes); scrolling and dragging allocate nothing per frame. Measure before building — M0 exists
so M4 is engineering, not hope.

---

## Parity matrix

Scoreboard against the parity floor (GNOME Calendar) plus the differentiators. ✅ done ·
🔶 partial · ⬜ planned (milestone in brackets).

| Capability | GNOME Calendar | Slate today | Target |
|---|---|---|---|
| Month / week / day views | ✅ | ✅ | ✅ |
| Agenda list | ✅ | ✅ | ✅ |
| Year view | ⬜ (lacks it) | ✅ heatmap | ✅ |
| Multiple calendars, colours, visibility | ✅ | ✅ | ✅ |
| Recurring events — display incl. overrides | ✅ | ✅ | ✅ |
| Recurring events — scoped edit/delete | 🔶 | ✅ all three scopes | ✅ |
| Per-event time zones | 🔶 | ✅ incl. start ≠ end | ✅ |
| Secondary time-zone column | ⬜ (lacks it) | ✅ | ✅ |
| CalDAV sync | ✅ via GOA | ✅ built-in | ✅ |
| Sync conflict resolution UI | ⬜ (lacks it) | ✅ incl. per-field | ✅ |
| ICS feed subscriptions | ✅ | ✅ | ✅ |
| Drag to move / resize / create | 🔶 | ✅ | ✅ |
| Search | ✅ | ✅ in-app + launcher | ✅ |
| Undo | ⬜ (lacks it) | ✅ | ✅ |
| NL quick-add | ⬜ (lacks it) | ✅ en + el | ✅ |
| Meeting-link join | ✅ | ✅ event + notification | ✅ |
| Reminders / VALARM round-trip | 🔶 | ✅ | ✅ |
| Invitations (iMIP receive) | 🔶 read-only | ✅ accept/decline + reply | ✅ |
| Free/busy (RFC 6638) | ⬜ (lacks it) | ⬜ | ⬜ M7+ |
| Birthdays from contacts | ⬜ (lacks it) | ✅ | ✅ |
| Tasks (VTODO) | ⬜ (separate app) | ✅ grouped list | ✅ deepen |
| Import/export .ics, file-manager open | ✅ | ✅ | ✅ |
| Panel applet | ⬜ (lacks it) | ✅ | ✅ |
| Launcher integration | 🔶 GNOME Shell | ✅ plugin | ✅ |
| Background reminders daemon | ✅ (evolution-data-server) | ✅ | ✅ |
| Interop: khal/Thunderbird-readable storage | ⬜ (lacks it) | ✅ vdir | ✅ |

**Where it stands:** every row the parity floor has is now green, and Slate is ahead of it on
conflict resolution, undo, quick-add, secondary time zones, birthdays, tasks, the applet, and
vdir interop. What remains is the year view (M5), free/busy (M7+), and the 1.0 work in M8.

## Non-goals

Unchanged from the gap analysis: no AI scheduling, no social feeds, no heavy theming. Weather
overlay remains the allowed exception — optional, off by default, open-meteo (keyless). No
internal compatibility layers; no second sync engine; no UI-side workarounds for substrate bugs.

## Risks

- ~~Writing an event re-serialises it from the model~~ — **closed.** The
  substrate now edits events and tasks by patching the file rather than
  rebuilding it, so parameters on modelled properties survive too
  (`SUMMARY;LANGUAGE=en-gb` verified through a save). `Event::other` remains,
  but only for the paths that serialise from nothing: a brand-new file, and
  export.
- **Export drops parameters on the properties it displays.** `Export calendar…`
  serialises from the model rather than copying the stored components, so an
  event survives with its attendees, organizer, `STATUS`, `X-` properties and
  the rest — but `SUMMARY;LANGUAGE=en-gb` comes back as plain `SUMMARY`
  (verified). This is the last place the old re-serialisation residue lives:
  editing was moved onto the patcher, and export is one of the two paths that
  has no original bytes to patch. Fixing it means emitting each file's stored
  components verbatim under one wrapper — which is exactly the shape that gave
  Circle a duplication bug when done per-record instead of per-file, so it
  wants doing carefully or not at all.
- **Undo restores whole files, not single records.** The journal snapshots each
  touched file before and after, so undoing an edit inside a file that holds
  several records also reverts anything else edited in that file since. For
  events this is usually right — a master and its overrides share a UID and are
  one logical event — but a task file another program wrote can hold several
  unrelated tasks, and there the restore is too wide. The clean fix journals the
  component rather than the file and merges on restore, which wants the
  component-addressed removal the substrate is growing; not worth a second
  implementation here in the meantime.
- **Read-side `RECURRENCE-ID` status** is the unknown that can reorder M1 — hence M0.
- **Time-grid performance under iced/wgpu** is the biggest UI unknown; the M0 baseline exists so
  M4 is built on measurements, not optimism.
- **Outlook interop long tail** (Windows TZs, legacy names in invites) — quirks discipline, same
  as DAV.
- **libcosmic is unpinned by convention** — API drift on the default branch is absorbed at
  `cargo update` time; the committed lockfile is the reproducibility mechanism.
- **M7 spans three repositories** (Slate, Envelope, cosmic-pim) — the D-Bus contract is kept
  minimal precisely so the milestones stay independently shippable.
