//! Packs Natural Earth vector data into `crates/photo-core/data/map.bin`, the offline
//! world map behind the Map view.
//!
//! Download these GeoJSON files into one folder from
//! https://github.com/nvkelso/natural-earth-vector/tree/master/geojson (v5.1.2):
//! `ne_110m_land`, `ne_50m_land`, `ne_10m_land`, `ne_50m_lakes`, `ne_10m_lakes`,
//! `ne_10m_urban_areas`, `ne_{110m,50m,10m}_admin_0_boundary_lines_land`,
//! `ne_{50m,10m}_admin_1_states_provinces_lines`,
//! `ne_{50m,10m}_rivers_lake_centerlines`, `ne_10m_roads` and
//! `ne_50m_admin_0_countries` (for country labels), then
//! cargo run --release -p photo-core --example build_map -- <folder>
//!
//! Natural Earth is in the public domain (https://www.naturalearthdata.com/).
//!
//! Format (little-endian): `"PMMP" version:u32 layers:u32`, then per layer
//! `name_len:u8 name kind:u8 (0 area, 1 line) tile_deg:u16 tiles:u32`, and per tile
//! `x:u16 y:u16 bytes:u32 data`. A tile covers longitudes `-180 + x*deg ..` and
//! latitudes `-90 + y*deg ..`. Its data is varints: the feature count, then per
//! feature its ring (or line) count, and per ring its point count and the points as
//! zigzag deltas of (lon, lat) in 1e-4 degrees, starting from (0, 0). Areas are
//! clipped to their tile and filled even-odd per feature; lines are split at tile
//! edges. After the layers: `labels:u32`, per label `name_len:u16 name lon:i32 lat:i32
//! min_zoom:u8` (degrees × 1e4).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::Value;

/// Web Mercator stops here; nothing beyond is ever shown.
const MAX_LAT: f64 = 85.06;
const SCALE: f64 = 1e4;

type Ring = Vec<(f64, f64)>;

/// (name, file, area?, tile size in degrees)
const LAYERS: &[(&str, &str, bool, u16)] = &[
    ("land0", "ne_110m_land", true, 360),
    ("land1", "ne_50m_land", true, 30),
    ("land2", "ne_10m_land", true, 5),
    ("lakes1", "ne_50m_lakes", true, 30),
    ("lakes2", "ne_10m_lakes", true, 5),
    ("urban2", "ne_10m_urban_areas", true, 5),
    ("border0", "ne_110m_admin_0_boundary_lines_land", false, 360),
    ("border1", "ne_50m_admin_0_boundary_lines_land", false, 30),
    ("border2", "ne_10m_admin_0_boundary_lines_land", false, 5),
    ("state1", "ne_50m_admin_1_states_provinces_lines", false, 30),
    ("state2", "ne_10m_admin_1_states_provinces_lines", false, 5),
    ("river1", "ne_50m_rivers_lake_centerlines", false, 30),
    ("river2", "ne_10m_rivers_lake_centerlines", false, 5),
    ("road2", "ne_10m_roads", false, 5),
];

