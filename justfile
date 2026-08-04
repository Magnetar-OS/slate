# Name of the application's binary.
name := 'cosmic-calendar'
applet := 'cosmic-calendar-applet'
daemon := 'cosmic-calendar-daemon'
launcher := 'cosmic-calendar-launcher'
# The unique ID of the application.
appid := 'io.github.entro314labs.Calendar'
applet-appid := 'io.github.entro314labs.CalendarApplet'

# Path to root file system, which defaults to `/`.
rootdir := ''
# The prefix for the `/usr` directory.
prefix := '/usr'
# The location of the cargo target directory.
cargo-target-dir := env('CARGO_TARGET_DIR', 'target')

# Install destinations
base-dir := absolute_path(clean(rootdir / prefix))
bin-dst := base-dir / 'bin' / name
applet-bin-dst := base-dir / 'bin' / applet
daemon-bin-dst := base-dir / 'bin' / daemon
launcher-bin-dst := base-dir / 'bin' / launcher
desktop-dst := base-dir / 'share' / 'applications' / (appid + '.desktop')
applet-desktop-dst := base-dir / 'share' / 'applications' / (applet-appid + '.desktop')
systemd-dst := base-dir / 'lib' / 'systemd' / 'user' / (daemon + '.service')
launcher-dst := base-dir / 'share' / 'pop-launcher' / 'plugins' / 'calendar'
appdata-dst := base-dir / 'share' / 'metainfo' / (appid + '.metainfo.xml')
icons-dst := base-dir / 'share' / 'icons' / 'hicolor'
icon-svg-dst := icons-dst / 'scalable' / 'apps' / (appid + '.svg')

# Default recipe which runs `just build-release`
default: build-release

# Runs `cargo clean`
clean:
    cargo clean

# Removes vendored dependencies
clean-vendor:
    rm -rf .cargo vendor vendor.tar

# `cargo clean` and removes vendored dependencies
clean-dist: clean clean-vendor

# Compiles with debug profile
build-debug *args:
    cargo build {{args}}

# Compiles with release profile
build-release *args: (build-debug '--release' args)

# Compiles release profile with vendored dependencies
build-vendored *args: vendor-extract (build-release '--frozen --offline' args)

# Runs a clippy check
check *args:
    cargo clippy --all-features {{args}} -- -W clippy::pedantic

# Runs a clippy check with JSON message format
check-json: (check '--message-format=json')

# Runs the test suite
test *args:
    cargo test {{args}}

# Run the application for testing purposes
run *args:
    env RUST_LOG=cosmic_calendar=debug RUST_BACKTRACE=full cargo run {{args}}

# Installs every component
install:
    install -Dm0755 {{ cargo-target-dir / 'release' / name }} {{bin-dst}}
    install -Dm0755 {{ cargo-target-dir / 'release' / applet }} {{applet-bin-dst}}
    install -Dm0755 {{ cargo-target-dir / 'release' / daemon }} {{daemon-bin-dst}}
    install -Dm0755 {{ cargo-target-dir / 'release' / launcher }} {{launcher-bin-dst}}
    install -Dm0644 resources/app.desktop {{desktop-dst}}
    install -Dm0644 resources/applet.desktop {{applet-desktop-dst}}
    install -Dm0644 resources/app.metainfo.xml {{appdata-dst}}
    install -Dm0644 resources/icons/hicolor/scalable/apps/icon.svg {{icon-svg-dst}}
    install -Dm0644 resources/cosmic-calendar-daemon.service {{systemd-dst}}
    install -Dm0644 resources/launcher/plugin.ron {{launcher-dst}}/plugin.ron
    @echo 'Installed. Enable reminders with: systemctl --user enable --now {{daemon}}'

# Uninstalls installed files
uninstall:
    rm -f {{bin-dst}} {{applet-bin-dst}} {{daemon-bin-dst}} {{launcher-bin-dst}}
    rm -f {{desktop-dst}} {{applet-desktop-dst}} {{appdata-dst}} {{icon-svg-dst}} {{systemd-dst}}
    rm -rf {{launcher-dst}}

# Installs into the current user's home, no root needed. Handy for trying it out.
install-user:
    just rootdir='' prefix={{home_directory()}}/.local install
    install -Dm0644 resources/launcher/plugin.ron \
        {{home_directory()}}/.local/share/pop-launcher/plugins/calendar/plugin.ron

# Runs the reminder daemon in the foreground
run-daemon *args:
    env RUST_LOG=cosmic_calendar=debug cargo run --bin {{daemon}} {{args}}

# Runs the panel applet in the foreground
run-applet *args:
    env RUST_LOG=cosmic_calendar=debug cargo run --bin {{applet}} {{args}}

# Vendor dependencies locally
vendor:
    mkdir -p .cargo
    cargo vendor | head -n -1 > .cargo/config.toml
    echo 'directory = "vendor"' >> .cargo/config.toml
    tar pcf vendor.tar vendor
    rm -rf vendor

# Extracts vendored dependencies
vendor-extract:
    rm -rf vendor
    tar pxf vendor.tar
