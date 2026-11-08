#!/usr/bin/env python3
"""Mint a small, spec-conformant iOS backup as a test fixture.

This is the **independent oracle** for `ios-backup-core`: every byte of
cryptography here comes from Python's `cryptography` package (OpenSSL-backed)
and every property list from the standard library's `plistlib`. Neither shares
a line of lineage with the Rust implementation under test, so a fixture that
this script encrypts and `ios-backup-core` decrypts has been round-tripped
across two independent implementations of the same published format.

That is what makes the end-to-end tests T2 rather than T3. A fixture minted by
the same code that reads it would prove only self-consistency.

Determinism: all key material is derived from a fixed seed, so re-running this
script reproduces the fixture byte for byte. Regenerate with the exact command
recorded in `tests/data/README.md`.

Usage:
    python3 tools/mint_encrypted_backup.py <output-dir> [--password PW] [--plain]
"""

from __future__ import annotations

import argparse
import datetime as dt
import hashlib
import os
import plistlib
import random
import sqlite3
import struct
import sys
from pathlib import Path

from cryptography.hazmat.primitives import hashes
from cryptography.hazmat.primitives.ciphers import Cipher, algorithms, modes
from cryptography.hazmat.primitives.kdf.pbkdf2 import PBKDF2HMAC
from cryptography.hazmat.primitives.keywrap import aes_key_wrap

# Low round counts keep the fixture fast to read in CI. A real backup uses
# ITER ~10_000 and DPIC ~10_000_000; the *algorithm* under test is identical,
# only the cost differs, and the real-backup test covers the production values.
ITER = 1_000
DPIC = 1_000

# Protection classes a backup actually carries, and the one the manifest uses.
CLASSES = (1, 2, 3, 4)
MANIFEST_CLASS = 4

# Files.flags: 1 = file, 2 = directory, 4 = symlink.
FLAG_FILE = 1
FLAG_DIR = 2

WRAP_PASSCODE = 2


def tlv(tag: str, value: bytes) -> bytes:
    """One keybag TLV: 4-byte ASCII tag, 4-byte big-endian length, value."""
    return tag.encode("ascii") + struct.pack(">I", len(value)) + value


def tlv_u32(tag: str, value: int) -> bytes:
    return tlv(tag, struct.pack(">I", value))


def pbkdf2(digest, password: bytes, salt: bytes, rounds: int, length: int) -> bytes:
    return PBKDF2HMAC(algorithm=digest, length=length, salt=salt, iterations=rounds).derive(
        password
    )


def derive_backup_key(password: bytes, salt: bytes, dpsl: bytes) -> bytes:
    """The documented two-round derivation: SHA-256 pre-round, then SHA-1."""
    first = pbkdf2(hashes.SHA256(), password, dpsl, DPIC, 32)
    return pbkdf2(hashes.SHA1(), first, salt, ITER, 32)


def aes_cbc_encrypt(key: bytes, data: bytes, iv: bytes = b"\x00" * 16) -> bytes:
    """AES-CBC with the all-zero IV a backup uses, PKCS#7-padded.

    PKCS#7 and not zero padding: that is what iOS actually writes, confirmed
    against the `iphone-dataprotection` reference implementation, whose
    `removePadding` reads the final byte as a pad count (RFC 1423) and works on
    real backups. A fixture padded with zeros would let a reader that mishandles
    PKCS#7 pass.

    Note this means a plaintext whose length is already a block multiple gains a
    WHOLE extra block of padding, which is the case most likely to be got wrong.
    """
    pad = 16 - (len(data) % 16)  # always 1..=16, never 0 — PKCS#7 adds a full block
    encryptor = Cipher(algorithms.AES(key), modes.CBC(iv)).encryptor()
    return encryptor.update(data + bytes([pad]) * pad) + encryptor.finalize()


