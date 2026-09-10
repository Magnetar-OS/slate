# Name of the application's binary.
name := 'slate'
applet := 'slate-applet'
daemon := 'slate-daemon'
launcher := 'slate-launcher'
# The unique ID of the application.
appid := 'com.magnetaros.Slate'
applet-appid := 'com.magnetaros.SlateApplet'

# Path to root file system, which defaults to `/`.
rootdir := ''
# The prefix for the `/usr` directory.
prefix := '/usr'
# Set to '1' to install the debug build instead.
debug := '0'
# The location of the cargo target directory.
cargo-target-dir := env('CARGO_TARGET_DIR', 'target')
profile-dir := cargo-target-dir / (if debug == '1' { 'debug' } else { 'release' })

# Sources. Named after the IDs they install as, because desktop-file-validate
# and appstreamcli both check the file name against the component ID — a source
# called `app.desktop` fails validation that the installed copy would pass.
desktop-src := 'resources' / (appid + '.desktop')
applet-desktop-src := 'resources' / (applet-appid + '.desktop')
metainfo-src := 'resources' / (appid + '.metainfo.xml')
icon-dir := 'resources' / 'icons' / 'hicolor'
icon-src := icon-dir / 'scalable' / 'apps' / (appid + '.svg')
icon-symbolic-src := icon-dir / 'symbolic' / 'apps' / (appid + '-symbolic.svg')
# The scalable SVG is what modern toolkits pick up; the PNGs are rasterised
# from it at each size so the panel and the icon grid get pixel-exact art
# instead of a downscaled smudge.
icon-sizes := '16x16 24x24 32x32 48x48 64x64 128x128 256x256 512x512'

# Install destinations
base-dir := absolute_path(clean(rootdir / prefix))
bin-dst := base-dir / 'bin' / name
applet-bin-dst := base-dir / 'bin' / applet
daemon-bin-dst := base-dir / 'bin' / daemon
launcher-bin-dst := base-dir / 'bin' / launcher
desktop-dst := base-dir / 'share' / 'applications' / (appid + '.desktop')
applet-desktop-dst := base-dir / 'share' / 'applications' / (applet-appid + '.desktop')
systemd-dst := base-dir / 'lib' / 'systemd' / 'user' / (daemon + '.service')
# One directory per plugin, named for the plugin rather than for what it
# searches: `calendar` would collide with any other calendar plugin installed.
launcher-dst := base-dir / 'share' / 'pop-launcher' / 'plugins' / name
appdata-dst := base-dir / 'share' / 'metainfo' / (appid + '.metainfo.xml')
icons-dst := base-dir / 'share' / 'icons' / 'hicolor'
icon-svg-dst := icons-dst / 'scalable' / 'apps' / (appid + '.svg')
icon-symbolic-dst := icons-dst / 'symbolic' / 'apps' / (appid + '-symbolic.svg')

# Default recipe which runs `just build-release`
default: build-release

# Everything CI runs, in the order that fails cheapest first
check-all: validate-metadata fmt-check check test

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
    cargo build --locked {{args}}

# Compiles with release profile
build-release *args: (build-debug '--release' args)

# Compiles release profile with vendored dependencies
build-vendored *args: vendor-extract (build-release '--frozen --offline' args)

# Runs a clippy check
check *args:
    cargo clippy --all-features --all-targets --locked {{args}} -- -W clippy::pedantic

# Runs a clippy check with JSON message format
check-json: (check '--message-format=json')

# Checks formatting without rewriting anything
#
# Scoped to this package: `--all` would reach through the path dependency and
# reformat the cosmic-pim checkout, which is a different repository.
fmt-check:
    cargo fmt -p slate -- --check

# Rewrites formatting
fmt:
    cargo fmt -p slate

# Runs the test suite
test *args:
    cargo test --locked {{args}}

# Checks the desktop entries and AppStream metadata against their specs
#
# Offline: the metainfo names a remote icon on `main`, which appstreamcli cannot
# fetch from a branch that has not been merged yet. `validate-metadata-urls`
# does the reachability pass where it makes sense — after merge, not on a PR.
validate-metadata:
    desktop-file-validate {{desktop-src}}
    desktop-file-validate {{applet-desktop-src}}
    appstreamcli validate --no-net {{metainfo-src}}

# Also checks that the remote icon and URLs actually resolve
validate-metadata-urls:
    appstreamcli validate {{metainfo-src}}

# Run the application for testing purposes
run *args:
    env RUST_LOG=slate=debug RUST_BACKTRACE=full cargo run {{args}}

