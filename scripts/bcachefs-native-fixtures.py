#!/usr/bin/env python3
"""CC0-1.0: disposable Linux-only bcachefs formatter/FUSE/kernel read oracle.

Run inside an isolated guest with the native bcachefs tools and fuse3. Creates
new image files only, records every mounted name/metadata/hash, and requires
the reference fsck's acceptance. The reader under test is never an oracle.
"""
from pathlib import Path
import argparse
import hashlib
import json
import os
import random
import stat
import subprocess
import time
import zlib


def digest(path):
    result = hashlib.sha256()
    with path.open('rb') as stream:
        while block := stream.read(1024 * 1024):
            result.update(block)
    return result.hexdigest()


def manifest(root):
    rows = []
    for parent, directories, files in os.walk(root, followlinks=False):
        directories.sort(); files.sort()
        for name in directories + files:
            full = Path(parent) / name
            info = full.lstat()
            kind = 'dir' if stat.S_ISDIR(info.st_mode) else ('file' if stat.S_ISREG(info.st_mode) else ('link' if stat.S_ISLNK(info.st_mode) else 'special'))
            row = {'path': str(full.relative_to(root)), 'kind': kind, 'mode': info.st_mode,
                   'inode': info.st_ino, 'size': info.st_size, 'nlink': info.st_nlink,
                   'atime_ns': info.st_atime_ns, 'mtime_ns': info.st_mtime_ns, 'ctime_ns': info.st_ctime_ns}
            if kind == 'file': row['sha256'] = digest(full)
            if kind == 'link': row['target'] = os.readlink(full)
            rows.append(row)
    return sorted(rows, key=lambda row: row['path'])