def file_metadata(size: int, protection_class: int, wrapped_key: bytes, relative_path: str,
                  flags: int, mtime: int, omit_size: bool = False) -> bytes:
    """The NSKeyedArchiver archive iOS stores in `Files.file`.

    The shape matters: `EncryptionKey` is a UID reference into `$objects`, and
    the object it names holds the wrapped key in `NS.data` behind a 4-byte
    little-endian protection-class prefix.
    """
    objects: list = [
        "$null",
        {
            "$class": plistlib.UID(4),
            "Size": size,
            "ProtectionClass": protection_class,
            "Mode": 0o100644 if flags == FLAG_FILE else 0o040755,
            "UserID": 501,
            "GroupID": 501,
            "InodeNumber": 1_000_000 + size,
            "LastModified": mtime,
            "LastStatusChange": mtime,
            "Birth": mtime,
            "RelativePath": plistlib.UID(3),
            "Flags": 0,
        },
        {
            "$class": plistlib.UID(5),
            "NS.data": struct.pack("<I", protection_class) + wrapped_key,
        },
        relative_path,
        {"$classname": "MBFile", "$classes": ["MBFile", "NSObject"]},
        {"$classname": "NSMutableData", "$classes": ["NSMutableData", "NSData", "NSObject"]},
    ]
    if omit_size:
        del objects[1]["Size"]
    if wrapped_key == b"":
        # A directory carries no EncryptionKey at all.
        del objects[1]["ProtectionClass"]
        objects[1].pop("EncryptionKey", None)
    else:
        objects[1]["EncryptionKey"] = plistlib.UID(2)

    archive = {
        "$version": 100000,
        "$archiver": "NSKeyedArchiver",
        "$top": {"root": plistlib.UID(1)},
        "$objects": objects,
    }
    return plistlib.dumps(archive, fmt=plistlib.FMT_BINARY)


