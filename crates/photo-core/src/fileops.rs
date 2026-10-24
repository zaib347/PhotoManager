//! Changes to the user's files, made safe.
//!
//! Nothing is ever deleted here. Removing a file moves it into a hidden
//! `.photomanager-trash/<op>/` folder at the top of its library folder (flat, as
//! `<n> <file name>`, so deep folders don't make trash paths too long for the
//! Recycle Bin; the journal keeps where it came from): a rename on
//! the same drive, so it's instant and can be undone at any time. Emptying hands the
//! files to the OS trash, and only on drives where that really keeps them (see
//! `volume::has_recycle_bin`).
//!
//! Moving files within a library folder (the organizer) works the same way, with the
//! destination in place of the trash folder.
//!
//! Every operation is journaled: the plan, with each file's `files` row, is written
//! before anything moves, and each file's state is recorded after it moves. `recover`
//! finishes the bookkeeping for an operation a crash interrupted.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde::Serialize;

use crate::db::Db;
use crate::{Error, Result, library, now_ms, volume};

/// Folder, at the top of each library folder, that removed files are moved into.
pub const TRASH_DIR: &str = ".photomanager-trash";
/// Files moved per write transaction.
const BATCH: usize = 64;
/// Errors kept in a report; the rest are only counted.
const MAX_ERRORS: usize = 20;

/// What an operation does with its files.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Into `TRASH_DIR`; the `files` row goes until an undo.
    Trash,
    /// To another place in the same library folder; the `files` row follows.
    Move,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Trash => "trash",
            Kind::Move => "move",
        }
    }
}

fn kind_of(conn: &Connection, op_id: i64) -> Result<Kind> {
    let kind: Option<String> = conn
        .query_row(
            "SELECT json_extract(plan_json, '$.kind') FROM file_ops WHERE id = ?1",
            params![op_id],
            |r| r.get(0),
        )
        .optional()?
        .flatten();
    Ok(if kind.as_deref() == Some("move") {
        Kind::Move
    } else {
        Kind::Trash
    })
}

#[derive(Serialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct OpSummary {
    pub id: i64,
    /// Why: "duplicates", "similar", "removed", "organize".
    pub reason: String,
    pub created_at: i64,
    /// running | done | undone | emptied
    pub status: String,
    /// Files currently moved (in the trash folder, or at their new place): what an
    /// undo would put back.
    pub in_trash: i64,
    pub in_trash_bytes: i64,
    pub failed: i64,
}

#[derive(Serialize, Debug, Clone, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    pub op_id: i64,
    pub done: usize,
    pub bytes: i64,
    pub failed: usize,
    /// The first few problems, for showing to the user.
    pub errors: Vec<String>,
}

impl Report {
    fn fail(&mut self, what: &str, why: impl std::fmt::Display) {
        self.failed += 1;
        if self.errors.len() < MAX_ERRORS {
            self.errors.push(format!("{what}: {why}"));
        }
    }
}

/// One file in an operation, as journaled.
struct Item {
    seq: i64,
    root_id: i64,
    rel_path: String,
    /// Where it goes, relative to the same root: the trash folder or the destination.
    trash_rel: String,
    size: i64,
    mtime: i64,
    quick_hash: Option<Vec<u8>>,
    full_hash: Option<Vec<u8>>,
    asset_id: Option<i64>,
}

const ITEM_COLUMNS: &str =
    "seq, root_id, rel_path, trash_rel, size, mtime, quick_hash, full_hash, asset_id";

fn read_item(r: &rusqlite::Row) -> rusqlite::Result<Item> {
    Ok(Item {
        seq: r.get(0)?,
        root_id: r.get(1)?,
        rel_path: r.get(2)?,
        trash_rel: r.get(3)?,
        size: r.get(4)?,
        mtime: r.get(5)?,
        quick_hash: r.get(6)?,
        full_hash: r.get(7)?,
        asset_id: r.get(8)?,
    })
}

