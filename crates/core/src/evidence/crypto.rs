//! evidence / crypto domain modules.

pub mod bde_image;
pub mod bde_vmk;
pub mod bitlocker;
pub mod encrypted_image;
#[cfg(test)]
pub(crate) mod encrypted_image_tests;
pub(crate) mod luks_metadata;

pub mod filevault;
