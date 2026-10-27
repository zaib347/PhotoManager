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

# Rebuild the bundled world map (crates/photo-core/data/map.bin) from Natural Earth GeoJSON
cargo run --release -p photo-core --example build_map -- ./natural-earth
```

## Data credits

Place names come from [GeoNames](https://www.geonames.org/) (`cities15000`, admin1 codes
and country info), licensed under [CC BY 4.0](https://creativecommons.org/licenses/by/4.0/).
They are repacked into a compact binary file with no other changes.

The world map is made from [Natural Earth](https://www.naturalearthdata.com/) vector data
(v5.1.2: land, lakes, urban areas, borders, rivers and roads), which is in the public
domain. It is clipped into tiles and packed into a compact binary file.

The optional AI features download their models only when the user installs them, and run
them on the user's computer:

- **Search by description:** OpenAI's [CLIP](https://github.com/openai/CLIP) ViT-B/32 (MIT
  license), in the 8-bit ONNX export by [Xenova](https://huggingface.co/Xenova/clip-vit-base-patch32),
  pinned to one revision.
- **People:** [YuNet](https://github.com/ShiqiYu/libfacedetection) face detection (MIT
  license, Shiqi Yu) and [SFace](https://arxiv.org/abs/2205.12010) face recognition (Apache
  2.0, Yaoyao Zhong and Weihong Deng), from the
  [OpenCV model zoo](https://huggingface.co/opencv), pinned to one revision each. Faces and
  people stay in the library on the user's computer.
- **People without a visible face:** [YOLOX](https://github.com/Megvii-BaseDetection/YOLOX)-S
  object detection (Apache 2.0, Megvii), from the
  [OpenCV model zoo](https://huggingface.co/opencv/object_detection_yolox), pinned to one
  revision. Only its "person" class is used, to keep photos with someone in them out of
  No people.
- **Upscale:** [Real-ESRGAN](https://github.com/xinntao/Real-ESRGAN) General x4v3 (BSD
  3-Clause license, Xintao Wang et al.), in the ONNX export by
  [Qualcomm AI Hub](https://huggingface.co/qualcomm/Real-ESRGAN-General-x4v3) (release 0.65.0).
- **Text in photos:** the text recognition built into Windows (`Windows.Media.Ocr`);
  nothing is downloaded.
- **Runtime:** Microsoft's [ONNX Runtime](https://github.com/microsoft/onnxruntime) 1.24.4
  (MIT license), from its official NuGet package.

Every file is checked against a SHA-256 fixed in `crates/photo-ai/src/models.rs`.

## Layout

- `crates/photo-core/` — storage, schema, timeline queries, thumbnails. No Tauri dependency.
- `crates/photo-ai/` — the optional AI features (model downloads, ONNX Runtime, search by
  description, people, upscaling, text in photos). Nothing in it runs until the user
  installs a feature.
- `src-tauri/` — desktop shell: commands and the `thumb://`, `media://` and `face://`
  protocols.
- `src/` — React UI (`timeline/` holds the virtual grid and date scrubber).
