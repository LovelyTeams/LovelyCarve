use crate::error::{Result, UbiDumpError};
use crate::ubifs::common::UBI_VID_MAGIC;
use crate::utils::crc::crc32_jamcrc;
use byteorder::{BigEndian, ReadBytesExt};
use std::io::{Cursor, Read};

/// UBI 卷ID头结构
#[derive(Debug, Clone)]
pub struct UbiVidHead {
    /// 魔术数字
    pub magic: [u8; 4],
    /// 版本
    pub version: u8,
    /// 卷类型
    pub vol_type: u8,
    /// 复制标志
    pub copy_flag: u8,
    /// 兼容性
    pub compat: u8,
    /// 卷ID
    pub vol_id: u32,
    /// 逻辑块号
    pub lnum: u32,
    /// 数据大小
    pub data_size: u32,
    /// 使用的EB数量
    pub used_ebs: u32,
    /// 数据填充大小
    pub data_pad: u32,
    /// 数据CRC
    pub data_crc: u32,
    /// 序列号
    pub sqnum: u64,
    /// 头CRC
    pub _hdr_crc: u32,
}

impl UbiVidHead {
    /// VID头大小
    pub const HDR_SIZE: usize = 64;

    /// 解析二进制数据为VID头
    pub fn parse(data: &[u8]) -> Result<Self> {
        if data.len() < Self::HDR_SIZE {
            return Err(UbiDumpError::ParseError(format!(
                "VID头数据不足: 预期 {} 字节, 实际 {} 字节",
                Self::HDR_SIZE,
                data.len()
            )));
        }

        let mut cursor = Cursor::new(data);

        // 读取魔术数字
        let mut magic = [0u8; 4];
        cursor.read_exact(&mut magic)?;

        // 验证魔术数字
        if magic != *UBI_VID_MAGIC {
            return Err(UbiDumpError::ParseError(format!(
                "VID头魔术数字错误: 预期 {:?}, 实际 {:?}",
                UBI_VID_MAGIC, magic
            )));
        }

        // 读取版本、卷类型、复制标志、兼容性
        let version = cursor.read_u8()?;
        let vol_type = cursor.read_u8()?;
        let copy_flag = cursor.read_u8()?;
        let compat = cursor.read_u8()?;

        // 读取卷ID和逻辑块号
        let vol_id = cursor.read_u32::<BigEndian>()?;
        let lnum = cursor.read_u32::<BigEndian>()?;

        // 跳过4字节保留字段
        cursor.set_position(cursor.position() + 4);

        // 读取数据大小、使用的EB数量、数据填充大小、数据CRC
        let data_size = cursor.read_u32::<BigEndian>()?;
        let used_ebs = cursor.read_u32::<BigEndian>()?;
        let data_pad = cursor.read_u32::<BigEndian>()?;
        let data_crc = cursor.read_u32::<BigEndian>()?;

        // 跳过4字节保留字段
        cursor.set_position(cursor.position() + 4);

        // 读取序列号
        let sqnum = cursor.read_u64::<BigEndian>()?;

        // 跳过12字节保留字段
        cursor.set_position(cursor.position() + 12);

        // 读取头CRC
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
            vol_type,
            copy_flag,
            compat,
            vol_id,
            lnum,
            data_size,
            used_ebs,
            data_pad,
            data_crc,
            sqnum,
            _hdr_crc: hdr_crc,
        })
    }
}

impl std::fmt::Display for UbiVidHead {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "VID: magic={:?}, v{}, vt={}, cp={}, compat={}, volid=0x{:x}, lnum={}, dsize={}, usedebs={}, datapad={}, datacrc=0x{:x}, sqnum=0x{:x}",
            self.magic,
            self.version,
            self.vol_type,
            self.copy_flag,
            self.compat,
            self.vol_id,
            self.lnum,
            self.data_size,
            self.used_ebs,
            self.data_pad,
            self.data_crc,
            self.sqnum
        )
    }
}
