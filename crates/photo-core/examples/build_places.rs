//! Packs GeoNames towns and landmarks into `crates/photo-core/data/places.bin`.
//!
//! Download into one folder from https://download.geonames.org/export/dump/:
//! `allCountries.zip`, `alternateNamesV2.zip` (both read zipped), `admin1CodesASCII.txt`
//! and `countryInfo.txt`, then
//! cargo run -p photo-core --example build_places -- <folder>
//!
//! Kept: towns of 15,000+ people and capitals (as GeoNames' `cities15000`), smaller
//! towns with an English Wikipedia article at least [`REMOTE_KM`] from those, and
//! landmarks of the kinds in [`LANDMARKS`] with one, named by the article's title.
//!
//! GeoNames data is CC BY 4.0 (https://www.geonames.org/).

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use photo_core::places::{self, City};

/// Parts of a city (boroughs, districts) and places that no longer exist.
const SKIPPED_TOWNS: &[&str] = &["PPLX", "PPLH", "PPLQ", "PPLW", "PPLCH"];
/// Landmarks (GeoNames `class.code`) that name photos taken out of town.
const LANDMARKS: &[&str] = &[
    // Water
    "H.LK", "H.LKS", "H.RSV", "H.LGN", "H.GLCR", "H.WTRF", // Parks and reserves
    "L.PRK", "L.RESN", "L.RESW", // Mountains, passes, valleys, islands, beaches
    "T.MT", "T.PK", "T.VLC", "T.PASS", "T.VAL", "T.GRGE", "T.CNYN", "T.ISL", "T.BCH",
    // Forts, castles, ruins, historic sites, monuments, dams
    "S.HSTS", "S.RUIN", "S.ANS", "S.CSTL", "S.FT", "S.MNMT", "S.PAL", "S.DAM",
];
const BIG_TOWN: u32 = 15_000;
/// Smaller towns are kept only this far from every town of `BIG_TOWN`+: they name
/// photos in hills and countryside (Naran) without making the data four times bigger
/// with every village around the cities, which have their names already.
const REMOTE_KM: f64 = 25.0;

/// Whether some town in `grid` (1° cells) is within `REMOTE_KM` of (`lat`, `lon`).
fn near_big_town(grid: &HashMap<(i32, i32), Vec<(f64, f64)>>, lat: f64, lon: f64) -> bool {
    let (row, col) = (lat.floor() as i32, lon.floor() as i32);
    let cos_lat = lat.to_radians().cos();
    (-1..=1).any(|dr| {
        (-1..=1).any(|dc| {
            grid.get(&(row + dr, col + dc)).is_some_and(|towns| {
                towns.iter().any(|&(a, b)| {
                    let (dy, dx) = ((a - lat) * 111.2, (b - lon) * 111.2 * cos_lat);
                    (dx * dx + dy * dy).sqrt() < REMOTE_KM
                })
            })
        })
    })
}

fn small_file(path: &Path) -> Vec<Vec<String>> {
    std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("{}: {e}", path.display()))
        .lines()
        .filter(|l| !l.starts_with('#') && !l.is_empty())
        .map(|l| l.split('\t').map(str::to_owned).collect())
        .collect()
}

/// Calls `f` with the tab-separated fields of each line of `name` inside `zip`.
fn each_zipped_line(zip: &Path, name: &str, mut f: impl FnMut(&[&str])) {
    let file = File::open(zip).unwrap_or_else(|e| panic!("{}: {e}", zip.display()));
    let mut archive = zip::ZipArchive::new(file).expect("a zip file");
    let entry = archive.by_name(name).expect("the GeoNames file in its zip");
    for line in BufReader::with_capacity(1 << 20, entry).lines() {
        let line = line.expect("readable text");
        let fields: Vec<&str> = line.split('\t').collect();
        f(&fields);
    }
}

/// "http://en.wikipedia.org/wiki/Lake_Saiful_Muluk" → "Lake Saiful Muluk".
fn wikipedia_title(url: &str) -> Option<String> {
    let (_, title) = url.split_once("en.wikipedia.org/wiki/")?;
    let title = title.split(['#', '?']).next()?;
    let mut bytes = Vec::with_capacity(title.len());
    let raw = title.as_bytes();
    let mut i = 0;
    while i < raw.len() {
        match raw[i] {
            b'%' if i + 2 < raw.len() => {
                let hex = std::str::from_utf8(&raw[i + 1..i + 3]).ok()?;
                bytes.push(u8::from_str_radix(hex, 16).ok()?);
                i += 3;
            }
            b'_' => {
                bytes.push(b' ');
                i += 1;
            }
            b => {
                bytes.push(b);
                i += 1;
            }
        }
    }
    let title = String::from_utf8(bytes).ok()?;
    // "Naran (Pakistan)" → "Naran"
    let title = match title.find(" (") {
        Some(at) if title.ends_with(')') => title[..at].to_owned(),
        _ => title,
    };
    let title = title.trim();
    (!title.is_empty() && !title.contains(':')).then(|| title.to_owned())
}

