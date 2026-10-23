//! Packs GeoNames city data into `crates/photo-core/data/places.bin`.
//!
//! Download into one folder from https://download.geonames.org/export/dump/:
//! `cities15000.zip` (unzipped), `admin1CodesASCII.txt` and `countryInfo.txt`, then
//! cargo run -p photo-core --example build_places -- <folder>
//!
//! GeoNames data is CC BY 4.0 (https://www.geonames.org/).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use photo_core::places::{self, City};

/// Parts of a city (boroughs, districts) and places that no longer exist.
const SKIPPED_FEATURES: &[&str] = &["PPLX", "PPLH", "PPLQ", "PPLW"];

fn lines(path: &Path) -> Vec<Vec<String>> {
    std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("{}: {e}", path.display()))
        .lines()
        .filter(|l| !l.starts_with('#') && !l.is_empty())
        .map(|l| l.split('\t').map(str::to_owned).collect())
        .collect()
}

fn main() {
    let dir = PathBuf::from(
        std::env::args()
            .nth(1)
            .expect("usage: build_places <geonames-folder>"),
    );
    let countries: HashMap<String, String> = lines(&dir.join("countryInfo.txt"))
        .into_iter()
        .map(|f| (f[0].clone(), f[4].clone()))
        .collect();
    let regions: HashMap<String, String> = lines(&dir.join("admin1CodesASCII.txt"))
        .into_iter()
        .map(|f| (f[0].clone(), f[1].clone()))
        .collect();

    let mut cities: Vec<City> = lines(&dir.join("cities15000.txt"))
        .into_iter()
        .filter(|f| !SKIPPED_FEATURES.contains(&f[7].as_str()))
        .filter_map(|f| {
            let code = f[8].clone();
            Some(City {
                id: f[0].parse().ok()?,
                lat: f[4].parse().ok()?,
                lon: f[5].parse().ok()?,
                name: f[1].clone(),
                region: regions.get(&format!("{code}.{}", f[10])).cloned(),
                country: countries.get(&code)?.clone(),
                country_code: code,
                population: f[14].parse().unwrap_or(0),
            })
        })
        .collect();
    cities.sort_unstable_by_key(|c| c.id);

    let packed = places::pack(&cities);
    assert_eq!(places::unpack(&packed).unwrap().len(), cities.len());
    let out = Path::new(env!("CARGO_MANIFEST_DIR")).join("data/places.bin");
    std::fs::create_dir_all(out.parent().unwrap()).unwrap();
    std::fs::write(&out, &packed).unwrap();
    println!(
        "{} cities, {:.0} KiB -> {}",
        cities.len(),
        packed.len() as f64 / 1024.0,
        out.display()
    );
}
