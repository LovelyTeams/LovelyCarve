/*
 * BSD 2-Clause License
 *
 * Copyright (c) 2021, Khaled Emara
 * All rights reserved.
 *
 * Redistribution and use in source and binary forms, with or without
 * modification, are permitted provided that the following conditions are met:
 *
 * 1. Redistributions of source code must retain the above copyright notice, this
 *    list of conditions and the following disclaimer.
 *
 * 2. Redistributions in binary form must reproduce the above copyright notice,
 *    this list of conditions and the following disclaimer in the documentation
 *    and/or other materials provided with the distribution.
 *
 * THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS"
 * AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE
 * IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE ARE
 * DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT HOLDER OR CONTRIBUTORS BE LIABLE
 * FOR ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL
 * DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR
 * SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER
 * CAUSED AND ON ANY THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY,
 * OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE
 * OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.
 */
#![allow(dead_code)]
pub const XFS_SB_MAGIC: u32 = 0x58465342; // Superblock
pub const XFS_AGF_MAGIC: u32 = 0x58414746; // Free Space
pub const XFS_AGI_MAGIC: u32 = 0x58414749; // Inode Information
pub const XFS_AGFL_MAGIC: u32 = 0x5841464c; // Free Space List
pub const XFS_DINODE_MAGIC: u16 = 0x494e; // Inodes
pub const XFS_DQUOT_MAGIC: u16 = 0x4451; // Quota Inodes
pub const XFS_SYMLINK_MAGIC: u32 = 0x58534c4d; // Symbolic Links
pub const XFS_ABTB_CRC_MAGIC: u32 = 0x41423342; // Free Space by Block B+tree
pub const XFS_ABTC_CRC_MAGIC: u32 = 0x41423343; // Free Space by Size B+tree
pub const XFS_IBT_CRC_MAGIC: u32 = 0x49414233; // Inode B+tree
pub const XFS_FIBT_CRC_MAGIC: u32 = 0x46494233; // Free Inode B+tree
pub const XFS_BMAP_MAGIC: u32 = 0x424d4150; // B+Tree Extent List, V5
pub const XFS_BMAP_CRC_MAGIC: u32 = 0x424d4133; // B+Tree Extent List, V5
pub const XLOG_HEADER_MAGIC_NUM: u32 = 0xfeedbabe; // Log Records
pub const XFS_DA_NODE_MAGIC: u16 = 0xfebe; // Directory/Attribute Node
pub const XFS_DA3_NODE_MAGIC: u16 = 0x3ebe; // Directory/Attribute Node, V5
pub const XFS_DIR2_BLOCK_MAGIC: u32 = 0x58443242; // Block Directory Data
pub const XFS_DIR3_BLOCK_MAGIC: u32 = 0x58444233; // Block Directory Data, V5
pub const XFS_DIR2_DATA_MAGIC: u32 = 0x58443244; // Leaf Directory Data
pub const XFS_DIR3_DATA_MAGIC: u32 = 0x58444433; // Leaf Directory Data, V5
pub const XFS_DIR2_LEAF1_MAGIC: u16 = 0xd2f1; // Leaf Directory
pub const XFS_DIR3_LEAF1_MAGIC: u16 = 0x3df1; // Leaf Directory, V5
pub const XFS_DIR2_LEAFN_MAGIC: u16 = 0xd2ff; // Node Directory
pub const XFS_DIR3_LEAFN_MAGIC: u16 = 0x3dff; // Node Directory, V5
pub const XFS_DIR3_FREE_MAGIC: u32 = 0x58444633; // Node Directory Free Space
pub const XFS_ATTR_LEAF_MAGIC: u16 = 0xfbee; // Leaf Attribute
pub const XFS_ATTR3_LEAF_MAGIC: u16 = 0x3bee; // Leaf Attribute, V5
pub const XFS_ATTR3_RMT_MAGIC: u32 = 0x5841524d; // Remote Attribute Value
pub const XFS_RMAP_CRC_MAGIC: u32 = 0x524d4233; // Reverse Mapping B+tree
pub const XFS_RTRMAP_CRC_MAGIC: u32 = 0x4d415052; // Real-Time Reverse Mapping B+tree
pub const XFS_REFC_CRC_MAGIC: u32 = 0x52334643; // Reference Count B+tree
pub const XFS_MD_MAGIC: u32 = 0x5846534d; // Metadata Dumps

pub type XfsIno = u64; // absolute inode number
pub type XfsOff = i64; // file offset
pub type XfsDaddr = i64; // disk address (sectors)
pub type XfsAgnumber = u32; // AG number
pub type XfsAgblock = u32; // AG relative block number
pub type XfsExtlen = u32; // extent length in blocks
pub type XfsExtnum = i32; // number of extends in a data fork
pub type XfsAextnum = i16; // number of extents in an attribute fork
pub type XfsDablk = u32; // block number for directories and extended attributes
pub type XfsDahash = u32; // hash of a directory file name or extended attribute name
pub type XfsFsblock = u64; // filesystem block number combining AG number
pub type XfsRfsblock = u64; // raw filesystem block number
pub type XfsRtblock = u64; // extent number in the real-time sub-volume
pub type XfsFileoff = u64; // block offset into a file
pub type XfsFilblks = u64; // block count for a file
pub type XfsFsize = i64; // byte size of a file