fn items(conn: &Connection, op_id: i64, state: &str) -> Result<Vec<Item>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {ITEM_COLUMNS} FROM file_op_items WHERE op_id = ?1 AND state = ?2 ORDER BY seq"
    ))?;
    let rows = stmt
        .query_map(params![op_id, state], read_item)?
        .collect::<rusqlite::Result<_>>()?;
    Ok(rows)
}

fn mtime_ms(md: &std::fs::Metadata) -> i64 {
    md.modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_millis() as i64)
}

/// Library folder paths, looked up once per operation. `None`: drive not connected.
struct Roots<'a> {
    db: &'a Db,
    known: HashMap<i64, Option<PathBuf>>,
}

impl<'a> Roots<'a> {
    fn new(db: &'a Db) -> Self {
        Roots {
            db,
            known: HashMap::new(),
        }
    }

    fn get(&mut self, root_id: i64) -> Result<Option<PathBuf>> {
        if let Some(path) = self.known.get(&root_id) {
            return Ok(path.clone());
        }
        let path = library::root_path(&self.db.read(), root_id)?;
        self.known.insert(root_id, path.clone());
        Ok(path)
    }
}

/// Moves files into the trash folder next to them. With `keep_a_copy`, refuses (before
/// touching anything) if that would leave a photo with no copy at all.
pub fn trash(db: &Db, file_ids: &[i64], reason: &str, keep_a_copy: bool) -> Result<Report> {
    let files: Vec<(i64, Option<String>)> = file_ids.iter().map(|&id| (id, None)).collect();
    let op_id = plan(&mut db.write(), &files, Kind::Trash, reason, keep_a_copy)?;
    run(db, op_id, Kind::Trash)
}

/// Moves files to new paths in their own library folder (`moves`: file id → new path
/// relative to the root). Folders left empty by the move are removed.
pub fn relocate(db: &Db, moves: &[(i64, String)], reason: &str) -> Result<Report> {
    let files: Vec<(i64, Option<String>)> = moves
        .iter()
        .map(|(id, to)| (*id, Some(to.clone())))
        .collect();
    let op_id = plan(&mut db.write(), &files, Kind::Move, reason, false)?;
    let report = run(db, op_id, Kind::Move)?;
    let mut roots = Roots::new(db);
    for item in items(&db.read(), op_id, "moved")? {
        if let Some(root) = roots.get(item.root_id)? {
            prune_up(&root, &item.rel_path);
        }
    }
    Ok(report)
}

/// Carries out a planned operation.
fn run(db: &Db, op_id: i64, kind: Kind) -> Result<Report> {
    let mut report = Report {
        op_id,
        ..Default::default()
    };
    let mut roots = Roots::new(db);
    let planned = items(&db.read(), op_id, "planned")?;
    for chunk in planned.chunks(BATCH) {
        // (seq, Ok(()) moved | Err(why))
        let mut results: Vec<(&Item, std::result::Result<(), String>)> = Vec::new();
        for item in chunk {
            let outcome = match roots.get(item.root_id)? {
                None => Err("its drive isn't connected".to_owned()),
                Some(root) => move_out(&root, item),
            };
            results.push((item, outcome));
        }
        let mut conn = db.write();
        let tx = conn.transaction()?;
        for (item, outcome) in results {
            match outcome {
                Ok(()) => {
                    moved(&tx, op_id, item, kind)?;
                    report.done += 1;
                    report.bytes += item.size;
                }
                Err(why) => {
                    set_state(&tx, op_id, item.seq, "failed", Some(&why))?;
                    report.fail(&item.rel_path, why);
                }
            }
        }
        tx.commit()?;
    }
    finish(&mut db.write(), op_id, "done")?;
    Ok(report)
}

