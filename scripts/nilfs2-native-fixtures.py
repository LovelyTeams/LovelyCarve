#!/usr/bin/env python3
"""CC0-1.0. Invented NILFS2 inputs written and read by native Linux tools.

Run as root on a disposable Linux test host. Only newly created regular image
files inside --output are formatted. No Rust encoder supplies fixture bytes.
"""
import argparse
import hashlib
import json
import mmap
import os
import pathlib
import stat
import struct
import subprocess
import zlib


def run(*args):
    result = subprocess.run(args, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
    if result.returncode:
        print(result.stdout.decode("utf8", errors="replace"), flush=True)
        result.check_returncode()
    return result.stdout.decode("utf8", errors="replace")


def manifest(root):
    records = {}
    for directory, names, files in os.walk(root, followlinks=False):
        for name in names + files:
            path = pathlib.Path(directory) / name
            info = path.lstat()
            record = {"inode": info.st_ino, "mode": info.st_mode, "size": info.st_size,
                      "mtime_ns": info.st_mtime_ns, "ctime_ns": info.st_ctime_ns}
            if stat.S_ISREG(info.st_mode):
                with path.open("rb") as source:
                    record["sha256"] = hashlib.file_digest(source, "sha256").hexdigest()
            elif stat.S_ISLNK(info.st_mode):
                record["target"] = os.readlink(path)
            records["/" + path.relative_to(root).as_posix()] = record
    return records


def populate(root, block, entries):
    (root / "sub").mkdir()
    (root / "large-dir").mkdir()
    (root / "hello").write_bytes(b"native NILFS2 evidence\n")
    (root / "empty").touch()
    (root / "versioned").write_bytes(b"snapshot one")
    (root / "vanished").write_bytes(b"retained in snapshot one")
    (root / "中文\\evidence").write_bytes(b"unicode and literal backslash")
    (root / ("x" * 255)).write_bytes(b"maximum native name")
    (root / "sub/nested").write_bytes(b"nested")
    os.link(root / "hello", root / "hardlink")
    os.symlink("sub/nested", root / "link")
    os.symlink("sub/" + "y" * 700, root / "long-link")
    os.mkfifo(root / "fifo")
    os.mknod(root / "device", stat.S_IFCHR | 0o600, os.makedev(1, 3))
    with (root / "large").open("wb") as output:
        data = bytes((index * 17 + 5) & 255 for index in range(1024 * 1024))
        for _ in range(12):
            output.write(data)
        output.write(data[:37])
    with (root / "sparse").open("wb") as output:
        output.write(b"head")
        output.seek(32 * 1024 * 1024 + 13)
        output.write(b"tail")
    with (root / "fragmented").open("wb") as output:
        for index in range(1000):
            output.seek(index * block * 2 + block // 2)
            output.write(bytes([index & 255]) * 257)
    for index in range(entries):
        (root / "large-dir" / f"entry-{index:05}-中文").write_bytes(f"record {index}\n".encode())
    os.utime(root / "hello", ns=(-315619200123456789, 1700000000987654321))


def snapshot_readback(device, mount, block, entries):
    snapshots = []
    run("mount", "-t", "nilfs2", "-o", "noatime,nogc", device, str(mount))
    try:
        populate(mount, block, entries)
        snapshots.append(int(run("mkcp", "-s", "-p", device).strip()))
        (mount / "versioned").write_bytes(b"snapshot two")
        (mount / "vanished").unlink()
        (mount / "added").write_bytes(b"created after snapshot one")
        snapshots.append(int(run("mkcp", "-s", "-p", device).strip()))
        (mount / "versioned").write_bytes(b"current checkpoint")
        (mount / "added").write_bytes(b"changed after snapshot two")
        os.sync()
        records = manifest(mount)
    finally:
        run("umount", str(mount))
    history = {}
    for number in snapshots:
        run("mount", "-t", "nilfs2", "-o", f"ro,norecovery,cp={number}", device, str(mount))
        try:
            history[str(number)] = manifest(mount)
            assert (mount / "versioned").read_bytes() == (b"snapshot one" if number == snapshots[0] else b"snapshot two")
            assert (mount / "vanished").exists() == (number == snapshots[0])
        finally:
            run("umount", str(mount))
    return records, history


def cleaned_copy(source, output):
    name = "nilfs2-4096-512-cleaned"
    image = (output / (name + ".img")).resolve()
    run("cp", "--no-clobber", "--sparse=always", "--reflink=auto", str(source), str(image))
    device = run("losetup", "--find", "--show", str(image)).strip()
    mount = output / "mount"
    mount.mkdir()
    try:
        run("mount", "-t", "nilfs2", "-o", "noatime", device, str(mount))
        try:
            before = run("lssu", "-a", device)
            snapshots = json.loads(source.with_suffix(".json").read_text())["snapshots"]
            garbage = mount / ".gc-staging"
            for generation in range(4):
                with garbage.open("wb") as temporary:
                    data = bytes((index * 29 + generation) & 255 for index in range(1024 * 1024))
                    for _ in range(24):
                        temporary.write(data)
                    temporary.flush()
                    os.fsync(temporary.fileno())
                run("mkcp", device)
            garbage.unlink()
            run("mkcp", device)
            cleaned = run("nilfs-clean", "-p", "0", "-m", "1", "-v", device)
            after = run("lssu", "-a", device)
            records = manifest(mount)
        finally:
            run("umount", str(mount))
        history = {}
        for number in snapshots:
            run("mount", "-t", "nilfs2", "-o", f"ro,norecovery,cp={number}", device, str(mount))
            try:
                history[number] = manifest(mount)
            finally:
                run("umount", str(mount))
        inspection = run("nilfs-tune", "-l", device)
        summary = run("dumpseg", device, "0")
    finally:
        run("losetup", "--detach", device)
    digest = hashlib.sha256()
    compressor = zlib.compressobj(9)
    with image.open("rb") as raw, (output / (name + ".zlib")).open("xb") as packed:
        while data := raw.read(1024 * 1024):
            digest.update(data)
            packed.write(compressor.compress(data))
        packed.write(compressor.flush())
    info = {"generator": pathlib.Path(__file__).name, "license": "CC0-1.0", "mkfs_version": run("mkfs.nilfs2", "-V"),
            "kernel": os.uname().release, "source_sha256": json.loads(source.with_suffix(".json").read_text())["image_sha256"],
            "inspection": inspection, "segment_zero": summary, "cleaner": cleaned,
            "usage_before": before, "usage_after": after, "image_sha256": digest.hexdigest(), "records": records, "snapshots": history}
    # Independent observation of native GC records, not a Rust-generated claim.
    summaries = 0
    with image.open("rb") as source_file, mmap.mmap(source_file.fileno(), 0, access=mmap.ACCESS_READ) as raw:
        seed = struct.unpack_from("<I", raw, 1024 + 12)[0]
        block = 1024 << struct.unpack_from("<I", raw, 1024 + 20)[0]
        at = 0
        while True:
            at = raw.find(b"\x11\xfa\xaf\x1e", at)
            if at < 0:
                break
            start = at - 8
            at += 4
            if start < 0 or start % block:
                continue
            flags = struct.unpack_from("<H", raw, start + 14)[0]
            size = struct.unpack_from("<I", raw, start + 48)[0]
            if flags & 16 and size >= 64 and start + size <= len(raw):
                checksum = (~zlib.crc32(raw[start + 8:start + size], (~seed) & 0xFFFFFFFF)) & 0xFFFFFFFF
                if checksum == struct.unpack_from("<I", raw, start + 4)[0]:
                    summaries += 1
    assert summaries > 0 and before != after
    info["gc_summary_count"] = summaries
    (output / (name + ".json")).write_text(json.dumps(info, ensure_ascii=False, indent=2), encoding="utf8")
    print(name, digest.hexdigest(), "native cleaner/kernel/snapshot readback verified", flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=pathlib.Path, required=True)
    parser.add_argument("--gc-copy-source", type=pathlib.Path, help="Copy a generated 4096/512 image, then force native garbage collection")
    args = parser.parse_args()
    args.output.mkdir(exist_ok=False, parents=True)
    if args.gc_copy_source:
        cleaned_copy(args.gc_copy_source, args.output)
        return
    version = run("mkfs.nilfs2", "-V")
    for block, segment, feature, entries in [(1024, 2048, "block_count", 10000), (2048, 1024, "block_count", 1500),
                                              (4096, 512, "block_count", 1500), (4096, 2048, "none", 1500)]:
        name = f"nilfs2-{block}-{segment}-{feature}"
        image = (args.output / (name + ".img")).resolve()
        with image.open("xb") as output:
            output.truncate(512 * 1024 * 1024)
        mkfs = run("mkfs.nilfs2", "-K", "-b", str(block), "-B", str(segment), "-L", "LovelyCarve oracle", "-O", feature, str(image))
        mount = args.output / "mount"
        mount.mkdir(exist_ok=True)
        device = run("losetup", "--find", "--show", str(image)).strip()
        try:
            run("nilfs-tune", "-U", "12345678-1234-5678-9abc-def012345678", device)
            records, history = snapshot_readback(device, mount, block, entries)
            inspection = run("nilfs-tune", "-l", device)
            summary = run("dumpseg", device, "0")
        finally:
            run("losetup", "--detach", device)
        digest = hashlib.sha256()
        compressor = zlib.compressobj(9)
        with image.open("rb") as source, (args.output / (name + ".zlib")).open("xb") as output:
            while data := source.read(1024 * 1024):
                digest.update(data)
                output.write(compressor.compress(data))
            output.write(compressor.flush())
        info = {"generator": pathlib.Path(__file__).name, "license": "CC0-1.0", "mkfs_version": version,
                "kernel": os.uname().release, "mkfs": mkfs, "inspection": inspection, "segment_zero": summary,
                "image_sha256": digest.hexdigest(), "records": records, "snapshots": history}
        (args.output / (name + ".json")).write_text(json.dumps(info, ensure_ascii=False, indent=2), encoding="utf8")
        print(name, digest.hexdigest(), "native mkfs/kernel/snapshot readback verified", flush=True)


if __name__ == "__main__":
    main()
