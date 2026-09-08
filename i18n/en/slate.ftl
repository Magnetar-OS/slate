app-title = Slate
about = About
repository = Repository
file = File
view = View
settings = Settings

## Navigation / views
month = Month
week = Week
day = Day
agenda = Agenda
year = Year
today = Today
previous = Previous
next = Next

## Sidebar
sidebar = Sidebar
calendars = Calendars
new-calendar = New calendar
calendar-name = Calendar name
no-calendars = No calendars yet. Create one to get started.

## Event editor
new-event = New event
edit-event = Edit event
event-summary = Title
event-location = Location
event-description = Description
event-calendar = Calendar
event-starts = Starts
event-ends = Ends
all-day = All day
save = Save
cancel = Cancel
delete = Delete

## Recurrence
repeats = Repeats
repeat-never = Does not repeat
repeat-daily = Daily
repeat-weekly = Weekly
repeat-monthly = Monthly
repeat-yearly = Yearly
repeat-interval = Interval
repeat-ends = Ends
repeat-ends-never = Never
repeat-ends-after = After
repeat-ends-on = On date
occurrences = occurrences

## Empty states
no-events-range = No events in this range.
all-day-events = All day

## Settings
first-day-of-week = First day of week
show-week-numbers = Show week numbers
time-format-24h = Use 24-hour time

## Per-calendar defaults
calendar-defaults = Per-calendar defaults
calendar-default-inherit = Default reminder
calendar-duration-default = 1 hour (default)
duration-minutes = { $minutes } minutes
duration-hours = { $hours } { $hours ->
        [one] hour
       *[other] hours
    }

## Weekday abbreviations (column headers)
weekday-mon = Mon
weekday-tue = Tue
weekday-wed = Wed
weekday-thu = Thu
weekday-fri = Fri
weekday-sat = Sat
weekday-sun = Sun

## Month names
month-january = January
month-february = February
month-march = March
month-april = April
month-may = May
month-june = June
month-july = July
month-august = August
month-september = September
month-october = October
month-november = November
month-december = December

## Year view
year-day-events = { $count } { $count ->
        [one] event
       *[other] events
    }

## Misc
more-events = +{ $count } more
week-abbrev = W{ $number }
untitled-event = (No title)

## Errors
error-load-calendars = Could not read your calendars.
error-save-event = Could not save the event.
error-delete-event = Could not delete the event.
error-invalid-time-range = The end time must be after the start time.
error-summary-required = Give the event a title.

## Reminders
default-reminder = Default reminder
default-reminder-description = Used for events that carry no reminder of their own.
reminder-none = None
reminder-minutes = { $minutes } { $minutes ->
        [one] minute
       *[other] minutes
    } before
reminder-hours = { $hours } { $hours ->
        [one] hour
       *[other] hours
    } before
reminder-day-before = 1 day before
reminder-now = Starting now
reminder-in = In { $minutes } { $minutes ->
        [one] minute
       *[other] minutes
    }
reminder-at = At { $time }
reminders-missed = { $count } { $count ->
        [one] reminder
       *[other] reminders
    } passed while this machine was asleep.

## Import / export
import = Import calendar…
export = Export calendar…
import-done = Imported { $added } new and { $updated } updated events.
import-empty = No events found in { $path }.
export-done = Exported to { $path }.
error-remote-file = Only local files are supported.

## Accounts and sync
accounts = Accounts
add-account = Add account…
add = Add
remove = Remove
account-name = Name
server-url = Server address
username = Username
password = Password
app-password-hint = Many providers require an app-specific password rather than your normal one.
no-accounts-description = Add a CalDAV account to sync your calendars with a server.
sync-now = Sync now
syncing = Syncing…
error-no-account-store = Account storage is unavailable, so accounts cannot be saved.
error-url-scheme = The server address must start with https://
error-url-insecure = Refusing to send your password over an unencrypted connection. Use https://

## Scope prompt for repeating events
scope-save-title = You're editing a repeating event
scope-save-body = Apply the change to this occurrence only, to everything from here on, or to the whole series?
scope-delete-title = Delete a repeating event
scope-delete-body = Delete only this occurrence, everything from here on, or the whole series?
scope-this = This event
scope-following = This and following events
scope-all = All events

