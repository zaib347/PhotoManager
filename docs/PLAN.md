# PhotoManager — Build Plan

Working name: PhotoManager (rename after trademark/domain check).
Desktop memory manager for libraries of millions of photos and videos across drives.

## Locked decisions

| Area | Decision |
|---|---|
| Platforms | Windows, macOS, Linux (desktop first) |
| Scale target | Millions of assets across internal/external/NAS drives |
| Identity | Memory manager: timeline, events, trips, "on this day" |
| Media | Photos **and videos** in v1 (incl. Live Photo pairing) |
| Stack | Rust + Tauri v2 + React/TypeScript/Vite + SQLite (`rusqlite`, bundled) |
| AI | Optional downloads only (model + ONNX Runtime fetched on demand) |
| Monetization | Annual subscription only, via a merchant of record |
| Signing | Apple Developer account + Windows code-signing cert already available |
| Mobile | Not planned. `photo-core` stays Tauri-free so it *could* be reused later |

## Repository layout

```
Cargo.toml              workspace
crates/photo-core/      DB, schema, queries, scanning, thumbnails — no Tauri, no UI
src-tauri/              desktop shell: commands, thumb:// protocol, app state
src/                    React UI
docs/                   this plan and design notes
```

Rule: anything that is not window/IPC glue goes in `photo-core`. That keeps it testable from
`cargo test` and benchmarkable from `cargo run --example`, and keeps a mobile option open.

## Architecture rules (the things that matter at millions of rows)

1. **Two SQLite files.** `library.sqlite` (index + user data) and `thumbs.sqlite` (JPEG blobs).
   Thumbnails never live as millions of loose files.
2. **One writer, many readers.** WAL mode, a single writer connection, a small reader pool.
   Write in batches (10k–50k rows per transaction), never one transaction per scan.
3. **Thumbnails over a custom URI scheme** (`thumb://<asset_id>`), served from Rust.
   Never base64 over IPC.
4. **Timeline is day-bucketed.** `day_buckets(day, count)` drives layout and the scrubber;
   the UI fetches asset ids per visible day range (keyset on `taken_at`), never `OFFSET`.
5. **Custom virtual scroller.** 1M photos × 200px rows exceeds browser max element height
   (~33M px in Chromium, less in WebKit). The grid owns its own scroll offset and a
   Google-Photos-style date scrubber instead of relying on native scroll height.
6. **Files ≠ assets.** `files` is where bytes live; `assets` is the photo/video the user cares about.
   User data (`user_meta`, tags, albums) hangs off assets, so it survives moves, renames and copies.
7. **Drives by identity, not letter.** `volumes.uuid` (volume serial / FS UUID); paths are relative.
   A missing file is only marked missing after a *complete* scan of a *mounted* volume.
8. **Staged pipeline with priorities.** discover → header/EXIF → thumbnail → quick hash → full hash
   (only on size+quick-hash collisions) → phash/blur → (AI). Visible thumbnails jump the queue.
9. **Thumbnail fast path.** Embedded EXIF thumbnail / RAW preview first (instant, but ~160px),
   then upgrade to a real 256px thumbnail in the background.
10. **No OpenCV, no bundled ffmpeg, no bundled AI runtime.** Small installer.

## Video in v1

| Concern | Approach |
|---|---|
| Metadata (date, GPS, duration) | Parse MP4/MOV atoms in Rust (candidate: `nom-exif`; validate) |
| Poster frames | ffmpeg downloaded on first video encounter (LGPL build, sidecar); later native (Media Foundation / AVFoundation) |
| Playback | `<video>` fed by a custom protocol with HTTP range support |
| Codec caveats | WebView2: H.264 ok, HEVC needs the OS extension. WKWebView: HEVC ok. WebKitGTK: depends on installed GStreamer plugins — show "open in system player" fallback |
| Live Photos | Pair HEIC/JPG + MOV by Apple `ContentIdentifier`; show as one asset |

## Format support

| Format | Decoder |
|---|---|
| JPEG | `zune-jpeg` (via `image`) + embedded EXIF thumbnail |
| PNG/WebP/GIF | `image` |
| HEIC/HEIF | OS-native: WIC (Windows, needs HEIF extension), ImageIO (macOS), libheif (Linux) |
| RAW | Embedded preview JPEG (`rawler` or own TIFF/IFD parsing) |
| Video | see above |

