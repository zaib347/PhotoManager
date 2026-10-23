//! Places: offline reverse geocoding against a bundled list of cities.
//!
//! The list is GeoNames `cities15000` (every town of 15,000+ people, ~32k entries,
//! CC BY 4.0), packed by `examples/build_places.rs` into `data/places.bin`. Each
//! photo with GPS gets the nearest city, where big cities reach further so their
//! suburbs don't take the photos of the city itself.
//!
//! Only places that photos actually use are copied into the `places` table, so
//! filters and lists are plain SQL.

use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

use rusqlite::{Connection, params};
use serde::Serialize;

use crate::db::Db;
use crate::{Error, Result};

const DATA: &[u8] = include_bytes!("../data/places.bin");
const MAGIC: &[u8; 4] = b"PMPL";
const VERSION: u32 = 1;

/// Photos further than this from any town (plus the town's own reach) get no place.
const MAX_KM: f64 = 60.0;
/// A city of population `p` reaches `REACH_KM * sqrt(p)` km past its centre:
/// ~15 km for London, ~1 km for a town of 30,000.
const REACH_KM: f64 = 0.005;
const EARTH_KM: f64 = 6371.0;
/// `assets.place_id` for photos that were looked up but are nowhere near a town.
pub const NOWHERE: i64 = 0;
/// Assets geocoded per write transaction, so the writer is never held for long.
const BATCH: usize = 20_000;

/// One city as stored in the bundled file.
#[derive(Debug, Clone, PartialEq)]
pub struct City {
    /// GeoNames id; stable across data updates, so it's what assets store.
    pub id: u32,
    pub lat: f64,
    pub lon: f64,
    pub name: String,
    /// First-level division (state, province), if GeoNames names one.
    pub region: Option<String>,
    pub country_code: String,
    pub country: String,
    pub population: u32,
}

/// Packs cities into the bundled format:
/// `"PMPL" version:u32 regions:u32 countries:u32 cities:u32`, then the region names,
/// the countries (`code\tname`) and the city names as `len:u16 bytes`, then per city
/// `id:u32 lat:i32 lon:i32 region:u16 country:u16 population_k:u16` (degrees × 1e5;
/// region `u16::MAX` for none). Integers are little-endian.
pub fn pack(cities: &[City]) -> Vec<u8> {
    fn intern<K: Eq + std::hash::Hash + Copy>(
        list: &mut Vec<K>,
        seen: &mut HashMap<K, u16>,
        key: K,
    ) -> u16 {
        *seen.entry(key).or_insert_with(|| {
            list.push(key);
            (list.len() - 1) as u16
        })
    }
    let mut regions: Vec<&str> = Vec::new();
    let mut countries: Vec<(&str, &str)> = Vec::new();
    let (mut region_ids, mut country_ids) = (HashMap::new(), HashMap::new());
    let mut rows = Vec::with_capacity(cities.len());
    for c in cities {
        let region = c
            .region
            .as_deref()
            .map_or(u16::MAX, |r| intern(&mut regions, &mut region_ids, r));
        let key = (c.country_code.as_str(), c.country.as_str());
        rows.push((region, intern(&mut countries, &mut country_ids, key)));
    }

    let mut out = Vec::new();
    out.extend_from_slice(MAGIC);
    for n in [
        VERSION,
        regions.len() as u32,
        countries.len() as u32,
        cities.len() as u32,
    ] {
        out.extend_from_slice(&n.to_le_bytes());
    }
    fn string(out: &mut Vec<u8>, s: &str) {
        out.extend_from_slice(&(s.len() as u16).to_le_bytes());
        out.extend_from_slice(s.as_bytes());
    }
    for r in &regions {
        string(&mut out, r);
    }
    for (code, name) in &countries {
        string(&mut out, &format!("{code}\t{name}"));
    }
    for c in cities {
        string(&mut out, &c.name);
    }
    for (c, (region, country)) in cities.iter().zip(rows) {
        out.extend_from_slice(&c.id.to_le_bytes());
        out.extend_from_slice(&((c.lat * 1e5).round() as i32).to_le_bytes());
        out.extend_from_slice(&((c.lon * 1e5).round() as i32).to_le_bytes());
        out.extend_from_slice(&region.to_le_bytes());
        out.extend_from_slice(&country.to_le_bytes());
        let thousands = (c.population / 1000).min(u16::MAX as u32) as u16;
        out.extend_from_slice(&thousands.to_le_bytes());
    }
    out
}

struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        if self.0.len() < n {
            return Err(Error::Invalid("Place data is truncated".into()));
        }
        let (head, rest) = self.0.split_at(n);
        self.0 = rest;
        Ok(head)
    }

    fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().unwrap()))
    }

    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }

    fn string(&mut self) -> Result<&'a str> {
        let len = self.u16()? as usize;
        std::str::from_utf8(self.take(len)?)
            .map_err(|_| Error::Invalid("Place data has a bad name".into()))
    }
}

pub fn unpack(data: &[u8]) -> Result<Vec<City>> {
    let mut r = Reader(data);
    if r.take(4)? != MAGIC || r.u32()? != VERSION {
        return Err(Error::Invalid("Unknown place data format".into()));
    }
    let (n_regions, n_countries, n_cities) = (r.u32()?, r.u32()?, r.u32()?);
    let regions: Vec<&str> = (0..n_regions).map(|_| r.string()).collect::<Result<_>>()?;
    let countries: Vec<(&str, &str)> = (0..n_countries)
        .map(|_| {
            let s = r.string()?;
            s.split_once('\t')
                .ok_or_else(|| Error::Invalid("Place data has a bad country".into()))
        })
        .collect::<Result<_>>()?;
    let names: Vec<&str> = (0..n_cities).map(|_| r.string()).collect::<Result<_>>()?;
    names
        .into_iter()
        .map(|name| {
            let id = r.u32()?;
            let lat = r.u32()? as i32 as f64 / 1e5;
            let lon = r.u32()? as i32 as f64 / 1e5;
            let region = r.u16()?;
            let (country_code, country) = *countries
                .get(r.u16()? as usize)
                .ok_or_else(|| Error::Invalid("Place data has a bad country".into()))?;
            Ok(City {
                id,
                lat,
                lon,
                name: name.to_owned(),
                region: regions.get(region as usize).map(|s| (*s).to_owned()),
                country_code: country_code.to_owned(),
                country: country.to_owned(),
                population: r.u16()? as u32 * 1000,
            })
        })
        .collect()
}

/// Cities bucketed into 1° cells for nearest-city lookups.
pub struct Index {
    cities: Vec<City>,
    /// `cells[c]..cells[c + 1]` is the range of `order` in cell `c`.
    cells: Vec<u32>,
    order: Vec<u32>,
}

fn cell(lat_row: usize, lon_col: usize) -> usize {
    lat_row * 360 + lon_col
}

fn row_col(lat: f64, lon: f64) -> (usize, usize) {
    let row = ((lat + 90.0).floor() as i64).clamp(0, 179) as usize;
    let col = ((lon + 180.0).floor() as i64).rem_euclid(360) as usize;
    (row, col)
}

/// Flat-earth distance: within the ~100 km that matters here it is off by well under
/// 1%, and it is several times cheaper than haversine (dense areas test ~1000 towns).
fn distance_km(lat1: f64, lon1: f64, cos_lat: f64, lat2: f64, lon2: f64) -> f64 {
    let mut dl = (lon2 - lon1).abs();
    if dl > 180.0 {
        dl = 360.0 - dl;
    }
    let (x, y) = ((dl * cos_lat).to_radians(), (lat2 - lat1).to_radians());
    EARTH_KM * (x * x + y * y).sqrt()
}

impl Index {
    pub fn new(cities: Vec<City>) -> Index {
        let mut keyed: Vec<(usize, u32)> = cities
            .iter()
            .enumerate()
            .map(|(i, c)| {
                let (row, col) = row_col(c.lat, c.lon);
                (cell(row, col), i as u32)
            })
            .collect();
        keyed.sort_unstable();
        let mut cells = vec![0u32; 180 * 360 + 1];
        for &(c, _) in &keyed {
            cells[c + 1] += 1;
        }
        for c in 1..cells.len() {
            cells[c] += cells[c - 1];
        }
        Index {
            cities,
            cells,
            order: keyed.into_iter().map(|(_, i)| i).collect(),
        }
    }

