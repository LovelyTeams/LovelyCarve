use std::sync::Arc;

use byteorder::{ByteOrder, LittleEndian};

use crate::{array::RealmRef, realm::Realm};

/// Read a value from a Realm node payload. The width here corresponds to the
/// width value read from the Realm node header.
pub(crate) fn read_array_value(payload: &[u8], width: u8, index: usize) -> u64 {
    match width {
        0 => 0,
        1 => {
            let offset = index >> 3;
            ((payload[offset] >> (index & 7)) & 0x01) as u64
        }
        2 => {
            let offset = index >> 2;
            ((payload[offset] >> ((index & 3) << 1)) & 0x03) as u64
        }
        4 => {
            let offset = index >> 1;
            ((payload[offset] >> ((index & 1) << 2)) & 0x0F) as u64
        }
        8 => payload[index] as u64,
        16 => {
            let offset = index * 2;
            LittleEndian::read_u16(&payload[offset..offset + 2]) as u64
        }
        32 => {
            let offset = index * 4;
            LittleEndian::read_u32(&payload[offset..offset + 4]) as u64
        }
        64 => {
            let offset = index * 8;
            LittleEndian::read_u64(&payload[offset..offset + 8])
        }
        _ => {
            panic!("invalid width {width}");
        }
    }
}

/// Find the index of the child node that contains the specified
/// element index. Element index zero corresponds to the first element
/// of the first leaf node contained in the subtree corresponding with
/// the specified 'offsets' array.
///
/// Returns (child_ndx, ndx_in_child).
#[inline]
fn find_child_from_offsets(
    realm: Arc<Realm>,
    offsets_header: RealmRef,
    width: u8,
    elem_ndx: usize,
) -> crate::RealmResult<(usize, usize)> {
    let header = realm.header(offsets_header)?;
    let offsets_data = realm.payload(offsets_header, header.payload_len());
    let offsets_size = header.size;
    let child_index = upper_bound(offsets_data, width, offsets_size as usize, elem_ndx as u64);
    let elem_ndx_offset = if child_index == 0 {
        0
    } else {
        read_array_value(offsets_data, width, child_index - 1) as usize
    };
    let index_in_child = elem_ndx - elem_ndx_offset;
    Ok((child_index, index_in_child))
}

/// For a B+Tree node, calculate the child index and index within that child
/// node for the given element index.
fn find_bptree_child(
    realm: Arc<Realm>,
    first_value: u64,
    index: usize,
) -> crate::RealmResult<(usize, usize)> {
    if first_value % 2 != 0 {
        // Case 1/2: No offsets array (compact form)
        let elems_per_child = (first_value / 2) as usize;
        let child_ndx = index / elems_per_child;
        let ndx_in_child = index % elems_per_child;

        return Ok((child_ndx, ndx_in_child));
    }

    // Case 2/2: Offsets array (general form)
    let offsets_ref = RealmRef::new(first_value as usize);
    let offsets_header = realm.header(offsets_ref)?;
    let offsets_width = offsets_header.width();
    let (child_index, index_in_child) =
        find_child_from_offsets(realm.clone(), offsets_ref, offsets_width, index)?;

    Ok((child_index, index_in_child))
}

pub(crate) fn find_bptree_child_in_payload(
    realm: Arc<Realm>,
    payload: &[u8],
    width: u8,
    index: usize,
) -> crate::RealmResult<(RealmRef, usize)> {
    let first_value = read_array_value(payload, width, 0);
    let (child_index, index_in_child) = find_bptree_child(realm, first_value, index)?;
    let child_ref = RealmRef::new(read_array_value(payload, width, 1 + child_index) as usize);
    Ok((child_ref, index_in_child))
}

/// Converts a byte vector to a string, assuming it is null-terminated.
pub(crate) fn string_from_bytes(mut bytes: Vec<u8>) -> String {
    assert!(
        !bytes.is_empty(),
        "string cannot be empty (should have a trailing \\0"
    );
    assert!(
        bytes[bytes.len() - 1] == 0,
        "string must end with a \\0 byte"
    );

    bytes.pop();

    unsafe { String::from_utf8_unchecked(bytes) }
}

