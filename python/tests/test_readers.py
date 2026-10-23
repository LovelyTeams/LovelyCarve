"""Independent, invented FAT12/MBR and standard-library archive fixtures."""

import concurrent.futures
import io
import os
from pathlib import Path
import struct
import tarfile
import tempfile
import unittest
import zipfile

import lovelycarve
from lovelycarve import Image


CONTENT = b"LovelyCarve read-only Python reader\n" * 7


def fat12_volume():
    """A conventional 1.44 MiB FAT12 floppy with one contiguous file."""
    volume = bytearray(2880 * 512)
    volume[:3] = b"\xeb\x3c\x90"
    volume[3:11] = b"LC-PY   "
    struct.pack_into("<HBHBHHBHHHII", volume, 11,
                     512, 1, 1, 2, 224, 2880, 0xF0, 9, 18, 2, 0, 0)
    volume[38] = 0x29
    volume[43:54] = b"LOVELYCARVE"
    volume[54:62] = b"FAT12   "
    volume[510:512] = b"\x55\xaa"
    for sector in (1, 10):
        offset = sector * 512
        volume[offset:offset + 6] = b"\xf0\xff\xff\xff\x0f\x00"
    offset = 19 * 512
    volume[offset:offset + 11] = b"HELLO   TXT"
    volume[offset + 11] = 0x20
    struct.pack_into("<H", volume, offset + 26, 2)
    struct.pack_into("<I", volume, offset + 28, len(CONTENT))
    volume[33 * 512:33 * 512 + len(CONTENT)] = CONTENT
    return volume


class ReaderTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.directory = Path(self.temporary.name)
        self.volume = self.directory / "卷镜像.img"
        self.volume.write_bytes(fat12_volume())

    def find_file(self, image):
        roots = image.children()
        root = next(node for node in roots if node.is_dir)
        return next(node for node in image.children(root) if node.name.lower() == "hello.txt")

    def test_public_scope(self):
        self.assertEqual(lovelycarve.__version__, "1.2.0")
        for name in ("carve", "search", "Case", "Task", "export", "parse", "Plugin"):
            self.assertFalse(hasattr(lovelycarve, name))

    def test_raw_fat_file_and_ranges(self):
        original = self.volume.read_bytes()
        with Image(self.volume) as image:
            self.assertEqual(image.info.source_format, "raw")
            self.assertEqual(image.read(0, 512), original[:512])
            self.assertEqual(image.read(image.size - 3, 30), original[-3:])
            self.assertEqual(image.read(image.size + 1, 30), b"")
            self.assertEqual(image.read(0, 0), b"")
            self.assertEqual(image.partitions[0].kind, "fat12")
            node = self.find_file(image)
            self.assertEqual(image.stat(node).size, len(CONTENT))
            self.assertEqual(image.read_file(node), CONTENT)
            self.assertEqual(image.read_file(node.id, 5, 20), CONTENT[5:25])
            with image.open(node) as stream:
                self.assertEqual(stream.read(13), CONTENT[:13])
                self.assertEqual(stream.tell(), 13)
                self.assertEqual(stream.seek(-5, os.SEEK_CUR), 8)
                buffer = bytearray(7)
                self.assertEqual(stream.readinto(buffer), 7)
                self.assertEqual(buffer, CONTENT[8:15])
                stream.seek(-9, os.SEEK_END)
                self.assertEqual(stream.read(), CONTENT[-9:])
                stream.seek(len(CONTENT) + 99)
                self.assertEqual(stream.read(), b"")
                self.assertFalse(stream.writable())
                with self.assertRaises(io.UnsupportedOperation):
                    stream.write(b"no")
        self.assertEqual(self.volume.read_bytes(), original)

    def test_partition_offsets(self):
        volume = fat12_volume()
        disk = bytearray(512 * 2048) + volume
        disk[446 + 4] = 1  # FAT12 partition type.
        struct.pack_into("<II", disk, 446 + 8, 2048, len(volume) // 512)
        disk[510:512] = b"\x55\xaa"
        path = self.directory / "partitioned.img"
        path.write_bytes(disk)
        with Image(path) as image:
            self.assertEqual(image.info.table_scheme, "mbr")
            self.assertEqual(image.partitions[0].start_lba, 2048)
            self.assertEqual(image.read_file(self.find_file(image)), CONTENT)

    def test_split_raw_and_first_segment_identity(self):
        volume = fat12_volume()
        first = self.directory / "split.001"
        second = self.directory / "split.002"
        first.write_bytes(volume[:1024 * 1024])
        second.write_bytes(volume[1024 * 1024:])
        with Image(first) as image:
            self.assertEqual(image.size, len(volume))
            self.assertEqual(image.read(1024 * 1024 - 8, 16), volume[1024 * 1024 - 8:1024 * 1024 + 8])
            self.assertEqual(image.read_file(self.find_file(image)), CONTENT)
        with self.assertRaises(OSError):
            Image(second)

    def test_archives_and_buffered_io(self):
        zip_path = self.directory / "archive.zip"
        with zipfile.ZipFile(zip_path, "w", zipfile.ZIP_DEFLATED) as archive:
            archive.writestr("hello.txt", CONTENT)
        tar_path = self.directory / "archive.tar"
        with tarfile.open(tar_path, "w") as archive:
            info = tarfile.TarInfo("hello.txt")
            info.size = len(CONTENT)
            archive.addfile(info, io.BytesIO(CONTENT))
        for path in (zip_path, tar_path):
            with self.subTest(path=path), Image(path) as image:
                self.assertEqual(image.read(0, 8), path.read_bytes()[:8])
                node = self.find_file(image)
                with io.BufferedReader(image.open(node), buffer_size=32) as stream:
                    self.assertEqual(stream.read(3), CONTENT[:3])
                    stream.seek(9)
                    self.assertEqual(stream.read(), CONTENT[9:])

    def test_close_and_invalid_arguments(self):
        image = Image(self.volume)
        node = self.find_file(image)
        stream = image.open(node)
        with self.assertRaises(IsADirectoryError):
            image.open(image.children()[0])
        for operation in (
            lambda: image.open(node, "wb"),
            lambda: image.read(-1, 4),
            lambda: image.read(0, -1),
            lambda: image.read(0, 64 * 1024 * 1024 + 1),
            lambda: image.read_file(node, -1),
            lambda: stream.seek(-1),
            lambda: stream.seek(0, 19),
            lambda: stream.read(-2),
        ):
            with self.assertRaises(ValueError):
                operation()
        with self.assertRaises(TypeError):
            stream.readinto(b"readonly")
        stream.close()
        with self.assertRaises(ValueError):
            stream.read()
        stream = image.open(node)
        image.close()
        image.close()
        self.assertTrue(image.closed)
        for operation in (image.children, lambda: image.read(0, 1), stream.read, stream.tell):
            with self.assertRaises(ValueError):
                operation()
        stream.close()

    def test_parallel_readers(self):
        with Image(self.volume) as image:
            node = self.find_file(image)
            with concurrent.futures.ThreadPoolExecutor(max_workers=4) as executor:
                results = list(executor.map(lambda _: image.read_file(node), range(12)))
            self.assertEqual(results, [CONTENT] * 12)

    def test_large_file_crosses_native_limit(self):
        # STORED ZIP avoids expensive compression and exercises >64 MiB chunking.
        path = self.directory / "large.zip"
        block = b"abcdef01" * (1024 * 1024)
        with zipfile.ZipFile(path, "w") as archive:
            with archive.open("large.bin", "w") as stream:
                for _ in range(9):
                    stream.write(block)
        with Image(path) as image:
            node = image.children(image.children()[0])[0]
            with image.open(node) as stream:
                data = stream.read()
                self.assertEqual(len(data), len(block) * 9)
                self.assertEqual(data, block * 9)

    def test_truncated_allocated_file_fails(self):
        # File metadata claims two clusters but the FAT chain has only one.
        volume = fat12_volume()
        struct.pack_into("<I", volume, 19 * 512 + 28, 1024)
        self.volume.write_bytes(volume)
        with Image(self.volume) as image:
            node = self.find_file(image)
            with self.assertRaises(OSError):
                image.read_file(node)

    def test_missing_image(self):
        with self.assertRaises(OSError):
            Image(self.directory / "missing.img")


if __name__ == "__main__":
    unittest.main()
