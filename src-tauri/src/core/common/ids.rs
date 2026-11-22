//! Typed identifiers for evidence, filesystem nodes and volumes.
//!
//! All three are plain strings on the wire, in case storage and in snapshot
//! files, and many functions take two or three of them side by side. The
//! newtypes keep them apart at compile time while `#[serde(transparent)]`
//! keeps every serialized form unchanged. The composite keys [`VolumeRef`] and
//! [`NodeRef`] replace tuple keys and separator-joined strings, so a lookup
//! cannot swap its parts and a per-image purge compares the image instead of
//! matching a string prefix.
//!
//! Commands take their ids in these types, and so does the evidence session
//! (`DiskImage::id`). Converting from a string is always explicit (`From`,
//! [`ImageId::new`]), which is the point: an `&ImageId` parameter cannot
//! receive a node id by accident. The other direction is free: every id
//! dereferences to `&str`, so the readers and stores that still take `&str`
//! accept one unchanged, and they convert when they are touched.

use std::borrow::Borrow;
use std::fmt;
use std::ops::Deref;

use serde::{Deserialize, Serialize};

macro_rules! string_id {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        #[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            pub fn new(id: impl Into<String>) -> Self {
                Self(id.into())
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(&self.0)
            }
        }

        impl Deref for $name {
            type Target = str;

            fn deref(&self) -> &str {
                &self.0
            }
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                &self.0
            }
        }

        impl Borrow<str> for $name {
            fn borrow(&self) -> &str {
                &self.0
            }
        }

        impl From<String> for $name {
            fn from(id: String) -> Self {
                Self(id)
            }
        }

        impl From<&str> for $name {
            fn from(id: &str) -> Self {
                Self(id.to_owned())
            }
        }

        impl From<$name> for String {
            fn from(id: $name) -> Self {
                id.0
            }
        }

        impl PartialEq<str> for $name {
            fn eq(&self, other: &str) -> bool {
                self.0 == other
            }
        }

        impl PartialEq<&str> for $name {
            fn eq(&self, other: &&str) -> bool {
                self.0 == *other
            }
        }

        impl PartialEq<String> for $name {
            fn eq(&self, other: &String) -> bool {
                &self.0 == other
            }
        }

        impl PartialEq<$name> for str {
            fn eq(&self, other: &$name) -> bool {
                self == other.0
            }
        }

        impl PartialEq<$name> for &str {
            fn eq(&self, other: &$name) -> bool {
                *self == other.0
            }
        }

        impl PartialEq<$name> for String {
            fn eq(&self, other: &$name) -> bool {
                *self == other.0
            }
        }
    };
}

string_id! {
    /// Loaded evidence: the key of `AppState::images` and the evidence uid
    /// in the case ledger.
    ImageId
}

string_id! {
    /// A filesystem node within one evidence (a directory listing's `items[].id`).
    NodeId
}

string_id! {
    /// A volume within one evidence (`part:2`, `root`, …): the unit a
    /// directory snapshot covers and the key it is filed under.
    VolumeKey
}

/// One volume of one evidence: the key of per-volume snapshot maps.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct VolumeRef {
    pub image: ImageId,
    pub volume: VolumeKey,
}

impl VolumeRef {
    pub fn new(image: impl Into<ImageId>, volume: impl Into<VolumeKey>) -> Self {
        Self {
            image: image.into(),
            volume: volume.into(),
        }
    }
}

/// One node of one evidence: the key of per-node preview caches.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct NodeRef {
    pub image: ImageId,
    pub node: NodeId,
}

impl NodeRef {
    pub fn new(image: impl Into<ImageId>, node: impl Into<NodeId>) -> Self {
        Self {
            image: image.into(),
            node: node.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::{ImageId, NodeRef, VolumeKey, VolumeRef};

    #[test]
    fn ids_serialize_as_plain_strings() {
        let header = serde_json::json!({ "image": ImageId::new("img-1"), "volume": VolumeKey::new("part:2") });
        assert_eq!(
            header,
            serde_json::json!({ "image": "img-1", "volume": "part:2" })
        );
        assert_eq!(
            serde_json::from_str::<VolumeKey>("\"root\"").expect("parse"),
            VolumeKey::new("root")
        );
    }

    #[test]
    fn string_keyed_maps_look_up_by_str() {
        let images = HashMap::from([(ImageId::new("img-1"), 1)]);
        assert_eq!(images.get("img-1"), Some(&1));
        assert!(ImageId::new("img-1") == "img-1");
    }

    #[test]
    fn ids_read_as_str_where_untyped_code_expects_one() {
        fn untyped(id: &str) -> usize {
            id.len()
        }
        let id = ImageId::new("img-1");
        assert_eq!(untyped(&id), 5);
        assert!("img-1" == id && String::from("img-1") == id);
    }

    #[test]
    fn composite_keys_keep_their_parts_apart() {
        let snapshots = HashMap::from([(VolumeRef::new("img-1", "part:2"), "snapshot")]);
        assert_eq!(
            snapshots.get(&VolumeRef::new("img-1", "part:2")),
            Some(&"snapshot")
        );
        assert_eq!(snapshots.get(&VolumeRef::new("part:2", "img-1")), None);
        // A separator-joined key could not tell these two nodes apart.
        assert_ne!(NodeRef::new("a\u{1f}b", "c"), NodeRef::new("a", "b\u{1f}c"));
    }
}