## Milestones and exit criteria

| # | Milestone | Weeks | Exit test |
|---|---|---|---|
| M0 | Technical spike | 2–3 | 1M synthetic assets: smooth scroll, scrubber jump < 100 ms, cached thumb < 50 ms, flat memory. Real-folder thumbnail bench numbers recorded. |
| M1 | Indexer | 4–5 | Kill mid-scan → resumes. Unplug drive → nothing marked deleted. Unchanged 100k rescan in seconds. |
| M2 | Browsing | 4–5 | Viewer, metadata panel, filters, favorites/tags/albums, video playback. First demo build. |
| M3 | Memory features | 4 | On This Day, event detection (adaptive time gaps), trips (GPS away from home), offline place names (GeoNames), time correction incl. bulk camera-clock shift. |
| M4 | Cleanup & safety | 4 | Exact dupes (size → quick hash → full hash), near-dupes/bursts (phash within time window, BK-tree), best-shot ranking, planned/previewed/journaled/undoable ops, trash-only delete. |
| T | Tools | 4–5 | Export copies (resize, convert incl. HEIC → JPEG, compress, metadata kept, remove location), watermark, lossless rotate, non-destructive editor. Upscale and other AI tools go in M7. |
| F | Features (non-AI) | 7–9 | Copy/paste edits, XMP sidecars, map view, Google Takeout/iCloud import, collage and print layouts, memory slideshow and video. No AI, no downloads; see "F checklist". |
| — | **Free public beta** | ~month 5–6 | Real libraries, real crash reports. Installer, code signing and auto-update are done just before. |
| M5 | Platform hardening | 4 | HEIC/RAW/video on all 3 OSes, long/Unicode paths, macOS permissions, AppImage/deb, signed + notarized builds, auto-update. |
| M6 | Annual subscription | 2–3 | MoR checkout, signed offline license token, 14–30 day grace. Browsing, timeline and export are never paywalled. |
| M7 | AI pack (opt-in) | 6–8 | Built last. Nothing is downloaded unless the user turns a feature on. Model manager (download, verify, delete), `ort` with `load-dynamic`; search by description, then people (face grouping), then upscale. See "M7 checklist". |

Realistic solo estimate: **9–12 months** to paid 1.0.

## Free vs Pro (annual)

- **Free:** browse, timeline, on-this-day, albums/tags/favorites, basic search, exact duplicates, export.
- **Pro:** events & trips, near-duplicates + best-shot, organizer with undo, advanced filters, AI pack.
  Pro should be the part that keeps improving, which is what justifies a recurring price.

## M0 checklist

- [x] Tauri v2 + React scaffold, Cargo workspace, `photo-core` crate
- [x] Schema v1 migration (`user_version`-based)
- [x] Synthetic generator (event-clustered timestamps, ~10% videos)
- [x] `day_buckets` layout + per-day asset id queries
- [x] `thumb://` async protocol backed by `thumbs.sqlite` (placeholder renderer stands in for the pipeline)
- [x] Custom virtual timeline grid + date scrubber + perf overlay
- [x] `thumb_bench` example: embedded-EXIF vs full-decode throughput on a real folder
- [ ] In-app exit tests (scroll fps, thumb latency) on this machine and a low-end laptop

## M1 checklist

- [x] Volume identity (Windows volume serial; macOS/Linux use mount path until M5)
- [x] Roots: add (rejects overlapping folders), remove, list with online/offline status
- [x] Drive plug/unplug polling (5 s) → `volumes-changed` event
- [x] Scanner: parallel walk, skips hidden + system folders (`$RECYCLE.BIN`, `@eaDir`, …)
- [x] Unchanged files skipped by size + mtime; changed files re-extracted
- [x] Metadata: EXIF date/offset/camera/GPS/orientation/size (incl. thumbnail-IFD fallback),
      MP4/MOV atoms (date, duration, size, GPS), filename dates, epoch filenames, mtime
