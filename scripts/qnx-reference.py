"""Compare public qnxprobe readers with committed fixtures, without mounting.

Pass qnxprobe.py from abrignoni/qnxprobe commit
dffd27ab242c16100b40114f248e816fb302cc52 (MIT). Nothing is downloaded or extracted.
"""
import hashlib
import importlib.util
import io
from pathlib import Path
import sys
import tarfile
import zlib


def walk(reader):
    result = {}
    def visit(node, path, ancestors):
        for name, child in reader.listdir(node):
            item_path = path + '/' + name
            mode, size, _ = reader.entry(child)
            if mode & 0xf000 == 0x4000:
                assert child not in ancestors, item_path
                result[item_path] = None
                visit(child, item_path, ancestors | {child})
            else:
                result[item_path] = b''.join(reader.read_file(child, size))
    visit(reader.root, '', {reader.root})
    return result


def main():
    source = Path(sys.argv[1]).resolve()
    spec = importlib.util.spec_from_file_location('qnx_reference', source)
    oracle = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = oracle
    spec.loader.exec_module(oracle)
    root = Path(__file__).resolve().parent.parent / 'vendor/qnx-reader/tests/fixtures'
    image = zlib.decompress((root / 'qnx4.zlib').read_bytes())
    files = walk(oracle.Qnx4Walker(io.BytesIO(image), 0))
    expected = {
        '/.bitmap': bytes(32), '/small.txt': b'hello QNX!\r\n',
        '/big.bin': bytes((index * 13 + 7) % 256 for index in range(1300)),
        '/a_long_name_for_qnx4.txt': b'hello QNX!\r\n', '/alias.txt': b'hello QNX!\r\n',
        '/link': b'small.txt', '/nested': None, '/nested/测试.txt': b'hello QNX!\r\n',
    }
    assert files == expected
    image = zlib.decompress((root / 'nfi-qnx6.zlib').read_bytes())
    # Independent fixture's last 4096-byte area holds its newer superblock.
    files = walk(oracle.Qnx6Walker(io.BytesIO(image), 0, len(image) - 4096))
    count = 0
    with tarfile.open(root / 'nfi-qnx6.tar.gz', 'r:gz') as archive:
        for member in archive:
            path = '/' + member.name.rstrip('/')
            assert path in files, path
            if member.isfile():
                assert files[path] == archive.extractfile(member).read(), path
            elif member.issym():
                assert files[path] == member.linkname.encode(), path
            elif member.isdir():
                assert files[path] is None, path
            count += 1
    print(f'Independent qnxprobe oracle passed: QNX4 {len(expected)} entries; QNX6 {count} TAR entries')
    print('Oracle script SHA-256:', hashlib.sha256(source.read_bytes()).hexdigest())


if __name__ == '__main__':
    main()
