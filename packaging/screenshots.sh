#!/usr/bin/env bash
# Regenerates the screenshots the AppStream metadata points at.
#
# Flathub wants screenshots of the app and nothing else. A capture of an
# ordinary desktop carries whatever is behind the window into a public
# listing — other windows, file names, addresses — so this runs Slate inside
# a nested compositor and captures *that* output, which contains only Slate.
#
# The calendar it shows is generated here, so the shots never contain real
# events, and the settings live in a throwaway config directory, so the
# machine's own Slate settings are untouched.
#
# Requires: cosmic-comp (the session already provides it) and grim.
# Usage: packaging/screenshots.sh [output-dir]
set -euo pipefail

OUT="${1:-resources/screenshots}"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

command -v grim >/dev/null || { echo "grim is not installed" >&2; exit 1; }
command -v cosmic-comp >/dev/null || { echo "cosmic-comp is not installed" >&2; exit 1; }

cargo build --release --bin slate
SLATE="$(pwd)/target/release/slate"
mkdir -p "$OUT"

# --- a week of plausible events, generated relative to today -----------------
CAL="$WORK/calendars/work"
mkdir -p "$CAL"
printf 'Work' > "$CAL/displayname"
printf '#2d7dd2' > "$CAL/color"
PERSONAL="$WORK/calendars/personal"
mkdir -p "$PERSONAL"
printf 'Personal' > "$PERSONAL/displayname"
printf '#9b59b6' > "$PERSONAL/color"

python3 - "$WORK/calendars" <<'PYEOF'
import datetime, sys, uuid, pathlib

root = pathlib.Path(sys.argv[1])
today = datetime.date.today()
monday = today - datetime.timedelta(days=today.weekday())

# (calendar, day offset, hour, minute, minutes, summary, location, rrule)
plan = [
    ("work", 0, 9, 30, 30, "Standup", "https://meet.google.com/abc-defg-hij", "FREQ=WEEKLY;BYDAY=MO,WE,FR"),
    ("work", 0, 11, 0, 60, "Design review", "Studio", ""),
    ("work", 1, 10, 0, 90, "Client workshop", "Kolonaki", ""),
    ("work", 1, 15, 0, 45, "Roadmap sync", "", ""),
    ("work", 2, 14, 0, 60, "1:1 with Maria", "", ""),
    ("personal", 3, 13, 0, 60, "Lunch with Maria", "Kolonaki", ""),
    ("work", 3, 16, 0, 45, "Sprint planning", "", ""),
    ("work", 4, 15, 0, 120, "Release retro", "Studio", ""),
    ("personal", 5, 10, 0, 180, "Farmers market", "Kypseli", ""),
]
for cal, day, h, m, mins, summary, loc, rrule in plan:
    d = monday + datetime.timedelta(days=day)
    start = datetime.datetime(d.year, d.month, d.day, h, m)
    end = start + datetime.timedelta(minutes=mins)
    uid = f"{uuid.uuid4()}@screenshots"
    lines = [
        "BEGIN:VCALENDAR", "VERSION:2.0", "PRODID:-//Slate//Screenshots//EN",
        "BEGIN:VEVENT", f"UID:{uid}", f"DTSTAMP:{start:%Y%m%dT%H%M%S}Z",
        f"DTSTART;TZID=Europe/Athens:{start:%Y%m%dT%H%M%S}",
        f"DTEND;TZID=Europe/Athens:{end:%Y%m%dT%H%M%S}",
        f"SUMMARY:{summary}",
    ]
    if loc:
        lines.append(f"LOCATION:{loc}")
    if rrule:
        lines.append(f"RRULE:{rrule}")
    lines += ["END:VEVENT", "END:VCALENDAR"]
    (root / cal / f"{uid}.ics").write_text("\r\n".join(lines) + "\r\n")

# An all-day span, so the band above the hour grid is not empty.
d = monday + datetime.timedelta(days=2)
uid = f"{uuid.uuid4()}@screenshots"
(root / "personal" / f"{uid}.ics").write_text("\r\n".join([
    "BEGIN:VCALENDAR", "VERSION:2.0", "PRODID:-//Slate//Screenshots//EN",
    "BEGIN:VEVENT", f"UID:{uid}", f"DTSTAMP:{d:%Y%m%d}T000000Z",
    f"DTSTART;VALUE=DATE:{d:%Y%m%d}",
    f"DTEND;VALUE=DATE:{d + datetime.timedelta(days=2):%Y%m%d}",
    "SUMMARY:Conference", "END:VEVENT", "END:VCALENDAR",
]) + "\r\n")
print("seeded", file=sys.stderr)
PYEOF

# --- a nested compositor, so the capture holds only the app ------------------
cosmic-comp --no-xwayland >"$WORK/comp.log" 2>&1 &
COMP=$!
trap 'kill "$COMP" 2>/dev/null || true; rm -rf "$WORK"' EXIT
sleep 5

# The display the nested compositor took: the highest-numbered new socket.
NESTED="$(ls -t /run/user/"$(id -u)"/wayland-* 2>/dev/null | grep -v '\.lock$' | head -1 | xargs basename)"
[ -n "$NESTED" ] || { echo "the nested compositor did not start" >&2; exit 1; }
echo "nested display: $NESTED"

shoot() {
  local name="$1" view="$2"
  local conf="$WORK/xdg/cosmic/com.magnetaros.Slate/v1"
  mkdir -p "$conf"
  printf '%s' "$view" > "$conf/view"
  printf 'true' > "$conf/show_week_numbers"

  # `COSMIC_SINGLE_INSTANCE=false`, or this hands its command line to the
  # Slate already running on the desktop and exits without drawing anything.
  env -u X_PRIVILEGED_WAYLAND_SOCKET \
      WAYLAND_DISPLAY="$NESTED" \
      COSMIC_SINGLE_INSTANCE=false \
      COSMIC_PIM_CALENDAR_DIR="$WORK/calendars" \
      XDG_CONFIG_HOME="$WORK/xdg" \
      XDG_DATA_HOME="$WORK/data" \
      "$SLATE" >"$WORK/$name.log" 2>&1 &
  local pid=$!
  sleep 8
  WAYLAND_DISPLAY="$NESTED" grim "$OUT/$name.png"
  kill "$pid" 2>/dev/null || true
  wait "$pid" 2>/dev/null || true
  crop "$OUT/$name.png"
  echo "wrote $OUT/$name.png"
}

# The compositor's output is larger than the window it holds, so each capture
# has a band of empty desktop around it. The window is the only thing drawn,
# which makes "everything that is not the corner colour" an exact crop.
crop() {
  python3 - "$1" <<'PYCROP'
import sys
from PIL import Image, ImageChops

path = sys.argv[1]
im = Image.open(path).convert("RGB")
flat = Image.new("RGB", im.size, im.getpixel((0, 0)))
box = ImageChops.difference(im, flat).convert("L").point(lambda v: 255 if v > 8 else 0).getbbox()
if box:
    im.crop(box).save(path)
PYCROP
}

shoot week   Week
shoot month  Month
shoot year   Year
shoot agenda Agenda