- [x] Quick hash on every file; full hash only on quick-hash collisions
- [x] Moves/renames keep the asset (and favorites, tags, albums); identical copies join one asset
- [x] Batched commits (256 files/transaction), timeline refreshes every 5 s during a scan
- [x] Missing files only marked after a complete, error-free scan of a mounted root
- [x] Resume after crash: `running` jobs restart on launch; committed files skip as unchanged
- [x] Thumbnails: embedded EXIF JPEG at scan time; full decode on first view (bounded pool);
      offline/undecodable items get an uncached placeholder
- [x] UI: folder sidebar, add/rescan/stop/remove, live progress, timeline updates during scan
- [ ] Not yet: HEIC/RAW decoding (M5), video poster frames (M2), Live Photo pairing (M2),
      non-UTF-8 paths on Linux (counted as errors), filesystem UUIDs on macOS/Linux (M5)

## M2 checklist — complete

Batch 1 (browsing core):
- [x] Views: Photos, Favorites, Videos, Hidden (`View` filter shared by layout and day queries)
- [x] Full-screen viewer: fit, wheel zoom at cursor, drag pan, double-click zoom, ←/→ across days,
      neighbour preloading, blurred thumbnail while the original loads
- [x] `media://` protocol: originals served directly (byte ranges for video), Rust-decoded preview
      for formats the webview can't show, clear message + "Open in default app" otherwise
- [x] Video playback in the viewer (formats the webview supports)
- [x] Info panel: capture time + its source (warns when it is only the file date), camera,
      dimensions, duration, GPS, every copy with drive status and "Show in folder"
- [x] Selection: click/Ctrl/Shift ranges across days, select a whole day, Esc to clear
- [x] Favorite / rating / hide for one item (viewer keys F, 0–5) or a selection

Batch 2:
- [x] Albums: create (sidebar + or from a selection), add/remove, rename, delete, cover,
      album grid (hidden items still show), album chips in the info panel
- [x] Tags: case-insensitive, add from selection or info panel, tag views, unused tags removed
- [x] Filters: type, rating, date range, camera, folder; combine with any view; chips to remove.
      At 1M assets (indexes added in schema v4): type 89 ms, camera 130 ms, one year 45 ms;
      indexes cost ~37 MB per million assets
- [x] Video poster frames via the OS instead of downloading ffmpeg: Windows shell thumbnails
      (what Explorer shows; ~0.3 s per video, cached after), system ffmpeg elsewhere if present.
      Same path gives HEIC/RAW thumbnails and viewer previews where Windows has the codec.
      macOS QuickLook in M5.
- [x] Live Photos: same-name still + clip (≤ 4.5 s) in one folder become one item (schema v5
      `paired_with`); LIVE badge, plays on hover in the grid, ◉ Live / L in the viewer;
      clip is released as a normal video if its photo disappears
- [x] Hide/unhide adjusts only the affected day counts (verified equal to a full rebuild)

## M3 checklist

- [x] Date fixing: set the earliest item to a date (rest keep their spacing) or shift by
      days/hours/minutes; restore original; corrections survive rescans and file edits (schema v6)
- [x] Select all in the current view + filter (Ctrl+A), e.g. filter one camera → shift its clock
- [x] On This Day: same calendar day in earlier years, favorites/ratings first, dismissible per day
- [x] Events: group photos by time gaps (adaptive per person), rename/merge/split, event view
      (gaps judged against the ~10 gaps on each side; 1M assets: first detection 12 s, re-run 2.7 s,
      list 30 ms; edited events are never redrawn, removed ones never come back)
- [x] Places: offline reverse geocoding against GeoNames `cities15000` (31.8k towns, districts
      and historical places dropped; 945 KiB embedded, ~565 KiB compressed). Nearest town where
      big cities reach further (5 m × √population, so suburbs don't take the city's photos),
      none past 60 km. Schema v8 `places` table (only towns in use) + `assets.place_id`
      (0 = nothing nearby). Place in the info panel, place and country filters, events named
      after their most common place until renamed. 1M assets (752k with GPS): first lookup
      22 s in 20k-row batches (lookups ~2.5 µs, the row updates dominate), one-place filter
      146 ms. Also fixed: merge/split/create now pull in the loose items inside the new span
      right away, so the next detection doesn't change the edited event
