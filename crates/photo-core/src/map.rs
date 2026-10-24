//! The Map view: the bundled offline world map, photos grouped into clusters by where
//! they were taken, city search, and setting a location by hand.
//!
//! The map itself is Natural Earth data (public domain) packed by
//! `examples/build_map.rs` into `data/map.bin`; the frontend draws it. Clusters come
//! from [`Points`], every visible geotagged photo in Web Mercator coordinates, loaded
//! when the map opens: grouping a million of them for one view takes milliseconds.

use std::collections::HashMap;
use std::f64::consts::PI;

use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};

use crate::db::Db;
use crate::places::Index;
use crate::{Error, Result};

const DATA: &[u8] = include_bytes!("../data/map.bin");

/// Web Mercator stops at this latitude.
pub const MAX_LAT: f64 = 85.051_128_78;
/// Clusters are about this many pixels apart on screen (map tiles are 256 px).
const CLUSTER_PX: f64 = 72.0;

/// The packed world map (format in `examples/build_map.rs`).
pub fn data() -> &'static [u8] {
    DATA
}

/// Longitude and latitude as Web Mercator x and y, each 0 to 1 (y grows southwards).
pub fn project(lat: f64, lon: f64) -> (f64, f64) {
    let lat = lat.clamp(-MAX_LAT, MAX_LAT).to_radians();
    let x = (lon + 180.0) / 360.0;
    let y = (1.0 - (lat.tan() + 1.0 / lat.cos()).ln() / PI) / 2.0;
    (x, y)
}

/// The latitude at Web Mercator `y`.
fn unproject_lat(y: f64) -> f64 {
    (PI * (1.0 - 2.0 * y)).sinh().atan().to_degrees()
}

/// A rectangle of the map in degrees. `west > east` crosses the antimeridian.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
pub struct Area {
    pub west: f64,
    pub south: f64,
    pub east: f64,
    pub north: f64,
}

impl Area {
    /// SQL that keeps assets inside the area (`a` is the assets table).
    pub fn sql(&self) -> String {
        let num = |v: f64| if v.is_finite() { v } else { 0.0 };
        let lon = if self.west <= self.east {
            format!("a.lon BETWEEN {} AND {}", num(self.west), num(self.east))
        } else {
            format!(
                "(a.lon >= {} OR a.lon <= {})",
                num(self.west),
                num(self.east)
            )
        };
        format!(
            " AND a.lat BETWEEN {} AND {} AND {lon}",
            num(self.south),
            num(self.north)
        )
    }
}

/// A grid cell being gathered: count, sums of x and y, newest (taken_at, id).
type Cell = (u32, f64, f64, (i64, i64));

struct Point {
    x: f32,
    y: f32,
    id: i64,
    taken_at: i64,
}

/// Every visible photo and video with a location.
pub struct Points {
    points: Vec<Point>,
}

#[derive(Serialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Cluster {
    /// Where the marker goes: the middle of its photos.
    pub lat: f64,
    pub lon: f64,
    pub count: u32,
    /// The newest photo, shown on the marker.
    pub cover_id: i64,
    /// The grid cell the cluster gathers; opening it filters the timeline to this.
    pub area: Area,
}

impl Points {
    pub fn load(conn: &Connection) -> Result<Points> {
        let mut stmt = conn.prepare(
            "SELECT a.id, a.lat, a.lon, a.taken_at FROM assets a
             LEFT JOIN user_meta u ON u.asset_id = a.id
             WHERE a.lat IS NOT NULL AND a.lon IS NOT NULL AND a.status = 0
               AND a.paired_with IS NULL AND COALESCE(u.hidden, 0) = 0
               AND NOT (a.lat = 0 AND a.lon = 0)",
        )?;
        let rows = stmt.query_map([], |r| {
            let (lat, lon): (f64, f64) = (r.get(1)?, r.get(2)?);
            let (x, y) = project(lat, lon);
            Ok(Point {
                x: x as f32,
                y: y as f32,
                id: r.get(0)?,
                taken_at: r.get(3)?,
            })
        })?;
        Ok(Points {
            points: rows.collect::<rusqlite::Result<_>>()?,
        })
    }

    pub fn len(&self) -> usize {
        self.points.len()
    }

    pub fn is_empty(&self) -> bool {
        self.points.is_empty()
    }

    /// The area holding every point, if there are any.
    pub fn bounds(&self) -> Option<Area> {
        let first = self.points.first()?;
        let (mut x0, mut y0, mut x1, mut y1) = (first.x, first.y, first.x, first.y);
        for p in &self.points {
            (x0, y0, x1, y1) = (x0.min(p.x), y0.min(p.y), x1.max(p.x), y1.max(p.y));
        }
        Some(Area {
            west: x0 as f64 * 360.0 - 180.0,
            east: x1 as f64 * 360.0 - 180.0,
            north: unproject_lat(y0 as f64),
            south: unproject_lat(y1 as f64),
        })
    }

