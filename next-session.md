# Next session

Where PhotoManager stands as of 2026-10-23, and what to pick up next.
The full plan and milestone checklists are in [docs/PLAN.md](docs/PLAN.md).

## Where things stand

| Milestone | State |
|---|---|
| M0 Foundations, M1 Scanner + timeline, M2 Organize | Done, merged to `main` |
| M3 Memory features | In progress |

**M3 done so far:**
- Date fixing, Select all / Ctrl+A, On This Day, Events. All merged to `main`.
- **Places**. On branch **`m3-places`**, **not merged yet**.

## First thing to do

1. Ask whether Places work in the app (needs photos with GPS):
   - On first start the existing library gets places in the background (log line
     "N places looked up"), then events are re-detected.
   - The info panel (**I**) shows "Town, Region, Country" above the coordinates.
   - **Filter** has **Country** (if more than one) and **Place** lists (with search); chips show names.
   - Untitled events show the place name instead of dates, with the dates underneath.
2. If they're OK, merge:
   `git checkout main && git merge --no-ff m3-places`
3. Start a new branch for the next feature.

## What's next in M3

1. **Trips.** Multi-day events far from home. Work out "home" from where most photos are taken
   (`assets.place_id`; synthetic libraries have a Lahore home and trips elsewhere).
2. **Browse by place.** Country → town page, like the Events page. The info panel's place could
   link to it.

## How the places code works

- `crates/photo-core/src/places.rs`: bundled data format (`pack`/`unpack`), 1° grid `Index`,
  `assign` (batched; runs in scan finalize before event detection, and once at startup via
  `event_commands::catch_up`).
- Data: `crates/photo-core/data/places.bin`, rebuilt with `examples/build_places.rs` from GeoNames
  dumps (see README; CC BY 4.0 credit is in the README).
- `assets.place_id`: GeoNames id, `0` = nothing within reach, `NULL` = not looked up yet. A rescan
  that changes GPS resets it.
- Events store their most common place in `events.place_id`.

After M3 come:
- **M4:** cleanup and safety, then the beta.
- **M5:** platform hardening (macOS QuickLook thumbnails, Unix volume UUIDs, HEIC/RAW).
- **M6:** subscription (annual plans only).
- **M7:** optional AI pack.

## How the events code works

- **Core logic:** `crates/photo-core/src/events.rs`.
  - Schema v7 adds `assets.event_id` and `events.count` / `cover_asset_id`.
  - Event status is one of `suggested`, `accepted` (edited by the user) or `rejected` (removed).
  - Event times are wall-clock ms, so the frontend formats them as UTC.
- **When detection runs:**
  - after every scan (`scan.rs` finalize);
  - after a date shift or restore (`dates.rs`);
  - once at startup when no events exist yet (`src-tauri/src/event_commands.rs`, which emits `events-changed`).
- **Frontend:**
  - `src/events/` holds the Events page, the event header and date formatting.
  - `"events"` is a `BuiltinView` that shows a page, not a timeline. `TimelineView` excludes it.
- **Known cost:** at 1M photos a re-run of detection holds the DB writer for about 2.7 s. If that becomes noticeable, move the read and sort onto a reader connection.
- Merge, split and create pull in loose or suggested items inside the edited event's new span
  right away (`settle`), matching what detection would do.
- **Filter options** take about 1 s at 1M photos (mostly the camera count). They're loaded when the
  panel opens.

## Working conventions

- Windows 11, Rust 1.93, Node 22.
- **No Python.** Edit files with the Edit tool, or with node scripts in `$TEMP/pmscripts/*.cjs`.
- Run `cargo fmt --all` after Rust edits. Edit anchors must match the formatted code.
- **Checks before every commit:**
  - `cargo fmt --all --check`
  - `cargo clippy --workspace --all-targets -- -D warnings`
  - `cargo test --workspace`
  - `npm run build`
- The frontend targets **ES2020**, so don't use `.at()` or `Intl.DateTimeFormat.formatRange`.
- **Git workflow:**
  - Work on feature branches; merge to `main` only after the user confirms.
  - Commits end with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- **Don't launch app windows** while the user may be using the app. The user runs `npm run tauri dev` themselves.
- **Speed checks:** run `cargo run --release -p photo-core --example scale_bench -- <db> 1000000` for 1M-photo timings.

## Waiting on the user

- **GitHub push:** they need to create an empty repo and share its URL.
- **Not yet confirmed working in the app:** Live Photos (pairing needs a rescan) and faster hide.
