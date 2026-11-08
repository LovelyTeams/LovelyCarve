use crate::error::{Result, UbiDumpError};
use crate::ubifs::common::UBI_EC_MAGIC;
use crate::utils::crc::crc32_jamcrc;
use byteorder::{BigEndian, ReadBytesExt};
use std::io::{Cursor, Read};

/// UBI 擦除计数头结构
#[derive(Debug, Clone)]
pub struct UbiEcHeader {
    /// 魔术数字
    pub magic: [u8; 4],
    /// 版本
    pub version: u8,
    /// 擦除计数
    pub erase_count: u64,
    /// VID头偏移量
    pub vid_hdr_ofs: u32,
    /// 数据偏移量
    pub data_ofs: u32,
    /// 镜像序列
    pub image_seq: u32,
    /// CRC校验值
    pub _hdr_crc: u32,
}

impl UbiEcHeader {
    /// EC头大小
    pub const HDR_SIZE: usize = 64;

    /// 解析二进制数据为EC头
    pub fn parse(data: &[u8]) -> Result<Self> {
        if data.len() < Self::HDR_SIZE {
            return Err(UbiDumpError::ParseError(format!(
                "EC头数据不足: 预期 {} 字节, 实际 {} 字节",
                Self::HDR_SIZE,
                data.len()
            )));
        }

        let mut cursor = Cursor::new(data);

        // 读取魔术数字
        let mut magic = [0u8; 4];
        cursor.read_exact(&mut magic)?;

        // 验证魔术数字
        if magic != *UBI_EC_MAGIC {
            return Err(UbiDumpError::ParseError(format!(
                "EC头魔术数字错误: 预期 {:?}, 实际 {:?}",
                UBI_EC_MAGIC, magic
            )));
        }

        // 读取版本
        let version = cursor.read_u8()?;

        // 跳过3字节保留字段
        cursor.set_position(cursor.position() + 3);

        // 读取擦除计数
        let erase_count = cursor.read_u64::<BigEndian>()?;

        // 读取VID头偏移量
        let vid_hdr_ofs = cursor.read_u32::<BigEndian>()?;

        // 读取数据偏移量
        let data_ofs = cursor.read_u32::<BigEndian>()?;

        // 读取镜像序列
        let image_seq = cursor.read_u32::<BigEndian>()?;

        // 跳过32字节保留字段
        cursor.set_position(cursor.position() + 32);

        // 读取CRC
        let hdr_crc = cursor.read_u32::<BigEndian>()?;

        // 验证CRC
        let computed_crc = crc32_jamcrc(&data[..60]);
        if computed_crc != hdr_crc {
            return Err(UbiDumpError::CrcError {
                expected: hdr_crc,
                actual: computed_crc,
            });
        }

        Ok(Self {
            magic,
            version,
            erase_count,
            vid_hdr_ofs,
            data_ofs,
            image_seq,
            _hdr_crc: hdr_crc,
        })
    }
}

impl std::fmt::Display for UbiEcHeader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "EC: magic={:?}, v{}, ec={}, vidhdr=0x{:x}, data=0x{:x}, imgseq=0x{:x}",
            self.magic,
            self.version,
            self.erase_count,
            self.vid_hdr_ofs,
            self.data_ofs,
            self.image_seq
        )
    }
}
