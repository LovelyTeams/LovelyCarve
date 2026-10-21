//! evidence / readers domain modules.

#[cfg(test)]
pub(in crate::core) mod ad1_evidence_tests;
pub mod ad1_image;
pub mod docker_image;
pub(in crate::core) mod docker_manifest;
pub mod ewf_image;
pub mod folder_image;
pub mod ldplayer_image;
pub(crate) mod load_reader;
pub mod mi_bak;
pub mod optical_disc;
pub mod pmfx;
pub mod sevenz_image;
pub mod tar_image;
pub mod vhd_image;
pub mod vmdk_image;
pub mod vmdk_sparse;
pub mod zip_image;