fn main() {
    let dir = PathBuf::from(
        std::env::args()
            .nth(1)
            .expect("usage: build_places <geonames-folder>"),
    );
    let countries: HashMap<String, String> = small_file(&dir.join("countryInfo.txt"))
        .into_iter()
        .map(|f| (f[0].clone(), f[4].clone()))
        .collect();
    let regions: HashMap<String, String> = small_file(&dir.join("admin1CodesASCII.txt"))
        .into_iter()
        .map(|f| (f[0].clone(), f[1].clone()))
        .collect();

    // GeoNames id → English Wikipedia title.
    let mut wiki: HashMap<u32, String> = HashMap::new();
    each_zipped_line(
        &dir.join("alternateNamesV2.zip"),
        "alternateNamesV2.txt",
        |f| {
            if f.len() > 3
                && f[2] == "link"
                && let Ok(id) = f[1].parse()
                && let Some(title) = wikipedia_title(f[3])
            {
                wiki.entry(id).or_insert(title);
            }
        },
    );
    println!("{} places with an English Wikipedia article", wiki.len());

    let mut cities: Vec<City> = Vec::new();
    let (mut towns, mut small_towns, mut landmarks) = (0, 0, 0);
    // Towns of `BIG_TOWN`+ and capitals.
    let mut big_ids: HashSet<u32> = HashSet::new();
    each_zipped_line(&dir.join("allCountries.zip"), "allCountries.txt", |f| {
        if f.len() < 15 {
            return;
        }
        let Ok(id) = f[0].parse::<u32>() else { return };
        let (class, code) = (f[6], f[7]);
        let population: u32 = f[14].parse().unwrap_or(0);
        let title = wiki.get(&id);
        let (name, landmark) = if class == "P" {
            if SKIPPED_TOWNS.contains(&code) {
                return;
            }
            let big = population >= BIG_TOWN || code == "PPLC";
            if !big && title.is_none() {
                return;
            }
            if big {
                towns += 1;
                big_ids.insert(id);
            } else {
                small_towns += 1;
            }
            (f[1].to_owned(), false)
        } else {
            let Some(title) = title else { return };
            if !LANDMARKS.contains(&format!("{class}.{code}").as_str()) {
                return;
            }
            landmarks += 1;
            (title.clone(), true)
        };
        let country_code = f[8].to_owned();
        let Some(country) = countries.get(&country_code) else {
            return;
        };
        let (Ok(lat), Ok(lon)) = (f[4].parse(), f[5].parse()) else {
            return;
        };
        cities.push(City {
            id,
            lat,
            lon,
            name,
            region: regions.get(&format!("{country_code}.{}", f[10])).cloned(),
            country: country.clone(),
            country_code,
            population,
            landmark,
        });
    });
    let mut grid: HashMap<(i32, i32), Vec<(f64, f64)>> = HashMap::new();
    for c in cities.iter().filter(|c| big_ids.contains(&c.id)) {
        let key = (c.lat.floor() as i32, c.lon.floor() as i32);
        grid.entry(key).or_default().push((c.lat, c.lon));
    }
    cities.retain(|c| c.landmark || big_ids.contains(&c.id) || !near_big_town(&grid, c.lat, c.lon));
    let remote = cities
        .iter()
        .filter(|c| !c.landmark && !big_ids.contains(&c.id))
        .count();
    cities.sort_unstable_by_key(|c| c.id);
    println!(
        "{towns} towns of {BIG_TOWN}+, {small_towns} smaller towns ({remote} kept, {REMOTE_KM} km \
         from bigger ones), {landmarks} landmarks"
    );

    let packed = places::pack(&cities);
    assert_eq!(places::unpack(&packed).unwrap().len(), cities.len());
    let out = Path::new(env!("CARGO_MANIFEST_DIR")).join("data/places.bin");
    std::fs::create_dir_all(out.parent().unwrap()).unwrap();
    std::fs::write(&out, &packed).unwrap();
    println!(
        "{} places, {:.0} KiB -> {}",
        cities.len(),
        packed.len() as f64 / 1024.0,
        out.display()
    );
}

#[cfg(test)]
mod tests {
    use super::wikipedia_title;

    #[test]
    fn titles() {
        assert_eq!(
            wikipedia_title("http://en.wikipedia.org/wiki/Lake_Saiful_Muluk").as_deref(),
            Some("Lake Saiful Muluk")
        );
        assert_eq!(
            wikipedia_title("https://en.wikipedia.org/wiki/Naran_(Pakistan)").as_deref(),
            Some("Naran")
        );
        assert_eq!(
            wikipedia_title("https://en.wikipedia.org/wiki/K%C3%B6nigssee").as_deref(),
            Some("Königssee")
        );
        assert_eq!(wikipedia_title("https://de.wikipedia.org/wiki/Foo"), None);
    }
}
