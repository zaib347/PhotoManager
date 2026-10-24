# Next session

Where PhotoManager stands as of 2026-10-24 (evening), and what to pick up next.
The full plan and checklists are in [docs/PLAN.md](docs/PLAN.md).

## Start here

**F1–F4 are built, committed and waiting for the user to test** (built 2026-10-24 while
the user was out; they asked for all of them in one go without stopping for approval).
They sit on **stacked branches**, each started from the one before, so merging the last
one brings all four:

| Branch | Feature | Commit |
|---|---|---|
| `f-paste-edits` | F1 copy/paste edits | 550777c |
| `f-xmp` | F2 XMP sidecars | 5f6563e |
| `f-map` | F3 map view | 737cc72 |
| `f-import` | F4 Google Takeout / iCloud import | 697cbd7 |

- **Not merged and not pushed.** Ask the user before merging `f-import` into `main`
  (fast-forward) and pushing; then delete the four branches.
- All checks passed on each branch (fmt, clippy -D warnings, all tests, `npm run build`).
  None of it has been run in the app window yet.
- Schema is now **v17** (v15 XMP, v16 `user_meta.location_set`, v17 imports).

**What the user should try in the app:**
1. Paste edits: edit a photo → Copy edit → select others → Paste edit (both choices).
2. XMP: rescan a folder with Lightroom/darktable `.xmp` files → ratings, keywords (tags),
   descriptions (Info panel, editable now). Sidebar "Sidecars": turn on writing, change a
   rating/tag, check the `.xmp` next to the photo a few seconds later.
3. Map: sidebar Map → pan (drag), zoom (wheel, double-click, +/−), town search, click a
   cluster → timeline filtered to that area ("Map area" chip). Selection bar → Location →
   pick a town, or "Pick on the map…". Check light and dark themes.
4. Import: add a Takeout or iCloud export folder, scan it, then the folder's "Import…" →
   Import from export → report; Undo in the same panel.

**Then:** F5 collage and print layouts, F6 memory slideshow and video.

## Where things stand

| Milestone | State |
|---|---|
| M0–M4, Tools (export, watermark, rotate, editor, faster JPEG, rename) | Done, on `main` |
| M5 part 1 (Windows: long/Unicode paths, RAW, video details) + CI | Done, on `main` |
| F (6 non-AI features) | F1–F4 built (unmerged branches); F5, F6 next |
| Beta prep, M6 subscription | After F |
| M7 AI pack (search, people, upscale; opt-in downloads) | Last |
| M5 parts 3–4 (macOS/Linux, signing, notarization) | Later, when the user says |

- `main` = origin/main (8f93bec + nothing new). The work is on the branches above.
- **Testing:** the user's team has the installer
  (`target/release/bundle/nsis/PhotoManager_0.1.0_x64-setup.exe`, built locally with
  `npm run tauri build`, unsigned). Expect bug reports; fix those before new work when
  they come in.
- Disk: `target/debug` cleaned 2026-10-24 (20 GB); `target/release` is 2.3 GB.

## How the F1–F4 code works

- **Paste edits:** `edit::paste` (one transaction, videos skipped, `PasteWhat::Tone` keeps
  each photo's crop/straighten); `edit_paste` command drops thumbnails. Copied recipe lives
  in `src/editor/copied.ts` (memory only).
- **XMP:** `crates/photo-core/src/xmp.rs`. `parse` (quick-xml 0.37, namespace-aware),
  `merge` (our values go in their own `rdf:Description`; the same properties are removed
  elsewhere; idempotent), `embedded` (JPEG APP1). Scan: `.xmp` files collected in the walk,
  `import_sidecars` in finalize (`xmp_seen` by mtime), embedded XMP applied in commit. Reads
  only fill blanks (rating, note) and add tags; label "Favorite" ⇒ favorite, other labels ⇒
  tags. Writing: `settings` table (`xmp_write`, `xmp_favorite`), triggers fill `xmp_dirty`,
  `xmp_commands::run_writer` polls every 3 s. Sidecar name: `IMG_1.xmp`, or `IMG_1.JPG.xmp`
  when another file shares the stem or that one exists. `user_meta.note` is the description.
- **Map:** data `crates/photo-core/data/map.bin` (9.5 MB) from `examples/build_map.rs`
  (Natural Earth 5.1.2 GeoJSON, public domain; format in the example's doc comment). Frontend
  `src/map/world.ts` decodes tiles lazily into `Path2D`s and draws per zoom level;
  `MapPage.tsx` handles pan/zoom, clusters (DOM markers), placing mode. Backend
  `map::Points` (loaded on `map_load`, ~0.4 s at 1M) and `clusters` (4–25 ms per view at
  1M). `Filter.area` filters the timeline. `map::set_location` sets `location_set`, which
  `scan::update_asset` respects.
- **Import:** `crates/photo-core/src/imports.rs`. `plan` reads Takeout JSONs (name
  matching: `.supplemental-metadata` cut short, `(n)` counters, long names, `-edited`
  copies) and iCloud `Photo Details*.csv` / `Albums/*.csv`; `run` applies in one
  transaction with a journal (`imports`, `import_items`, `import_albums`); `undo` takes it
  back. Imported dates use `taken_src` 5 and the date-fix original columns.

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

## GitHub

`origin` = https://github.com/zaib347/PhotoManager (public). Push `main` after each merge.
CI (`ci.yml`) checks every branch on Windows; **Build apps** (`build-apps.yml`, run from the
Actions tab or a `v*` tag) makes unsigned installers. macOS/Linux entries are commented out.

## The user's choices

- Windows only for now; macOS/Linux later. Windows code signing later; macOS signing and
  notarization at the very end. Installer polish and auto-update just before the beta.
- Build each feature on its own branch and ask before merging; they test in batches.
- AI features last, each off by default with its model downloaded only on request.

## Waiting on the user

- Live Photos and faster hide: confirmed working by the user (2026-10-24).
- **F1–F4** (branches above): built, waiting for the user to test and say merge.
