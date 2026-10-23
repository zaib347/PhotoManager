# PhotoManager

Desktop memory manager for very large photo and video libraries. Rust + Tauri v2 + React.
See [docs/PLAN.md](docs/PLAN.md) for the product plan and milestones.

## Develop

```sh
npm install
npm run tauri dev            # desktop app with hot reload
cargo test -p photo-core     # core tests
```

## Benchmarks and dev tools

```sh
# Database behaviour with N synthetic assets
cargo run --release -p photo-core --example scale_bench -- ./bench.sqlite 1000000

# Thumbnail throughput on a real folder of JPEGs (run each mode twice: cold, then warm cache)
cargo run --release -p photo-core --example thumb_bench -- "D:\Photos" embedded
cargo run --release -p photo-core --example thumb_bench -- "D:\Photos" decode 5000

# Indexer on a real folder (read-only; library written to <scratch-dir>)
cargo run --release -p photo-core --example scan_bench -- "D:\Photos" ./scratch

# What the indexer extracts from one file
cargo run -p photo-core --example exif_dump -- "D:\Photos\IMG_0001.jpg"

# Rebuild the bundled place list (crates/photo-core/data/places.bin) from GeoNames dumps
cargo run -p photo-core --example build_places -- ./geonames
```

## Data credits

Place names come from [GeoNames](https://www.geonames.org/) (`cities15000`, admin1 codes
and country info), licensed under [CC BY 4.0](https://creativecommons.org/licenses/by/4.0/).
They are repacked into a compact binary file with no other changes.

## Layout

- `crates/photo-core/` — storage, schema, timeline queries, thumbnails. No Tauri dependency.
- `src-tauri/` — desktop shell: commands and the `thumb://` protocol.
- `src/` — React UI (`timeline/` holds the virtual grid and date scrubber).
