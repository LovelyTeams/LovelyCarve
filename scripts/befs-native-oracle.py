#!/usr/bin/env python3
# SPDX-License-Identifier: CC0-1.0
"""Audit a pinned native Haiku BFS image against publisher artifacts.

Only public test inputs are cached; no formatter or native filesystem library
is linked by LovelyCarve. Run through `just test-befs-native`.
"""
from pathlib import Path
import argparse
import hashlib
import os
import re
import shutil
import subprocess
import tempfile
import urllib.request
import zipfile

ZIP_URL = 'https://haiku-nightly.cdn.haiku-os.org/x86_64/haiku-master-hrev60295-x86_64-anyboot.zip'
ZIP_SHA = 'c235ac08f39e7a9b6a83b84a18d39bad264e0cc98a5616b5dcff498304c70715'
ISO_SHA = 'd5817bb4b77692f152156a45af3d9344fbf3c28914773bb38f3833bd8477a316'
PACKAGE_URL = 'https://eu.hpkg.haiku-os.org/haiku/master/x86_64/r1~beta6_hrev60295/packages/haiku-r1~beta6_hrev60295-1-x86_64.hpkg'
PACKAGE_SHA = 'f624ff224ac0cc68c22300e15babc3e058b09fc145f0af19f3257c0c5e9c85dc'


def digest(path):
    result = hashlib.sha256()
    with path.open('rb') as source:
        for raw in iter(lambda: source.read(8 * 1024 * 1024), b''):
            result.update(raw)
    return result.hexdigest()


def artifact(cache, url, size, checksum, offline):
    path = cache / url.rsplit('/', 1)[1]
    if path.exists() and path.stat().st_size == size and digest(path) == checksum:
        return path
    if offline:
        raise RuntimeError(f'Verified public input missing from cache: {path.name}')
    if path.exists() and path.stat().st_size >= size:
        raise RuntimeError(f'Unverified cache file already exists: {path}; use a new cache directory')
    with urllib.request.urlopen(urllib.request.Request(url, method='HEAD'), timeout=30) as response:
        if int(response.headers['Content-Length']) != size:
            raise RuntimeError('Publisher artifact length changed')
        etag = response.headers.get('ETag')
    for _ in range(5):
        start = path.stat().st_size if path.exists() else 0
        if start == size:
            break
        headers = {'Range': f'bytes={start}-'} if start else {}
        if etag:
            headers['If-Match'] = etag
        request = urllib.request.Request(url, headers=headers)
        with urllib.request.urlopen(request, timeout=60) as response:
            if start and (response.status != 206 or not re.fullmatch(
                    rf'bytes {start}-\d+/{size}', response.headers.get('Content-Range', ''))):
                raise RuntimeError('Server did not honor the verified resume range')
            with path.open('ab') as output:
                for raw in iter(lambda: response.read(1024 * 1024), b''):
                    output.write(raw)
        print(f'{path.name}: {path.stat().st_size}/{size} bytes', flush=True)
    if path.stat().st_size != size or digest(path) != checksum:
        raise RuntimeError(f'Public input incomplete or SHA-256 mismatch: {path.name}')
    return path


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--cache', type=Path, default=Path(tempfile.gettempdir()) / 'lovelycarve-befs-references')
    parser.add_argument('--offline', action='store_true', help='require already verified public inputs')
    args = parser.parse_args()
    cache = args.cache.resolve()
    cache.mkdir(parents=True, exist_ok=True)
    archive = artifact(cache, ZIP_URL, 484684629, ZIP_SHA, args.offline)
    artifact(cache, PACKAGE_URL, 40048778, PACKAGE_SHA, args.offline)
    image = cache / 'native-haiku.img'
    if image.exists():
        if image.stat().st_size != 690814976 or digest(image) != ISO_SHA:
            raise RuntimeError('Existing native image does not match pinned publisher input; use a new cache directory')
    else:
        with zipfile.ZipFile(archive) as source:
            info = source.getinfo('haiku-master-hrev60295-x86_64-anyboot.iso')
            if info.file_size != 690814976:
                raise RuntimeError('Unexpected native ISO length')
            with source.open(info) as content, image.open('xb') as output:
                shutil.copyfileobj(content, output, 8 * 1024 * 1024)
        if digest(image) != ISO_SHA:
            raise RuntimeError('Decoded native ISO SHA-256 mismatch')
    environment = os.environ.copy()
    environment['BEFS_ORACLE_IMAGE'] = str(image)
    subprocess.run(['just', 'test-befs', 'native_haiku', '--', '--ignored', '--nocapture'],
                   cwd=Path(__file__).resolve().parent.parent, env=environment, check=True)


if __name__ == '__main__':
    main()
