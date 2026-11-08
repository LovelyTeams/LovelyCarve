#!/usr/bin/env python3
"""Generate the drop-recreate residue fixtures for the AUTOINCREMENT high-water
diagnostic (Detector A), per `docs/design/drop-recreate-attribution.md`.

These are SYNTHETIC, built with the real sqlite3 engine via Python's stdlib
`sqlite3` module. They exercise the `table_instance_risk` HINT flag, which fires
only when a residue record attributed to an AUTOINCREMENT table has a
`rowid > sqlite_sequence` high-water mark — and which the design deliberately
frames as a hint, NOT a per-row predecessor assertion.

Run from anywhere:  python3 gen.py
Writes b_autoinc.db, b_plainpk.db, upd_autoinc.db beside this script.
Deterministic: re-running reproduces byte-identical files on the same engine.
"""

import os
import sqlite3

HERE = os.path.dirname(os.path.abspath(__file__))


def _fresh(path):
    """Remove a db and any sidecars so each run starts clean."""
    for suffix in ("", "-wal", "-shm", "-journal"):
        p = path + suffix
        if os.path.exists(p):
            os.remove(p)


def _connect(path):
    _fresh(path)
    con = sqlite3.connect(path)
    con.execute("PRAGMA page_size=4096")
    con.execute("PRAGMA auto_vacuum=NONE")
    con.execute("PRAGMA secure_delete=OFF")
    return con


def gen_b_autoinc():
    """`b_autoinc.db` — AUTOINCREMENT drop-recreate (Detector A FIRES).

    AUTOINCREMENT `students`: INSERT 10, DROP, CREATE same schema, INSERT 5.
    sqlite_sequence is RESET to 5 on the recreate (the DROP deleted its row).
    Residue rowids 6..10 survive in the reused pages and exceed seq=5, so the
    flag fires on the recovered 6..10. Ground truth: live ids 1..5 (NEW), residue
    ids 6..10 (OLD) genuinely predate the current instance.
    """
    path = os.path.join(HERE, "b_autoinc.db")
    con = _connect(path)
    schema = (
        "CREATE TABLE students("
        "id INTEGER PRIMARY KEY AUTOINCREMENT, name TEXT, affil TEXT)"
    )
    con.execute(schema)
    con.executemany(
        "INSERT INTO students(name, affil) VALUES (?, ?)",
        [("OLD-NAME-%d" % i, "OLD-AFFIL-%d" % i) for i in range(1, 11)],
    )
    con.commit()
    con.execute("DROP TABLE students")
    con.commit()
    con.execute(schema)
    con.executemany(
        "INSERT INTO students(name, affil) VALUES (?, ?)",
        [("NEW-NAME-%d" % i, "NEW-AFFIL-%d" % i) for i in range(1, 6)],
    )
    con.commit()
    con.close()
    return path


def gen_b_plainpk():
    """`b_plainpk.db` — plain INTEGER PRIMARY KEY drop-recreate (flag NEVER fires).

    Identical construction to b_autoinc but with a plain `INTEGER PRIMARY KEY`
    (no AUTOINCREMENT). No sqlite_sequence row exists, so Detector A has no
    high-water mark to compare against and the flag never fires — the honest
    limit the design documents (the survey's exact undecidable 0B case).
    Ground truth: live ids 1..5 (NEW), residue ids 6..10 (OLD).
    """
    path = os.path.join(HERE, "b_plainpk.db")
    con = _connect(path)
    schema = "CREATE TABLE students(id INTEGER PRIMARY KEY, name TEXT, affil TEXT)"
    con.execute(schema)
    con.executemany(
        "INSERT INTO students(id, name, affil) VALUES (?, ?, ?)",
        [(i, "OLD-NAME-%d" % i, "OLD-AFFIL-%d" % i) for i in range(1, 11)],
    )
    con.commit()
    con.execute("DROP TABLE students")
    con.commit()
    con.execute(schema)
    con.executemany(
        "INSERT INTO students(id, name, affil) VALUES (?, ?, ?)",
        [(i, "NEW-NAME-%d" % i, "NEW-AFFIL-%d" % i) for i in range(1, 6)],
    )
    con.commit()
    con.close()
    return path