# The fixture's contents: (domain, relative path, payload). Chosen to exercise
# the reader rather than to be realistic — a file smaller than one AES block, a
# file spanning several blocks, and a file whose length is an exact multiple of
# the block size (the case a padding-stripping reader corrupts).
CONTENT = [
    ("HomeDomain", "Library/Preferences/com.apple.example.plist", b"tiny"),
    ("HomeDomain", "Library/SMS/sms.db", b"SQLite format 3\x00" + b"A" * 300),
    ("AppDomain-com.example.app", "Documents/exact.bin", b"B" * 32),
    ("CameraRollDomain", "Media/DCIM/100APPLE/IMG_0001.JPG", bytes(range(256)) * 3),
]
DIRECTORY = ("HomeDomain", "Library/SMS")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output", type=Path, help="directory to create the backup in")
    parser.add_argument("--password", default="test-password-1234")
    parser.add_argument(
        "--plain",
        action="store_true",
        help="mint an UNENCRYPTED backup (IsEncrypted false, plaintext blobs)",
    )
    parser.add_argument("--seed", type=int, default=20260821)
    parser.add_argument(
        "--blob-file-id",
        action="store_true",
        help="store the first file's fileID as a BLOB rather than TEXT. SQLite "
             "has column affinity, not type enforcement, so this is legal and a "
             "reader that matches only on TEXT drops the row",
    )
    parser.add_argument(
        "--lie",
        choices=["unencrypted", "encrypted"],
        help="write a Manifest.plist whose IsEncrypted DISAGREES with the bytes: "
             "'unencrypted' encrypts everything but declares false; 'encrypted' "
             "leaves plaintext but declares true",
    )
    parser.add_argument(
        "--evil-file-id",
        action="store_true",
        help="mint the first file with a path-traversing fileID, to prove a "
             "reader cannot be walked out of the backup directory",
    )
    parser.add_argument(
        "--omit-size",
        action="store_true",
        help="mint the first file with NO Size key in its metadata, so a reader "
             "that treats an absent size as zero returns an empty file",
    )
    args = parser.parse_args()

    rng = random.Random(args.seed)

    def rand(n: int) -> bytes:
        return bytes(rng.getrandbits(8) for _ in range(n))

    out: Path = args.output
    out.mkdir(parents=True, exist_ok=True)
    password = args.password.encode("utf-8")
    encrypted = not args.plain

    class_keys: dict[int, bytes] = {}
    keybag_blob = b""
    manifest_key = b""

    if encrypted:
        salt, dpsl = rand(20), rand(20)
        backup_key = derive_backup_key(password, salt, dpsl)

        keybag_blob = (
            tlv_u32("VERS", 5)
            + tlv_u32("TYPE", 1)              # 1 = Backup
            + tlv("UUID", rand(16))           # the bag's own UUID
            + tlv("HMCK", rand(40))
            + tlv_u32("WRAP", 1)              # the bag's own WRAP
            + tlv("SALT", salt)
            + tlv_u32("ITER", ITER)
            + tlv("DPSL", dpsl)
            + tlv_u32("DPIC", DPIC)
        )
        for protection_class in CLASSES:
            class_key = rand(32)
            class_keys[protection_class] = class_key
            keybag_blob += (
                tlv("UUID", rand(16))         # opens a class block
                + tlv_u32("CLAS", protection_class)
                + tlv_u32("WRAP", WRAP_PASSCODE)
                + tlv_u32("KTYP", 0)
                + tlv("WPKY", aes_key_wrap(backup_key, class_key))
            )

        manifest_key = rand(32)

    # ---- Manifest.db -------------------------------------------------------
    db_path = out / "Manifest.db"
    db_path.unlink(missing_ok=True)
    conn = sqlite3.connect(db_path)
    conn.execute(
        "CREATE TABLE Files (fileID TEXT PRIMARY KEY, domain TEXT, "
        "relativePath TEXT, flags INTEGER, file BLOB)"
    )

    mtime = int(dt.datetime(2026, 8, 21, 6, 0, 0, tzinfo=dt.timezone.utc).timestamp())

    # A directory row, which carries no content blob and no encryption key.
    dir_domain, dir_path = DIRECTORY
    dir_id = hashlib.sha1(f"{dir_domain}-{dir_path}".encode("utf-8")).hexdigest()
    conn.execute(
        "INSERT INTO Files VALUES (?,?,?,?,?)",
        (dir_id, dir_domain, dir_path, FLAG_DIR,
         file_metadata(0, 0, b"", dir_path, FLAG_DIR, mtime)),
    )

    for index, (domain, relative_path, payload) in enumerate(CONTENT):
        file_id = hashlib.sha1(f"{domain}-{relative_path}".encode("utf-8")).hexdigest()
        if args.evil_file_id and index == 0:
            # Not a digest at all: a relative path climbing out of the backup.
            file_id = "../../../../../../../../etc/passwd"
        protection_class = CLASSES[index % len(CLASSES)]

        if encrypted:
            file_key = rand(32)
            wrapped = aes_key_wrap(class_keys[protection_class], file_key)
            blob = aes_cbc_encrypt(file_key, payload)
        else:
            wrapped, blob = b"", payload

        stored_file_id = (
            file_id.encode("ascii") if args.blob_file_id and index == 0 else file_id
        )
        conn.execute(
            "INSERT INTO Files VALUES (?,?,?,?,?)",
            (stored_file_id, domain, relative_path, FLAG_FILE,
             file_metadata(len(payload), protection_class, wrapped, relative_path,
                           FLAG_FILE, mtime, omit_size=args.omit_size and index == 0)),
        )

        if args.evil_file_id and index == 0:
            # No blob written: the point is whether the reader RESOLVES the
            # path, not whether something happens to be there.
            continue
        blob_dir = out / file_id[:2]
        blob_dir.mkdir(exist_ok=True)
        (blob_dir / file_id).write_bytes(blob)

    conn.commit()
    conn.close()

    if encrypted:
        plaintext_db = db_path.read_bytes()
        db_path.write_bytes(aes_cbc_encrypt(manifest_key, plaintext_db))

    # ---- Manifest.plist ----------------------------------------------------
    # `--lie` flips only the DECLARATION, never the bytes. That is the point:
    # the reader must survey the effective state rather than trust the flag.
    declared_encrypted = encrypted
    if args.lie == "unencrypted":
        declared_encrypted = False
    elif args.lie == "encrypted":
        declared_encrypted = True

    manifest = {
        "Version": "10.0",
        "Date": dt.datetime(2026, 8, 21, 6, 0, 0),
        "SystemDomainsVersion": "24.0",
        "IsEncrypted": declared_encrypted,
        "WasPasscodeSet": declared_encrypted,
        "Lockdown": {"ProductType": "iPhone13,3", "ProductVersion": "26.2",
                     "BuildVersion": "23C5000", "DeviceName": "Fixture"},
        "Applications": {},
    }
    if encrypted:
        manifest["BackupKeyBag"] = keybag_blob
        manifest["ManifestKey"] = struct.pack("<I", MANIFEST_CLASS) + aes_key_wrap(
            class_keys[MANIFEST_CLASS], manifest_key
        )
    (out / "Manifest.plist").write_bytes(plistlib.dumps(manifest, fmt=plistlib.FMT_BINARY))

    (out / "Status.plist").write_bytes(
        plistlib.dumps(
            {"IsFullBackup": True, "Version": "3.3", "SnapshotState": "finished",
             "BackupState": "new", "UUID": "00000000-0000-0000-0000-0000000000FF",
             "Date": dt.datetime(2026, 8, 21, 6, 0, 0)},
            fmt=plistlib.FMT_BINARY,
        )
    )
    (out / "Info.plist").write_bytes(
        plistlib.dumps(
            {"Device Name": "Fixture", "Product Type": "iPhone13,3",
             "Product Version": "26.2", "Serial Number": "FIXTURE00000",
             "Unique Identifier": "0" * 40, "IMEI": "000000000000000"},
            fmt=plistlib.FMT_BINARY,
        )
    )

    kind = "encrypted" if encrypted else "unencrypted"
    print(f"minted {kind} backup at {out} (password: {args.password if encrypted else 'n/a'})")
    return 0


if __name__ == "__main__":
    sys.exit(main())