/// Writes the journal: an operation row and one item per file, nothing moved yet.
/// `files`: file id and, for moves, its new path.
fn plan(
    conn: &mut Connection,
    files: &[(i64, Option<String>)],
    kind: Kind,
    reason: &str,
    keep_a_copy: bool,
) -> Result<i64> {
    let tx = conn.transaction()?;
    let mut rows: Vec<Item> = Vec::new();
    {
        let mut stmt = tx.prepare(
            "SELECT root_id, rel_path, size, mtime, quick_hash, full_hash, asset_id FROM files
             WHERE id = ?1 AND missing_since IS NULL",
        )?;
        let mut seen = HashSet::new();
        for (id, to) in files {
            if !seen.insert(*id) {
                continue;
            }
            let row = stmt
                .query_row(params![id], |r| {
                    Ok(Item {
                        seq: rows.len() as i64,
                        root_id: r.get(0)?,
                        rel_path: r.get(1)?,
                        trash_rel: to.clone().unwrap_or_default(),
                        size: r.get(2)?,
                        mtime: r.get(3)?,
                        quick_hash: r.get(4)?,
                        full_hash: r.get(5)?,
                        asset_id: r.get(6)?,
                    })
                })
                .optional()?;
            rows.extend(row);
        }
    }
    if rows.is_empty() {
        return Err(Error::Invalid(match kind {
            Kind::Trash => "Nothing to remove".into(),
            Kind::Move => "Nothing to move".into(),
        }));
    }
    if keep_a_copy {
        let mut leaving: HashMap<i64, i64> = HashMap::new();
        for asset in rows.iter().filter_map(|r| r.asset_id) {
            *leaving.entry(asset).or_default() += 1;
        }
        let mut present =
            tx.prepare("SELECT count(*) FROM files WHERE asset_id = ?1 AND missing_since IS NULL")?;
        for (asset, n) in leaving {
            let have: i64 = present.query_row(params![asset], |r| r.get(0))?;
            if have <= n {
                return Err(Error::Invalid(
                    "That would remove every copy of a photo, so nothing was removed".into(),
                ));
            }
        }
    }

    let plan = plan_json(kind, reason);
    tx.execute(
        "INSERT INTO file_ops (plan_json, status, created_at) VALUES (?1, 'running', ?2)",
        params![plan, now_ms()],
    )?;
    let op_id = tx.last_insert_rowid();
    {
        let mut insert = tx.prepare(&format!(
            "INSERT INTO file_op_items (op_id, {ITEM_COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)"
        ))?;
        for r in &mut rows {
            if kind == Kind::Trash {
                r.trash_rel = format!("{TRASH_DIR}/{op_id}/{}", trash_name(r.seq, &r.rel_path));
            }
            insert.execute(params![
                op_id,
                r.seq,
                r.root_id,
                r.rel_path,
                r.trash_rel,
                r.size,
                r.mtime,
                r.quick_hash,
                r.full_hash,
                r.asset_id
            ])?;
        }
    }
    tx.commit()?;
    Ok(op_id)
}

/// `{"kind":"trash","reason":"…"}` without pulling in a JSON library for one object.
fn plan_json(kind: Kind, reason: &str) -> String {
    let escaped: String = reason
        .chars()
        .filter(|c| !c.is_control())
        .flat_map(|c| match c {
            '"' | '\\' => vec!['\\', c],
            c => vec![c],
        })
        .collect();
    format!(
        "{{\"kind\":\"{}\",\"reason\":\"{escaped}\"}}",
        kind.as_str()
    )
}

/// Moves one file to its new place if it is still the file that was scanned.
fn move_out(root: &Path, item: &Item) -> std::result::Result<(), String> {
    let src = root.join(&item.rel_path);
    let dst = root.join(&item.trash_rel);
    let md = std::fs::metadata(&src).map_err(|e| format!("can't read it ({e})"))?;
    if !md.is_file() || md.len() as i64 != item.size || mtime_ms(&md) != item.mtime {
        return Err("it changed since the last scan; rescan the folder first".into());
    }
    if dst.exists() {
        return Err("a file already exists where it would go".into());
    }
    if let Some(parent) = dst.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("can't make its folder ({e})"))?;
    }
    std::fs::rename(&src, &dst).map_err(|e| format!("can't move it ({e})"))
}

