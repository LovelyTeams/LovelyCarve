"""Build SQLite databases with known deletions for deleted-record recovery tests.

Usage: make_corpus.py <out-dir>

Each scenario writes <name>.db (plus a -wal / -journal sidecar where the
scenario needs one) and <name>.truth.json:
  {"tables": {"<table>": {"live": [[...]], "deleted": [[...]], "prior": [[...]]}}}
`deleted` rows were removed; `prior` rows are pre-UPDATE versions of live rows.
"""
import json
import os
import random
import shutil
import sqlite3
import sys

OUT = sys.argv[1]
os.makedirs(OUT, exist_ok=True)
WORDS = "alpha bravo charlie delta echo foxtrot golf hotel india juliet kilo lima mike november oscar papa".split()


def text(rng, n):
    return " ".join(rng.choice(WORDS) for _ in range(n))


def rows_of(con, table):
    return [list(r) for r in con.execute(f"SELECT * FROM {table} ORDER BY rowid")]


def save_truth(name, truth):
    with open(os.path.join(OUT, f"{name}.truth.json"), "w", encoding="utf-8") as f:
        json.dump({"tables": truth}, f, ensure_ascii=False)


def fresh(name, **pragmas):
    path = os.path.join(OUT, f"{name}.db")
    for suffix in ("", "-wal", "-journal", "-shm"):
        if os.path.exists(path + suffix):
            os.remove(path + suffix)
    con = sqlite3.connect(path, isolation_level=None)
    for key, value in pragmas.items():
        con.execute(f"PRAGMA {key}={value}")
    return path, con


def messages_table(con, rng, count, body_words=(4, 12)):
    con.execute(
        "CREATE TABLE messages (id INTEGER PRIMARY KEY, sender TEXT NOT NULL, body TEXT, ts INTEGER, amount REAL)"
    )
    con.execute("BEGIN")
    for i in range(1, count + 1):
        con.execute(
            "INSERT INTO messages VALUES (?, ?, ?, ?, ?)",
            (i, f"user{rng.randint(1, 40):02d}", f"msg{i}: " + text(rng, rng.randint(*body_words)),
             1_600_000_000 + i * 37, round(rng.uniform(1, 999), 2)),
        )
    con.execute("COMMIT")


def delete_some(con, rng, table, fraction):
    ids = [r[0] for r in con.execute(f"SELECT rowid FROM {table}")]
    victims = sorted(rng.sample(ids, int(len(ids) * fraction)))
    deleted = [list(r) for r in con.execute(
        f"SELECT * FROM {table} WHERE rowid IN ({','.join(map(str, victims))}) ORDER BY rowid")]
    con.execute("BEGIN")
    con.executemany(f"DELETE FROM {table} WHERE rowid = ?", [(v,) for v in victims])
    con.execute("COMMIT")
    return deleted


def scenario_basic(rng):
    path, con = fresh("basic_delete", secure_delete="OFF", journal_mode="DELETE")
    messages_table(con, rng, 600)
    deleted = delete_some(con, rng, "messages", 0.3)
    save_truth("basic_delete", {"messages": {"live": rows_of(con, "messages"), "deleted": deleted, "prior": []}})
    con.close()


def scenario_update(rng):
    path, con = fresh("updates", secure_delete="OFF")
    messages_table(con, rng, 300)
    ids = rng.sample(range(1, 301), 60)
    prior = [list(r) for r in con.execute(
        f"SELECT * FROM messages WHERE id IN ({','.join(map(str, ids))}) ORDER BY id")]
    con.execute("BEGIN")
    for i in ids:
        con.execute("UPDATE messages SET body = ?, amount = amount + 1000 WHERE id = ?",
                    (f"edited{i}: " + text(rng, 3), i))
    con.execute("COMMIT")
    deleted = delete_some(con, rng, "messages", 0.1)
    save_truth("updates", {"messages": {"live": rows_of(con, "messages"), "deleted": deleted, "prior": prior}})
    con.close()


