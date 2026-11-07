"""Read-only disk image, filesystem and archive access.

Node identifiers returned by ``Image.children`` are opaque and belong to that
image. File content is returned as bytes, without interpreting its format.
"""

from __future__ import annotations

import io
import json
import operator
import os
from dataclasses import dataclass
from typing import Any

from . import _native

__version__ = "1.3.0"
__all__ = [
    "Image", "ImageFile", "ImageInfo", "Partition", "Node", "Timestamps",
    "FileStat", "DataRun", "__version__",
]


@dataclass(frozen=True)
class Timestamps:
    """Source timestamp strings; missing values and timezone evidence are preserved."""

    modified: str | None = None
    accessed: str | None = None
    created: str | None = None
    mft_changed: str | None = None
    birth: str | None = None


@dataclass(frozen=True)
class Partition:
    id: int
    kind: str
    start_lba: int
    length_lba: int
    label: str | None
    unique_guid: bytes | None

    @property
    def node_id(self) -> str:
        return f"part:{self.id}"


@dataclass(frozen=True)
class ImageInfo:
    path: str
    source_format: str
    size_bytes: int
    sector_size: int
    partitions: tuple[Partition, ...]
    table_scheme: str | None
    disk_id: str | None
    on_disk_bytes: int | None


@dataclass(frozen=True)
class Node:
    id: str
    name: str
    parent_id: str | None
    kind: str
    size: int
    deleted: bool
    inode: int | None
    timestamps: Timestamps
    link_target: str | None

    @property
    def is_dir(self) -> bool:
        return self.kind == "dir"

    @property
    def is_file(self) -> bool:
        return self.kind == "file"


@dataclass(frozen=True)
class DataRun:
    logical_offset: int
    physical_offset: int
    length: int


@dataclass(frozen=True)
class FileStat:
    node_id: str
    size: int
    inode: int | None
    mode: int | None
    data_runs: tuple[DataRun, ...]


def _node_id(node: str | Node) -> str:
    if isinstance(node, Node):
        return node.id
    if not isinstance(node, str):
        raise TypeError("expected a Node or its string identifier")
    return node


def _unsigned(value: int, name: str) -> int:
    result = operator.index(value)
    if result < 0 or result > (1 << 64) - 1:
        raise ValueError(f"{name} must fit in an unsigned 64-bit integer")
    return result


class Image:
    """Open an image or archive read-only, with automatic format detection.

    Closing the image invalidates its file objects. Native I/O releases the GIL.
    Case management, analysis, carving, exports and plugins are not exposed.
    """

    def __init__(self, path: str | os.PathLike[str]) -> None:
        self._reader = _native.Image(os.fspath(path))
        try:
            metadata = json.loads(self._reader.info_json())
            partitions = tuple(
                Partition(
                    id=item["id"], kind=item["kind"], start_lba=item["start_lba"],
                    length_lba=item["length_lba"], label=item["label"],
                    unique_guid=bytes(item["unique_guid"]) if item["unique_guid"] else None,
                )
                for item in metadata["partitions"]
            )
            self._info = ImageInfo(
                path=metadata["path"], source_format=metadata["source_format"],
                size_bytes=metadata["size_bytes"], sector_size=metadata["sector_size"],
                partitions=partitions, table_scheme=metadata["table_scheme"],
                disk_id=metadata["disk_id"], on_disk_bytes=metadata["on_disk_bytes"],
            )
        except BaseException:
            self._reader.close()
            raise

    @property
    def info(self) -> ImageInfo:
        """Immutable metadata, also available after close."""
        return self._info

    @property
    def size(self) -> int:
        return self.info.size_bytes

    @property
    def partitions(self) -> tuple[Partition, ...]:
        return self.info.partitions

    @property
    def closed(self) -> bool:
        return self._reader.closed

    def _check_open(self) -> None:
        if self.closed:
            raise ValueError("I/O operation on closed image")

    def close(self) -> None:
        self._reader.close()

    def __enter__(self) -> Image:
        self._check_open()
        return self

    def __exit__(self, *args: Any) -> None:
        self.close()

    def read(self, offset: int, size: int) -> bytes:
        """Read at most 64 MiB of logical image bytes, clipped at EOF."""
        self._check_open()
        return self._reader.read(_unsigned(offset, "offset"), _unsigned(size, "size"))

    def children(self, parent: str | Node = "root") -> list[Node]:
        """List a directory; root returns volume or archive entry points.

        Listing skips forensic deleted-record scans and unallocated-space nodes.
        """
        self._check_open()
        result = []
        for item in json.loads(self._reader.children_json(_node_id(parent))):
            result.append(Node(
                id=item["id"], name=item["name"], parent_id=item["parent_id"],
                kind=item["kind"], size=item["size"], deleted=item["deleted"],
                inode=item["inode"], timestamps=Timestamps(**item["timestamps"]),
                link_target=item.get("link_target"),
            ))
        return result

    def stat(self, node: str | Node) -> FileStat:
        self._check_open()
        metadata = json.loads(self._reader.stat_json(_node_id(node)))
        return FileStat(
            node_id=metadata["node_id"], size=metadata["size"], inode=metadata["inode"],
            mode=metadata["mode"],
            data_runs=tuple(DataRun(**run) for run in metadata["data_runs"]),
        )

    def open(self, node: str | Node, mode: str = "rb") -> ImageFile:
        """Open a seekable binary file. Only mode 'rb' is accepted."""
        self._check_open()
        if mode != "rb":
            raise ValueError("images are read-only; mode must be 'rb'")
        if isinstance(node, Node) and node.is_dir:
            raise IsADirectoryError(node.name)
        return ImageFile(self, _node_id(node))

    def read_file(self, node: str | Node, offset: int = 0, size: int = -1) -> bytes:
        """Read file bytes; size=-1 reads through EOF. Large reads allocate memory."""
        offset = _unsigned(offset, "offset")
        with self.open(node) as stream:
            stream.seek(offset)
            return stream.read(size)


