use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

use rusqlite::{Connection, OpenFlags};

use crate::Result;

/// A SQLite database in WAL mode with one writer connection and a small pool of
/// read-only connections. Readers never block the writer and vice versa.
pub struct Db {
    writer: Mutex<Connection>,
    readers: Vec<Mutex<Connection>>,
    next: AtomicUsize,
}

impl Db {
    pub fn open(
        path: &Path,
        readers: usize,
        migrate: fn(&mut Connection) -> Result<()>,
    ) -> Result<Self> {
        let mut writer = Connection::open(path)?;
        configure(&writer)?;
        writer.query_row("PRAGMA journal_mode = WAL", [], |_| Ok(()))?;
        migrate(&mut writer)?;

        let readers = (0..readers.max(1))
            .map(|_| {
                let conn = Connection::open_with_flags(
                    path,
                    OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
                )?;
                configure(&conn)?;
                Ok(Mutex::new(conn))
            })
            .collect::<Result<Vec<_>>>()?;

        Ok(Self {
            writer: Mutex::new(writer),
            readers,
            next: AtomicUsize::new(0),
        })
    }

    pub fn write(&self) -> MutexGuard<'_, Connection> {
        self.writer.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Returns an idle reader if one is free, otherwise waits on one round-robin.
    pub fn read(&self) -> MutexGuard<'_, Connection> {
        let n = self.readers.len();
        let start = self.next.fetch_add(1, Ordering::Relaxed);
        for i in 0..n {
            if let Ok(guard) = self.readers[(start + i) % n].try_lock() {
                return guard;
            }
        }
        self.readers[start % n]
            .lock()
            .unwrap_or_else(|e| e.into_inner())
    }
}

fn configure(conn: &Connection) -> Result<()> {
    conn.busy_timeout(Duration::from_secs(5))?;
    conn.execute_batch(
        "PRAGMA synchronous = NORMAL;
         PRAGMA foreign_keys = ON;
         PRAGMA temp_store = MEMORY;
         PRAGMA cache_size = -32000;
         PRAGMA mmap_size = 268435456;
         PRAGMA journal_size_limit = 67108864;",
    )?;
    Ok(())
}
