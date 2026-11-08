use std::borrow::Cow;
use std::sync::Arc;

use tracing::{debug, info, instrument, warn};

use crate::array::{Array, RealmRef, RefOrTaggedValue};
use crate::realm::Realm;
use crate::traits::Node;
use crate::utils;
use crate::value::Value;

#[derive(Debug, Clone)]
pub(crate) struct Index {
    array: Array,
    offsets: Array,
}

impl Node for Index {
    fn from_ref(realm: Arc<Realm>, ref_: RealmRef) -> crate::RealmResult<Self> {
        let array = Array::from_ref(Arc::clone(&realm), ref_)?;
        assert!(array.node.header.size >= 1);

        let keys = array.get_node(0)?.unwrap();

        Ok(Self {
            array,
            offsets: keys,
        })
    }
}

type KeyType = u32;

impl Index {
    const KEY_SIZE: u8 = 4; // 32 bits for the key
    const KEY_SIZE_BITS: u8 = Self::KEY_SIZE * 8;

    #[instrument(level = "debug", skip(self))]
    pub(crate) fn find_first(&self, value: &Value) -> crate::RealmResult<Option<usize>> {
        let value = Self::coerce_to_string(value);

        let mut value_offset: usize = 0;
        let mut key = Self::create_key(&value);

        debug!("finding first occurrence of '{value:?}', key = {key:?}");

        let mut current_index = Cow::Borrowed(self);
        loop {
            debug!(
                "current_index: {current_index:?}, value_offset = {value_offset}, key = {key:?}"
            );

            // Find the position matching the key
            let pos = utils::lower_bound(
                current_index.offsets.node.payload(),
                Self::KEY_SIZE_BITS,
                current_index.offsets.node.header.size as usize,
                key as u64,
            );
            debug!("lower_bound: value = {value:?}, key = {key:?}, pos = {pos}");

            // If key is outside range, we know there can be no match.
            if pos == current_index.offsets.node.header.size as usize {
                info!("No match found for key = {key:?} in current_index");

                return Ok(None);
            }

            assert!(pos < current_index.array.node.header.size as usize);

            let pos_refs = pos + 1;
            let ref_ = current_index.array.get(pos_refs);

            if current_index.array.node.header.is_inner_bptree() {
                let ref_ = RealmRef::new(ref_ as usize);
                current_index =
                    Cow::Owned(Self::from_ref(Arc::clone(&self.array.node.realm), ref_)?);

                info!("Going to sub-index at {ref_:?} (current was inner B+Tree)");

                continue;
            }

            let stored_key = current_index.offsets.get(pos) as KeyType;
            if stored_key != key {
                warn!(
                    "Key mismatch: stored_key = {stored_key:?}, expected key = {key:?} at pos = {pos}",
                );

                return Ok(None);
            }

            match RefOrTaggedValue::from_raw(ref_) {
                RefOrTaggedValue::TaggedValue(row_index) => {
                    return Ok(Some(row_index as usize));
                }
                RefOrTaggedValue::Ref(ref_) => {
                    let array = Array::from_ref(Arc::clone(&self.array.node.realm), ref_)?;
                    let is_sub_index = array.node.header.context_flag();

                    if !is_sub_index {
                        info!(
                            "Found row index at pos {pos}: {ref_:?}, value = {:?}",
                            value
                        );
                        return Ok(Some(array.get(0) as usize));
                    }

                    // Otherwise, go into the sub-index.
                    current_index =
                        Cow::Owned(Self::from_ref(Arc::clone(&self.array.node.realm), ref_)?);

                    info!("going to sub-index at {ref_:?}");

                    // Go to next key part of the string. If the offset exceeds the string length, the key will be 0
                    value_offset += Self::KEY_SIZE as usize;

                    // Update 4 byte index key
                    key = Self::create_key_with_offset(&value, value_offset);
                }
            }
        }
    }

    fn create_key(value: &[u8]) -> KeyType {
        let mut key: KeyType = 0;

        for (i, c) in value.iter().enumerate().take(Self::KEY_SIZE as usize) {
            // Index 0 shift left by 24, index 1 by 16...
            let shl = (Self::KEY_SIZE - 1 - i as u8) * 8;
            key |= (*c as u32) << shl;
        }

        key
    }

    /// Index works as follows: All non-NULL values are stored as if they had appended an 'X'
    /// character at the end. So "foo" is stored as if it was "fooX", and "" (empty string) is
    /// stored as "X". And NULLs are stored as empty strings.
    fn create_key_with_offset(value: &[u8], offset: usize) -> u32 {
        if offset > value.len() {
            return 0;
        }

        // For very short strings
        let tail = value.len() - offset;
        if tail < Self::KEY_SIZE as usize {
            let mut buf = [b'\0'; Self::KEY_SIZE as usize];
            buf[tail] = b'X';
            for (i, c) in value.iter().skip(offset).enumerate() {
                buf[i] = *c;
            }
            return Self::create_key(&buf);
        }

        Self::create_key(&value[offset..])
    }

    fn coerce_to_string(value: &Value) -> Cow<'_, [u8]> {
        match value {
            Value::String(s) => Cow::Borrowed(s.as_bytes()),
            Value::Int(n) => {
                let mut str = Vec::with_capacity(std::mem::size_of_val(n));
                str.extend_from_slice(&n.to_le_bytes());
                Cow::Owned(str)
            }
            Value::Bool(b) => Cow::Owned(vec![if *b { 1 } else { 0 }]),
            Value::Timestamp(dt) => {
                let s = dt.timestamp() as u64;
                let ns = dt.timestamp_subsec_nanos();
                debug!("coercing timestamp {dt:?} to string, value = {s} . {ns}");
                let mut str =
                    Vec::with_capacity(std::mem::size_of_val(&s) + std::mem::size_of_val(&ns));
                str.extend_from_slice(&s.to_le_bytes());
                str.extend_from_slice(&ns.to_le_bytes());
                Cow::Owned(str)
            }
            _ => unimplemented!("Unsupported value type for coercion to string: {value:?}"),
        }
    }
}