class ImageFile(io.RawIOBase):
    """A seekable, read-only binary stream tied to its parent Image.

    Use io.BufferedReader for workloads with many tiny reads. Formats without
    native random access may decompress/stream the prefix again after a seek.
    """

    def __init__(self, image: Image, node_id: str) -> None:
        super().__init__()
        self._image = image
        self._node_id = node_id
        self._size = image.stat(node_id).size
        self._position = 0

    def _check_open(self) -> None:
        if self.closed:
            raise ValueError("I/O operation on closed file")
        self._image._check_open()

    def readable(self) -> bool:
        self._check_open()
        return True

    def seekable(self) -> bool:
        self._check_open()
        return True

    def writable(self) -> bool:
        self._check_open()
        return False

    def write(self, buffer: Any) -> int:
        self._check_open()
        raise io.UnsupportedOperation("image files are read-only")

    def truncate(self, size: int | None = None) -> int:
        self._check_open()
        raise io.UnsupportedOperation("image files are read-only")

    def tell(self) -> int:
        self._check_open()
        return self._position

    def seek(self, offset: int, whence: int = os.SEEK_SET) -> int:
        self._check_open()
        offset = operator.index(offset)
        whence = operator.index(whence)
        if whence == os.SEEK_SET:
            position = offset
        elif whence == os.SEEK_CUR:
            position = self._position + offset
        elif whence == os.SEEK_END:
            position = self._size + offset
        else:
            raise ValueError("invalid whence")
        if position < 0 or position > (1 << 64) - 1:
            raise ValueError("seek position outside unsigned 64-bit range")
        self._position = position
        return position

    def readinto(self, buffer: Any) -> int:
        self._check_open()
        with memoryview(buffer).cast("B") as view:
            if view.readonly:
                raise TypeError("readinto() requires a writable buffer")
            size = min(len(view), _native.MAX_READ, max(0, self._size - self._position))
            data = self._image._reader.read_file(self._node_id, self._position, size)
            if len(data) != size:
                raise OSError("unexpected short read before file EOF")
            view[:len(data)] = data
            self._position += len(data)
            return len(data)

    def read(self, size: int = -1) -> bytes:
        self._check_open()
        size = operator.index(size)
        if size < -1:
            raise ValueError("size must be -1 or non-negative")
        remaining = max(0, self._size - self._position)
        if size >= 0:
            remaining = min(remaining, size)
        chunks = []
        while remaining:
            size = min(remaining, _native.MAX_READ)
            data = self._image._reader.read_file(self._node_id, self._position, size)
            if len(data) != size:
                raise OSError("unexpected short read before file EOF")
            chunks.append(data)
            self._position += len(data)
            remaining -= len(data)
        return b"".join(chunks)

    def readall(self) -> bytes:
        return self.read()

    def __enter__(self) -> ImageFile:
        self._check_open()
        return self
