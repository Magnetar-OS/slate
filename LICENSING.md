# Licensing

**Slate is GPL-3.0-only.** That is the licence of everything in this repository and
of the binaries it produces.

## The two-licence arrangement

| Layer | Licence |
|---|---|
| This repository — Slate and its binaries | **GPL-3.0-only** |
| [cosmic-pim](https://github.com/entro314-labs/cosmic-pim), the shared substrate it links | **MPL-2.0** |

The substrate is MPL-2.0 on purpose. Its boundary is the *file*: modify a file in
cosmic-pim and you publish that file, but linking it imposes nothing on your own
code. That lets one sync engine be shared by this app, by the rest of the suite,
and by consumers that are not GPL — while improvements to the engine itself stay
public.

MPL-2.0 is a "Secondary Licence" under its own §3.3, so GPL-3 absorbs it: the
binary distributed from this repository is GPL-3 as a whole, and the MPL'd files
remain MPL for anyone who extracts them.

**The trap.** MPL-2.0 Exhibit B ("Incompatible With Secondary Licenses") turns
that compatibility off. If it ever appears on a file in cosmic-pim, this
application can no longer legally link it. The headers there are Exhibit A only:

```rust
// SPDX-License-Identifier: MPL-2.0
```

## The full rationale

Why MPL and not MIT, why not LGPL, the provenance of the borrowed code, and the
outstanding obligations live in
[cosmic-pim/LICENSING.md](https://github.com/entro314-labs/cosmic-pim/blob/main/LICENSING.md).
That is the canonical copy; this file only records what the arrangement means
for **this** repository.

One item from it is worth repeating, because it blocks distribution rather than
merely being untidy: parts of the substrate derive from the
[Meltemi](https://github.com/entro314-labs/meltemi) project, which carries no
licence declaration at all. Until that is fixed, the substrate must not be
published to crates.io or offered upstream.