def gen_upd_autoinc():
    """`upd_autoinc.db` — Codex BLOCKER-1 case (flag FIRES on a CURRENT row).

    AUTOINCREMENT `t`: INSERT one row (id=5), UPDATE its id to 1000, then DELETE
    it. sqlite_sequence stays at 5 (UPDATE of the rowid does not advance the
    INSERT high-water mark), so the residue rowid 1000 > seq 5 trips the flag —
    yet the row was a CURRENT-instance row that an UPDATE moved, NOT a dropped
    predecessor. This fixture is the proof the flag is a HINT, not an assertion:
    `r > seq` is reachable without any drop-recreate.
    Ground truth: live 0, residue rowid 1000 (a moved-then-deleted current row).
    """
    path = os.path.join(HERE, "upd_autoinc.db")
    con = _connect(path)
    con.execute("CREATE TABLE t(id INTEGER PRIMARY KEY AUTOINCREMENT, v TEXT)")
    con.execute("INSERT INTO t(id, v) VALUES (5, 'MOVED-ROW')")
    con.commit()
    con.execute("UPDATE t SET id=1000 WHERE id=5")
    con.commit()
    con.execute("DELETE FROM t WHERE id=1000")
    con.commit()
    con.close()
    return path


def _connect_persist(path):
    """Like `_connect` but in PERSIST journal mode, so the rollback `-journal`
    SURVIVES the commit (header zeroed, page images intact) — the prior schema
    the Detector-B sidecar read recovers."""
    con = _connect(path)
    con.execute("PRAGMA journal_mode=PERSIST")
    return con


def gen_b_journal_altered():
    """`b_journal_altered.db` + `-journal` — Detector B FIRES (sidecar schema change).

    `students` (id, name): INSERT 10, DELETE id>=4 (leaving recoverable residue in
    its pages), then — as the LAST, journaled transaction — `ALTER TABLE students
    ADD COLUMN extra`. The PERSIST `-journal` preserves the prior page-1 schema,
    whose CREATE SQL has NO `extra` column while the current schema does, so the
    prior-vs-current CREATE SQL differ → Detector B fires for `students` on the
    residue attributed to it. Ground truth: an unambiguous table-level schema
    change captured in the sidecar (a CREATE/ALTER within the window).
    """
    path = os.path.join(HERE, "b_journal_altered.db")
    con = _connect_persist(path)
    con.execute("CREATE TABLE students(id INTEGER PRIMARY KEY, name TEXT)")
    con.executemany(
        "INSERT INTO students(id, name) VALUES (?, ?)",
        [(i, "DEL-NAME-%d" % i) for i in range(1, 11)],
    )
    con.commit()
    con.execute("DELETE FROM students WHERE id >= 4")
    con.commit()
    con.execute("ALTER TABLE students ADD COLUMN extra TEXT")
    con.commit()
    con.close()
    return path


def gen_b_journal_dml():
    """`b_journal_dml.db` + `-journal` — Detector B does NOT fire (anti-FP case).

    Identical setup to `b_journal_altered` through the DELETE, but the LAST
    (journaled) transaction is DML only — `INSERT INTO students VALUES(99,...)`.
    The PERSIST `-journal`'s prior page-1 carries the SAME CREATE SQL as current,
    so Detector B stays silent — the false-predecessor hint the design refuses to
    raise on a no-schema-change transaction. Ground truth: a DML-only boundary,
    not a schema change.
    """
    path = os.path.join(HERE, "b_journal_dml.db")
    con = _connect_persist(path)
    con.execute("CREATE TABLE students(id INTEGER PRIMARY KEY, name TEXT)")
    con.executemany(
        "INSERT INTO students(id, name) VALUES (?, ?)",
        [(i, "DEL-NAME-%d" % i) for i in range(1, 11)],
    )
    con.commit()
    con.execute("DELETE FROM students WHERE id >= 4")
    con.commit()
    con.execute("INSERT INTO students(id, name) VALUES (99, 'LATER')")
    con.commit()
    con.close()
    return path


def md5(path):
    import hashlib

    h = hashlib.md5()
    with open(path, "rb") as fh:
        for chunk in iter(lambda: fh.read(65536), b""):
            h.update(chunk)
    return h.hexdigest()


def main():
    a = gen_b_autoinc()
    p = gen_b_plainpk()
    u = gen_upd_autoinc()
    alt = gen_b_journal_altered()
    dml = gen_b_journal_dml()
    files = [
        ("b_autoinc.db", a),
        ("b_plainpk.db", p),
        ("upd_autoinc.db", u),
        ("b_journal_altered.db", alt),
        ("b_journal_dml.db", dml),
    ]
    # The `-journal` sidecars (Detector B) carry their own provenance line; their
    # md5 varies per run (the journal embeds a random checksum nonce), so they are
    # listed for completeness but the `.db` files are the deterministic anchors.
    for label, path in files:
        print("%-24s %s  %d bytes" % (label, md5(path), os.path.getsize(path)))
        jrnl = path + "-journal"
        if os.path.exists(jrnl):
            print(
                "%-24s %s  %d bytes"
                % (os.path.basename(jrnl), md5(jrnl), os.path.getsize(jrnl))
            )


if __name__ == "__main__":
    main()
