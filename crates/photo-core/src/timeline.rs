use rusqlite::types::Value;
use rusqlite::{Connection, params_from_iter};
use serde::{Deserialize, Serialize};

use crate::Result;
use crate::places::{PLACE_COLUMNS, Place, place_row};

pub const DAY_MS: i64 = 86_400_000;

pub fn local_day(taken_at: i64, tz_offset_min: i32) -> i64 {
    (taken_at + tz_offset_min as i64 * 60_000).div_euclid(DAY_MS)
}

/// Which assets a timeline shows. `All` is served from precomputed day buckets;
/// the others are small or rarely used enough to aggregate on demand.
/// From JSON: `"all"`, `"favorites"`, … or `{"album": 3}`, `{"tag": 7}`, `{"event": 9}`,
/// `{"trip": 2}`.
#[derive(Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum View {
    #[default]
    All,
    Favorites,
    Videos,
    Hidden,
    Album(i64),
    Tag(i64),
    Event(i64),
    Trip(i64),
}

impl View {
    /// SQL condition over `assets a LEFT JOIN user_meta u`.
    fn condition(self) -> String {
        // Present, and not the clip half of a Live Photo.
        const PRESENT: &str = "a.status = 0 AND a.paired_with IS NULL";
        const VISIBLE: &str =
            "a.status = 0 AND a.paired_with IS NULL AND COALESCE(u.hidden, 0) = 0";
        match self {
            View::All => VISIBLE.to_owned(),
            View::Favorites => format!("{VISIBLE} AND u.favorite = 1"),
            View::Videos => format!("{VISIBLE} AND a.media_kind = 1"),
            View::Hidden => format!("{PRESENT} AND u.hidden = 1"),
            // Albums are explicit choices, so hidden items still show in them.
            View::Album(id) => format!(
                "{PRESENT} AND a.id IN (SELECT asset_id FROM album_assets WHERE album_id = {id})"
            ),
            View::Event(id) => format!("{VISIBLE} AND a.event_id = {id}"),
            // Every photo of the trip's days, located or not.
            View::Trip(id) => format!(
                "{VISIBLE} AND a.local_day BETWEEN (SELECT first_day FROM trips WHERE id = {id})
                     AND (SELECT last_day FROM trips WHERE id = {id})"
            ),
            View::Tag(id) => format!(
                "{VISIBLE} AND a.id IN (SELECT asset_id FROM asset_tags WHERE tag_id = {id})"
            ),
        }
    }
}

const FROM: &str = "FROM assets a LEFT JOIN user_meta u ON u.asset_id = a.id";

#[derive(Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum MediaFilter {
    Photo,
    Video,
}

/// Narrows any view. Empty fields don't filter. Camera names are matched against the
/// model (an empty string matches photos with no camera recorded).
#[derive(Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Filter {
    pub media: Option<MediaFilter>,
    pub cameras: Vec<String>,
    pub root_ids: Vec<i64>,
    /// Inclusive range of local days.
    pub from_day: Option<i64>,
    pub to_day: Option<i64>,
    pub min_rating: Option<u8>,
    /// Towns (GeoNames ids) and countries (ISO codes); either matches.
    pub place_ids: Vec<i64>,
    pub countries: Vec<String>,
    /// A rectangle of the map (opened from a map cluster).
    pub area: Option<crate::map::Area>,
    /// Only these items (search results); an empty list matches nothing.
    pub ids: Option<Vec<i64>>,
}

impl Filter {
    pub fn is_empty(&self) -> bool {
        *self == Filter::default()
    }

