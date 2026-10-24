# Next session

Where PhotoManager stands as of 2026-10-24 (night), and what to pick up next.
The full plan and checklists are in [docs/PLAN.md](docs/PLAN.md).

## Start here

**M7, the AI pack, is complete** (the user asked for all of it, then OCR, in one go):

| Branch | Feature | State |
|---|---|---|
| `m7-ai` (from `f-slideshow`) | M7.1 search by description, smart albums | merged to `main` and pushed |
| `m7-people-upscale-text` (from `m7-ai`) | M7.2 People, M7.3 Upscale, M7.4 Text in photos (Windows OCR) | merged to `main` and pushed |

- Schema is **v20** (v18 search embeddings, v19 faces/people, v20 text FTS5); thumbs DB v2
  (face portraits).
- On 2026-10-24 the user asked to push: `main` now has F5, F6 and all of M7, pushed to origin.
- **After M7 the user wants detailed testing of the whole app, then the free trial and
  subscription system (M6) as the very last step.**

**What the user should try in the app (AI page in the sidebar):**
1. Search by description: Install (~167 MB), wait for indexing, type "beach", "food"…
2. People: Install (~39 MB). Faces are found in the background (People appears in the
   sidebar). Click a name to name someone; Ctrl-click two people → "Same person: merge";
   in a person's photos select wrong ones → "Not this person"; one photo → "Use as their
   picture"; Hide. A photo's info lists the named people in it.
3. Upscale: Install (4.5 MB). Select small photos → Export → Enlarge (AI) 2× or 4×.
4. Text in photos: Turn on (no download). Search words seen in screenshots/receipts; a
   photo's info shows its text with Copy text.

**Earlier: milestone F is complete.** F1–F4 are merged into `main` locally (the user confirmed them
2026-10-24). F5 and F6 were built right after, at the user's request, without stopping
for approval:

| Branch | Feature | State |
|---|---|---|
| `f-collage` | F5 collage and print layouts | committed, waiting for the user's test |
| `f-slideshow` (from `f-collage`) | F6 memory slideshow and video | committed, waiting for the user's test |

- F5–F6 were merged to `main` and pushed with M7 (2026-10-24).
- Schema is **v17**.

**What the user should try in the app:**
1. Collage: select 2–9 photos → Collage. Layouts (Grid, Mosaic, Feature, Strips), sizes
   (Square, Story, Wide, prints 4×6/5×7/8×10/A4 at 300 dpi), portrait/landscape, spacing,
   corners, background. Drag a photo onto another to swap; click one, then drag to move it
   in its frame, scroll or slider to zoom. Save… → JPEG, opens its folder.
2. Slideshow: ▶ Play in an event, a trip, On This Day, or select photos → ▶ Slideshow.
   Space/arrows/Esc, speed, ♪ Music… (mp3/m4a/wav/flac/ogg), Save as video… → MP4 made in
   the background ("Making video N%" in the toolbar, Stop).