/// Lower/upper bound in sorted sequence
///
/// ```ignore
///   3 3 3 4 4 4 5 6 7 9 9 9
///   ^     ^     ^     ^     ^
///   |     |     |     |     |
///   |     |     |     |      -- Lower and upper bound of 15
///   |     |     |     |
///   |     |     |      -- Lower and upper bound of 8
///   |     |     |
///   |     |      -- Upper bound of 4
///   |     |
///   |      -- Lower bound of 4
///   |
///    -- Lower and upper bound of 1
/// ```
///
/// These functions are semantically identical to std::lower_bound() and
/// std::upper_bound() in C++.
///
/// We currently use binary search. See for example
/// http://www.tbray.org/ongoing/When/200x/2003/03/22/Binary.
#[inline]
pub(crate) fn lower_bound(data: &[u8], width: u8, mut size: usize, value: u64) -> usize {
    // The binary search used here is carefully optimized. Key trick is to use a single
    // loop controlling variable (size) instead of high/low pair, and to keep updates
    // to size done inside the loop independent of comparisons. Further key to speed
    // is to avoid branching inside the loop, using conditional moves instead. This
    // provides robust performance for random searches, though predictable searches
    // might be slightly faster if we used branches instead. The loop unrolling yields
    // a final 5-20% speedup depending on circumstances.

    let mut low = 0;

    while size >= 8 {
        // The following code (at X, Y and Z) is 3 times manually unrolled instances of (A) below.
        // These code blocks must be kept in sync. Meassurements indicate 3 times unrolling to give
        // the best performance. See (A) for comments on the loop body.
        // (X)
        let mut half = size / 2;
        let mut other_half = size - half;
        let mut probe = low + half;
        let mut other_low = low + other_half;
        let mut v = read_array_value(data, width, probe);
        size = half;
        low = if v < value { other_low } else { low };

        // (Y)
        half = size / 2;
        other_half = size - half;
        probe = low + half;
        other_low = low + other_half;
        v = read_array_value(data, width, probe);
        size = half;
        low = if v < value { other_low } else { low };

        // (Z)
        half = size / 2;
        other_half = size - half;
        probe = low + half;
        other_low = low + other_half;
        v = read_array_value(data, width, probe);
        size = half;
        low = if v < value { other_low } else { low };
    }
    while size > 0 {
        // (A)
        // To understand the idea in this code, please note that
        // for performance, computation of size for the next iteration
        // MUST be INDEPENDENT of the conditional. This allows the
        // processor to unroll the loop as fast as possible, and it
        // minimizes the length of dependence chains leading up to branches.
        // Making the unfolding of the loop independent of the data being
        // searched, also minimizes the delays incurred by branch
        // mispredictions, because they can be determined earlier
        // and the speculation corrected earlier.

        // Counterintuitive:
        // To make size independent of data, we cannot always split the
        // range at the theoretical optimal point. When we determine that
        // the key is larger than the probe at some index K, and prepare
        // to search the upper part of the range, you would normally start
        // the search at the next index, K+1, to get the shortest range.
        // We can only do this when splitting a range with odd number of entries.
        // If there is an even number of entries we search from K instead of K+1.
        // This potentially leads to redundant comparisons, but in practice we
        // gain more performance by making the changes to size predictable.

        // if size is even, half and other_half are the same.
        // if size is odd, half is one less than other_half.
        let half = size / 2;
        let other_half = size - half;
        let probe = low + half;
        let other_low = low + other_half;
        let v = read_array_value(data, width, probe);
        size = half;
        // for max performance, the line below should compile into a conditional
        // move instruction. Not all compilers do this. To maximize chance
        // of succes, no computation should be done in the branches of the
        // conditional.
        low = if v < value { other_low } else { low };
    }

    low
}

/// See [`lower_bound`].
#[inline]
fn upper_bound(data: &[u8], width: u8, mut size: usize, value: u64) -> usize {
    let mut low = 0;
    while size >= 8 {
        let mut half = size / 2;
        let mut other_half = size - half;
        let mut probe = low + half;
        let mut other_low = low + other_half;
        let mut v = read_array_value(data, width, probe);
        size = half;
        low = if value >= v { other_low } else { low };

        half = size / 2;
        other_half = size - half;
        probe = low + half;
        other_low = low + other_half;
        v = read_array_value(data, width, probe);
        size = half;
        low = if value >= v { other_low } else { low };

        half = size / 2;
        other_half = size - half;
        probe = low + half;
        other_low = low + other_half;
        v = read_array_value(data, width, probe);
        size = half;
        low = if value >= v { other_low } else { low };
    }

    while size > 0 {
        let half = size / 2;
        let other_half = size - half;
        let probe = low + half;
        let other_low = low + other_half;
        let v = read_array_value(data, width, probe);
        size = half;
        low = if value >= v { other_low } else { low };
    }

    low
}