/// Records a file as moved. Trashed: its `files` row goes (the journal keeps a copy),
/// and its asset leaves the timeline if that was its last copy. Moved: the row follows.
fn moved(tx: &Transaction, op_id: i64, item: &Item, kind: Kind) -> Result<()> {
    set_state(tx, op_id, item.seq, "moved", None)?;
    if kind == Kind::Move {
        tx.execute(
            "UPDATE files SET rel_path = ?3 WHERE root_id = ?1 AND rel_path = ?2",
            params![item.root_id, item.rel_path, item.trash_rel],
        )?;
        return Ok(());
    }
    tx.execute(
        "DELETE FROM files WHERE root_id = ?1 AND rel_path = ?2",
        params![item.root_id, item.rel_path],
    )?;
    if let Some(asset) = item.asset_id {
        tx.execute(
            "UPDATE assets SET status = 1 WHERE id = ?1
               AND NOT EXISTS (SELECT 1 FROM files WHERE asset_id = ?1 AND missing_since IS NULL)",
            params![asset],
        )?;
    }
    Ok(())
}

fn set_state(
    tx: &Transaction,
    op_id: i64,
    seq: i64,
    state: &str,
    error: Option<&str>,
) -> Result<()> {
    tx.execute(
        "UPDATE file_op_items SET state = ?3, error = ?4 WHERE op_id = ?1 AND seq = ?2",
        params![op_id, seq, state, error],
    )?;
    Ok(())
}

fn finish(conn: &mut Connection, op_id: i64, status: &str) -> Result<()> {
    conn.execute(
        "UPDATE file_ops SET status = ?2, completed_at = ?3 WHERE id = ?1",
        params![op_id, status, now_ms()],
    )?;
    // Assets may have left or come back to the timeline.
    crate::timeline::rebuild_day_buckets(conn)
}

/// Puts an operation's files back where they were, with their library data.
pub fn undo(db: &Db, op_id: i64) -> Result<Report> {
    require_status(&db.read(), op_id, &["done"])?;
    let kind = kind_of(&db.read(), op_id)?;
    db.write().execute(
        "UPDATE file_ops SET status = 'undoing' WHERE id = ?1",
        params![op_id],
    )?;
    let mut report = Report {
        op_id,
        ..Default::default()
    };
    let mut roots = Roots::new(db);
    for chunk in items(&db.read(), op_id, "moved")?.chunks(BATCH) {
        let mut results = Vec::new();
        for item in chunk {
            let outcome = match roots.get(item.root_id)? {
                None => Err("its drive isn't connected".to_owned()),
                Some(root) => move_back(&root, item),
            };
            results.push((item, outcome));
        }
        let mut conn = db.write();
        let tx = conn.transaction()?;
        for (item, outcome) in results {
            match outcome {
                Ok(()) => {
                    restored(&tx, op_id, item, kind)?;
                    report.done += 1;
                    report.bytes += item.size;
                }
                // Left in the trash folder; the operation can be undone again later.
                Err(why) => report.fail(&item.rel_path, why),
            }
        }
        tx.commit()?;
    }
    let left = count_state(&db.read(), op_id, "moved")?;
    finish(
        &mut db.write(),
        op_id,
        if left > 0 { "done" } else { "undone" },
    )?;
    match kind {
        Kind::Trash => {
            for folder in trash_folders(db, op_id)? {
                prune_empty(&folder);
            }
        }
        Kind::Move => {
            let mut roots = Roots::new(db);
            for item in items(&db.read(), op_id, "restored")? {
                if let Some(root) = roots.get(item.root_id)? {
                    prune_up(&root, &item.trash_rel);
                }
            }
        }
    }
    Ok(report)
}

fn move_back(root: &Path, item: &Item) -> std::result::Result<(), String> {
    let src = root.join(&item.trash_rel);
    let dst = root.join(&item.rel_path);
    if !src.is_file() {
        return Err("it's no longer where it was moved to".into());
    }
    if dst.exists() {
        return Err("another file now has its name".into());
    }
    if let Some(parent) = dst.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("can't recreate its folder ({e})"))?;
    }
    std::fs::rename(&src, &dst).map_err(|e| format!("can't move it back ({e})"))
}