    /// Extra `AND …` conditions and their bound values, with placeholders numbered
    /// from `first`. Integers are inlined; strings are always bound.
    fn sql(&self, first: usize) -> (String, Vec<Value>) {
        let mut sql = String::new();
        let mut values = Vec::new();
        match self.media {
            Some(MediaFilter::Photo) => sql.push_str(" AND a.media_kind != 1"),
            Some(MediaFilter::Video) => sql.push_str(" AND a.media_kind = 1"),
            None => {}
        }
        if !self.cameras.is_empty() {
            // Written so SQLite can use the camera index (COALESCE would defeat it).
            let named: Vec<&String> = self.cameras.iter().filter(|c| !c.is_empty()).collect();
            let mut parts = Vec::new();
            if !named.is_empty() {
                let slots: Vec<String> = (0..named.len())
                    .map(|i| format!("?{}", first + i))
                    .collect();
                parts.push(format!("a.camera_model IN ({})", slots.join(", ")));
                values.extend(named.into_iter().cloned().map(Value::Text));
            }
            if self.cameras.iter().any(String::is_empty) {
                parts.push("a.camera_model IS NULL".to_owned());
            }
            sql.push_str(&format!(" AND ({})", parts.join(" OR ")));
        }
        if !self.root_ids.is_empty() {
            let ids: Vec<String> = self.root_ids.iter().map(i64::to_string).collect();
            sql.push_str(&format!(
                " AND EXISTS (SELECT 1 FROM files f WHERE f.asset_id = a.id AND f.missing_since IS NULL AND f.root_id IN ({}))",
                ids.join(", ")
            ));
        }
        if let Some(day) = self.from_day {
            sql.push_str(&format!(" AND a.local_day >= {day}"));
        }
        if let Some(day) = self.to_day {
            sql.push_str(&format!(" AND a.local_day <= {day}"));
        }
        if let Some(rating) = self.min_rating {
            sql.push_str(&format!(" AND u.rating >= {rating}"));
        }
        if !self.place_ids.is_empty() || !self.countries.is_empty() {
            let mut parts = Vec::new();
            if !self.place_ids.is_empty() {
                let ids: Vec<String> = self.place_ids.iter().map(i64::to_string).collect();
                parts.push(format!("a.place_id IN ({})", ids.join(", ")));
            }
            if !self.countries.is_empty() {
                let slots: Vec<String> = (0..self.countries.len())
                    .map(|i| format!("?{}", first + values.len() + i))
                    .collect();
                parts.push(format!(
                    "a.place_id IN (SELECT id FROM places WHERE country_code IN ({}))",
                    slots.join(", ")
                ));
                values.extend(self.countries.iter().cloned().map(Value::Text));
            }
            // `place_id > 0` lets SQLite use the partial place index.
            sql.push_str(&format!(" AND a.place_id > 0 AND ({})", parts.join(" OR ")));
        }
        if let Some(area) = &self.area {
            sql.push_str(&area.sql());
        }
        if let Some(ids) = &self.ids {
            if ids.is_empty() {
                sql.push_str(" AND 0");
            } else {
                let list: Vec<String> = ids.iter().map(i64::to_string).collect();
                sql.push_str(&format!(" AND a.id IN ({})", list.join(", ")));
            }
        }
        (sql, values)
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CameraOption {
    /// Empty string for photos without a recorded camera.
    pub model: String,
    pub count: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlaceOption {
    #[serde(flatten)]
    pub place: Place,
    pub count: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CountryOption {
    pub code: String,
    pub name: String,
    pub count: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FilterOptions {
    pub cameras: Vec<CameraOption>,
    pub places: Vec<PlaceOption>,
    pub countries: Vec<CountryOption>,
    pub first_day: Option<i64>,
    pub last_day: Option<i64>,
}

/// The most common places offered as filter choices.
const PLACE_OPTIONS: usize = 200;

/// Every place with photos and every country, most photos first.
fn place_options(conn: &Connection) -> Result<(Vec<PlaceOption>, Vec<CountryOption>)> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {PLACE_COLUMNS}, n FROM
             (SELECT place_id, count(*) AS n FROM assets
              WHERE place_id > 0 AND status = 0 AND paired_with IS NULL GROUP BY place_id)
         JOIN places p ON p.id = place_id ORDER BY n DESC, p.name"
    ))?;
    let mut places: Vec<PlaceOption> = stmt
        .query_map([], |r| {
            Ok(PlaceOption {
                place: place_row(r, 0)?.expect("joined place"),
                count: r.get(5)?,
            })
        })?
        .collect::<rusqlite::Result<_>>()?;
    let mut countries: Vec<CountryOption> = Vec::new();
    for p in &places {
        match countries
            .iter_mut()
            .find(|c| c.code == p.place.country_code)
        {
            Some(c) => c.count += p.count,
            None => countries.push(CountryOption {
                code: p.place.country_code.clone(),
                name: p.place.country.clone(),
                count: p.count,
            }),
        }
    }
    countries.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.name.cmp(&b.name)));
    places.truncate(PLACE_OPTIONS);
    Ok((places, countries))
}

/// Choices for the filter panel, most common first.
pub fn filter_options(conn: &Connection) -> Result<FilterOptions> {
    let (places, countries) = place_options(conn)?;
    let mut stmt = conn.prepare(
        "SELECT COALESCE(camera_model, ''), count(*) FROM assets WHERE status = 0
         GROUP BY 1 ORDER BY 2 DESC LIMIT 100",
    )?;
    let cameras = stmt
        .query_map([], |r| {
            Ok(CameraOption {
                model: r.get(0)?,
                count: r.get(1)?,
            })
        })?
        .collect::<rusqlite::Result<_>>()?;
    let (first_day, last_day) =
        conn.query_row("SELECT min(day), max(day) FROM day_buckets", [], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })?;
    Ok(FilterOptions {
        cameras,
        places,
        countries,
        first_day,
        last_day,
    })
}