def run(args, log):
    with log.open('wb') as output:
        subprocess.run(args, stdout=output, stderr=subprocess.STDOUT, check=True, timeout=600)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--tool', default='bcachefs')
    parser.add_argument('--mount-backend', choices=['auto', 'fuse', 'kernel'], default='auto',
                        help='auto uses the native kernel for gzip (reference FUSE gzip reads crash)')
    parser.add_argument('--only', choices=['512-none', '4096-lz4', '4096-zstd', '4096-gzip'])
    args = parser.parse_args()
    if os.name != 'posix' or os.geteuid() != 0: raise SystemExit('Run in a disposable Linux guest as root')
    args.output.mkdir(parents=True, exist_ok=False)
    output = args.output.resolve()
    source = output / 'source'; source.mkdir()
    (source / 'dir').mkdir(); (source / 'many').mkdir()
    (source / 'hello.txt').write_bytes(b'Native bcachefs forensic evidence\n')
    (source / 'dir/nested.txt').write_bytes(b'nested content\n')
    (source / 'empty').write_bytes(b'')
    (source / '中文\\证据.txt').write_bytes('文件系统取证\n'.encode())
    (source / ('n' * 255)).write_bytes(b'maximum component\n')
    (source / 'compressible.bin').write_bytes(bytes(range(256)) * 8192)
    randomizer = random.Random(20261026)
    (source / 'random.bin').write_bytes(randomizer.randbytes(3 * 1024 * 1024 + 37))
    with (source / 'sparse.bin').open('xb') as stream:
        stream.seek(16 * 1024 * 1024); stream.write(b'sparse tail bytes\n')
    for index in range(1500): (source / f'many/entry-{index:04}.txt').write_bytes(f'record {index}\n'.encode())
    os.link(source / 'hello.txt', source / 'alias.txt')
    os.symlink('hello.txt', source / 'link')
    os.mkfifo(source / 'pipe')
    for path in source.rglob('*'):
        if path.is_file() and not path.is_symlink(): os.utime(path, ns=(-315619200000000000+123456789, 1600000000000000000+987654321))
    expected = {row['path']: row for row in manifest(source)}
    version = subprocess.check_output([args.tool, 'version'], text=True).strip()
    cases = [('512-none', '512', 'none', 'none', 'crc32c'),
             ('4096-lz4', '4096', 'crc32c', 'lz4', 'siphash'),
             ('4096-zstd', '4096', 'crc64', 'zstd', 'siphash'),
             ('4096-gzip', '4096', 'xxhash', 'gzip', 'siphash')]
    for name, block_size, checksum, compression, string_hash in cases:
        if args.only and name != args.only: continue
        print('Formatting', name, flush=True)
        image = output / (name + '.img')
        with image.open('xb') as stream: stream.truncate(512 * 1024 * 1024)
        run([args.tool, 'format', '-q', '--block_size='+block_size, '--metadata_checksum='+checksum,
             '--data_checksum='+checksum, '--compression='+compression, '--str_hash='+string_hash,
             '--source='+str(source), str(image)], output / (name+'.format.log'))
        run([args.tool, 'fsck', '-n', str(image)], output / (name+'.fsck-before.log'))
        original = digest(image)
        mount = output / (name + '-mount'); mount.mkdir()
        log = (output / (name + '.mount.log')).open('wb')
        backend = args.mount_backend
        if backend == 'auto': backend = 'kernel' if compression == 'gzip' else 'fuse'
        process = None
        try:
            if backend == 'kernel':
                subprocess.run(['mount', '-t', 'bcachefs', '-o', 'loop,ro,nochanges', str(image), str(mount)], stdout=log, stderr=subprocess.STDOUT, check=True, timeout=60)
            else:
                process = subprocess.Popen([args.tool, 'fusemount', '-f', '-o', 'ro,nochanges', str(image), str(mount)], stdout=log, stderr=subprocess.STDOUT)
            deadline = time.monotonic() + 60
            while not os.path.ismount(mount):
                if process is not None and process.poll() is not None: raise RuntimeError(f'{name}: native FUSE exited {process.returncode}')
                if time.monotonic() > deadline: raise RuntimeError(f'{name}: native FUSE readiness timeout')
                time.sleep(0.2)
            rows = manifest(mount)
            actual = {row['path']: row for row in rows if row['path'] != 'lost+found'}
            assert actual.keys() == expected.keys(), (name, actual.keys() ^ expected.keys())
            for path, row in actual.items():
                control = expected[path]
                assert row['kind'] == control['kind'], (name, path, row, control)
                if row['kind'] == 'file': assert row['sha256'] == control['sha256'], (name, path)
                if row['kind'] == 'link': assert row['target'] == control['target'], (name, path)
        finally:
            if os.path.ismount(mount): subprocess.run(['umount' if backend == 'kernel' else 'fusermount3', '-u' if backend == 'fuse' else '--', str(mount)], check=True, timeout=30)
            if process is not None:
                try: process.wait(timeout=30)
                except subprocess.TimeoutExpired:
                    process.terminate(); process.wait(timeout=10)
            log.close()
        assert digest(image) == original, 'native read-only mount changed the image'
        run([args.tool, 'show-super', str(image)], output / (name+'.super.txt'))
        run([args.tool, 'list', '-b', 'extents', str(image)], output / (name+'.extents.txt'))
        run([args.tool, 'fsck', '-n', str(image)], output / (name+'.fsck-after.log'))
        (output / (name+'.json')).write_text(json.dumps({'tool_version':version, 'mount_backend':backend, 'kernel':os.uname().release, 'sha256':original,
             'size':image.stat().st_size, 'block_size':int(block_size), 'checksum':checksum,
             'compression':compression, 'entries':rows}, ensure_ascii=False, indent=2)+'\n', encoding='utf-8')
        with image.open('rb') as stream, (output/(name+'.zlib')).open('xb') as packed:
            encoder = zlib.compressobj(6)
            while block := stream.read(1024*1024): packed.write(encoder.compress(block))
            packed.write(encoder.flush())
        print('Verified', name, len(rows), 'entries', original, flush=True)


if __name__ == '__main__': main()
