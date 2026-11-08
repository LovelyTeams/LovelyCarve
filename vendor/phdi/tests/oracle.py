"""Independent Python decoder for the public single-sparse-extent fixture.

Usage: python tests/oracle.py DiskDescriptor.xml new-output.raw
It deliberately refuses parent chains, split images, and unsupported signatures.
The raw output is consumed by the ignored Rust oracle test.
"""
import hashlib
from pathlib import Path
import struct
import sys
import xml.etree.ElementTree as ET


def decode(descriptor: Path, output: Path) -> None:
    tree = ET.parse(descriptor)
    sectors = int(tree.findtext("Disk_Parameters/Disk_size"))
    storages = tree.findall("StorageData/Storage")
    assert len(storages) == 1
    storage = storages[0]
    assert int(storage.findtext("Start")) == 0
    assert int(storage.findtext("End")) == sectors
    images = storage.findall("Image")
    assert len(images) == 1 and images[0].findtext("Type") == "Compressed"
    for shot in tree.findall("Snapshots/Shot"):
        assert shot.findtext("ParentGUID") == "{00000000-0000-0000-0000-000000000000}"
    extent = descriptor.parent / images[0].findtext("File")
    digest = hashlib.sha256()
    with extent.open("rb") as source, output.open("xb") as target:
        header = source.read(64)
        assert header[:16] == b"WithoutFreeSpace"
        version, _, _, tracks, count = struct.unpack_from("<5I", header, 16)
        assert version == 2
        assert struct.unpack_from("<Q", header, 36)[0] == sectors
        table = struct.unpack(f"<{count}I", source.read(count * 4))
        block_size = tracks * 512
        remaining = sectors * 512
        for entry in table:
            length = min(remaining, block_size)
            if entry:
                source.seek(entry * 512)
                block = source.read(length)
                assert len(block) == length
            else:
                block = bytes(length)
            target.write(block)
            digest.update(block)
            remaining -= length
        assert remaining == 0
    print(f"logical size={sectors * 512} SHA256={digest.hexdigest()}")


if __name__ == "__main__":
    decode(Path(sys.argv[1]), Path(sys.argv[2]))