- [x] Trips: runs of days over 100 km from home (home = town with the most photo days that
      year; thin years borrow the nearest year's, so moves work). Up to 2 days without located
      photos inside a trip, a day at home ends it; 2–90 days, 10+ located photos away. A trip is
      a day range (schema v9 `trips` + `trip_places`), shown with every photo of those days.
      Named after its towns ("Islamabad & Gilgit") or countries; rename/remove like events.
      Trips page, trip view, trip link in the info panel. 1M assets: detection 236 ms (reads
      only per-day place counts from the covering index), list 9 ms, view 28 ms
- [x] Browse by place: Places page (sidebar) lists countries by photo count, each with town
      cards (latest photo as cover); a town or country opens the timeline filtered to it.
      `places::browse`; counts match the timeline (hidden photos left out). ~0.9 s at 1M photos
      (one grouped query when the page opens)

## M4 checklist

- [x] Safe file operations (`fileops`, schema v10 `file_op_items`): removing moves a file into
      `.photomanager-trash/<op>/` at the top of its library folder (a same-drive rename, instant,
      undoable any time; never scanned). The plan and each file's `files` row are journaled
      before anything moves; files changed since the last scan are skipped; `recover` settles a
      crashed operation at startup. Emptying sends to the OS Recycle Bin only on drives Windows
      calls fixed (on network shares and USB sticks the shell deletes for good, silently); other
      drives are left with a note. Nothing in the app deletes a file outright
- [x] Exact duplicates: assets with 2+ present files. Duplicates page with space used, which copy
      to keep (suggested: reachable, fewest "copy/backup/download/(1)" hints, oldest, shortest
      path; changeable), remove per photo or all at once (never the last copy), Recently removed
      with Undo / Send to Recycle Bin. Fixed in the scanner: identical files in the same batch of
      256 were never joined (each only checked rows already committed)
- [x] Near-duplicates and bursts (`similar`, schema v11): 64-bit DCT pHash + sharpness
      (Laplacian variance) from the grid thumbnail, dark borders trimmed (letterboxed EXIF
      thumbnails), near-blank images skipped. Similar shots: next 30 photos within 3 min,
      ≤ 10 bits. Near copies at any time apart: ≤ 3 bits, found via four 16-bit chunk buckets
      (pigeonhole) instead of a BK-tree. Hashing runs in the background after scans and at
      startup, pauses while a scan runs (~4 ms per thumbnail on one core, parallel). Similar
      photos page: tick what to keep, remove the rest through `fileops` (undoable), or
      "Keep all" (group stays hidden). 1M hashed photos: grouping 3.3 s on a reader
- [x] Best-shot ranking (schema v12 `focus`, `exposure`, `quality_v`): photos in similar groups
      are assessed in the background from their originals at 1024 px. Focus = Laplacian
      variance of the 4 sharpest of 8×8 tiles (a sharp subject on a soft background counts);
      exposure = clipped highlights/shadows and overall brightness, signed (dark/bright).
      Choice: favorite, rating, then 75% focus relative to the group's sharpest + 25% exposure;
      copies by pixels then bytes. The page says why ("Sharpest", "Best exposed", "Your
      favorite", …) and flags "Blurry", "Too dark", "Overexposed". Faces and closed eyes need a
      model: AI pack (M7)
- [x] Remove photos anywhere: selection bar **Remove**, Delete key in the grid and viewer; every
      copy goes to the trash folder; events, trips and similar groups refresh after removals and
      undos. **Recently removed** page in the sidebar
- [x] Organizer (`organize`): moves one library folder's files into `2023/2023-06`, `2023/06`
      or `2023/2023-06-14`, names kept (clash → " (2)"), Live Photo clips follow their photo,
      files dated only by their modified time left alone unless asked. Preview (counts + first
      100 moves) before anything moves; runs through `fileops` as a journaled `move` operation
      (the `files` row follows the file, emptied folders removed); Undo moves everything back
- [x] Faster assessment: Windows' decoder (WIC) at a power-of-two reduced size (a 12 MP JPEG at
      1/4, 1008 px), then focus/exposure on that without enlarging: ~120 ms per photo instead of
      ~200–250 ms (release, one photo; photos run in parallel). Falls back to a full decode
      elsewhere. Pure-Rust scaled decoding (`jpeg-decoder`) was tried and isn't faster than
      zune's full decode: entropy decoding dominates. Assessment v2 re-scores every grouped photo
      once, in the background

## Tools checklist

Ground rule: tools write new files; originals are only replaced through `fileops` (undoable).

- [x] Export (`export`): selection → **Export** → format (same as original / JPEG / WebP / PNG;
      HEIC and other non-web formats become JPEG), quality, size (full / 4096 / 2048 / 1080 long
      edge, Lanczos3 via `fast_image_resize`), remove location, folder. EXIF rebuilt for the copy
      (date, camera, GPS kept; orientation applied to pixels; MakerNote and thumbnail dropped),
      ICC profile kept (`img-parts`). Never overwrites (" (2)"). Background job, up to 4 threads,
      progress + Stop in the toolbar. HEIC and other OS formats decode at full size through WIC
      (Windows); macOS/Linux decoders come with M5. WebP via libwebp (`webp` crate)
- [ ] Better JPEG compression (mozjpeg) and AVIF output
- [ ] "Shrink originals": replace originals with compressed versions through `fileops`
- [x] Watermark (text / logo, position incl. tiled, size, opacity) as an export step, with a live preview on the first selected photo. Text uses a system font (Segoe UI on Windows; Arial/Helvetica and DejaVu paths listed for M5).
- [x] Rotate without touching files: `user_meta.turns` (schema v13) on top of EXIF orientation,
      applied in thumbnails (cached ones turned in place), viewer (turned photos decoded at full
      size), hashing and export. Image URLs carry `?t=<turns>` so cached images never show the
      old way round. Selection bar ⟲ ⟳, viewer buttons and R / Shift+R. Not written into the file
      (other apps still see the original orientation); "Save rotation to file" could come later
- [x] Non-destructive editor: crop (free / original / square / 4:3 / 3:2 / 16:9), straighten,
      exposure, contrast, highlights, shadows, warmth, tint, vibrance, saturation, sharpen,
      auto-enhance, six looks, hold-to-compare, Revert to original. The recipe is JSON in
      `user_meta.edit` (schema v14) with `edit_v` counting saves for image URLs; it is applied
      after turns in thumbnails, viewer (full size), export and the watermark preview. Turning a
      cropped photo turns its crop too. Rendering is CPU (rayon): ~130 ms per 1600 px preview,
      ~0.5 s for a 12 MP photo with every adjustment (release, 8 threads)
- [x] Faster JPEG everywhere (`jpeg.rs`, `jpeg-encoder`): thumbnails, viewer and previews use
      `Show` (4:2:0 below q90, ~3× faster); exports use `Keep` (4:4:4, optimized tables:
      ~37% smaller at the same quality, same speed). 12 MP at q85: `image` 441 ms / 1545 KB,
      Keep 400 ms / 972 KB, Show 145 ms / 1405 KB. jpeg-encoder 0.7.1 corrupts files when
      4:2:0 is combined with optimized tables, so that pairing is never used and a test
      round-trips every mode
- [x] Rename from a pattern (`rename.rs`): presets and parts {date} {time} {year} {month}
      {day} {place} {event} {trip} {camera} {counter} {original}; preview first, then
      journaled through `fileops` (`relocate_with`, Undo in Organize files). RAW+JPEG pairs,
      Live Photo clips and .xmp/.aae/.thm sidecars (journaled companions with no library
      row) are renamed together; taken names get " (2)"; moves are ordered so a file can
      take a name another one is leaving (re-numbering works). Names held by missing files
      count as taken
- [ ] Copy an edit and paste it onto other photos; edit several at once
- [ ] WebGL preview if CPU previews feel slow on low-end laptops

## Order from here (the user's choice, 2026-10-24)

1. **F: the six non-AI features**, in the order below (smallest first).
2. Beta prep and **M6** (subscription), as planned.
3. **M7: the three AI features, last.** Each is the user's choice: off by default, and its
   model downloads only when the user turns that feature on.

## F checklist (non-AI features)

Each feature is its own branch, merged after the user's go-ahead. Everything runs offline
and needs no downloads.

1. - [x] **Copy and paste edits** (small, ~2–3 days)
   - Editor and viewer: **Copy edit**. Selection bar: **Paste edit** with a choice of
     everything, or light and colour only (skip crop and straighten, which rarely suit
     another photo).
   - Applies the copied recipe to each photo through `edit::save` in one batch; thumbnails
     rebuild; each photo can still be reverted on its own.
2. - [ ] **XMP sidecars** (small, ~3–4 days)
   - Read on scan: ratings, keywords (as tags), title/description and labels from existing
     `.xmp` files (Lightroom, darktable, digiKam), so a library brought in from them keeps
     its work. Embedded XMP in JPEGs too.
   - Write (a setting, off by default, since it adds files to the user's folders): ratings,
     tags, favorite (as a 5-star or a label, to choose) and description to `<name>.xmp`.
     Merges with what's there; never touches the photo file itself.
   - Rename and the organizer already move sidecars along.
3. - [ ] **Map view** (medium, ~1–1.5 weeks)
   - Sidebar **Map**: photos as clusters by place (counts shrink into pins as you zoom),
     click a cluster to open the timeline filtered there. Uses `assets.lat/lon` and the
     places we already have; fast at 1M photos via a grid per zoom level.
   - Map pictures: **a built-in offline world map** down to city level (~20–40 MB bundled,
     works offline, fits the privacy story). Chosen by the user on 2026-10-24; no online
     tile provider for now.
   - Also: **set a location by hand** for photos without GPS (pick a place, or drop on map).
4. - [ ] **Import from Google Takeout / iCloud** (medium, ~1 week)
   - Google Takeout: reads the `.json` next to each photo (`photoTakenTime`, `geoData`,
     description, favorited), matching Google's shortened names and `-edited` copies; the
     capture date and place come from there when the photo's own EXIF lost them (common
     with Google's downloads). Albums become albums.
   - iCloud (privacy.apple.com export): reads its `Photo Details.csv` files for dates,
     favorites and hidden, and its album lists.
   - Shown as an import report (how many dates/places fixed); undoable like date fixes.
5. - [ ] **Collage and print layouts** (small–medium, ~1 week)
   - Select 2–9 photos → **Collage**: grid and mosaic templates, spacing, corner rounding,
     background colour, drag to swap, click to pan inside a cell.
   - Output sizes: square / story for social, and print sizes (4×6, 5×7, 8×10 in, A4) at
     300 dpi. Saved through the export pipeline (`jpeg::Purpose::Keep`, metadata from the
     first photo). Printing itself is from any app (or the OS print dialog later).
6. - [ ] **Memory slideshow and video** (medium, ~1.5–2 weeks)
   - From an event, a trip, On This Day or any selection: full-screen slideshow with
     gentle pan-and-zoom, the best shots first (best-shot ranking skips near-duplicates),
     optional music from the user's own file.
   - **Save as video**: H.264 MP4 through Windows' own encoder (Media Foundation; no
     ffmpeg needed), 1080p, with the music mixed in. A background job with progress and
     Stop, like export.

## M7 checklist (AI pack, opt-in, built last)

Off by default. **Settings → AI features** lists each feature with what it does, its
download size and an **Install** button; nothing is downloaded before that. Models run on
the user's PC only (no photos leave it), can be deleted any time, and are checked against a
SHA-256 before first use. ONNX Runtime via `ort` with `load-dynamic`, so the app itself
stays small and the runtime comes with the first model (DirectML on Windows for GPUs).
Indexing runs in the background, pauses during scans and on battery.

1. - [ ] **Search by description** (~150–350 MB model): "beach sunset", "birthday cake" find
   photos without tags. CLIP-style image + text embeddings; ~20–50 ms per photo to index on
   CPU, instant search. Then automatic smart albums (Food, Pets, Documents…) from the same
   model.
2. - [ ] **People (face grouping)** (~30 MB models): finds faces, groups the same person,
   the user names a group once. Opt-in again on its own (face data is sensitive), stored
   only in the library, removable with one click. First pass over a big library takes
   hours, then new photos only.
3. - [ ] **Upscale** (~20–70 MB model): enlarge small or old photos 2–4×, as an Export size
   option. Seconds to a minute per photo on CPU, so it's a deliberate action.

## M5 checklist

Order chosen by the user: Windows platform work first, then CI builds for all three OSes,
then macOS/Linux; Windows code signing later; macOS signing + notarization at the very end,
after testing; installer and auto-update just before the beta.

Part 1, Windows:
- [x] Long (> 260 characters) and Unicode paths: Rust's file APIs already handle them (test:
      scan, thumbnails, export, remove, undo under a 300+ character path with accents, CJK and
      emoji). WIC gets the `\\?\` form; the shell (thumbnails, video details) gets the 8.3 short
      form, falling back to WIC for still images when a drive has no short names. Removed files
      sit flat in the trash folder (`<n> <name>`) so deep folders don't stop the Recycle Bin.
      Left: a library folder that is itself near 260 characters can't use the Recycle Bin
- [x] RAW (CR2, CR3, NEF, ARW, DNG, ORF, PEF, SRW, RW2, RAF) through the camera's embedded
      JPEG preview (`raw.rs`): grid thumbnails (made from the big preview, not the 160 px EXIF
      one), viewer, editor, export (from the preview) and similar photos. CR3/RAF metadata from
      their CMT boxes / preview EXIF. Tested on synthetic files only; needs real RAW samples
- [x] Video details without ffmpeg: AVI, MKV, MTS, WebM (and MP4 our parser can't read) get
      duration, size and recording date from Windows' property handlers. Thumbnails already
      come from the shell. Not playable in the viewer (webview formats only): "Open in default
      app" stays the way to play them

- [x] Part 2, CI (Windows only for now, by the user's choice): `ci.yml` checks every branch
      (fmt, clippy, tests, build) with Rust pinned in `rust-toolchain.toml`; `build-apps.yml`
      (run it from the Actions tab, or push a `v*` tag) makes unsigned .msi/.exe installers.
      macOS and Linux were checked once (2026-10-24): everything compiled and every test passed
      except one test that assumed Windows' file order (fixed). Their matrix entries are
      commented out in both workflows, ready for part 3
Part 3, macOS (QuickLook/ImageIO decoders, disk UUIDs, permissions) and Linux (libheif,
ffmpeg, FS UUIDs, non-UTF-8 names, AppImage/deb).
Part 4, at the end: installer, Windows signing, macOS signing + notarization, auto-update,
crash reports.

## M0 results

### `scale_bench`, 1M synthetic assets (release, dev workstation, 2026-10-23)

| Measure | Result |
|---|---|
| Insert 1M assets + rebuild day buckets | 21.5 s (~47k rows/s, single writer) |
| Layout query (6,362 days) | 1.2 ms |
| Scrubber jump (fetch ids for 10 days, ~1.5k ids) | 1.3 ms avg |
| `library.sqlite` size | 92 MiB (~96 bytes/asset) |

Takeaways: query latency is not a concern at 1M assets. Insert speed is acceptable, since a real
scan is limited by disk reads, not SQLite. Still to measure: in-app scroll fps, thumbnail
latency over `thumb://`, and the same runs on a low-end laptop.

