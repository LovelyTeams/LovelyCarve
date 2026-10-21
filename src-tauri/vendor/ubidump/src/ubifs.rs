pub mod ec_header;
pub mod vid_header;
pub mod vtbl_record;
pub mod common {
    pub const UBI_EC_MAGIC: &[u8; 4] = b"UBI#";
    pub const UBI_VID_MAGIC: &[u8; 4] = b"UBI!";
    pub const VTBL_VOLID: u32 = 0x7fffefff;
}