/// Every day that has assets, newest first. Parallel arrays keep the IPC payload small.
#[derive(Serialize)]
pub struct Layout {
    pub days: Vec<i64>,
    pub counts: Vec<u32>,
}

/// Assets of one day in display order, as parallel arrays.
#[derive(Serialize)]
pub struct DayAssets {
    pub day: i64,
    pub ids: Vec<i64>,
    pub kinds: Vec<u8>,
    pub favorites: Vec<u8>,
    pub durations: Vec<Option<i64>>,
}

#[derive(Serialize)]
pub struct Stats {
    pub assets: i64,
    pub days: i64,
}

pub fn layout(conn: &Connection, view: View, filter: &Filter) -> Result<Layout> {
    let (extra, values) = filter.sql(1);
    let sql = if view == View::All && filter.is_empty() {
        "SELECT day, count FROM day_buckets ORDER BY day DESC".to_owned()
    } else {
        format!(
            "SELECT a.local_day, count(*) {FROM} WHERE {}{extra} GROUP BY a.local_day ORDER BY a.local_day DESC",
            view.condition()
        )
    };
    let mut stmt = conn.prepare_cached(&sql)?;
    let mut layout = Layout {
        days: Vec::new(),
        counts: Vec::new(),
    };
    let mut rows = stmt.query(params_from_iter(values))?;
    while let Some(row) = rows.next()? {
        layout.days.push(row.get(0)?);
        layout.counts.push(row.get(1)?);
    }
    Ok(layout)
}

/// Asset ids for each requested day, newest first within the day.
pub fn day_assets(
    conn: &Connection,
    view: View,
    filter: &Filter,
    days: &[i64],
) -> Result<Vec<DayAssets>> {
    let (extra, values) = filter.sql(2);
    let sql = format!(
        "SELECT a.id, a.media_kind, COALESCE(u.favorite, 0), a.duration_ms {FROM}
         WHERE a.local_day = ?1 AND {}{extra} ORDER BY a.taken_at DESC, a.id DESC",
        view.condition()
    );
    let mut stmt = conn.prepare_cached(&sql)?;
    days.iter()
        .map(|&day| {
            let mut out = DayAssets {
                day,
                ids: Vec::new(),
                kinds: Vec::new(),
                favorites: Vec::new(),
                durations: Vec::new(),
            };
            let bound = std::iter::once(Value::Integer(day)).chain(values.iter().cloned());
            let mut rows = stmt.query(params_from_iter(bound))?;
            while let Some(row) = rows.next()? {
                out.ids.push(row.get(0)?);
                out.kinds.push(row.get(1)?);
                out.favorites.push(row.get(2)?);
                out.durations.push(row.get(3)?);
            }
            Ok(out)
        })
        .collect()
}

pub fn stats(conn: &Connection) -> Result<Stats> {
    let sql = format!(
        "SELECT (SELECT count(*) {FROM} WHERE {}), (SELECT count(*) FROM day_buckets)",
        View::All.condition()
    );
    Ok(conn.query_row(&sql, [], |r| {
        Ok(Stats {
            assets: r.get(0)?,
            days: r.get(1)?,
        })
    })?)
}

pub fn rebuild_day_buckets(conn: &Connection) -> Result<()> {
    conn.execute_batch(&format!(
        "BEGIN;
         DELETE FROM day_buckets;
         INSERT INTO day_buckets (day, count)
             SELECT a.local_day, count(*) {FROM} WHERE {} GROUP BY a.local_day;
         COMMIT;",
        View::All.condition()
    ))?;
    Ok(())
}

/// Every asset id in a view, newest first (for "select all").
pub fn asset_ids(conn: &Connection, view: View, filter: &Filter) -> Result<Vec<i64>> {
    let (extra, values) = filter.sql(1);
    let sql = format!(
        "SELECT a.id {FROM} WHERE {}{extra} ORDER BY a.local_day DESC, a.taken_at DESC, a.id DESC",
        view.condition()
    );
    let mut stmt = conn.prepare(&sql)?;
    let ids = stmt
        .query_map(params_from_iter(values), |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    Ok(ids)
}