**Next (the user's order, 2026-10-24):** the user tests everything in detail; then the
free trial and **M6 subscription/payments** last. See docs/PLAN.md.

## Where things stand

| Milestone | State |
|---|---|
| M0–M4, Tools, M5 part 1 + CI | Done, on `main` |
| F1–F4 | Done, merged to `main` (not pushed) |
| F5–F6 | On `main`, pushed; the user still tests them |
| M7 AI pack (search, people, upscale, OCR; opt-in) | On `main`, pushed; the user still tests it |
| M6 free trial + subscription | Last, after detailed testing |
| Beta prep; M5 parts 3–4 (macOS/Linux, signing) | Later, when the user says |

- **Testing:** the user's team has the installer
  (`target/release/bundle/nsis/PhotoManager_0.1.0_x64-setup.exe`, built locally with
  `npm run tauri build`, unsigned). Expect bug reports; fix those before new work.

## How the M7 (AI pack) code works

- Crate `crates/photo-ai`. `models.rs`: packs (Runtime, Search, People, Upscale, Text)
  with pinned URLs, sizes and SHA-256s; a package can keep several files (Upscale's zip:
  `.onnx` + `.data`); Text has no files (installing writes the manifest). `runtime.rs`:
  ONNX Runtime 1.24.4 loaded from the download, CPU only (DirectML was slower and crashed).
- `clip.rs`/`search.rs`: CLIP embeddings of grid thumbnails, one photo per run (8-bit
  model is batch-dependent), z-score search. `faces.rs`: YuNet decode + SFace with
  OpenCV's alignment. `people.rs`: `Groups` (sums of face vectors per person), `assign`
  (≥ 0.45 to an average face, one face per person per photo), `merges` after each pass;
  `People` holds the groups and drops them whenever the user changes people (under the
  library write lock). `picture.rs`: photos read upright at reduced size. `upscale.rs`:
  128 px tiles, 12 px margins cut. `text.rs`: Windows.Media.Ocr on its own MTA thread,
  FTS5 `ai_text` (rowid = asset id; a trigger clears it with the asset).
- Tauri `ai_commands.rs`: install/remove (runtime only for packs that need it), one
  indexer thread taking turns (64 photos each for search, faces, text; a failing feature
  stops alone), threads = a quarter of logical CPUs in the background, physical cores for
  enlarging. `face://<face id>` serves portraits (made again from the photo if missing).
  Export takes the upscaler as `export::Enlarge` (photo-core stays model-free).
- Tests with real models: `PHOTO_AI_TEST_MODELS=<folder with the installed files>`
  (`onnxruntime.dll`, CLIP files, `detector.onnx`, `recognizer.onnx`,
  `real_esrgan_general_x4v3.onnx/.data`) and `PHOTO_AI_TEST_FACES=<LFW folder>` for
  People; downloads are `--ignored` tests. `examples/faces_eval.rs` measures grouping on
  a folder of people (LFW gave 16/16 people, 0 wrong faces).

## How the F5–F6 code works

- **Collage:** `crates/photo-core/src/collage.rs` renders a `Collage` (cells in canvas
  fractions + pan/zoom per photo; `cell_rect` and `crop_for` match `src/collage/templates.ts`
  exactly, so the DOM preview equals the file). Saved with `jpeg::encode_print` (JFIF dpi)
  and the first photo's EXIF without GPS (`export::exif_for_copy`). Templates in TS
  (`grid`, `mosaic` = justified rows by photo aspect, `feature`, `strips`; drawn landscape,
  transposed for portrait). Command `save_collage`; the save dialog needed
  `dialog:allow-save`.
- **Slideshow:** `movie::pick` (no videos, best of each similar group, cut to the best
  `MAX_PHOTOS`=150 by favorite/rating/focus/exposure, then time order). Frontend
  `src/slideshow/Slideshow.tsx`: CSS crossfade + pan/zoom matching the video's moves;
  portraits shown whole over a blurred copy; music via `read_audio` bytes → Blob URL.
- **Video:** `movie::make`: 1920×1080, 30 fps; frames rendered in parallel batches of 24
  (bilinear crop+resize with fast_image_resize), BT.709 NV12, written with a Media
  Foundation sink writer (H.264 Main 12 Mbit/s, AAC 192 kbit/s, 48 kHz stereo). Music
  decoded with an MF source reader, resampled/looped/faded in Rust (`fit_music`).
  Non-Windows builds return "works on Windows for now". Tauri: `make_video` thread with
  `video-progress` / `video-finished` events, `cancel_video` (partial file removed).
  Needs the `Win32_Media_MediaFoundation` feature of `windows`.

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
- F1–F4: confirmed working by the user (2026-10-24), merged locally.
- **F5–F6** (branches above): built, waiting for the user to test and say merge.
- **M7** (`m7-ai`, `m7-people-upscale-text`): built, waiting for the user to test and say merge.
