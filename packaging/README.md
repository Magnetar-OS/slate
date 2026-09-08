# Packaging

| Path | What |
| --- | --- |
| `linux/nfpm.yaml` | `.deb` / `.rpm` / Arch packages. Consumed by the release workflow; the file layout comes from `just rootdir=… install`, so the justfile stays the single source of truth. |
| `flatpak/io.github.entro314labs.Slate.yml` | Flathub manifest skeleton. Ships the app only — the applet, reminder daemon, and launcher plugin structurally require host paths and stay native (the manifest's header says why). Blocked on: a release tag, cosmic-pim as a git dependency, and a generated `cargo-sources.json`. |

The native packages install all four binaries; Flatpak is the app-only,
store-distributed complement, not a replacement.
