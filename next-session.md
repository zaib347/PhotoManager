# Next session

Where PhotoManager stands as of 2026-10-24 (evening), and what to pick up next.
The full plan and checklists are in [docs/PLAN.md](docs/PLAN.md).

## Start here tonight

**Milestone F, feature 1: copy and paste edits** (PLAN "F checklist"). New branch
`f-paste-edits`:
- Editor and viewer: **Copy edit** (keeps the photo's recipe in memory).
- Selection bar: **Paste edit**, with a choice of everything, or light and colour only
  (no crop/straighten).
- Applies through `edit::save` for each photo in one batch (videos skipped), bumps each
  photo's edit version (`photoLooks.setVersion`) so thumbnails refresh; each photo can still
  be reverted on its own.
Then F2 XMP sidecars, F3 map view, F4 Takeout/iCloud import, F5 collage, F6 slideshow/video.

**Map view decision (made 2026-10-24):** a **built-in offline world map** down to city level
(~20–40 MB bundled, works offline). No online tile provider for now.

## Where things stand

| Milestone | State |
|---|---|
| M0–M4, Tools (export, watermark, rotate, editor, faster JPEG, rename) | Done, on `main` |
| M5 part 1 (Windows: long/Unicode paths, RAW, video details) + CI | Done, on `main` |
| F (6 non-AI features) | Next, starting tonight |
| Beta prep, M6 subscription | After F |
| M7 AI pack (search, people, upscale; opt-in downloads) | Last |
| M5 parts 3–4 (macOS/Linux, signing, notarization) | Later, when the user says |

- Everything is merged and pushed (`main` = origin/main). No open branches with work.
- **Testing:** the user's team has the installer
  (`target/release/bundle/nsis/PhotoManager_0.1.0_x64-setup.exe`, built locally with
  `npm run tauri build`, unsigned). Expect bug reports; fix those before new work when
  they come in.
- Disk: `target/debug` cleaned 2026-10-24 (20 GB); `target/release` is 2.3 GB.

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

- **Not yet confirmed working in the app:** Live Photos (pairing needs a rescan) and faster hide.
