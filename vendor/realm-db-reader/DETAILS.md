# Technical details

This package implements reading binary Realm files. A Realm file is structured
as follows:

## File overview

| Region      | Offset | Length   | Description                                                    |
| ----------- | ------ | -------- | -------------------------------------------------------------- |
| File Header | 0      | 16       | Contains the file format version and the checksum of the file. |
| Slab        | 16     | variable | Contains the slab allocated data.                              |
| Optional    | 32     | 16       | Contains the optional data.                                    |

The entire file is little-endian. The slab contains the main data for the file,
and all addresses are file-relative 8-byte offsets, and are always aligned to 8
bytes.

## File header

| Field       | Offset | Type      | Description                                              |
| ----------- | ------ | --------- | -------------------------------------------------------- |
| `top_ref_0` | `0x00` | `u64`     | The first reference to the top level of the Realm file.  |
| `top_ref_1` | `0x08` | `u64`     | The second reference to the top level of the Realm file. |
| `mnemonic`  | `0x10` | `[u8; 4]` | ASCII "T-DB", identifying the file format.               |
| `version`   | `0x14` | `[u8; 2]` | The major and minorversion of the file format.           |
| `reserved`  | `0x16` | `u8`      | Reserved for future use, always `0` for now.             |
| `flags`     | `0x17` | `u8`      | Flags for the file.                                      |

The `flags` field is a bitfield. Bit 0 is the "switch bit", and controls which
of the two top-level references is the actual top-level. In other words, when
`flags & 1 == 0`, then `top_ref_0` is the reference to the offset of the
top-level node. When `flags & 1 == 1`, `top_ref_1` is the reference to the
offset of the top-level node.

## Slab

The slab is a contiguous region of memory that contains the main data for the
file. It contains interlinked nodes, and the `top_ref` (described above) points
to the offset of the first node in the slab.

Nodes are 8-byte aligned, and contain a header and a payload.

### Node header

| Field      | Offset | Type  | Description                                                    |
| ---------- | ------ | ----- | -------------------------------------------------------------- |
| `checksum` | `0x00` | `u32` | A dummy checksum, always `0x41414141`.                         |
| `flags`    | `0x04` | `u8`  | Flags for the node.                                            |
| `size`     | `0x05` | `u32` | The size of the node in bytes, stored in 24-bit little-endian. |

Offsets here are relative to the start of the node, and the length of the header
is 8 bytes.

The `flags` field is a bitfield, containing the following information (from
least to most significant bit):

| Bit | Description                                                    |
| --- | -------------------------------------------------------------- |
| 0   | `is_inner_bptree`: Does the node contain an inner B+Tree?      |
| 1   | `has_refs`: Does the node contain references?                  |
| 2   | `context_flag`: Meaning depends on context.                    |
| 3-4 | `width_scheme`: The width scheme of the node.                  |
| 5-7 | `width_ndx`: Value is interpreted based on the `width_scheme`. |

The `width_scheme` can have the following values:

| Value | Meaning of `width` | Number of bytes after header |
| ----- | ------------------ | ---------------------------- |
| 0     | Number of bits     | `ceil(width * size / 8)`     |
| 1     | Number of bytes    | `width * size`               |
| 2     | Ignored            | `width`                      |

The value of `width` is essentialy `1 << width_ndx`:

| `width_ndx` | `width` |
| ----------- | ------- |
| 0           | 1       |
| 1           | 2       |
| 2           | 4       |
| 3           | 8       |
| 4           | 16      |
| 5           | 32      |
| 6           | 64      |
| 7           | 128     |

### Node payload

The payload of a node is a sequence of bytes, the value of which is described by
the header (see above). As for how to actually interpret the bytes, that depends
on the context, and the type of the node. We'll discuss some of the common cases
here.

In general, the main interface, based on `realm-core` (the canonical C++
implementation), is the `Node::get` method, which boils down to:

```rust
impl Node {
    fn get(&self, index: usize) -> u64 {
        let width = self.header.width();

        self.get_direct(width, index)
    }
}
```

The `get_direct` method then extracts data based on the `width` (this is a
templated function in C++).

```rust
impl Node {
    fn get_direct(&self, width: u8, index: usize) -> u64 {
        let data: &[u8] = self.payload();

        // Note that we're ignoring some type conversions here.
        match width {
            0 => 0,
            1 => {
                let offset = index >> 3;
                (data[offset] >> (index & 7)) & 0x01
            },
            2 => {
                let offset = index >> 2;
                (data[offset] >> ((index & 3) << 1)) & 0x03
            },
            4 => {
                let offset = index >> 1;
                (data[offset] >> ((index & 1) << 2)) & 0x0F
            },
            8 => {
                data[index]
            },
            16 => {
                let offset = index * 2;
                LittleEndian::read_u16(&data[offset..offset + 2])
            },
            32 => {
                let offset = index * 4;
                LittleEndian::read_u32(&data[offset..offset + 4])
            },
            64 => {
                let offset = index * 8;
                LittleEndian::read_u64(&data[offset..offset + 8])
            },
            _ => {
                dbg_assert!(false, "invalid width");
                -1
            }
        }
    }
}
```

The `Array` subclasses in C++ call these methods to extract data from the node.
Some common cases are:

### String array

Its `get` method checks `header.has_refs()` (from the `flag`) to determine if
the node contains "long strings". If it doesn't, we're dealing with a "short
string" array, which stores strings in a consecutive list of fixed-length blocks
of `<width>` bytes. The longest string it can store is `width - 1` bytes.

An example when `width = 4` is the following sequence of bytes, where `x` is payload:

```
xxx0 xx01 x002 0003 0004
```

So each string is 0 terminated, and the last byte in a block tells how many 0s
are present, except for a null/`None` value, where the last byte is `width`.
If `width` is 0, all elements are `None`. The max width is 64.

Coming back to the `get` method, if `has_refs()` is `true`, we're dealing with
"long strings". It then further checks `header.has_context_flag()` to decide
between string arrays based on small blobs or big blobs (where `has_context_flag
== true` means big blobs), and calls `get_string` on them.

### Small blob array

Small blob arrays consist of two to three subnodes:

1. An integer array (see below), storing the offsets of the blobs.
2. A blob array, storing the actual blobs. Note: despite the name, a blob
   "array" is really a node with a single blob.
3. Another array, where each element is 1 or 0, indicating whether the element
   is null or not. This field is absent in older versions of the file format.

Imagine a small blob array with the following strings:

["a", "", "abc", null, "ab"]

This would be stored as:

| Field   | Value         |
| ------- | ------------- |
| offsets | 1, 1, 5, 5, 6 |
| blob    | aabcab        |
| nulls   | 0, 0, 0, 1, 0 |

The main entrypoint here is its `get` method:

```rust
impl SmallBlobArray {
    fn get(&self, index: usize) -> Option<BinaryData> {
        assert!(index < self.size());

        if nulls[index] == 1 {
            return None;
        }

        let end = offsets[index];
        let begin = if index > 0 {
            offsets[index - 1]
        } else {
            0
        };

        let blob = self.blob.payload();
        Some(BinaryData::from_slice(&blob[begin..end]))
    }
}
```

The `get_string` method can then be used to extract the string from the blob.

```rust
impl SmallBlobArray {
    fn get_string(&self, index: usize) -> Result<Option<String>, Utf8Error> {
        let Some(blob) = self.get(index) else {
            return Ok(None);
        };

        let s = String::from_utf8(&blob[..blob.len() - 1])?;

        Ok(Some(s))
    }
}
```
