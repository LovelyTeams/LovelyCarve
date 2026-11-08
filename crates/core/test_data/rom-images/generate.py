"""Independent synthetic CramFS v2 / Linux ROMFS encoders; no real firmware."""
import pathlib
import struct
import zlib

ROOT = pathlib.Path(__file__).resolve().parent
PAYLOAD = bytes(index % 251 for index in range(4096)) + bytes(4096) + b'last page\n' * 280
NOTE = b'LovelyCarve ROM image fixture.\n'

def align(value, alignment):
    return (value + alignment - 1) // alignment * alignment

def cramfs(big=False):
    order = '>' if big else '<'
    image = bytearray(76)
    def inode(mode, size, name_length, offset):
        if big:
            words = (mode << 16 | 1000, size << 8 | 100, (name_length // 4) << 26 | offset // 4)
        else:
            words = (mode | 1000 << 16, size | 100 << 24, name_length // 4 | (offset // 4) << 6)
        return struct.pack(order + 'III', *words)
    entries = [('nested', 0o40755, None), ('payload.bin', 0o100644, PAYLOAD), ('link', 0o120777, b'nested/note.txt'), ('empty', 0o100600, b''), ('pipe', 0o10600, None)]
    locations = {}
    for name, mode, content in entries:
        location = len(image)
        name_bytes = name.encode()
        length = align(len(name_bytes), 4)
        image.extend(bytes(12) + name_bytes.ljust(length, b'\0'))
        locations[name] = (location, mode, content, length)
    root_length = len(image) - 76
    nested_offset = len(image)
    image.extend(bytes(12) + b'note.txt')
    def store(content):
        if not content:
            return 0
        start = len(image)
        pages = [content[i:i+4096] for i in range(0, len(content), 4096)]
        image.extend(bytes(4 * len(pages)))
        for index, page in enumerate(pages):
            if any(page): image.extend(zlib.compress(page))
            struct.pack_into(order + 'I', image, start + index * 4, len(image))
        image.extend(bytes(align(len(image), 4) - len(image)))
        return start
    for name, (position, mode, content, length) in locations.items():
        if name == 'nested': size, offset = 20, nested_offset
        elif content is None: size, offset = 0, 0
        else: size, offset = len(content), store(content)
        image[position:position+12] = inode(mode, size, length, offset)
    note_offset = store(NOTE)
    image[nested_offset:nested_offset+12] = inode(0o100644, len(NOTE), 8, note_offset)
    struct.pack_into(order + 'IIII', image, 0, 0x28cd3d45, len(image), 0x101, 0)
    image[16:32] = b'Compressed ROMFS'
    struct.pack_into(order + 'IIII', image, 32, 0, 1, 5, 7)
    image[48:64] = b'LovelyCarve'.ljust(16, b'\0')
    image[64:76] = inode(0o40755, root_length, 0, 76)
    struct.pack_into(order + 'I', image, 32, zlib.crc32(image))
    return image

def romfs():
    image = bytearray(32)
    entries = [('.', 1, b''), ('nested', 1, b''), ('payload.bin', 2, PAYLOAD), ('link', 3, b'nested/note.txt'), ('alias.bin', 0, b''), ('empty', 2, b''), ('pipe', 7, b''), ('note.txt', 2, NOTE)]
    locations = {}
    for name, kind, content in entries:
        start = len(image)
        header_length = 16 + align(len(name.encode()) + 1, 16)
        image.extend(bytes(header_length) + content)
        image.extend(bytes(align(len(image), 16) - len(image)))
        locations[name] = (start, kind, content, header_length)
    children = ['nested', 'payload.bin', 'link', 'alias.bin', 'empty', 'pipe']
    for name, (start, kind, content, header_length) in locations.items():
        next_offset = 0
        if name in children:
            index = children.index(name)
            if index + 1 < len(children): next_offset = locations[children[index+1]][0]
        info = locations['nested'][0] if name == '.' else locations['note.txt'][0] if name == 'nested' else locations['payload.bin'][0] if kind == 0 else 0
        struct.pack_into('>IIII', image, start, next_offset | kind | (8 if kind == 1 else 0), info, len(content), 0)
        image[start+16:start+16+len(name)] = name.encode()
        words = struct.unpack('>' + 'I' * (header_length // 4), image[start:start+header_length])
        struct.pack_into('>I', image, start+12, -sum(words) & 0xffffffff)
    image[:8] = b'-rom1fs-'
    struct.pack_into('>II', image, 8, len(image), 0)
    image[16:27] = b'LovelyCarve'
    count = min(512, len(image)) // 4
    struct.pack_into('>I', image, 12, -sum(struct.unpack('>' + 'I' * count, image[:count*4])) & 0xffffffff)
    return image

if __name__ == '__main__':
    for name, content in [('synthetic-le.cramfs', cramfs()), ('synthetic-be.cramfs', cramfs(True)), ('synthetic.romfs', romfs())]:
        (ROOT / name).write_bytes(content)
    (ROOT / 'payload.bin').write_bytes(PAYLOAD)