    /// Clusters at `zoom` (the world is `256 * 2^zoom` px wide) within the Mercator
    /// rectangle `x0..x1`, `y0..y1`, on a grid fixed to the world so clusters don't
    /// jump while panning.
    pub fn clusters(&self, zoom: f64, x0: f64, y0: f64, x1: f64, y1: f64) -> Vec<Cluster> {
        let cells = (256.0 * 2f64.powf(zoom.clamp(0.0, 22.0)) / CLUSTER_PX)
            .floor()
            .max(1.0);
        let mut grid: HashMap<(u32, u32), Cell> = HashMap::new();
        for p in &self.points {
            let (x, y) = (p.x as f64, p.y as f64);
            if x < x0 || x > x1 || y < y0 || y > y1 {
                continue;
            }
            let key = (
                ((x * cells) as u32).min(cells as u32 - 1),
                ((y * cells) as u32).min(cells as u32 - 1),
            );
            let e = grid.entry(key).or_insert((0, 0.0, 0.0, (i64::MIN, 0)));
            e.0 += 1;
            e.1 += x;
            e.2 += y;
            e.3 = e.3.max((p.taken_at, p.id));
        }
        let mut out: Vec<Cluster> = grid
            .into_iter()
            .map(|((cx, cy), (n, sx, sy, (_, cover)))| {
                let (x, y) = (sx / n as f64, sy / n as f64);
                Cluster {
                    lat: unproject_lat(y),
                    lon: x * 360.0 - 180.0,
                    count: n,
                    cover_id: cover,
                    area: Area {
                        west: cx as f64 / cells * 360.0 - 180.0,
                        east: (cx + 1) as f64 / cells * 360.0 - 180.0,
                        north: unproject_lat(cy as f64 / cells),
                        south: unproject_lat((cy + 1) as f64 / cells),
                    },
                }
            })
            .collect();
        // Big clusters drawn last, on top.
        out.sort_by_key(|c| c.count);
        out
    }
}

#[derive(Serialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CityInfo {
    pub id: u32,
    pub name: String,
    pub region: Option<String>,
    pub country: String,
    pub lat: f64,
    pub lon: f64,
    pub population: u32,
}

impl From<&crate::places::City> for CityInfo {
    fn from(c: &crate::places::City) -> Self {
        CityInfo {
            id: c.id,
            name: c.name.clone(),
            region: c.region.clone(),
            country: c.country.clone(),
            lat: c.lat,
            lon: c.lon,
            population: c.population,
        }
    }
}

/// Towns whose name matches `query` (starts with it first, then contains it), bigger
/// towns first. Accents and case are ignored only as far as ASCII case goes.
pub fn search_cities(query: &str, limit: usize) -> Vec<CityInfo> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return Vec::new();
    }
    let mut hits: Vec<(bool, u32, &crate::places::City)> = Index::bundled()
        .cities()
        .iter()
        .filter_map(|c| {
            let name = c.name.to_lowercase();
            if name.starts_with(&q) {
                Some((false, c.population, c))
            } else if name.contains(&q) {
                Some((true, c.population, c))
            } else {
                None
            }
        })
        .collect();
    hits.sort_by(|a, b| a.0.cmp(&b.0).then(b.1.cmp(&a.1)));
    hits.into_iter()
        .take(limit)
        .map(|(_, _, c)| c.into())
        .collect()
}

/// Town labels for the map: name, lat, lon and population, biggest first.
pub fn city_labels() -> Vec<(String, f32, f32, u32)> {
    let mut cities: Vec<(String, f32, f32, u32)> = Index::bundled()
        .cities()
        .iter()
        .map(|c| (c.name.clone(), c.lat as f32, c.lon as f32, c.population))
        .collect();
    cities.sort_by_key(|c| std::cmp::Reverse(c.3));
    cities
}

