//! evidence / crypto domain modules.

pub mod bde_image;
pub mod bde_vmk;
pub mod bitlocker;
pub mod credential;
pub mod encrypted_image;
#[cfg(test)]
pub(in crate::core) mod encrypted_image_tests;
pub(in crate::core) mod luks_metadata;
pub mod veracrypt_volume;

pub mod filevault;