def scenario_drop(rng):
    path, con = fresh("drop_table", secure_delete="OFF")
    messages_table(con, rng, 200)
    con.execute("CREATE TABLE contacts (cid INTEGER PRIMARY KEY, name TEXT, phone TEXT)")
    con.execute("BEGIN")
    for i in range(1, 151):
        con.execute("INSERT INTO contacts VALUES (?, ?, ?)", (i, f"contact {text(rng, 2)} {i}", f"+86-139{i:08d}"))
    con.execute("COMMIT")
    contacts = rows_of(con, "contacts")
    con.execute("DROP TABLE contacts")
    deleted = delete_some(con, rng, "messages", 0.2)
    save_truth("drop_table", {
        "messages": {"live": rows_of(con, "messages"), "deleted": deleted, "prior": []},
        "contacts": {"live": [], "deleted": contacts, "prior": []},
    })
    con.close()


def scenario_overflow(rng):
    path, con = fresh("overflow", secure_delete="OFF", page_size=1024)
    messages_table(con, rng, 120, body_words=(150, 400))
    deleted = delete_some(con, rng, "messages", 0.25)
    save_truth("overflow", {"messages": {"live": rows_of(con, "messages"), "deleted": deleted, "prior": []}})
    con.close()


def scenario_utf16(rng):
    path, con = fresh("utf16", secure_delete="OFF", encoding='"UTF-16le"')
    messages_table(con, rng, 300)
    deleted = delete_some(con, rng, "messages", 0.3)
    save_truth("utf16", {"messages": {"live": rows_of(con, "messages"), "deleted": deleted, "prior": []}})
    con.close()


def scenario_secure_delete(rng):
    path, con = fresh("secure_delete", secure_delete="ON")
    messages_table(con, rng, 300)
    deleted = delete_some(con, rng, "messages", 0.3)
    save_truth("secure_delete", {"messages": {"live": rows_of(con, "messages"), "deleted": deleted, "prior": []}})
    con.close()


def scenario_autovacuum(rng):
    path, con = fresh("autovacuum", secure_delete="OFF", auto_vacuum="FULL")
    messages_table(con, rng, 400)
    deleted = delete_some(con, rng, "messages", 0.4)
    save_truth("autovacuum", {"messages": {"live": rows_of(con, "messages"), "deleted": deleted, "prior": []}})
    con.close()


def scenario_wal(rng):
    path, con = fresh("wal", secure_delete="OFF", journal_mode="WAL", wal_autocheckpoint=0)
    messages_table(con, rng, 400)
    con.execute("PRAGMA wal_checkpoint(TRUNCATE)")
    deleted = delete_some(con, rng, "messages", 0.25)
    live = rows_of(con, "messages")
    # Copy while the connection is open so the -wal is not checkpointed away.
    snap = os.path.join(OUT, "wal_snapshot.db")
    for suffix in ("", "-wal"):
        if os.path.exists(snap + suffix):
            os.remove(snap + suffix)
    shutil.copyfile(path, snap)
    shutil.copyfile(path + "-wal", snap + "-wal")
    con.close()
    for suffix in ("", "-wal", "-shm"):
        if os.path.exists(path + suffix):
            os.remove(path + suffix)
    save_truth("wal_snapshot", {"messages": {"live": live, "deleted": deleted, "prior": []}})


def scenario_persist_journal(rng):
    path, con = fresh("persist", secure_delete="OFF", journal_mode="PERSIST")
    messages_table(con, rng, 300)
    deleted = delete_some(con, rng, "messages", 0.15)
    save_truth("persist", {"messages": {"live": rows_of(con, "messages"), "deleted": deleted, "prior": []}})
    con.close()


def main():
    rng = random.Random(20261003)
    for scenario in (scenario_basic, scenario_update, scenario_drop, scenario_overflow, scenario_utf16,
                     scenario_secure_delete, scenario_autovacuum, scenario_wal, scenario_persist_journal):
        scenario(rng)
    for name in sorted(os.listdir(OUT)):
        print(f"{os.path.getsize(os.path.join(OUT, name)):>9} {name}")


if __name__ == "__main__":
    main()
