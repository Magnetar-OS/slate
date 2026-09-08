# Performance baselines

The roadmap's M0 gate: measured numbers for the two CPU-side legs of drawing a
busy calendar, so later interaction work (M4 and beyond) is compared against a
record rather than a feeling. The budget it serves: a full frame under ~4 ms at
240 Hz with 200+ visible events.

Neither harness needs a display; the GPU leg (iced/wgpu render) is not covered
here and is observed in the running app (`RUST_LOG=slate=debug`, compositor
frame stats).

## How to run

```sh
# Store query path (cosmic-pim checkout beside this one):
cargo test --release -p cosmic-pim-core --test perf -- --ignored --nocapture

# Time-grid layout:
cargo test --release timegrid_layout_baseline -- --ignored --nocapture
```

Re-run both after touching the index query, the expansion path, or
`timegrid::layout_day`, and compare against the table. A regression is treated
as a failure of the change that caused it.

## Recorded baselines

Machine: CachyOS, kernel 7.2.2-1, release profile (thin LTO, 1 codegen unit).
Date: 2026-09-01.

| Leg | Workload | min | p50 | p95 |
|---|---|---|---|---|
| `Store::occurrences` (query + expand + merge) | 31-day window, 410 instances (200 one-offs + 50 weekly series) | 348 µs | 351 µs | 398 µs |
| `TimeGrid::layout_day` × 7 | 210 heavily-overlapping blocks (busy week view) | 4 µs | 4 µs | 6 µs |

## Reading

- The **query path** runs once per reload (navigation, edit, fs change), not
  per frame — at ~0.35 ms it is invisible even if it ran every frame.
- The **layout path** runs per view construction; at 4 µs for a busy week it
  leaves the entire frame budget to widget tree diffing and the GPU. The
  spacer-based positioning allocates small vectors per day; not worth
  optimising at three orders of magnitude under budget.
- Occurrence expansion stays uncached by design (see README, "Files are the
  source of truth") — these numbers are why that choice costs nothing.
