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

    -- User-authored data. Scans never write here.
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
