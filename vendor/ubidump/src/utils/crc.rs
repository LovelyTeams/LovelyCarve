use crc::{Algorithm, Crc};

/// CRC32-JAMCRC 算法配置
static CRC32_JAMCRC: Crc<u32> = Crc::<u32>::new(&Algorithm {
    width: 32,
    poly: 0x04C11DB7,
    init: 0xFFFFFFFF,
    refin: true,
    refout: true,
    xorout: 0x00000000,
    check: 0x340BC6D9,
    residue: 0x00000000,
});

/// CRC32-JAMCRC 计算结果
pub type Crc32Jamcrc = u32;

/// 计算数据的 CRC32-JAMCRC 校验值
///
/// # 参数
/// * `data` - 要计算 CRC 的数据
///
/// # 返回值
/// 计算得到的 CRC32-JAMCRC 校验值
pub fn crc32_jamcrc(data: &[u8]) -> Crc32Jamcrc {
    CRC32_JAMCRC.checksum(data)
}