    /// The bundled city list, decoded on first use.
    pub fn bundled() -> &'static Index {
        static INDEX: OnceLock<Index> = OnceLock::new();
        INDEX.get_or_init(|| Index::new(unpack(DATA).expect("bundled place data")))
    }

    pub fn len(&self) -> usize {
        self.cities.len()
    }

    pub fn is_empty(&self) -> bool {
        self.cities.is_empty()
    }

    /// The city a photo taken at (`lat`, `lon`) belongs to, if any is close enough.
    pub fn nearest(&self, lat: f64, lon: f64) -> Option<&City> {
        if !(-90.0..=90.0).contains(&lat) || !(-180.0..=180.0).contains(&lon) {
            return None;
        }
        // Reach never exceeds ~25 km, so everything in range is within one cell of
        // latitude (111 km); cells shrink in longitude towards the poles.
        let (row, col) = row_col(lat, lon);
        let widest = (lat.abs() + 1.0).min(89.0).to_radians().cos();
        let span = ((1.0 / widest).ceil() as usize).min(180);
        let cos_lat = lat.to_radians().cos();
        let mut best: Option<(f64, &City)> = None;
        for r in row.saturating_sub(1)..=(row + 1).min(179) {
            for dc in 0..=(2 * span).min(359) {
                let c = (col + 360 - span + dc) % 360;
                let (lo, hi) = (self.cells[cell(r, c)], self.cells[cell(r, c) + 1]);
                for &i in &self.order[lo as usize..hi as usize] {
                    let city = &self.cities[i as usize];
                    let reach = REACH_KM * (city.population as f64).sqrt();
                    let d = distance_km(lat, lon, cos_lat, city.lat, city.lon);
                    if d > MAX_KM + reach {
                        continue;
                    }
                    let score = d - reach;
                    if best.is_none_or(|(s, _)| score < s) {
                        best = Some((score, city));
                    }
                }
            }
        }
        best.map(|(_, c)| c)
    }
}

#[derive(Serialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Place {
    pub id: i64,
    pub name: String,
    pub region: Option<String>,
    pub country: String,
    pub country_code: String,
}

/// Columns of `places p` in the order `place_row` reads them.
pub const PLACE_COLUMNS: &str = "p.id, p.name, p.region, p.country, p.country_code";

/// Reads a `Place` from five columns starting at `at` (all NULL for none).
pub fn place_row(r: &rusqlite::Row, at: usize) -> rusqlite::Result<Option<Place>> {
    let Some(id) = r.get::<_, Option<i64>>(at)? else {
        return Ok(None);
    };
    Ok(Some(Place {
        id,
        name: r.get(at + 1)?,
        region: r.get(at + 2)?,
        country: r.get(at + 3)?,
        country_code: r.get(at + 4)?,
    }))
}

/// True when some photos with GPS haven't been given a place yet.
pub fn needs_assign(conn: &Connection) -> Result<bool> {
    let needed: bool = conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM assets WHERE lat IS NOT NULL AND place_id IS NULL)",
        [],
        |r| r.get(0),
    )?;
    Ok(needed)
}

/// Gives every photo with GPS but no place yet its place. Returns how many were
/// looked up. The writer is released between batches so other writes get a turn.
pub fn assign(db: &Db) -> Result<usize> {
    let index = Index::bundled();
    let (mut after, mut done) = (i64::MIN, 0);
    while let Some((last, n)) = assign_batch(&mut db.write(), index, after)? {
        after = last;
        done += n;
    }
    Ok(done)
}

/// `assign` for tests that hold a plain connection.
#[cfg(test)]
pub(crate) fn assign_conn(conn: &mut Connection) -> Result<usize> {
    let (mut after, mut done) = (i64::MIN, 0);
    while let Some((last, n)) = assign_batch(conn, Index::bundled(), after)? {
        after = last;
        done += n;
    }
    Ok(done)
}

/// Looks up one batch of unplaced assets with ids above `after`. Returns the last id
/// and how many were done, or `None` when there are none left.
fn assign_batch(conn: &mut Connection, index: &Index, after: i64) -> Result<Option<(i64, usize)>> {
    let tx = conn.transaction()?;
    let batch: Vec<(i64, f64, f64)> = {
        let mut stmt = tx.prepare_cached(
            "SELECT id, lat, lon FROM assets
             WHERE lat IS NOT NULL AND place_id IS NULL AND id > ?1 ORDER BY id LIMIT ?2",
        )?;
        stmt.query_map(params![after, BATCH as i64], |r| {
            Ok((
                r.get(0)?,
                r.get(1)?,
                r.get::<_, Option<f64>>(2)?.unwrap_or(f64::NAN),
            ))
        })?
        .collect::<rusqlite::Result<_>>()?
    };
    let Some(&(last, _, _)) = batch.last() else {
        return Ok(None);
    };
    {
        let mut place = tx.prepare_cached(
            "INSERT INTO places (id, name, region, country, country_code)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT (id) DO UPDATE SET name = excluded.name, region = excluded.region,
                 country = excluded.country, country_code = excluded.country_code",
        )?;
        let mut write = tx.prepare_cached("UPDATE assets SET place_id = ?2 WHERE id = ?1")?;
        let mut stored: HashSet<u32> = HashSet::new();
        for &(id, lat, lon) in &batch {
            let city = index.nearest(lat, lon);
            if let Some(c) = city
                && stored.insert(c.id)
            {
                place.execute(params![c.id, c.name, c.region, c.country, c.country_code])?;
            }
            write.execute(params![id, city.map_or(NOWHERE, |c| c.id as i64)])?;
        }
    }
    tx.commit()?;
    Ok(Some((last, batch.len())))
}

