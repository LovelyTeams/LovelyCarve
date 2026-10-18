//! connection implementation.
use super::*;

/// Open a VFS-backed preview session read-only, transparently applying
/// SQLCipher key/params when the file is not a plaintext SQLite database.
/// Returns the connection plus whether the database was encrypted.
///
/// Detection is by header: a plaintext database starts with
/// `"SQLite format 3\0"`; an encrypted one does not (its header is
/// ciphertext). For an encrypted file we apply `key` then run a cheap
/// schema probe — a wrong or missing key trips `SQLITE_NOTADB` here, and
/// we surface it as a `SQLITE_LOCKED:` error the frontend recognises to
/// prompt for a key (rather than a confusing "file is not a database").
pub(super) fn open_session_conn(
    session: &SqliteSession,
    key: &SqliteKey,
) -> anyhow::Result<(Connection, bool)> {
    open_conn_with_head(session.head16, key, || {
        sqlite_vfs::open_readonly(&session.main_name)
    })
}

#[cfg(test)]
pub(super) fn open_path_conn(path: &Path, key: &SqliteKey) -> anyhow::Result<(Connection, bool)> {
    open_conn_with_head(read_head16(path), key, || {
        Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_ONLY
                | OpenFlags::SQLITE_OPEN_NO_MUTEX
                | OpenFlags::SQLITE_OPEN_URI,
        )
    })
}

pub(super) fn open_conn_with_head<F>(
    head: Option<[u8; 16]>,
    key: &SqliteKey,
    open: F,
) -> anyhow::Result<(Connection, bool)>
where
    F: FnOnce() -> rusqlite::Result<Connection>,
{
    // Too short to be any SQLite/SQLCipher database → "not a database",
    // which is a different (and more honest) message than "encrypted,
    // provide a key". Catches 0-byte / truncated fragments.
    let Some(head) = head else {
        anyhow::bail!("不是 SQLite 数据库（文件过小或不可读）");
    };
    let plain_magic = looks_like_sqlite(&head);

    let conn = open()?;

    // Apply a key when the header isn't plaintext magic, OR when the
    // caller supplied key material anyway — the latter covers
    // `cipher_plaintext_header_size` databases, whose page-1 header (incl.
    // the magic) stays cleartext while the page bodies are encrypted.
    let attempted_decrypt = !plain_magic || key.has_material();
    if attempted_decrypt {
        apply_key(&conn, key)?;
    }

    // Touch the schema so a bad key fails here, deterministically, rather
    // than on the first table read deeper in the call.
    if conn
        .query_row("SELECT count(*) FROM sqlite_master", [], |_| Ok(()))
        .is_err()
    {
        if plain_magic && !attempted_decrypt {
            // Plaintext header but no key tried: genuinely corrupt, or a
            // plaintext-header encrypted DB awaiting a key. Offer the key
            // prompt either way, with an honest hint. (Note: we never
            // include the underlying SQLite error text — on the key paths
            // it can embed the PRAGMA statement, hence the raw key.)
            anyhow::bail!(
                "SQLITE_LOCKED: 无法读取数据库架构（可能已损坏，或为保留明文头的加密库，请尝试提供密钥）"
            );
        }
        let detail = if key.has_material() {
            "密钥或兼容参数不正确"
        } else {
            "数据库已加密，请提供密钥"
        };
        anyhow::bail!("SQLITE_LOCKED: {detail}");
    }
    Ok((conn, attempted_decrypt))
}

/// Apply key material and cipher parameters to a freshly opened
/// connection. Order matters: `key` first, then `cipher_compatibility`
/// (which resets the parameter set for a SQLCipher major version),
/// then the individual `cipher_page_size` / `kdf_iter` overrides.
///
/// Errors from the key-bearing PRAGMAs are deliberately *replaced* with a
/// generic message (never chained): a raw `rusqlite` error can embed the
/// offending SQL statement — i.e. the `x'…'` raw key — which must not
/// reach the UI or logs.
pub(super) fn apply_key(conn: &Connection, key: &SqliteKey) -> anyhow::Result<()> {
    if let Some(raw) = key
        .raw_hex_key
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        // Raw 256-bit key (optionally key+salt). Must be applied as a
        // hex blob literal `x'…'`, not a quoted string, so we build the
        // PRAGMA by hand — after validating the hex to keep it injection-safe.
        let cleaned: String = raw
            .strip_prefix("0x")
            .unwrap_or(raw)
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect();
        if cleaned.is_empty()
            || !cleaned.len().is_multiple_of(2)
            || !cleaned.bytes().all(|b| b.is_ascii_hexdigit())
        {
            anyhow::bail!(
                "原始密钥必须是偶数长度的十六进制字符串（例如 32 字节为 64 个十六进制字符）"
            );
        }
        conn.execute_batch(&format!("PRAGMA key = \"x'{cleaned}'\";"))
            .map_err(|_| anyhow::anyhow!("应用原始密钥失败（格式或长度不被接受）"))?;
    } else if let Some(pass) = key.passphrase.as_deref().filter(|s| !s.is_empty()) {
        // rusqlite quotes/escapes the value as a string literal — the
        // exact form SQLCipher expects for a passphrase. Drop the source
        // error too: pragma_update inlines the value into the statement.
        conn.pragma_update(None, "key", pass)
            .map_err(|_| anyhow::anyhow!("应用口令失败"))?;
    }

    if let Some(compat) = key.compatibility {
        if (1..=4).contains(&compat) {
            conn.execute_batch(&format!("PRAGMA cipher_compatibility = {compat};"))?;
        }
    }
    if let Some(page_size) = key.page_size {
        conn.execute_batch(&format!("PRAGMA cipher_page_size = {page_size};"))?;
    }
    if let Some(kdf_iter) = key.kdf_iter {
        conn.execute_batch(&format!("PRAGMA kdf_iter = {kdf_iter};"))?;
    }
    Ok(())
}

/// Read the first 16 bytes of `path`, or `None` if the file is shorter
/// than that (or unreadable). `Some` + [`looks_like_sqlite`] ⇒ plaintext
/// SQLite header; `None` ⇒ too small to be any database.
#[cfg(test)]
pub(super) fn read_head16(path: &Path) -> Option<[u8; 16]> {
    let mut buf = [0u8; 16];
    File::open(path)
        .and_then(|mut f| f.read_exact(&mut buf))
        .ok()?;
    Some(buf)
}
