#!/usr/bin/env python3
"""CC0-1.0. Build invented GFS2 inputs using Linux mkfs, kernel, and fsck.

Run as root on a disposable Linux test machine. Only newly created regular
image files in --output are formatted; loop mounts are always unmounted.
No Rust filesystem encoder/reader contributes bytes to this native oracle.
"""
import argparse
import hashlib
import json
import os
import pathlib
import stat
import subprocess
import zlib


def run(*args):
    result = subprocess.run(args, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
    if result.returncode:
        print(result.stdout.decode("utf8", errors="replace"), flush=True)
        result.check_returncode()
    return result.stdout.decode("utf8", errors="replace")


def populate(root):
    (root / "sub").mkdir()
    (root / "large-dir").mkdir()
    (root / "hello").write_bytes(b"native GFS2 evidence\n")
    (root / "empty").touch()
    (root / "中文\\evidence").write_bytes(b"unicode and literal backslash")
    (root / ("x" * 255)).write_bytes(b"maximum native name")
    (root / "sub/nested").write_bytes(b"nested")
    os.link(root / "hello", root / "hardlink")
    os.symlink("sub/nested", root / "link")
    os.mkfifo(root / "fifo")
    with (root / "large").open("wb") as output:
        data = bytes((index * 17 + 5) & 255 for index in range(1024 * 1024))
        for _ in range(12):
            output.write(data)
        output.write(data[:37])
    with (root / "sparse").open("wb") as output:
        output.write(b"head")
        output.seek(32 * 1024 * 1024 + 13)
        output.write(b"tail")
    for index in range(1500):
        (root / "large-dir" / f"entry-{index:04}-中文").write_bytes(f"record {index}\n".encode())
    os.setxattr(root / "hello", "user.inline", b"\0\xffnative-inline")
    os.setxattr(root / "hello", "user.large", bytes((index * 23 + 7) & 255 for index in range(12003)))
    for index in range(24):
        os.setxattr(root / "hello", f"user.item{index:02}", bytes([index]) * 256)
    os.utime(root / "hello", ns=(-315619200123456789, 1700000000987654321))


def manifest(root):
    records = {}
    for directory, names, files in os.walk(root, followlinks=False):
        for name in names + files:
            path = pathlib.Path(directory) / name
            info = path.lstat()
            record = {"inode": info.st_ino, "mode": info.st_mode, "size": info.st_size,
                      "atime_ns": info.st_atime_ns, "mtime_ns": info.st_mtime_ns,
                      "ctime_ns": info.st_ctime_ns}
            if stat.S_ISREG(info.st_mode):
                record["sha256"] = hashlib.file_digest(path.open("rb"), "sha256").hexdigest()
            elif stat.S_ISLNK(info.st_mode):
                record["target"] = os.readlink(path)
            record["attributes"] = {key: os.getxattr(path, key, follow_symlinks=False).hex()
                                    for key in os.listxattr(path, follow_symlinks=False)}
            records["/" + path.relative_to(root).as_posix()] = record
    return records


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=pathlib.Path, required=True)
    args = parser.parse_args()
    args.output.mkdir(exist_ok=False, parents=True)
    version = run("mkfs.gfs2", "-V")
    for block, version_number in [(512, 1801), (1024, 1802), (2048, 1801), (4096, 1802)]:
        name = f"gfs2-{block}-{version_number}"
        image = (args.output / (name + ".img")).resolve()
        with image.open("xb") as output:
            output.truncate(512 * 1024 * 1024)
        mkfs = run("mkfs.gfs2", "-O", "-K", "-b", str(block), "-J", "8", "-j", "2", "-r", "32",
                   "-p", "lock_nolock", "-t", "lovelycarve:oracle", "-U", "12345678-1234-5678-9abc-def012345678",
                   "-o", f"format={version_number}", str(image))
        mount = args.output / "mount"
        mount.mkdir(exist_ok=True)
        run("mount", "-t", "gfs2", "-o", "loop,noatime", str(image), str(mount))
        try:
            populate(mount)
            records = manifest(mount)
            os.sync()
        finally:
            run("umount", str(mount))
        fsck = run("fsck.gfs2", "-n", str(image))
        h = hashlib.sha256()
        compressor = zlib.compressobj(9)
        with image.open("rb") as source, (args.output / (name + ".zlib")).open("xb") as output:
            while data := source.read(1024 * 1024):
                h.update(data)
                output.write(compressor.compress(data))
            output.write(compressor.flush())
        info = {"generator": pathlib.Path(__file__).name, "mkfs_version": version, "kernel": os.uname().release,
                "mkfs": mkfs, "fsck": fsck, "image_sha256": h.hexdigest(), "records": records}
        (args.output / (name + ".json")).write_text(json.dumps(info, ensure_ascii=False, indent=2), encoding="utf8")
        print(name, h.hexdigest(), "native mkfs/kernel/fsck verified", flush=True)


if __name__ == "__main__":
    main()
