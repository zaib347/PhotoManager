# Next session

Where PhotoManager stands as of 2026-10-23, and what to pick up next.
The full plan and milestone checklists are in [docs/PLAN.md](docs/PLAN.md).

## Where things stand

| Milestone | State |
|---|---|
| M0 Foundations, M1 Scanner + timeline, M2 Organize | Done, merged to `main` |
| M3 Memory features | In progress |

**Done:** M3 except Browse by place (Date fixing, On This Day, Events, Places, Trips), all on `main`.
**M4 built, not yet tested by the user** (they said "we test later"). On `main`: exact
duplicates, safe file operations, similar photos, best-shot ranking. On branch **`m4-remove`**
(not merged): remove photos from the timeline/viewer, Recently removed page, organizer.

**Test list for M4** (needs real photo files, not synthetic):
- Duplicates: copies in two folders → Duplicates page → remove → Undo.
- Similar photos: burst shots → groups, Best badge with a reason, Blurry/Too dark notes.
- Remove: select photos → **Remove** (or Delete) → gone from the timeline → **Recently removed**
  → Undo brings them back. Delete in the viewer removes the current photo.
- Organize files: pick a folder → Preview → Move → folders `2023/2023-06/…` → Undo moves back.
- Send to Recycle Bin from Recently removed frees the space.

Best-shot test: in a burst group the badge should say why ("Sharpest", "Best exposed"…) and
blurry/dark shots get a warning under them. Assessment of grouped photos runs after hashing
(log line "N shots assessed").

**Tools milestone (new, before the beta; installer/signing/auto-update are left for the very
end, by the user's choice):** export merged to `main`; watermark on branch **`tools-watermark`**,
not merged yet. Test export: select photos → **Export** → pick format/size/quality/folder →
toolbar shows progress → files keep their date (check a HEIC → JPEG too, including its rotation,
which isn't verified). Test watermark: in the Export panel tick **Add a watermark** → type text or
pick a logo PNG → the preview on the first selected photo updates → exported copies carry it.
Next tools: lossless rotate, then the editor (see PLAN "Tools checklist").

## First thing to do

1. Ask whether Similar photos work in the app. Needs real photos (synthetic ones have no files):
   a few shots of the same scene taken seconds apart, or a photo plus a resized/WhatsApp copy.
   - On start (and after each scan) photos are hashed in the background; the page says
     "Still comparing N photos" until done.
   - **Similar photos** in the sidebar: groups with the best one ticked (Best badge).
   - **Remove N** per group, **Keep all** (group disappears), or remove unticked in all shown.
   - **Recently removed** (shared with Duplicates) → **Undo** brings removed photos back.
2. Merge: `git checkout main && git merge --no-ff m4-remove` (the user may ask to merge before
   testing, as with earlier branches).
3. Next: the free public beta (see PLAN). Still open: Browse by place (M3), faster assessment
   (DCT-scaled decode).

## How the similar-photos code works

- `crates/photo-core/src/similar.rs`: `fingerprint` (pHash + sharpness), `hash_pending`
  (batches of 256, parallel, uses or creates grid thumbnails), `detect` (groups on a reader,
  short write), `groups` with `best_index`, `dismiss`.
- Tauri `similar_commands::run_in_background`: startup and after each scan; `scans::start` sets
  `hash_cancel` so hashing pauses during scans.
- Thresholds: `WINDOW_MS` 3 min / `SIMILAR_DIST` 10 bits / `COPY_DIST` 3 bits. Untested on real
  libraries yet; expect to tune them.

## How the cleanup code works

- `crates/photo-core/src/fileops.rs`: `trash` (plan → move → record, per 64 files), `undo`,
  `empty` (OS trash only where `volume::has_recycle_bin`), `recover` (startup), `list`.
- `crates/photo-core/src/duplicates.rs`: `overview`, `groups` (paged), `extras` (which file ids
  to remove), `keep_rank` heuristics.
- Tauri: `src-tauri/src/cleanup_commands.rs`; refuses to move files while a scan runs.
- Scanner: `twin_in_batch` joins identical new files within one commit batch.

## Disk space

D: filled up (0 GB) once: `target/` had grown to 17 GB. `cargo clean` was run on 2026-10-24
(freed 13.6 GB). Checks run with `CARGO_INCREMENTAL=0` to keep the incremental cache from
growing back. Run `cargo clean` again if space runs low.

## How the trips code works

- `crates/photo-core/src/trips.rs`. A trip is a local-day range; `View::Trip` shows every
  visible photo of those days. `trip_places` holds the away towns, with counts and first days.
- Runs after places and events: scan finalize, date shift/restore, startup `catch_up`,
  synthetic generate/clear.
- Status works like events: `suggested` (redrawn), `accepted` (renamed, days fixed), `rejected`
  (removed; overlapping candidates are dropped).

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