fn read(dir: &Path, name: &str) -> Value {
    let path = dir.join(format!("{name}.geojson"));
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn ring(v: &Value) -> Ring {
    v.as_array()
        .into_iter()
        .flatten()
        .filter_map(|p| Some((p.get(0)?.as_f64()?, p.get(1)?.as_f64()?)))
        .collect()
}

/// Each feature's rings (areas) or lines.
fn features(geojson: &Value, area: bool) -> Vec<Vec<Ring>> {
    let mut out = Vec::new();
    for f in geojson["features"].as_array().into_iter().flatten() {
        let g = &f["geometry"];
        let coords = &g["coordinates"];
        match (g["type"].as_str(), area) {
            (Some("Polygon"), true) | (Some("MultiLineString"), false) => {
                out.push(coords.as_array().into_iter().flatten().map(ring).collect())
            }
            (Some("MultiPolygon"), true) => {
                for poly in coords.as_array().into_iter().flatten() {
                    out.push(poly.as_array().into_iter().flatten().map(ring).collect());
                }
            }
            (Some("LineString"), false) => out.push(vec![ring(coords)]),
            _ => {}
        }
    }
    out
}

/// Sutherland–Hodgman: `ring` clipped to the rectangle. Concave rings may gain
/// zero-width bridges along the edges, which fill nothing.
fn clip_ring(ring: &Ring, x0: f64, y0: f64, x1: f64, y1: f64) -> Ring {
    // Each edge as (vertical?, position, keep the side above it?).
    let edges = [
        (true, x0, true),
        (true, x1, false),
        (false, y0, true),
        (false, y1, false),
    ];
    let mut out = ring.clone();
    for (vertical, at, above) in edges {
        if out.is_empty() {
            break;
        }
        let inside = |p: (f64, f64)| {
            let v = if vertical { p.0 } else { p.1 };
            if above { v >= at } else { v <= at }
        };
        let cross = |a: (f64, f64), b: (f64, f64)| {
            if vertical {
                (at, a.1 + (b.1 - a.1) * (at - a.0) / (b.0 - a.0))
            } else {
                (a.0 + (b.0 - a.0) * (at - a.1) / (b.1 - a.1), at)
            }
        };
        let input = std::mem::take(&mut out);
        let mut prev = *input.last().expect("not empty");
        for &p in &input {
            match (inside(p), inside(prev)) {
                (true, true) => out.push(p),
                (true, false) => {
                    out.push(cross(prev, p));
                    out.push(p);
                }
                (false, true) => out.push(cross(prev, p)),
                (false, false) => {}
            }
            prev = p;
        }
    }
    out
}

/// A line cut at the rectangle's edges into the pieces inside it.
fn clip_line(line: &Ring, x0: f64, y0: f64, x1: f64, y1: f64) -> Vec<Ring> {
    let inside = |p: (f64, f64)| p.0 >= x0 && p.0 <= x1 && p.1 >= y0 && p.1 <= y1;
    let mut pieces = Vec::new();
    let mut current: Ring = Vec::new();
    for w in 0..line.len() {
        let p = line[w];
        if w == 0 {
            if inside(p) {
                current.push(p);
            }
            continue;
        }
        let a = line[w - 1];
        // Liang–Barsky on segment a→p.
        let (dx, dy) = (p.0 - a.0, p.1 - a.1);
        let (mut t0, mut t1) = (0.0f64, 1.0f64);
        let mut visible = true;
        for (q, r) in [
            (-dx, a.0 - x0),
            (dx, x1 - a.0),
            (-dy, a.1 - y0),
            (dy, y1 - a.1),
        ] {
            if q == 0.0 {
                if r < 0.0 {
                    visible = false;
                }
            } else {
                let t = r / q;
                if q < 0.0 {
                    t0 = t0.max(t);
                } else {
                    t1 = t1.min(t);
                }
            }
        }
        if !visible || t0 > t1 {
            if current.len() > 1 {
                pieces.push(std::mem::take(&mut current));
            }
            current.clear();
            continue;
        }
        let start = (a.0 + dx * t0, a.1 + dy * t0);
        let end = (a.0 + dx * t1, a.1 + dy * t1);
        if current.is_empty() {
            current.push(start);
        }
        current.push(end);
        if t1 < 1.0 {
            pieces.push(std::mem::take(&mut current));
        }
    }
    if current.len() > 1 {
        pieces.push(current);
    }
    pieces
}

fn bbox(rings: &[Ring]) -> (f64, f64, f64, f64) {
    let mut b = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for p in rings.iter().flatten() {
        b = (b.0.min(p.0), b.1.min(p.1), b.2.max(p.0), b.3.max(p.1));
    }
    b
}

fn varint(out: &mut Vec<u8>, mut v: u64) {
    while v >= 0x80 {
        out.push((v as u8) | 0x80);
        v >>= 7;
    }
    out.push(v as u8);
}

fn zigzag(v: i64) -> u64 {
    ((v << 1) ^ (v >> 63)) as u64
}

/// Quantized points with repeats dropped.
fn quantize(ring: &Ring) -> Vec<(i64, i64)> {
    let mut out: Vec<(i64, i64)> = Vec::with_capacity(ring.len());
    for &(x, y) in ring {
        let q = ((x * SCALE).round() as i64, (y * SCALE).round() as i64);
        if out.last() != Some(&q) {
            out.push(q);
        }
    }
    out
}

fn encode_tile(features: &[Vec<Ring>], area: bool) -> Vec<u8> {
    let mut out = Vec::new();
    let kept: Vec<Vec<Vec<(i64, i64)>>> = features
        .iter()
        .map(|rings| {
            rings
                .iter()
                .map(quantize)
                .filter(|r| r.len() >= if area { 3 } else { 2 })
                .collect::<Vec<_>>()
        })
        .filter(|rings| !rings.is_empty())
        .collect();
    varint(&mut out, kept.len() as u64);
    for rings in kept {
        varint(&mut out, rings.len() as u64);
        for r in rings {
            varint(&mut out, r.len() as u64);
            let mut prev = (0i64, 0i64);
            for p in r {
                varint(&mut out, zigzag(p.0 - prev.0));
                varint(&mut out, zigzag(p.1 - prev.1));
                prev = p;
            }
        }
    }
    out
}

fn build_layer(features: Vec<Vec<Ring>>, area: bool, deg: u16) -> BTreeMap<(u16, u16), Vec<u8>> {
    let deg_f = deg as f64;
    let mut tiles: BTreeMap<(u16, u16), Vec<Vec<Ring>>> = BTreeMap::new();
    for rings in features {
        let (bx0, by0, bx1, by1) = bbox(&rings);
        if bx0 > bx1 {
            continue;
        }
        let tx0 = ((bx0 + 180.0) / deg_f).floor().max(0.0) as u16;
        let tx1 = ((bx1 + 180.0) / deg_f)
            .floor()
            .min((360 / deg).max(1) as f64 - 1.0) as u16;
        let ty0 = ((by0.max(-MAX_LAT) + 90.0) / deg_f).floor().max(0.0) as u16;
        let ty1 = ((by1.min(MAX_LAT) + 90.0) / deg_f).floor().max(0.0) as u16;
        for tx in tx0..=tx1.max(tx0) {
            for ty in ty0..=ty1.max(ty0) {
                let x0 = -180.0 + tx as f64 * deg_f;
                let y0 = (-90.0 + ty as f64 * deg_f).max(-MAX_LAT);
                let (x1, y1) = (
                    (x0 + deg_f).min(180.0),
                    (-90.0 + (ty + 1) as f64 * deg_f).min(MAX_LAT),
                );
                let clipped: Vec<Ring> = if area {
                    rings
                        .iter()
                        .map(|r| clip_ring(r, x0, y0, x1, y1))
                        .filter(|r| r.len() >= 3)
                        .collect()
                } else {
                    rings
                        .iter()
                        .flat_map(|r| clip_line(r, x0, y0, x1, y1))
                        .collect()
                };
                if !clipped.is_empty() {
                    tiles.entry((tx, ty)).or_default().push(clipped);
                }
            }
        }
    }
    tiles
        .into_iter()
        .map(|(k, f)| (k, encode_tile(&f, area)))
        .filter(|(_, data)| data != &[0])
        .collect()
}

fn main() {
    let dir = PathBuf::from(
        std::env::args()
            .nth(1)
            .expect("usage: build_map <natural-earth-folder>"),
    );
    let mut out = Vec::new();
    out.extend_from_slice(b"PMMP");
    out.extend_from_slice(&1u32.to_le_bytes());
    out.extend_from_slice(&(LAYERS.len() as u32).to_le_bytes());
    for &(name, file, area, deg) in LAYERS {
        let tiles = build_layer(features(&read(&dir, file), area), area, deg);
        let bytes: usize = tiles.values().map(Vec::len).sum();
        println!(
            "{name}: {} tiles, {:.1} MB",
            tiles.len(),
            bytes as f64 / 1e6
        );
        out.push(name.len() as u8);
        out.extend_from_slice(name.as_bytes());
        out.push(if area { 0 } else { 1 });
        out.extend_from_slice(&deg.to_le_bytes());
        out.extend_from_slice(&(tiles.len() as u32).to_le_bytes());
        for ((x, y), data) in tiles {
            out.extend_from_slice(&x.to_le_bytes());
            out.extend_from_slice(&y.to_le_bytes());
            out.extend_from_slice(&(data.len() as u32).to_le_bytes());
            out.extend_from_slice(&data);
        }
    }

    // Country labels, bigger countries first.
    let countries = read(&dir, "ne_50m_admin_0_countries");
    let mut labels: Vec<(String, f64, f64, u8)> = countries["features"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|f| {
            let p = &f["properties"];
            let name = p["NAME"].as_str()?.to_owned();
            let x = p["LABEL_X"].as_f64()?;
            let y = p["LABEL_Y"].as_f64()?;
            let min = p["MIN_LABEL"].as_f64().unwrap_or(5.0);
            Some((name, x, y, min.round().clamp(0.0, 12.0) as u8))
        })
        .collect();
    labels.sort_by_key(|l| l.3);
    out.extend_from_slice(&(labels.len() as u32).to_le_bytes());
    for (name, x, y, min) in labels {
        out.extend_from_slice(&(name.len() as u16).to_le_bytes());
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(&((x * SCALE).round() as i32).to_le_bytes());
        out.extend_from_slice(&((y * SCALE).round() as i32).to_le_bytes());
        out.push(min);
    }

    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("data/map.bin");
    std::fs::write(&path, &out).unwrap();
    println!("{}: {:.1} MB", path.display(), out.len() as f64 / 1e6);
}