/// Places photos at (`lat`, `lon`), kept over what their files say (a later rescan of
/// a changed file leaves it). Their towns, and trips, are worked out again.
pub fn set_location(db: &Db, ids: &[i64], lat: f64, lon: f64) -> Result<usize> {
    if !(-90.0..=90.0).contains(&lat) || !(-180.0..=180.0).contains(&lon) {
        return Err(Error::Invalid("That isn't a place on Earth".into()));
    }
    let changed = {
        let mut conn = db.write();
        let tx = conn.transaction()?;
        let mut changed = 0;
        {
            let mut asset =
                tx.prepare("UPDATE assets SET lat = ?2, lon = ?3, place_id = NULL WHERE id = ?1")?;
            let mut meta = tx.prepare(
                "INSERT INTO user_meta (asset_id, location_set) VALUES (?1, 1)
                 ON CONFLICT (asset_id) DO UPDATE SET location_set = 1",
            )?;
            for &id in ids {
                if asset.execute(params![id, lat, lon])? > 0 {
                    meta.execute(params![id])?;
                    changed += 1;
                }
            }
        }
        tx.commit()?;
        changed
    };
    crate::places::assign(db)?;
    crate::trips::detect(&mut db.write())?;
    Ok(changed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn projection_round_trips() {
        for (lat, lon) in [(0.0, 0.0), (51.5, -0.12), (-33.9, 151.2), (80.0, 179.9)] {
            let (x, y) = project(lat, lon);
            assert!((0.0..=1.0).contains(&x) && (0.0..=1.0).contains(&y));
            assert!((unproject_lat(y) - lat).abs() < 1e-9);
            assert!((x * 360.0 - 180.0 - lon).abs() < 1e-9);
        }
        assert!(project(60.0, 0.0).1 < project(0.0, 0.0).1, "north is up");
    }

    #[test]
    fn bundled_map_has_every_layer() {
        let d = data();
        assert_eq!(&d[..4], b"PMMP");
        let layers = u32::from_le_bytes(d[8..12].try_into().unwrap());
        let mut at = 12;
        let mut names = Vec::new();
        for _ in 0..layers {
            let len = d[at] as usize;
            names.push(String::from_utf8(d[at + 1..at + 1 + len].to_vec()).unwrap());
            at += 1 + len + 1 + 2;
            let tiles = u32::from_le_bytes(d[at..at + 4].try_into().unwrap());
            at += 4;
            for _ in 0..tiles {
                let bytes = u32::from_le_bytes(d[at + 4..at + 8].try_into().unwrap()) as usize;
                at += 8 + bytes;
            }
        }
        assert!(names.contains(&"land2".to_owned()) && names.contains(&"road2".to_owned()));
        let labels = u32::from_le_bytes(d[at..at + 4].try_into().unwrap());
        assert!(labels > 150, "country labels: {labels}");
    }

    #[test]
    fn clusters_group_nearby_photos_and_set_locations() {
        let mut conn = Connection::open_in_memory().unwrap();
        crate::schema::migrate_library(&mut conn).unwrap();
        // Three in Paris, one in London, one hidden in Paris, one without GPS.
        conn.execute_batch(
            "INSERT INTO assets (id, media_kind, taken_at, taken_src, local_day, created_at, lat, lon) VALUES
               (1, 0, 10, 0, 0, 0, 48.8566, 2.3522), (2, 0, 30, 0, 0, 0, 48.857, 2.353),
               (3, 0, 20, 0, 0, 0, 48.86, 2.35), (4, 0, 0, 0, 0, 0, 51.507, -0.128),
               (5, 0, 0, 0, 0, 0, 48.8566, 2.3522), (6, 0, 0, 0, 0, 0, NULL, NULL);
             INSERT INTO user_meta (asset_id, hidden) VALUES (5, 1);",
        )
        .unwrap();
        let points = Points::load(&conn).unwrap();
        assert_eq!(points.len(), 4);
        let mut clusters = points.clusters(6.0, 0.0, 0.0, 1.0, 1.0);
        clusters.sort_by_key(|c| c.count);
        assert_eq!(
            clusters.iter().map(|c| c.count).collect::<Vec<_>>(),
            vec![1, 3]
        );
        let paris = &clusters[1];
        assert_eq!(paris.cover_id, 2, "newest photo");
        let a = paris.area;
        assert!(a.west < 2.35 && a.east > 2.354 && a.south < 48.856 && a.north > 48.86);
        // At world zoom, few clusters hold everything.
        let world = points.clusters(0.0, 0.0, 0.0, 1.0, 1.0);
        assert!(world.len() <= 2);
        assert_eq!(world.iter().map(|c| c.count).sum::<u32>(), 4);
        // Only what's in view.
        let (x, y) = project(51.507, -0.128);
        let near_london = points.clusters(6.0, x - 0.001, y - 0.001, x + 0.001, y + 0.001);
        assert_eq!(near_london.len(), 1);

        // The area filter matches the cluster.
        let n: i64 = conn
            .query_row(
                &format!("SELECT COUNT(*) FROM assets a WHERE 1 {}", a.sql()),
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            n, 4,
            "the three, and the hidden one (hiding is the view's job)"
        );

        let hits = search_cities("pari", 5);
        assert_eq!(hits[0].name, "Paris");
        assert!(search_cities("  ", 5).is_empty());
    }

    #[test]
    fn setting_a_location_finds_its_town() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(
            &dir.path().join("lib.sqlite"),
            1,
            crate::schema::migrate_library,
        )
        .unwrap();
        db.write()
            .execute_batch(
                "INSERT INTO assets (id, media_kind, taken_at, taken_src, local_day, created_at)
                 VALUES (1, 0, 0, 0, 0, 0)",
            )
            .unwrap();
        assert_eq!(set_location(&db, &[1, 99], 48.8566, 2.3522).unwrap(), 1);
        let place: Option<String> = db
            .read()
            .query_row(
                "SELECT p.name FROM assets a JOIN places p ON p.id = a.place_id WHERE a.id = 1",
                [],
                |r| r.get(0),
            )
            .ok();
        assert_eq!(place.as_deref(), Some("Paris"));
        assert!(set_location(&db, &[1], 95.0, 0.0).is_err());
    }
}