## Tasks
tasks = Tasks
no-tasks = Nothing to do.
show-completed = Show completed
high-priority = High
new-task = New task
no-writable-calendar = No writable calendar to add tasks to.
task-summary = Task
due = Has a due date
due-date = Due
priority = Priority
priority-none = None
priority-high = High
priority-medium = Medium
priority-low = Low
status = Status
status-needs-action = Not started
status-in-process = In progress
status-completed = Completed
status-cancelled = Cancelled
error-bad-time = That time could not be understood.
edit-task = Edit task

## Panel applet and launcher plugin
##
## These live in the same catalogue as the rest because the applet, the daemon
## and the launcher plugin share this crate's Fluent loader. Splitting them out
## would mean four catalogues for translators to keep aligned.
no-upcoming-events = No upcoming events
nothing-scheduled = Nothing scheduled
open-calendar = Open Calendar
tomorrow = Tomorrow
yesterday = Yesterday
modified-occurrence = This is one modified occurrence of a repeating event.

## Task list sections (task- prefix per multi-session i18n protocol)
task-section-overdue = Overdue
task-section-today = Today
task-section-upcoming = Upcoming
task-section-no-date = No date
task-section-completed = Completed

## ICS feed subscriptions
subscriptions = Subscriptions
add-subscription = Add subscription…
subscription-name = Name
subscription-url = Feed address
subscription-read-only = Read-only feed — refreshes on its own schedule
no-subscriptions-description = Subscribe to a public calendar feed — holidays, a team schedule — by its URL.
error-feed-scheme = The feed address must start with https:// or webcal://
remove-subscription-title = Remove this subscription?
remove-subscription-body = “{ $name }” will be removed from your calendar list. Subscribing to the same address again restores it.

## Sync conflicts
conflicts = Conflicts
conflicts-description = These events changed both here and on the server. Pick which version to keep — nothing resolves itself with time.
conflict-versions = Yours: { $yours } · Server's: { $theirs }
conflict-keep-mine = Keep mine
conflict-take-theirs = Take server's
conflict-merges-cleanly = Your edit and the server's touch different fields
conflict-merge-both = Merge both
conflict-choose-description = Both sides changed the fields below. Pick which version of each to keep; every other field keeps both edits.
conflict-was = Was: { $lines }
conflict-mine = Mine
conflict-theirs-label = Server's
conflict-absent = (removed)
conflict-apply-merge = Apply choices
conflict-merge-failed = The merged version could not be built — pick a whole side instead.

## Birthdays
show-birthdays = Show birthdays
show-birthdays-description = From the suite's address book, as all-day entries.
birthday-name = 🎂 { $name }
birthday-turns = 🎂 { $name } ({ $age })

## Invitations (iMIP, delivered by Envelope)
invitation-title = Invitation
invitation-body = { $organizer } invites you to “{ $summary }”.
invitation-when = When: { $when }
invitation-slot-free = The slot is free.
invitation-slot-busy = { $count ->
    [one] One event overlaps this slot.
    *[other] { $count } events overlap this slot.
}
invitation-accept = Accept
invitation-tentative = Maybe
invitation-decline = Decline
invitation-later = Decide later
invitation-unknown-organizer = Someone
invitation-not-for-me = This invitation is not addressed to this account.
invitation-stale = A newer version of this event is already on the calendar.
invitation-cancelled = The organizer cancelled — your calendar was updated.
invitation-reply-queued = Your reply is queued in Envelope.
invitation-reply-by-mail = Saved. Reply to the organizer from your mail client.
invitation-no-address = This account has no email address to answer as.

## Quick add
quick-add = Quick add…
quick-add-title = Quick add
quick-add-placeholder = lunch with Maria Thu 13:00 at Kolonaki
quick-add-hint = Type a title, then optionally a day, a time, and “at” a place.
quick-add-done = Added “{ $summary }”.

## Meetings
join-call = Join call

## History
undo = Undo
redo = Redo
undo-done = Change undone
redo-done = Change redone

## Search
find = Find…
search-placeholder = Search events
search-hint = Titles and locations, six months back and a year ahead.
search-no-results = Nothing matches.

## Grid interactions
snap-minutes = Drag snapping
snap-minutes-description = Dragged and resized events round to this step.
snap-minutes-value = { $minutes } minutes

## Time zones
secondary-timezone = Secondary time zone
secondary-timezone-description = A second hour column in the week and day views, e.g. America/New_York. Leave empty to turn it off.
time-zone = Time zone
time-zone-system = System time zone
time-zone-ends-in = Ends in
time-zone-same-as-start = Same as start
time-zone-different-end = Different end time zone…
time-zone-search-hint = Type a city or region
time-zone-no-match = No matching time zone
