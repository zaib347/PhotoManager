//! Schema migrations, tracked with `PRAGMA user_version`.
//!
//! Conventions: timestamps are Unix milliseconds (UTC). `local_day` is the day
//! number (days since 1970-01-01) in the asset's own local time, which is what
//! the timeline groups by.

use rusqlite::Connection;

use crate::Result;

const LIBRARY_MIGRATIONS: &[&str] = &[
    // v1
    "
    CREATE TABLE volumes (
        id              INTEGER PRIMARY KEY,
        uuid            TEXT NOT NULL UNIQUE,   -- volume serial / filesystem UUID
        label           TEXT,
        last_mount_path TEXT,
        status          TEXT NOT NULL DEFAULT 'online',
        created_at      INTEGER NOT NULL
    );

    CREATE TABLE roots (
        id             INTEGER PRIMARY KEY,
        volume_id      INTEGER NOT NULL REFERENCES volumes(id),
        rel_path       TEXT NOT NULL,           -- relative to the volume mount point
        excluded_globs TEXT,
        last_scan_at   INTEGER,
        UNIQUE (volume_id, rel_path)
    );

    -- The photo or video the user cares about. One asset may have many files.
    CREATE TABLE assets (
        id            INTEGER PRIMARY KEY,
        content_key   TEXT UNIQUE,              -- full content hash once known
        media_kind    INTEGER NOT NULL,         -- 0 photo, 1 video, 2 live photo
        taken_at      INTEGER NOT NULL,         -- best known capture time
        taken_src     INTEGER NOT NULL,         -- 0 exif, 1 video atom, 2 filename, 3 mtime, 4 user
        tz_offset_min INTEGER NOT NULL DEFAULT 0,
        local_day     INTEGER NOT NULL,
        width         INTEGER,
        height        INTEGER,
        orientation   INTEGER,
        duration_ms   INTEGER,
        camera_make   TEXT,
        camera_model  TEXT,
        lat           REAL,
        lon           REAL,
        place_id      INTEGER,
        phash         INTEGER,
        blur          REAL,
        best_file_id  INTEGER,
        created_at    INTEGER NOT NULL
    );
    CREATE INDEX idx_assets_timeline ON assets (local_day, taken_at, id);

    -- Where bytes physically live.
    CREATE TABLE files (
        id             INTEGER PRIMARY KEY,
        root_id        INTEGER NOT NULL REFERENCES roots(id),
        rel_path       TEXT NOT NULL,
        size           INTEGER NOT NULL,
        mtime          INTEGER NOT NULL,
        quick_hash     BLOB,                    -- hash of size + first 64 KiB
        full_hash      BLOB,
        asset_id       INTEGER REFERENCES assets(id),
        missing_since  INTEGER,
        last_seen_scan INTEGER,
        UNIQUE (root_id, rel_path)
    );
    CREATE INDEX idx_files_asset ON files (asset_id);
    CREATE INDEX idx_files_size_qhash ON files (size, quick_hash);

    -- User-authored data. Scans only fill blanks from XMP (see `xmp`).
    CREATE TABLE user_meta (
        asset_id       INTEGER PRIMARY KEY REFERENCES assets(id) ON DELETE CASCADE,
        rating         INTEGER,
        favorite       INTEGER NOT NULL DEFAULT 0,
        hidden         INTEGER NOT NULL DEFAULT 0,
        note           TEXT,
        corrected_time INTEGER
    );

    CREATE TABLE tags (
        id   INTEGER PRIMARY KEY,
        name TEXT NOT NULL UNIQUE
    );
    CREATE TABLE asset_tags (
        asset_id INTEGER NOT NULL REFERENCES assets(id) ON DELETE CASCADE,
        tag_id   INTEGER NOT NULL REFERENCES tags(id) ON DELETE CASCADE,
        PRIMARY KEY (asset_id, tag_id)
    ) WITHOUT ROWID;

    CREATE TABLE albums (
        id         INTEGER PRIMARY KEY,
        name       TEXT NOT NULL,
        created_at INTEGER NOT NULL
    );
    CREATE TABLE album_assets (
        album_id INTEGER NOT NULL REFERENCES albums(id) ON DELETE CASCADE,
        asset_id INTEGER NOT NULL REFERENCES assets(id) ON DELETE CASCADE,
        position INTEGER,
        PRIMARY KEY (album_id, asset_id)
    ) WITHOUT ROWID;

    CREATE TABLE events (
        id       INTEGER PRIMARY KEY,
        title    TEXT,
        start_at INTEGER NOT NULL,
        end_at   INTEGER NOT NULL,
        place_id INTEGER,
        status   TEXT NOT NULL DEFAULT 'suggested'  -- suggested | accepted | rejected
    );

    -- Timeline layout source: one row per local day that has assets.
    CREATE TABLE day_buckets (
        day   INTEGER PRIMARY KEY,
        count INTEGER NOT NULL
    );

    CREATE TABLE jobs (
        id              INTEGER PRIMARY KEY,
        kind            TEXT NOT NULL,
        state           TEXT NOT NULL,
        checkpoint_json TEXT,
        updated_at      INTEGER NOT NULL
    );

    CREATE TABLE file_ops (
        id           INTEGER PRIMARY KEY,
        plan_json    TEXT NOT NULL,
        status       TEXT NOT NULL,
        undo_json    TEXT,
        created_at   INTEGER NOT NULL,
        completed_at INTEGER
    );
    ",
    // v2: assets whose every file is confirmed deleted drop out of the timeline but
    // keep their user data, in case the files come back.
    "
    ALTER TABLE assets ADD COLUMN status INTEGER NOT NULL DEFAULT 0;  -- 0 present, 1 all files missing
    ",
    // v3: albums and tags.
    "
    ALTER TABLE albums ADD COLUMN cover_asset_id INTEGER;  -- NULL: newest item is the cover
    ALTER TABLE albums ADD COLUMN updated_at INTEGER;
    CREATE INDEX idx_album_assets_asset ON album_assets (asset_id);
    CREATE INDEX idx_asset_tags_tag ON asset_tags (tag_id, asset_id);
    CREATE UNIQUE INDEX idx_tags_name_nocase ON tags (name COLLATE NOCASE);
    ",
    // v4: filters and the Videos view read only matching rows instead of every asset.
    "
    CREATE INDEX idx_assets_kind_day ON assets (media_kind, local_day);
    CREATE INDEX idx_assets_camera_day ON assets (camera_model, local_day);
    ",
    // v5: Live Photo clips point at their still and leave the timeline.
    "
    ALTER TABLE assets ADD COLUMN paired_with INTEGER;
    CREATE INDEX idx_assets_paired ON assets (paired_with) WHERE paired_with IS NOT NULL;
    ",
    // v6: date corrections keep the extracted date so they can be undone.
    "
    ALTER TABLE user_meta ADD COLUMN original_taken_at INTEGER;
    ALTER TABLE user_meta ADD COLUMN original_tz_offset_min INTEGER;
    ALTER TABLE user_meta ADD COLUMN original_taken_src INTEGER;
    ",
    // v7: events. Times are wall-clock ms; count and cover are kept by `events::detect`.
    "
    ALTER TABLE assets ADD COLUMN event_id INTEGER;
    CREATE INDEX idx_assets_event ON assets (event_id, local_day) WHERE event_id IS NOT NULL;
    ALTER TABLE events ADD COLUMN count INTEGER NOT NULL DEFAULT 0;
    ALTER TABLE events ADD COLUMN cover_asset_id INTEGER;
    CREATE INDEX idx_events_start ON events (start_at);
    ",
    // v8: places. `assets.place_id` is a GeoNames id (0: no town nearby, NULL: not
    // looked up yet); `places` holds only the ones in use, copied from the bundled list.
    "
    CREATE TABLE places (
        id           INTEGER PRIMARY KEY,
        name         TEXT NOT NULL,
        region       TEXT,
        country      TEXT NOT NULL,
        country_code TEXT NOT NULL
    );
    CREATE INDEX idx_places_country ON places (country_code);
    -- Covers place filters and per-place counts without reading the asset rows.
    CREATE INDEX idx_assets_place_day ON assets (place_id, local_day, status, paired_with)
        WHERE place_id > 0;
    ",
    // v9: trips, runs of local days away from home. Membership is the day range;
    // `trip_places` holds the towns visited, kept by `trips::detect`.
    "
    CREATE TABLE trips (
        id             INTEGER PRIMARY KEY,
        title          TEXT,
        first_day      INTEGER NOT NULL,
        last_day       INTEGER NOT NULL,
        count          INTEGER NOT NULL DEFAULT 0,
        cover_asset_id INTEGER,
        status         TEXT NOT NULL DEFAULT 'suggested'  -- suggested | accepted | rejected
    );
    CREATE INDEX idx_trips_days ON trips (first_day);
    CREATE TABLE trip_places (
        trip_id   INTEGER NOT NULL REFERENCES trips(id) ON DELETE CASCADE,
        place_id  INTEGER NOT NULL,
        count     INTEGER NOT NULL,
        first_day INTEGER NOT NULL,
        PRIMARY KEY (trip_id, place_id)
    ) WITHOUT ROWID;
    ",
    // v10: journaled file operations. `file_ops` (from v1) holds one row per operation
    // (`plan_json` = {kind, reason}); each file it touches is written here before it
    // moves, with the old `files` row so an undo can put it back exactly.
    "
    CREATE TABLE file_op_items (
        op_id      INTEGER NOT NULL REFERENCES file_ops(id) ON DELETE CASCADE,
        seq        INTEGER NOT NULL,
        root_id    INTEGER NOT NULL,
        rel_path   TEXT NOT NULL,      -- where the file was, relative to its root
        trash_rel  TEXT NOT NULL,      -- where it went, relative to the same root
        size       INTEGER NOT NULL,
        mtime      INTEGER NOT NULL,
        quick_hash BLOB,
        full_hash  BLOB,
        asset_id   INTEGER,
        state      TEXT NOT NULL DEFAULT 'planned', -- planned | moved | restored | emptied | failed
        error      TEXT,
        PRIMARY KEY (op_id, seq)
    ) WITHOUT ROWID;
    CREATE INDEX idx_file_ops_status ON file_ops (status);
    ",
    // v11: similar photos. `phash` (64-bit pHash) and `blur` (sharpness: higher is
    // sharper) come from v1; `phash_v` is the hashing version that processed the asset
    // (NULL: not yet). Groups are recomputed by `similar::detect`.
    "
    ALTER TABLE assets ADD COLUMN phash_v INTEGER;
    CREATE INDEX idx_assets_phash_pending ON assets (id) WHERE phash_v IS NULL;
    CREATE TABLE similar_groups (
        id         INTEGER PRIMARY KEY,   -- the smallest member asset id
        kind       TEXT NOT NULL,         -- similar | copies
        first_wall INTEGER NOT NULL,
        count      INTEGER NOT NULL
    );
    CREATE INDEX idx_similar_groups_time ON similar_groups (first_wall);
    CREATE TABLE similar_members (
        asset_id INTEGER PRIMARY KEY,
        group_id INTEGER NOT NULL
    );
    CREATE INDEX idx_similar_members_group ON similar_members (group_id);
    -- Photos the user chose to keep all of; their group isn't shown again.
    CREATE TABLE similar_dismissed (
        asset_id INTEGER PRIMARY KEY,
        at       INTEGER NOT NULL
    );
    ",
    // v12: shot quality from a 1024 px decode, for photos in similar groups only.
    // `focus`: sharpness of the sharpest tiles; `exposure`: 0 (badly clipped or very
    // dark/bright) to 1; `quality_v`: the assessment version (NULL: not assessed).
    "
    ALTER TABLE assets ADD COLUMN focus REAL;
    ALTER TABLE assets ADD COLUMN exposure REAL;
    ALTER TABLE assets ADD COLUMN quality_v INTEGER;
    ",
    // v13: photos turned by the user, in quarter turns clockwise on top of EXIF
    // orientation. The file itself is never rewritten.
    "
    ALTER TABLE user_meta ADD COLUMN turns INTEGER NOT NULL DEFAULT 0;
    CREATE INDEX idx_user_meta_turned ON user_meta (asset_id) WHERE turns != 0;
    ",
    // v14: non-destructive edits. `edit`: the recipe (JSON, see `edit::Recipe`), NULL
    // when the photo shows as shot; `edit_v` counts saves so image URLs change.
    "
    ALTER TABLE user_meta ADD COLUMN edit TEXT;
    ALTER TABLE user_meta ADD COLUMN edit_v INTEGER NOT NULL DEFAULT 0;
    DROP INDEX idx_user_meta_turned;
    CREATE INDEX idx_user_meta_adjusted ON user_meta (asset_id) WHERE turns != 0 OR edit_v != 0;
    ",
    // v15: XMP sidecars (see `xmp`). `settings`: app settings kept with the library.
    // `xmp_seen`: sidecars already read, by mtime. `xmp_dirty`: assets whose sidecars
    // must be written, queued by the triggers below while writing is turned on; a
    // re-queued asset gets a new rowid, so the writer can tell it changed meanwhile.
    // `user_meta.note` is the photo's description.
    "
    CREATE TABLE settings (
        key   TEXT PRIMARY KEY,
        value TEXT NOT NULL
    ) WITHOUT ROWID;
    CREATE TABLE xmp_seen (
        root_id  INTEGER NOT NULL,
        rel_path TEXT NOT NULL,
        mtime    INTEGER NOT NULL,
        PRIMARY KEY (root_id, rel_path)
    ) WITHOUT ROWID;
    CREATE TABLE xmp_dirty (asset_id INTEGER NOT NULL UNIQUE);

    CREATE TRIGGER xmp_meta_insert AFTER INSERT ON user_meta
    WHEN (SELECT value FROM settings WHERE key = 'xmp_write') = '1'
      AND (NEW.rating IS NOT NULL OR NEW.favorite != 0 OR NEW.note IS NOT NULL)
    BEGIN
        INSERT OR REPLACE INTO xmp_dirty (asset_id) VALUES (NEW.asset_id);
    END;
    CREATE TRIGGER xmp_meta_update AFTER UPDATE OF rating, favorite, note ON user_meta
    WHEN (SELECT value FROM settings WHERE key = 'xmp_write') = '1'
      AND (OLD.rating IS NOT NEW.rating OR OLD.favorite != NEW.favorite OR OLD.note IS NOT NEW.note)
    BEGIN
        INSERT OR REPLACE INTO xmp_dirty (asset_id) VALUES (NEW.asset_id);
    END;
    CREATE TRIGGER xmp_tag_insert AFTER INSERT ON asset_tags
    WHEN (SELECT value FROM settings WHERE key = 'xmp_write') = '1'
    BEGIN
        INSERT OR REPLACE INTO xmp_dirty (asset_id) VALUES (NEW.asset_id);
    END;
    CREATE TRIGGER xmp_tag_delete AFTER DELETE ON asset_tags
    WHEN (SELECT value FROM settings WHERE key = 'xmp_write') = '1'
    BEGIN
        INSERT OR REPLACE INTO xmp_dirty (asset_id) VALUES (OLD.asset_id);
    END;
    CREATE TRIGGER xmp_tag_rename AFTER UPDATE OF name ON tags
    WHEN (SELECT value FROM settings WHERE key = 'xmp_write') = '1'
    BEGIN
        INSERT OR REPLACE INTO xmp_dirty (asset_id)
            SELECT asset_id FROM asset_tags WHERE tag_id = NEW.id;
    END;
    CREATE TRIGGER xmp_tag_drop BEFORE DELETE ON tags
    WHEN (SELECT value FROM settings WHERE key = 'xmp_write') = '1'
    BEGIN
        INSERT OR REPLACE INTO xmp_dirty (asset_id)
            SELECT asset_id FROM asset_tags WHERE tag_id = OLD.id;
    END;
    ",
    // v16: locations set by hand (see `map::set_location`), kept over the file's GPS.
    "
    ALTER TABLE user_meta ADD COLUMN location_set INTEGER NOT NULL DEFAULT 0;
    ",
    // v17: imports of Google Takeout and iCloud metadata (see `imports`), journaled for
    // undo. `import_items.field`: date | location | favorite | hidden | description |
    // album (with `album_id`). `import_albums`: albums the import created.
    "
    CREATE TABLE imports (
        id         INTEGER PRIMARY KEY,
        root_id    INTEGER NOT NULL,
        created_at INTEGER NOT NULL,
        report     TEXT NOT NULL,
        undone     INTEGER NOT NULL DEFAULT 0
    );
    CREATE TABLE import_items (
        import_id INTEGER NOT NULL REFERENCES imports(id) ON DELETE CASCADE,
        asset_id  INTEGER NOT NULL,
        field     TEXT NOT NULL,
        album_id  INTEGER NOT NULL DEFAULT 0,
        PRIMARY KEY (import_id, asset_id, field, album_id)
    ) WITHOUT ROWID;
    CREATE TABLE import_albums (
        import_id INTEGER NOT NULL REFERENCES imports(id) ON DELETE CASCADE,
        album_id  INTEGER NOT NULL,
        PRIMARY KEY (import_id, album_id)
    ) WITHOUT ROWID;
    ",
];

const THUMBS_MIGRATIONS: &[&str] = &[
    // v1
    "
    CREATE TABLE thumbs_256 (
        asset_id INTEGER PRIMARY KEY,
        data     BLOB NOT NULL
    );
    ",
];

pub fn migrate_library(conn: &mut Connection) -> Result<()> {
    apply(conn, LIBRARY_MIGRATIONS)
}

pub fn migrate_thumbs(conn: &mut Connection) -> Result<()> {
    apply(conn, THUMBS_MIGRATIONS)
}

fn apply(conn: &mut Connection, migrations: &[&str]) -> Result<()> {
    let current: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    for (i, sql) in migrations.iter().enumerate().skip(current as usize) {
        let tx = conn.transaction()?;
        tx.execute_batch(sql)?;
        tx.pragma_update(None, "user_version", i as i64 + 1)?;
        tx.commit()?;
    }
    Ok(())
}