/// Records a file as back: its `files` row returns as it was and its asset is present.
fn restored(tx: &Transaction, op_id: i64, item: &Item, kind: Kind) -> Result<()> {
    set_state(tx, op_id, item.seq, "restored", None)?;
    if kind == Kind::Move {
        tx.execute(
            "UPDATE files SET rel_path = ?2 WHERE root_id = ?1 AND rel_path = ?3",
            params![item.root_id, item.rel_path, item.trash_rel],
        )?;
        return Ok(());
    }
    tx.execute(
        "INSERT INTO files (root_id, rel_path, size, mtime, quick_hash, full_hash, asset_id)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
         ON CONFLICT (root_id, rel_path) DO UPDATE SET size = excluded.size,
             mtime = excluded.mtime, quick_hash = excluded.quick_hash,
             full_hash = excluded.full_hash, asset_id = excluded.asset_id, missing_since = NULL",
        params![
            item.root_id,
            item.rel_path,
            item.size,
            item.mtime,
            item.quick_hash,
            item.full_hash,
            item.asset_id
        ],
    )?;
    if let Some(asset) = item.asset_id {
        tx.execute("UPDATE assets SET status = 0 WHERE id = ?1", params![asset])?;
    }
    Ok(())
}

/// Sends an operation's files from the trash folder to the OS trash, freeing the
/// space. Drives without a real Recycle Bin are left alone, with the reason.
pub fn empty(db: &Db, op_id: i64) -> Result<Report> {
    require_status(&db.read(), op_id, &["done"])?;
    if kind_of(&db.read(), op_id)? != Kind::Trash {
        return Err(Error::Invalid("Only removed files can be emptied".into()));
    }
    let mut report = Report {
        op_id,
        ..Default::default()
    };
    let mut roots = Roots::new(db);
    let mut by_root: HashMap<i64, Vec<Item>> = HashMap::new();
    for item in items(&db.read(), op_id, "moved")? {
        by_root.entry(item.root_id).or_default().push(item);
    }
    for (root_id, group) in by_root {
        let Some(root) = roots.get(root_id)? else {
            report.fail(&group[0].rel_path, "its drive isn't connected");
            report.failed += group.len() - 1;
            continue;
        };
        if !volume::has_recycle_bin(&root) {
            let folder = root.join(TRASH_DIR).join(op_id.to_string());
            report.fail(
                &folder.to_string_lossy(),
                "this drive has no Recycle Bin, so the files were left here; delete this folder yourself to free the space",
            );
            report.failed += group.len() - 1;
            continue;
        }
        for chunk in group.chunks(BATCH) {
            let present: Vec<(&Item, PathBuf)> = chunk
                .iter()
                .map(|i| (i, root.join(&i.trash_rel)))
                .filter(|(_, p)| p.is_file())
                .collect();
            let paths: Vec<&PathBuf> = present.iter().map(|(_, p)| p).collect();
            let outcome = if paths.is_empty() {
                Ok(())
            } else {
                trash::delete_all(&paths).map_err(|e| e.to_string())
            };
            let mut conn = db.write();
            let tx = conn.transaction()?;
            for item in chunk {
                let gone = !root.join(&item.trash_rel).exists();
                if gone {
                    set_state(&tx, op_id, item.seq, "emptied", None)?;
                    report.done += 1;
                    report.bytes += item.size;
                } else if let Err(why) = &outcome {
                    report.fail(&item.rel_path, why);
                }
            }
            tx.commit()?;
        }
        prune_empty(&root.join(TRASH_DIR).join(op_id.to_string()));
        prune_empty(&root.join(TRASH_DIR));
    }
    let left = count_state(&db.read(), op_id, "moved")?;
    if left == 0 {
        finish(&mut db.write(), op_id, "emptied")?;
    }
    Ok(report)
}

/// Removes the folders above a moved file's old place that it left empty, up to (not
/// including) the library folder. Stops at the first folder that isn't empty.
fn prune_up(root: &Path, rel_file: &str) {
    let mut dir = Path::new(rel_file).parent();
    while let Some(d) = dir {
        if d.as_os_str().is_empty() || std::fs::remove_dir(root.join(d)).is_err() {
            break;
        }
        dir = d.parent();
    }
}

