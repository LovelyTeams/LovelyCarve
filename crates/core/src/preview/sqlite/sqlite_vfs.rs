//! Process-local SQLite VFS used by the native SQLite preview.
//!
//! The preview opens image-contained databases through an in-memory SQLite VFS
//! instead of writing them to a filesystem tempdir. The VFS is registered under
//! an explicit name and is not installed as SQLite's default, so the rest of the
//! app's normal `rusqlite` databases keep using the platform VFS.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::Context;
use rsqlite_vfs::memvfs::MemVfsUtil;
use rsqlite_vfs::transfer::DbTransfer;
use rsqlite_vfs::{OsCallback, VfsFilesManager, VfsResult};
use rusqlite::config::DbConfig;
use rusqlite::{Connection, OpenFlags};

pub const VFS_NAME: &str = "memvfs";

struct LovelyCarveSqliteOs;

impl OsCallback for LovelyCarveSqliteOs {
    fn sleep(&self, duration: Duration) {
        std::thread::sleep(duration);
    }

    fn random(&self, buffer: &mut [u8]) -> usize {
        let mut written = 0;
        while written < buffer.len() {
            let uuid = uuid::Uuid::new_v4();
            let bytes = uuid.as_bytes();
            let take = (buffer.len() - written).min(bytes.len());
            buffer[written..written + take].copy_from_slice(&bytes[..take]);
            written += take;
        }
        buffer.len()
    }

    fn epoch_timestamp_in_ms(&self) -> VfsResult<i64> {
        Ok(SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_millis().min(i64::MAX as u128) as i64)
            .unwrap_or(0))
    }
}

fn sqlite_vfs() -> anyhow::Result<MemVfsUtil> {
    // LovelyCarve's vendored VFS serializes registration and protects shared
    // file data, so a handle may be acquired from any preview worker.
    unsafe { rsqlite_vfs::memvfs::install(LovelyCarveSqliteOs, false) }
        .context("register SQLite preview VFS")
}

pub fn import_file(name: &str, bytes: &[u8]) -> anyhow::Result<()> {
    sqlite_vfs()?
        .import_db_unchecked(name, bytes)
        .with_context(|| format!("import {name} into SQLite preview VFS"))
}

pub fn delete_file(name: &str) {
    match sqlite_vfs().and_then(|vfs| vfs.remove(name).map_err(Into::into)) {
        Ok(_) => {}
        Err(error) => tracing::warn!(%error, %name, "remove SQLite preview VFS file"),
    }
}

pub fn exists(name: &str) -> bool {
    match sqlite_vfs().and_then(|vfs| vfs.contains(name).map_err(Into::into)) {
        Ok(exists) => exists,
        Err(error) => {
            tracing::warn!(%error, %name, "check SQLite preview VFS file");
            false
        }
    }
}

/// Open `name` read-only.
///
/// The VFS has no shared-memory methods, so SQLite runs WAL mode only with a
/// heap wal-index, which needs exclusive locking. The VFS locks are no-ops,
/// so this blocks no other preview connection. It lets a database whose
/// header says WAL mode open at all, and lets SQLite replay an imported
/// `<name>-wal` itself, checksums and SQLCipher frames included. Closing
/// never checkpoints, so the imported main file is not rewritten under
/// another connection still reading the WAL.
pub fn open_readonly(name: &str) -> rusqlite::Result<Connection> {
    let connection = Connection::open_with_flags_and_vfs(
        name,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        VFS_NAME,
    )?;
    connection.set_db_config(DbConfig::SQLITE_DBCONFIG_NO_CKPT_ON_CLOSE, true)?;
    connection.execute_batch("PRAGMA locking_mode = EXCLUSIVE;")?;
    Ok(connection)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One VFS file table serves every preview connection, from whichever
    /// thread runs the request.
    #[test]
    fn concurrent_sessions_share_the_vfs() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("seed.sqlite");
        let seed = Connection::open(&path).unwrap();
        seed.execute_batch("CREATE TABLE t(v); INSERT INTO t VALUES (1), (2), (3);")
            .unwrap();
        drop(seed);
        let bytes = std::fs::read(&path).unwrap();
        std::thread::scope(|scope| {
            for worker in 0..8 {
                let bytes = &bytes;
                scope.spawn(move || {
                    for round in 0..25 {
                        let name = format!(
                            "lovelycarve-vfs-test-{worker}-{round}-{}.sqlite",
                            uuid::Uuid::new_v4().simple()
                        );
                        import_file(&name, bytes).unwrap();
                        let connection = open_readonly(&name).unwrap();
                        let rows: i64 = connection
                            .query_row("SELECT count(*) FROM t", [], |row| row.get(0))
                            .unwrap();
                        assert_eq!(rows, 3);
                        drop(connection);
                        delete_file(&name);
                        delete_file(&format!("{name}-wal"));
                        assert!(!exists(&name));
                    }
                });
            }
        });
    }
}
