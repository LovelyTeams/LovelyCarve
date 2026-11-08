//! This file contains common type traits and aliases used within structures.

use crate::constants::UUID_SIZE;

/// An inline array of bytes representing a UUID within the filesystem.
pub type UuidBytes = [u8; UUID_SIZE];