/// The trash folders an operation used (its own, then the shared parent), on drives
/// that are connected.
fn trash_folders(db: &Db, op_id: i64) -> Result<Vec<PathBuf>> {
    let root_ids: Vec<i64> = {
        let conn = db.read();
        let mut stmt =
            conn.prepare("SELECT DISTINCT root_id FROM file_op_items WHERE op_id = ?1")?;
        stmt.query_map(params![op_id], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?
    };
    let mut roots = Roots::new(db);
    let mut out = Vec::new();
    for id in root_ids {
        if let Some(root) = roots.get(id)? {
            out.push(root.join(TRASH_DIR).join(op_id.to_string()));
            out.push(root.join(TRASH_DIR));
        }
    }
    Ok(out)
}

/// Removes empty folders under and including `dir`, deepest first. Folders that still
/// hold anything are left alone, so this can never delete a file.
/// A removed file's name in the trash folder: its number in the operation (so names
/// never clash) and its own name, shortened to keep paths short.
fn trash_name(seq: i64, rel_path: &str) -> String {
    const MAX_CHARS: usize = 80;
    let name = rel_path.rsplit('/').next().unwrap_or(rel_path);
    if name.chars().count() <= MAX_CHARS {
        return format!("{seq} {name}");
    }
    let (stem, ext) = match name.rsplit_once('.') {
        Some((stem, ext)) if ext.len() <= 10 => (stem, format!(".{ext}")),
        _ => (name, String::new()),
    };
    let stem: String = stem.chars().take(MAX_CHARS - ext.chars().count()).collect();
    format!("{seq} {stem}{ext}")
}

fn prune_empty(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        if entry.file_type().is_ok_and(|t| t.is_dir()) {
            prune_empty(&entry.path());
        }
    }
    let _ = std::fs::remove_dir(dir);
}

fn count_state(conn: &Connection, op_id: i64, state: &str) -> Result<i64> {
    Ok(conn.query_row(
        "SELECT count(*) FROM file_op_items WHERE op_id = ?1 AND state = ?2",
        params![op_id, state],
        |r| r.get(0),
    )?)
}

fn require_status(conn: &Connection, op_id: i64, allowed: &[&str]) -> Result<()> {
    let status: Option<String> = conn
        .query_row(
            "SELECT status FROM file_ops WHERE id = ?1",
            params![op_id],
            |r| r.get(0),
        )
        .optional()?;
    match status {
        Some(s) if allowed.contains(&s.as_str()) => Ok(()),
        Some(_) => Err(Error::Invalid(
            "That can't be done to these files any more".into(),
        )),
        None => Err(Error::Invalid("That operation no longer exists".into())),
    }
}

/// Recent operations of one kind, newest first.
pub fn list(conn: &Connection, kind: Kind, limit: usize) -> Result<Vec<OpSummary>> {
    let mut stmt = conn.prepare_cached(
        "SELECT o.id, COALESCE(json_extract(o.plan_json, '$.reason'), ''), o.created_at, o.status,
                COALESCE(SUM(i.state = 'moved'), 0),
                COALESCE(SUM(CASE WHEN i.state = 'moved' THEN i.size END), 0),
                COALESCE(SUM(i.state = 'failed'), 0)
         FROM file_ops o LEFT JOIN file_op_items i ON i.op_id = o.id
         WHERE json_extract(o.plan_json, '$.kind') = ?2
         GROUP BY o.id ORDER BY o.id DESC LIMIT ?1",
    )?;
    let rows = stmt
        .query_map(params![limit as i64, kind.as_str()], |r| {
            Ok(OpSummary {
                id: r.get(0)?,
                reason: r.get(1)?,
                created_at: r.get(2)?,
                status: r.get(3)?,
                in_trash: r.get(4)?,
                in_trash_bytes: r.get(5)?,
                failed: r.get(6)?,
            })
        })?
        .collect::<rusqlite::Result<_>>()?;
    Ok(rows)
}

