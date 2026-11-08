use crate::error::{Result, UbiDumpError};
use crate::utils::crc::crc32_jamcrc;
use byteorder::{BigEndian, ReadBytesExt};
use std::io::{Cursor, Read};

#[derive(Debug, Clone)]
/// UBI 卷表记录结构
pub struct UbiVtblRecord {
    /// 保留的PEB数量
    pub reserved_pebs: u32,
    /// 对齐大小
    pub alignment: u32,
    /// 数据填充大小
    pub data_pad: u32,
    /// 卷类型
    pub vol_type: u8,
    /// 更新标记
    pub upd_marker: u8,
    /// 名称长度
    pub name_len: u16,
    /// 卷名称
    pub name: Vec<u8>,
    /// 标志
    pub flags: u8,
    /// CRC校验值
    pub _crc: u32,
}

impl UbiVtblRecord {
    /// 卷表记录大小
    pub const RECORD_SIZE: usize = 172;

    /// 解析二进制数据为卷表记录
    pub fn parse(data: &[u8]) -> Result<Self> {
        if data.len() < Self::RECORD_SIZE {
            return Err(UbiDumpError::ParseError(format!(
                "卷表记录数据不足: 预期 {} 字节, 实际 {} 字节",
                Self::RECORD_SIZE,
                data.len()
            )));
        }

        let mut cursor = Cursor::new(data);

        // 读取保留的PEB数量、对齐大小、数据填充大小
        let reserved_pebs = cursor.read_u32::<BigEndian>()?;
        let alignment = cursor.read_u32::<BigEndian>()?;
        let data_pad = cursor.read_u32::<BigEndian>()?;

        // 读取卷类型、更新标记、名称长度
        let vol_type = cursor.read_u8()?;
        let upd_marker = cursor.read_u8()?;
        let name_len = cursor.read_u16::<BigEndian>()?;

        // 读取卷名称（128字节）
        let mut name_buffer = vec![0u8; 128];
        cursor.read_exact(&mut name_buffer)?;

        // 实际名称长度
        let actual_name_len = name_len as usize;
        if actual_name_len > 127 {
            return Err(UbiDumpError::ParseError(
                "UBI volume name exceeds 127 bytes".into(),
            ));
        }
        let name = name_buffer[..actual_name_len].to_vec();

        // 读取标志
        let flags = cursor.read_u8()?;

        // 跳过23字节保留字段
        cursor.set_position(cursor.position() + 23);

        // 读取CRC
        let crc = cursor.read_u32::<BigEndian>()?;

        // 验证CRC
        let computed_crc = crc32_jamcrc(&data[..Self::RECORD_SIZE - 4]);
        if computed_crc != crc {
            return Err(UbiDumpError::CrcError {
                expected: crc,
                actual: computed_crc,
            });
        }

        Ok(Self {
            reserved_pebs,
            alignment,
            data_pad,
            vol_type,
            upd_marker,
            name_len,
            name,
            flags,
            _crc: crc,
        })
    }

    /// 检查卷表记录是否为空
    pub fn is_empty(&self) -> bool {
        self.reserved_pebs == 0
            && self.alignment == 0
            && self.data_pad == 0
            && self.vol_type == 0
            && self.upd_marker == 0
            && self.name_len == 0
            && self.name.is_empty()
            && self.flags == 0
    }
}

impl std::fmt::Display for UbiVtblRecord {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let name_str = String::from_utf8_lossy(&self.name);
        write!(
            f,
            "VREC: rsvpebs={}, align={}, datapad={}, voltype={}, updmark={}, flags=0x{:x}, name={}",
            self.reserved_pebs,
            self.alignment,
            self.data_pad,
            self.vol_type,
            self.upd_marker,
            self.flags,
            name_str
        )
    }
}
