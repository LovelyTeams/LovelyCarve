#!/usr/bin/env python3
"""CC0-1.0. Generate invented OCFS2 images with native Linux tooling.

Run as root on a disposable Linux test host. Only new regular image files
inside --output are formatted. Rust parsers never contribute fixture bytes.
The shared invented payload/manifest routines come from the adjacent GFS2
native fixture script, independently of either Rust reader.
"""
import argparse
import errno
import fcntl
import hashlib
import importlib.util
import json
import os
import pathlib
import subprocess
import zlib


def run(*args):
    result = subprocess.run(args, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
    if result.returncode:
        print(result.stdout.decode("utf8", errors="replace"), flush=True)
        result.check_returncode()
    return result.stdout.decode("utf8", errors="replace")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=pathlib.Path, required=True)
    args = parser.parse_args()
    args.output.mkdir(exist_ok=False, parents=True)
    spec = importlib.util.spec_from_file_location("native_payload", pathlib.Path(__file__).with_name("gfs2-native-fixtures.py"))
    payload = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(payload)
    version = run("mkfs.ocfs2", "-V")
    for block, cluster, features in [(512, 4096, "nometaecc,noindexed-dirs,noinline-data,norefcount"),
                                      (1024, 4096, "metaecc"), (2048, 16384, "metaecc"), (4096, 65536, "metaecc")]:
        name = f"ocfs2-{block}-{cluster}"
        image = (args.output / (name + ".img")).resolve()
        with image.open("xb") as output:
            output.truncate(512 * 1024 * 1024)
        mkfs = run("mkfs.ocfs2", "-F", "-b", str(block), "-C", str(cluster), "-M", "local", "-N", "1",
                   "-J", "size=8M", "-L", "LovelyCarve oracle", "-U", "12345678-1234-5678-9abc-def012345678",
                   "--fs-feature-level=max-features", "--fs-features=" + features, str(image))
        mount = args.output / "mount"
        mount.mkdir(exist_ok=True)
        run("mount", "-t", "ocfs2", "-o", "loop,noatime", str(image), str(mount))
        try:
            payload.populate(mount)
            os.symlink("sub/" + "y" * 700, mount / "long-link")
            clone = mount / "clone"
            with (mount / "large").open("rb") as source, clone.open("wb") as destination:
                try:
                    fcntl.ioctl(destination.fileno(), 0x40049409, source.fileno())
                    destination.write(b"changed")
                except OSError as error:
                    if error.errno not in (errno.EINVAL, errno.EOPNOTSUPP):
                        raise
            if clone.stat().st_size == 0:
                clone.unlink()
            with (mount / "fragmented").open("wb") as output:
                for index in range(1000):
                    output.seek(index * cluster * 2 + cluster // 2)
                    output.write(bytes([index & 255]) * 257)
            clone = mount / "clone-fragmented"
            with (mount / "fragmented").open("rb") as source, clone.open("wb") as destination:
                try:
                    fcntl.ioctl(destination.fileno(), 0x40049409, source.fileno())
                    for index in range(0, 1000, 2):
                        os.pwrite(destination.fileno(), b"separate", index * cluster * 2 + cluster // 2)
                except OSError as error:
                    if error.errno not in (errno.EINVAL, errno.EOPNOTSUPP):
                        raise
            if clone.stat().st_size == 0:
                clone.unlink()
            with (mount / "unwritten").open("wb") as output:
                os.posix_fallocate(output.fileno(), 0, cluster * 4)
                os.pwrite(output.fileno(), b"material", cluster + 3)
            (mount / "preallocated-empty").touch()
            run("fallocate", "--keep-size", "--length", str(cluster * 8), str(mount / "preallocated-empty"))
            records = payload.manifest(mount)
            os.sync()
        finally:
            run("umount", str(mount))
        fsck = run("fsck.ocfs2", "-fn", str(image))
        digest = hashlib.sha256()
        compressor = zlib.compressobj(9)
        with image.open("rb") as source, (args.output / (name + ".zlib")).open("xb") as output:
            while data := source.read(1024 * 1024):
                digest.update(data)
                output.write(compressor.compress(data))
            output.write(compressor.flush())
        info = {"generator": pathlib.Path(__file__).name, "mkfs_version": version, "kernel": os.uname().release,
                "mkfs": mkfs, "fsck": fsck, "image_sha256": digest.hexdigest(), "records": records}
        (args.output / (name + ".json")).write_text(json.dumps(info, ensure_ascii=False, indent=2), encoding="utf8")
        print(name, digest.hexdigest(), "native mkfs/kernel/fsck verified", flush=True)


if __name__ == "__main__":
    main()