# Installs every component
# Installs files and refreshes the desktop and icon caches — nothing more.
# Enabling the daemon's unit is left to the user on purpose: installing files
# and activating services are separate acts.
install:
    install -Dm0755 {{ profile-dir / name }} {{bin-dst}}
    install -Dm0755 {{ profile-dir / applet }} {{applet-bin-dst}}
    install -Dm0755 {{ profile-dir / daemon }} {{daemon-bin-dst}}
    install -Dm0755 {{ profile-dir / launcher }} {{launcher-bin-dst}}
    install -Dm0644 {{desktop-src}} {{desktop-dst}}
    install -Dm0644 {{applet-desktop-src}} {{applet-desktop-dst}}
    install -Dm0644 {{metainfo-src}} {{appdata-dst}}
    install -Dm0644 {{icon-src}} {{icon-svg-dst}}
    install -Dm0644 {{icon-symbolic-src}} {{icon-symbolic-dst}}
    for size in {{icon-sizes}}; do \
        install -Dm0644 {{icon-dir}}/$size/apps/{{appid}}.png \
            {{icons-dst}}/$size/apps/{{appid}}.png; \
    done
    install -Dm0644 resources/slate-daemon.service {{systemd-dst}}
    install -Dm0644 resources/launcher/plugin.ron {{launcher-dst}}/plugin.ron
    # Guarded on rootdir: unguarded, these create cache files inside a staged
    # package root, which then ship in the package and conflict with every
    # other package's copy — a staged tree's caches belong to the package
    # manager's own hooks.
    if [ -z '{{rootdir}}' ]; then \
        update-desktop-database {{ base-dir / 'share' / 'applications' }} || true; \
        gtk-update-icon-cache -t {{icons-dst}} || true; \
    fi
    @echo 'Installed. Enable reminders with: systemctl --user enable --now {{daemon}}'

# Uninstalls installed files
uninstall:
    rm -f {{bin-dst}} {{applet-bin-dst}} {{daemon-bin-dst}} {{launcher-bin-dst}}
    rm -f {{desktop-dst}} {{applet-desktop-dst}} {{appdata-dst}} {{icon-svg-dst}} {{systemd-dst}}
    rm -f {{icon-symbolic-dst}}
    for size in {{icon-sizes}}; do \
        rm -f {{icons-dst}}/$size/apps/{{appid}}.png; \
    done
    rm -rf {{launcher-dst}}

# Installs into the current user's home, no root needed. Handy for trying it out.
install-user:
    just rootdir='' prefix={{home_directory()}}/.local install

# Runs the reminder daemon in the foreground
run-daemon *args:
    env RUST_LOG=slate=debug cargo run --bin {{daemon}} {{args}}

# Runs the panel applet in the foreground
#
# X_PRIVILEGED_WAYLAND_SOCKET is unset deliberately. cosmic-session exports it
# to every child — terminals included — as a file descriptor number that is only
# valid inside cosmic-panel. libcosmic's activation-token thread adopts it with
# `from_raw_fd` and unwraps, so a standalone applet run panics that thread on
# EBADF and silently loses its tokens. Unset, it connects through
# WAYLAND_DISPLAY like any other client. Inside the panel the variable is
# correct and this recipe is not what runs.
run-applet *args:
    env -u X_PRIVILEGED_WAYLAND_SOCKET RUST_LOG=slate=debug cargo run --bin {{applet}} {{args}}

# Vendor dependencies locally
#
# The tar is deterministic — sorted names, a fixed mtime from SOURCE_DATE_EPOCH
# — so a packager rebuilding the tarball gets the same bytes.
vendor:
    mkdir -p .cargo
    cargo vendor | head -n -1 > .cargo/config.toml
    echo 'directory = "vendor"' >> .cargo/config.toml
    tar --sort=name --mtime="@${SOURCE_DATE_EPOCH:-0}" --owner=0 --group=0 --numeric-owner -pcf vendor.tar vendor
    rm -rf vendor

# Extracts vendored dependencies
vendor-extract:
    rm -rf vendor
    tar pxf vendor.tar

# Bump the version, add a release to the metainfo, commit, and tag
#
# The metainfo edit is deliberately manual-looking: `<releases>` is what an app
# store shows as "what's new", and a generated "Initial release." for every tag
# is worse than nothing.
tag version:
    sed -i '0,/^version/s/^version.*/version = "{{version}}"/' Cargo.toml
    cargo check --locked
    @echo 'Now add a <release> entry for {{version}} to {{metainfo-src}}, then:'
    @echo '  git add Cargo.toml Cargo.lock {{metainfo-src}}'
    @echo '  git commit -m "release: {{version}}" && git tag -a {{version}} -m ""'