pub fn get(conn: &Connection, id: i64) -> Result<Option<Place>> {
    let mut stmt = conn.prepare_cached(&format!(
        "SELECT {PLACE_COLUMNS} FROM places p WHERE p.id = ?1"
    ))?;
    let mut rows = stmt.query(params![id])?;
    match rows.next()? {
        Some(row) => Ok(place_row(row, 0)?),
        None => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn city(id: u32, name: &str, lat: f64, lon: f64, population: u32) -> City {
        City {
            id,
            lat,
            lon,
            name: name.into(),
            region: Some("Punjab".into()),
            country_code: "PK".into(),
            country: "Pakistan".into(),
            population,
        }
    }

    #[test]
    fn pack_round_trips() {
        let mut cities = vec![
            city(1172451, "Lahore", 31.558, 74.35071, 13_004_000),
            city(1166000, "Shāhdara", 31.62, 74.28, 400_000),
        ];
        cities[1].region = None;
        assert_eq!(unpack(&pack(&cities)).unwrap(), cities);
    }

    #[test]
    fn big_cities_reach_past_nearby_towns() {
        let index = Index::new(vec![
            city(1, "Big City", 0.0, 0.0, 9_000_000),
            city(2, "Suburb", 0.18, 0.0, 200_000), // 20 km north
        ]);
        // 11 km from the centre, 9 km from the suburb: the city's reach wins.
        assert_eq!(index.nearest(0.1, 0.0).unwrap().name, "Big City");
        // Right next to the suburb's centre.
        assert_eq!(index.nearest(0.175, 0.0).unwrap().name, "Suburb");
        assert!(index.nearest(5.0, 5.0).is_none(), "too far from anything");
    }

    #[test]
    fn lookups_wrap_around_the_date_line() {
        let index = Index::new(vec![city(1, "Suva-ish", -18.0, 179.9, 80_000)]);
        assert_eq!(index.nearest(-18.0, -179.9).unwrap().id, 1);
    }

    #[test]
    fn bundled_data_finds_real_cities() {
        let index = Index::bundled();
        assert!(index.len() > 25_000);
        let lahore = index.nearest(31.5204, 74.3587).unwrap();
        assert_eq!(
            (lahore.name.as_str(), lahore.country_code.as_str()),
            ("Lahore", "PK")
        );
        assert_eq!(index.nearest(48.8584, 2.2945).unwrap().name, "Paris");
        assert_eq!(
            index.nearest(40.7580, -73.9855).unwrap().name,
            "New York City"
        );
        assert!(index.nearest(0.0, -140.0).is_none(), "mid-Pacific");
    }

    #[test]
    fn assigns_places_once() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(
            &dir.path().join("lib.sqlite"),
            1,
            crate::schema::migrate_library,
        )
        .unwrap();
        let conn = db.write();
        conn.execute_batch(
            "INSERT INTO assets (media_kind, taken_at, taken_src, local_day, lat, lon, created_at)
             VALUES (0, 0, 0, 0, 31.52, 74.36, 0), (0, 0, 0, 0, 0.0, -140.0, 0),
                    (0, 0, 0, 0, NULL, NULL, 0);",
        )
        .unwrap();
        assert!(needs_assign(&conn).unwrap());
        drop(conn);
        assert_eq!(assign(&db).unwrap(), 2);
        assert_eq!(assign(&db).unwrap(), 0);
        let conn = db.read();
        assert!(!needs_assign(&conn).unwrap());
        let ids: Vec<Option<i64>> = conn
            .prepare("SELECT place_id FROM assets ORDER BY id")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(ids[1..], [Some(NOWHERE), None]);
        let place = get(&conn, ids[0].unwrap()).unwrap().unwrap();
        assert_eq!(place.name, "Lahore");
    }
}