### `thumb_bench`, 127 real phone JPEGs (release, warm cache)

| Path | Throughput | Avg thumb |
|---|---|---|
| Embedded EXIF thumbnail | ~10,700 files/s (105/127 had one) | 27 KB |
| Full decode + resize + encode | ~26 files/s | 13 KB |

The embedded path is ~400× faster, which is why the scanner stores it during the scan
and full decodes only happen on demand, for visible items, in a bounded pool.

## M1 results

### `scan_bench` on real folders (release)

| Folder | Files | First scan | Unchanged rescan |
|---|---|---|---|
| Pictures (286 PNG screenshots, 1 MP4) | 287 | 0.4 s | ~10,600 files/s |
| Family phone photos (JPEG) | 137 | 0.5 s | ~11,300 files/s |

Capture-time sources on the phone-photo folder: EXIF 7, filename 120, mtime 10. Most files
were WhatsApp-style exports with EXIF stripped, so filename parsing (incl. epoch-ms names
like `1759554341584.jpg`) is not optional. One Xiaomi/MediaTek camera wrote its date and
make/model into the thumbnail IFD; the extractor now falls back to it.

Still to measure: first scan of a 100k+ library from an external HDD (target: disk-bound),
and the kill-mid-scan / unplug-drive exit tests in the real app.