/// Finishes the bookkeeping of operations a crash interrupted: files that did move are
/// recorded as moved (or back), the rest as not done. Operations on drives that
/// aren't connected wait for a later start. Returns how many were settled.
pub fn recover(db: &Db) -> Result<usize> {
    let open: Vec<(i64, String)> = {
        let conn = db.read();
        let mut stmt =
            conn.prepare("SELECT id, status FROM file_ops WHERE status IN ('running', 'undoing')")?;
        stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?
    };
    let mut settled = 0;
    for (op_id, status) in open {
        let mut roots = Roots::new(db);
        let kind = kind_of(&db.read(), op_id)?;
        let undoing = status == "undoing";
        let pending = items(&db.read(), op_id, if undoing { "moved" } else { "planned" })?;
        let mut waiting = false;
        let mut conn = db.write();
        let tx = conn.transaction()?;
        for item in &pending {
            let Some(root) = roots.get(item.root_id)? else {
                waiting = true;
                continue;
            };
            let at_home = root.join(&item.rel_path).exists();
            let in_trash = root.join(&item.trash_rel).exists();
            match (undoing, at_home, in_trash) {
                (false, false, true) => moved(&tx, op_id, item, kind)?,
                (false, _, _) => set_state(&tx, op_id, item.seq, "failed", Some("interrupted"))?,
                (true, true, false) => restored(&tx, op_id, item, kind)?,
                (true, _, _) => {} // still where it was moved to: stays undoable
            }
        }
        tx.commit()?;
        if !waiting {
            let left = count_state(&conn, op_id, "moved")?;
            let status = if undoing && left == 0 {
                "undone"
            } else {
                "done"
            };
            finish(&mut conn, op_id, status)?;
            settled += 1;
        }
    }
    Ok(settled)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A library with one folder holding `names`, each a separate small file, and the
    /// first `copies` of them registered as copies of one asset.
    fn setup(names: &[&str], copies: usize) -> (tempfile::TempDir, Db, PathBuf, Vec<i64>) {
        let dir = tempfile::tempdir().unwrap();
        let photos = dir.path().join("photos");
        std::fs::create_dir_all(photos.join("sub")).unwrap();
        let db = Db::open(
            &dir.path().join("lib.sqlite"),
            2,
            crate::schema::migrate_library,
        )
        .unwrap();
        let root = library::add_root(&db.write(), &photos).unwrap();
        let conn = db.write();
        conn.execute(
            "INSERT INTO assets (media_kind, taken_at, taken_src, local_day, created_at)
             VALUES (0, 0, 0, 0, 0)",
            [],
        )
        .unwrap();
        let asset = conn.last_insert_rowid();
        let mut ids = Vec::new();
        for (i, name) in names.iter().enumerate() {
            let path = photos.join(name);
            std::fs::write(&path, b"same bytes").unwrap();
            let md = std::fs::metadata(&path).unwrap();
            conn.execute(
                "INSERT INTO files (root_id, rel_path, size, mtime, asset_id) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![root, name, md.len() as i64, mtime_ms(&md), (i < copies).then_some(asset)],
            )
            .unwrap();
            ids.push(conn.last_insert_rowid());
        }
        drop(conn);
        (dir, db, photos, ids)
    }

    fn file_count(db: &Db) -> i64 {
        db.read()
            .query_row("SELECT count(*) FROM files", [], |r| r.get(0))
            .unwrap()
    }

    #[test]
    fn trash_and_undo_round_trip() {
        let (_dir, db, photos, ids) = setup(&["a.jpg", "sub/a copy.jpg", "b.jpg"], 2);
        let report = trash(&db, &ids[1..2], "duplicates", true).unwrap();
        assert_eq!((report.done, report.failed), (1, 0));
        assert!(!photos.join("sub/a copy.jpg").exists());
        let folder = photos.join(TRASH_DIR).join(report.op_id.to_string());
        let kept: Vec<String> = std::fs::read_dir(&folder)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(kept.len(), 1);
        assert!(kept[0].ends_with(" a copy.jpg"), "flat, numbered: {kept:?}");
        assert_eq!(file_count(&db), 2);

        let ops = list(&db.read(), Kind::Trash, 10).unwrap();
        assert_eq!((ops[0].in_trash, ops[0].status.as_str()), (1, "done"));

        let back = undo(&db, report.op_id).unwrap();
        assert_eq!(back.done, 1);
        assert!(photos.join("sub/a copy.jpg").is_file());
        assert!(
            !photos.join(TRASH_DIR).exists(),
            "empty trash folders are tidied away"
        );
        assert_eq!(file_count(&db), 3);
        assert_eq!(
            list(&db.read(), Kind::Trash, 10).unwrap()[0].status,
            "undone"
        );
        assert!(undo(&db, report.op_id).is_err(), "only once");
    }

    #[test]
    fn moves_round_trip_and_tidy_empty_folders() {
        let (_dir, db, photos, ids) = setup(&["sub/a.jpg", "b.jpg"], 0);
        let moves = vec![(ids[0], "2023/2023-06/a.jpg".to_owned())];
        let report = relocate(&db, &moves, "organize").unwrap();
        assert_eq!((report.done, report.failed), (1, 0));
        assert!(photos.join("2023/2023-06/a.jpg").is_file());
        assert!(!photos.join("sub").exists(), "the emptied folder is gone");
        let path: String = db
            .read()
            .query_row(
                "SELECT rel_path FROM files WHERE id = ?1",
                params![ids[0]],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            path, "2023/2023-06/a.jpg",
            "the library row follows the file"
        );
        assert_eq!(list(&db.read(), Kind::Move, 10).unwrap()[0].in_trash, 1);
        assert!(list(&db.read(), Kind::Trash, 10).unwrap().is_empty());
        assert!(empty(&db, report.op_id).is_err(), "moves can't be emptied");

        undo(&db, report.op_id).unwrap();
        assert!(photos.join("sub/a.jpg").is_file());
        assert!(!photos.join("2023").exists());
        let path: String = db
            .read()
            .query_row(
                "SELECT rel_path FROM files WHERE id = ?1",
                params![ids[0]],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(path, "sub/a.jpg");
        assert_eq!(file_count(&db), 2);
    }

    #[test]
    fn never_removes_the_last_copy() {
        let (_dir, db, photos, ids) = setup(&["a.jpg", "a2.jpg"], 2);
        assert!(trash(&db, &ids, "duplicates", true).is_err());
        assert!(photos.join("a.jpg").is_file() && photos.join("a2.jpg").is_file());
        assert!(
            list(&db.read(), Kind::Trash, 10).unwrap().is_empty(),
            "nothing journaled"
        );
    }

    #[test]
    fn skips_files_that_changed_since_the_scan() {
        let (_dir, db, photos, ids) = setup(&["a.jpg", "a2.jpg"], 2);
        std::fs::write(photos.join("a2.jpg"), b"edited since").unwrap();
        let report = trash(&db, &ids[1..], "duplicates", true).unwrap();
        assert_eq!((report.done, report.failed), (0, 1));
        assert!(photos.join("a2.jpg").is_file());
        assert_eq!(file_count(&db), 2);
    }

    #[test]
    fn recovers_an_interrupted_operation() {
        let (_dir, db, photos, ids) = setup(&["a.jpg", "a2.jpg", "a3.jpg"], 3);
        let files: Vec<(i64, Option<String>)> = ids[1..].iter().map(|&id| (id, None)).collect();
        let op = plan(&mut db.write(), &files, Kind::Trash, "duplicates", true).unwrap();
        // The app died after moving the first file but before recording it.
        let first = &items(&db.read(), op, "planned").unwrap()[0];
        move_out(&photos, first).unwrap();
        assert_eq!(recover(&db).unwrap(), 1);
        let ops = list(&db.read(), Kind::Trash, 10).unwrap();
        assert_eq!(
            (ops[0].status.as_str(), ops[0].in_trash, ops[0].failed),
            ("done", 1, 1)
        );
        assert_eq!(file_count(&db), 2, "the moved file's row is gone");
        assert!(
            photos.join("a3.jpg").is_file(),
            "the unmoved one is untouched"
        );
    }
}
