//! GGUF（llama.cpp / ggml 的大模型权重容器）解析器，为「原类型视图」提供
//! 结构化报告。
//!
//! 手写解析（仅依赖已有的 `byteorder`）：只读取文件**前部**的头部 + 元数据 KV +
//! 张量信息，**绝不读取张量权重数据本体**——一个 7B 模型的头区通常只有几百 KB，
//! 而权重有数 GB。覆盖 GGUF v2 / v3（两者头区布局一致）；v1 用 u32 计数/长度，
//! 早已绝迹，明确报「不支持」而非半解析出乱码。
//!
//! 取证容错：未识别的张量量化类型只记入 `parse_warnings`（按 `Unknown(id)` 显示），
//! 不影响其余字段；只有「根本不是 GGUF」「版本不支持」「头区被截断 / 损坏」才返回
//! `Err`。所有计数、长度都来自镜像内任意文件，按敌意数据对待——**绝不**用文件声明
//! 的计数去 `Vec::with_capacity` 预分配，而是按需 push、越界即
//! 报错；超大数组（如十万级 token 词表）只保留前若干元素的预览，其余跳过不分配。

use std::io::{Cursor, Read};

use anyhow::{bail, Result};
use byteorder::{LittleEndian, ReadBytesExt};
use serde::Serialize;

/// Magic：ASCII `GGUF` 按小端 u32 读出。
const GGUF_MAGIC: u32 = 0x4655_4747;

// ── GGUF 元数据值类型标签（u32） ───────────────────────────────────────────
const VT_UINT8: u32 = 0;
const VT_INT8: u32 = 1;
const VT_UINT16: u32 = 2;
const VT_INT16: u32 = 3;
const VT_UINT32: u32 = 4;
const VT_INT32: u32 = 5;
const VT_FLOAT32: u32 = 6;
const VT_BOOL: u32 = 7;
const VT_STRING: u32 = 8;
const VT_ARRAY: u32 = 9;
const VT_UINT64: u32 = 10;
const VT_INT64: u32 = 11;
const VT_FLOAT64: u32 = 12;

/// 数组值在报告里保留的预览元素数上限——tokenizer 词表 / merges 动辄十万级，
/// 全量既无取证意义又会撑爆前端，只留头部样本。
const MAX_ARRAY_PREVIEW: usize = 16;
/// 张量表保留的条目数上限（超大模型张量可达数千；`total_parameters` 与
/// `tensor_types` 汇总仍遍历全部张量，截断只影响明细表）。
const MAX_TENSORS: usize = 2048;
/// 单个 GGUF 字符串的长度上限——防御损坏的长度域触发巨量分配。键、字符串值、
/// 张量名都远小于此。
const MAX_STRING_LEN: u64 = 64 * 1024 * 1024;
/// 张量维度数上限——ggml 实际 ≤4，放宽到 8 留余量并防御损坏的 n_dims。
const MAX_DIMS: u32 = 8;
/// 元数据声明缺省时的张量数据对齐（GGUF 约定 `general.alignment` 默认 32）。
const DEFAULT_ALIGNMENT: u64 = 32;

#[derive(Debug, Serialize)]
pub struct GgufReport {
    pub version: u32,
    /// 张量总数（文件头声明值，可能 > `tensors.len()`，见 `tensors_truncated`）。
    pub tensor_count: u64,
    pub metadata_kv_count: u64,
    pub file_size: u64,
    /// 张量数据对齐（`general.alignment` 或默认 32）。
    pub alignment: u64,
    /// 张量数据区起点 = 张量信息区结束位置按 `alignment` 上取整。
    pub tensor_data_offset: u64,
    /// 全部张量元素数（各维度之积）之和——即模型参数量。
    pub total_parameters: u64,
    /// `general.architecture`，如 `llama` / `qwen2` / `hunyuan-dense`。
    pub architecture: Option<String>,
    /// `general.name`。
    pub name: Option<String>,
    /// `general.file_type`（量化方案 ID）。
    pub file_type: Option<u32>,
    /// `file_type` 的可读量化名，如 `Q4_K_M` / `BF16`。
    pub quantization: Option<String>,
    /// 按 ggml 张量类型聚合的分布（遍历全部张量统计）。
    pub tensor_types: Vec<TypeCount>,
    /// 全部元数据 KV（标量给 `value`；数组给 `array_len` + 头部预览）。
    pub metadata: Vec<MetaEntry>,
    /// 张量明细（最多 `MAX_TENSORS` 条）。
    pub tensors: Vec<TensorEntry>,
    /// 张量明细是否因上限被截断。
    pub tensors_truncated: bool,
    pub parse_warnings: Vec<String>,
}

/// 按 ggml 类型聚合的张量分布项。
#[derive(Debug, Serialize)]
pub struct TypeCount {
    pub type_name: String,
    pub count: u64,
    pub parameters: u64,
}

/// 单条元数据 KV。
#[derive(Debug, Serialize)]
pub struct MetaEntry {
    pub key: String,
    /// 值类型可读名：标量如 `string` / `u32` / `f32`；数组如 `string[]` / `f32[]`。
    pub type_name: String,
    /// 标量值的展示字符串；数组为空串（看 `array_len` / `array_preview`）。
    pub value: String,
    /// `Some(n)` 表示该值是数组，`n` 为元素总数。
    pub array_len: Option<u64>,
    /// 数组前 `MAX_ARRAY_PREVIEW` 个元素的展示字符串。
    pub array_preview: Option<Vec<String>>,
}

/// 单个张量的元信息（不含权重数据本体）。
#[derive(Debug, Serialize)]
pub struct TensorEntry {
    pub name: String,
    /// ggml 类型名，如 `BF16` / `Q4_K`；未识别为 `Unknown(id)`。
    pub type_name: String,
    pub dimensions: Vec<u64>,
    /// 元素数 = 各维度之积。
    pub parameters: u64,
    /// 相对张量数据区起点的偏移。
    pub offset: u64,
}

/// 解析 GGUF 头区。`data` 是文件前部（可能被读取上限截断）；`file_size` 是真实
/// 文件大小（用于一致性校验与展示）。
pub fn parse(data: &[u8], file_size: u64) -> Result<GgufReport> {
    let mut cur = Cursor::new(data);

    let magic = cur
        .read_u32::<LittleEndian>()
        .map_err(|_| anyhow::anyhow!("文件过短，不是 GGUF 文件"))?;
    if magic != GGUF_MAGIC {
        bail!("不是 GGUF 文件（magic = 0x{magic:08X}）");
    }

    let version = cur.read_u32::<LittleEndian>()?;
    if version != 2 && version != 3 {
        bail!("不支持的 GGUF 版本 v{version}（当前仅支持 v2 / v3）");
    }

    let tensor_count = cur.read_u64::<LittleEndian>()?;
    let metadata_kv_count = cur.read_u64::<LittleEndian>()?;

    let mut warnings: Vec<String> = Vec::new();

    // ── 元数据 KV ──────────────────────────────────────────────────────────
    // 计数不可信：不预分配，按需 push；任一项解析失败（多因头区超出读取上限或
    // 文件损坏）即整体报错——流位置一旦失准，后续 KV / 张量都无从续读。
    let mut metadata: Vec<MetaEntry> = Vec::new();
    for i in 0..metadata_kv_count {
        match parse_metadata_kv(&mut cur) {
            Ok(entry) => metadata.push(entry),
            Err(e) => bail!(
                "解析第 {} 个元数据项失败（文件损坏或头区超出读取上限）：{e}",
                i + 1
            ),
        }
    }

    // ── 张量信息 ───────────────────────────────────────────────────────────
    // 遍历全部张量做汇总；明细表仅保留前 MAX_TENSORS 条。
    let mut tensors: Vec<TensorEntry> = Vec::new();
    let mut total_parameters: u64 = 0;
    let mut type_acc: Vec<TypeCount> = Vec::new();
    let mut unknown_types: Vec<u32> = Vec::new();
    let mut tensors_truncated = false;

    for i in 0..tensor_count {
        let t = match parse_tensor_info(&mut cur, &mut unknown_types) {
            Ok(t) => t,
            Err(e) => bail!(
                "解析第 {} 个张量信息失败（文件损坏或头区超出读取上限）：{e}",
                i + 1
            ),
        };
        total_parameters = total_parameters.saturating_add(t.parameters);
        match type_acc.iter_mut().find(|tc| tc.type_name == t.type_name) {
            Some(tc) => {
                tc.count += 1;
                tc.parameters = tc.parameters.saturating_add(t.parameters);
            }
            None => type_acc.push(TypeCount {
                type_name: t.type_name.clone(),
                count: 1,
                parameters: t.parameters,
            }),
        }
        if tensors.len() < MAX_TENSORS {
            tensors.push(t);
        } else {
            tensors_truncated = true;
        }
    }

    // ── 派生 / 汇总字段 ─────────────────────────────────────────────────────
    let alignment = numeric_meta(&metadata, "general.alignment")
        .filter(|&a| a > 0)
        .unwrap_or(DEFAULT_ALIGNMENT);
    let info_end = cur.position();
    let tensor_data_offset = info_end.div_ceil(alignment).saturating_mul(alignment);

    let architecture = string_meta(&metadata, "general.architecture");
    let name = string_meta(&metadata, "general.name");
    let file_type =
        numeric_meta(&metadata, "general.file_type").and_then(|v| u32::try_from(v).ok());
    let quantization = file_type.map(|ft| file_type_name(ft).to_string());

    for id in unknown_types {
        warnings.push(format!("张量含未识别的 ggml 类型 id={id}，按 Unknown 显示"));
    }
    if file_size > 0 && tensor_data_offset > file_size {
        warnings.push(format!(
            "张量数据起点 {tensor_data_offset} 超过文件实际大小 {file_size}，文件可能被截断"
        ));
    }

    Ok(GgufReport {
        version,
        tensor_count,
        metadata_kv_count,
        file_size,
        alignment,
        tensor_data_offset,
        total_parameters,
        architecture,
        name,
        file_type,
        quantization,
        tensor_types: type_acc,
        metadata,
        tensors,
        tensors_truncated,
        parse_warnings: warnings,
    })
}

// ── 元数据解析 ─────────────────────────────────────────────────────────────

fn parse_metadata_kv(cur: &mut Cursor<&[u8]>) -> Result<MetaEntry> {
    let key = read_string(cur)?;
    let vtype = cur.read_u32::<LittleEndian>()?;

    if vtype == VT_ARRAY {
        let elem_type = cur.read_u32::<LittleEndian>()?;
        if elem_type == VT_ARRAY {
            bail!("GGUF 不支持嵌套数组");
        }
        let count = cur.read_u64::<LittleEndian>()?;
        // 只取前 MAX_ARRAY_PREVIEW 个元素，其余跳过不分配——词表数组可达十万级。
        let mut preview: Vec<String> = Vec::new();
        for i in 0..count {
            if (i as usize) < MAX_ARRAY_PREVIEW {
                preview.push(read_scalar_display(cur, elem_type)?);
            } else {
                skip_scalar(cur, elem_type)?;
            }
        }
        return Ok(MetaEntry {
            key,
            type_name: format!("{}[]", value_type_name(elem_type)),
            value: String::new(),
            array_len: Some(count),
            array_preview: Some(preview),
        });
    }

    let value = read_scalar_display(cur, vtype)?;
    Ok(MetaEntry {
        key,
        type_name: value_type_name(vtype).to_string(),
        value,
        array_len: None,
        array_preview: None,
    })
}

/// 读取一个标量值并返回其展示字符串。GGUF 数组元素类型保证是标量 / 字符串
/// （规范禁止嵌套数组）。浮点用各自原生 `Display`（最短往返表示）。
fn read_scalar_display(cur: &mut Cursor<&[u8]>, vtype: u32) -> Result<String> {
    Ok(match vtype {
        VT_UINT8 => cur.read_u8()?.to_string(),
        VT_INT8 => cur.read_i8()?.to_string(),
        VT_UINT16 => cur.read_u16::<LittleEndian>()?.to_string(),
        VT_INT16 => cur.read_i16::<LittleEndian>()?.to_string(),
        VT_UINT32 => cur.read_u32::<LittleEndian>()?.to_string(),
        VT_INT32 => cur.read_i32::<LittleEndian>()?.to_string(),
        VT_FLOAT32 => cur.read_f32::<LittleEndian>()?.to_string(),
        VT_BOOL => (cur.read_u8()? != 0).to_string(),
        VT_STRING => read_string(cur)?,
        VT_UINT64 => cur.read_u64::<LittleEndian>()?.to_string(),
        VT_INT64 => cur.read_i64::<LittleEndian>()?.to_string(),
        VT_FLOAT64 => cur.read_f64::<LittleEndian>()?.to_string(),
        VT_ARRAY => bail!("GGUF 不支持嵌套数组"),
        other => bail!("未知的 GGUF 值类型：{other}"),
    })
}

/// 跳过一个标量值（不分配）——用于数组里超出预览窗口的元素。
fn skip_scalar(cur: &mut Cursor<&[u8]>, vtype: u32) -> Result<()> {
    match scalar_size(vtype) {
        Some(n) => advance(cur, n),
        None if vtype == VT_STRING => {
            let len = cur.read_u64::<LittleEndian>()?;
            if len > MAX_STRING_LEN {
                bail!("GGUF 字符串长度异常：{len}");
            }
            advance(cur, len)
        }
        None => bail!("数组元素类型非法：{vtype}"),
    }
}

/// 标量值类型的定长字节数；字符串 / 数组 / 未知返回 None。
fn scalar_size(vtype: u32) -> Option<u64> {
    match vtype {
        VT_UINT8 | VT_INT8 | VT_BOOL => Some(1),
        VT_UINT16 | VT_INT16 => Some(2),
        VT_UINT32 | VT_INT32 | VT_FLOAT32 => Some(4),
        VT_UINT64 | VT_INT64 | VT_FLOAT64 => Some(8),
        _ => None,
    }
}

/// 前移游标 `n` 字节，越界即报错（不读出内容）。
fn advance(cur: &mut Cursor<&[u8]>, n: u64) -> Result<()> {
    let new = cur.position().saturating_add(n);
    if new > cur.get_ref().len() as u64 {
        bail!("读取越界（文件被截断或头区超出读取上限）");
    }
    cur.set_position(new);
    Ok(())
}

/// 读取 GGUF 字符串：u64 长度 + UTF-8 字节（非 \0 结尾，长度权威）。非法 UTF-8
/// 用替换符兜底而非报错——取证场景宁可显示残缺也不整体失败。
fn read_string(cur: &mut Cursor<&[u8]>) -> Result<String> {
    let len = cur.read_u64::<LittleEndian>()?;
    if len > MAX_STRING_LEN {
        bail!("GGUF 字符串长度异常：{len}");
    }
    let mut buf = vec![0u8; len as usize];
    cur.read_exact(&mut buf)?;
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

// ── 张量信息解析 ───────────────────────────────────────────────────────────

fn parse_tensor_info(cur: &mut Cursor<&[u8]>, unknown_types: &mut Vec<u32>) -> Result<TensorEntry> {
    let name = read_string(cur)?;

    let n_dims = cur.read_u32::<LittleEndian>()?;
    if n_dims > MAX_DIMS {
        bail!("张量维度数异常：{n_dims}");
    }
    let mut dimensions = Vec::with_capacity(n_dims as usize);
    for _ in 0..n_dims {
        dimensions.push(cur.read_u64::<LittleEndian>()?);
    }

    let type_id = cur.read_u32::<LittleEndian>()?;
    let type_name = match ggml_type_name(type_id) {
        Some(s) => s.to_string(),
        None => {
            if !unknown_types.contains(&type_id) {
                unknown_types.push(type_id);
            }
            format!("Unknown({type_id})")
        }
    };

    let offset = cur.read_u64::<LittleEndian>()?;
    let parameters = dimensions
        .iter()
        .copied()
        .fold(1u64, |acc, d| acc.saturating_mul(d));

    Ok(TensorEntry {
        name,
        type_name,
        dimensions,
        parameters,
        offset,
    })
}

// ── 元数据查表辅助 ─────────────────────────────────────────────────────────

/// 取某个字符串型标量元数据的值。
fn string_meta(meta: &[MetaEntry], key: &str) -> Option<String> {
    meta.iter()
        .find(|m| m.key == key && m.array_len.is_none() && m.type_name == "string")
        .map(|m| m.value.clone())
}

/// 取某个整型标量元数据的值（按其展示字符串解析为 u64）。
fn numeric_meta(meta: &[MetaEntry], key: &str) -> Option<u64> {
    meta.iter()
        .find(|m| m.key == key && m.array_len.is_none())
        .and_then(|m| m.value.parse::<u64>().ok())
}

// ── 类型名映射 ─────────────────────────────────────────────────────────────

fn value_type_name(vtype: u32) -> &'static str {
    match vtype {
        VT_UINT8 => "u8",
        VT_INT8 => "i8",
        VT_UINT16 => "u16",
        VT_INT16 => "i16",
        VT_UINT32 => "u32",
        VT_INT32 => "i32",
        VT_FLOAT32 => "f32",
        VT_BOOL => "bool",
        VT_STRING => "string",
        VT_ARRAY => "array",
        VT_UINT64 => "u64",
        VT_INT64 => "i64",
        VT_FLOAT64 => "f64",
        _ => "unknown",
    }
}

/// ggml 张量数据类型名（镜像 ggml 的类型枚举；未识别返回 None → `Unknown(id)`）。
fn ggml_type_name(id: u32) -> Option<&'static str> {
    Some(match id {
        0 => "F32",
        1 => "F16",
        2 => "Q4_0",
        3 => "Q4_1",
        6 => "Q5_0",
        7 => "Q5_1",
        8 => "Q8_0",
        9 => "Q8_1",
        10 => "Q2_K",
        11 => "Q3_K",
        12 => "Q4_K",
        13 => "Q5_K",
        14 => "Q6_K",
        15 => "Q8_K",
        16 => "IQ2_XXS",
        17 => "IQ2_XS",
        18 => "IQ3_XXS",
        19 => "IQ1_S",
        20 => "IQ4_NL",
        21 => "IQ3_S",
        22 => "IQ2_S",
        23 => "IQ4_XS",
        24 => "I8",
        25 => "I16",
        26 => "I32",
        27 => "I64",
        28 => "F64",
        29 => "IQ1_M",
        30 => "BF16",
        // 31/32/33 = 已移除的 Q4_0_4_4 / _4_8 / _8_8（运行时重打包，文件中不再出现）。
        34 => "TQ1_0",
        35 => "TQ2_0",
        39 => "MXFP4",
        _ => return None,
    })
}

/// `general.file_type`（量化方案 ID）→ 可读量化名。镜像 llama.cpp 的 `llama_ftype`
/// 枚举（注意：与上面的 ggml 张量类型是两套不同的编号）。
fn file_type_name(file_type: u32) -> &'static str {
    match file_type {
        0 => "F32",
        1 => "F16",
        2 => "Q4_0",
        3 => "Q4_1",
        // 4/5/6 = 已移除（Q4_1_SOME_F16 / Q4_2 / Q4_3）。
        7 => "Q8_0",
        8 => "Q5_0",
        9 => "Q5_1",
        10 => "Q2_K",
        11 => "Q3_K_S",
        12 => "Q3_K_M",
        13 => "Q3_K_L",
        14 => "Q4_K_S",
        15 => "Q4_K_M",
        16 => "Q5_K_S",
        17 => "Q5_K_M",
        18 => "Q6_K",
        19 => "IQ2_XXS",
        20 => "IQ2_XS",
        21 => "Q2_K_S",
        22 => "IQ3_XS",
        23 => "IQ3_XXS",
        24 => "IQ1_S",
        25 => "IQ4_NL",
        26 => "IQ3_S",
        27 => "IQ3_M",
        28 => "IQ2_S",
        29 => "IQ2_M",
        30 => "IQ4_XS",
        31 => "IQ1_M",
        32 => "BF16",
        // 33/34/35 = 旧文件里的 Q4_0_4_4 / _4_8 / _8_8（后续移除，保留以识别历史文件）。
        33 => "Q4_0_4_4",
        34 => "Q4_0_4_8",
        35 => "Q4_0_8_8",
        36 => "TQ1_0",
        37 => "TQ2_0",
        38 => "MXFP4_MOE",
        39 => "NVFP4",
        40 => "Q1_0",
        _ => "Unknown",
    }
}

// ── 单元测试 ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn put_str(b: &mut Vec<u8>, s: &str) {
        b.extend_from_slice(&(s.len() as u64).to_le_bytes());
        b.extend_from_slice(s.as_bytes());
    }

    /// 合成一个最小 GGUF v3：2 个元数据 KV（架构字符串 + file_type=29 BF16）+
    /// 1 个张量（token_embd.weight，维度 [4,8]，BF16）。
    fn minimal_gguf() -> Vec<u8> {
        let mut b = Vec::new();
        b.extend_from_slice(&GGUF_MAGIC.to_le_bytes());
        b.extend_from_slice(&3u32.to_le_bytes()); // version
        b.extend_from_slice(&1u64.to_le_bytes()); // tensor_count
        b.extend_from_slice(&2u64.to_le_bytes()); // metadata_kv_count

        put_str(&mut b, "general.architecture");
        b.extend_from_slice(&VT_STRING.to_le_bytes());
        put_str(&mut b, "llama");

        put_str(&mut b, "general.file_type");
        b.extend_from_slice(&VT_UINT32.to_le_bytes());
        b.extend_from_slice(&32u32.to_le_bytes()); // BF16 (llama_ftype 32)

        put_str(&mut b, "token_embd.weight");
        b.extend_from_slice(&2u32.to_le_bytes()); // n_dims
        b.extend_from_slice(&4u64.to_le_bytes()); // dim 0
        b.extend_from_slice(&8u64.to_le_bytes()); // dim 1
        b.extend_from_slice(&30u32.to_le_bytes()); // ggml type BF16
        b.extend_from_slice(&0u64.to_le_bytes()); // offset

        // 对齐填充 + 张量数据本体（BF16 [4,8] = 32 元素 × 2 字节）——真实文件里
        // 张量数据起点必落在 file_size 之内，补上才不触发「超出 EOF」截断告警。
        b.resize(b.len().div_ceil(32) * 32, 0);
        b.extend_from_slice(&[0u8; 64]);
        b
    }

    #[test]
    fn parses_minimal_gguf() {
        let buf = minimal_gguf();
        let r = parse(&buf, buf.len() as u64).unwrap();
        assert_eq!(r.version, 3);
        assert_eq!(r.tensor_count, 1);
        assert_eq!(r.metadata_kv_count, 2);
        assert_eq!(r.architecture.as_deref(), Some("llama"));
        assert_eq!(r.file_type, Some(32));
        assert_eq!(r.quantization.as_deref(), Some("BF16"));
        assert_eq!(r.total_parameters, 32); // 4 * 8
        assert_eq!(r.tensors.len(), 1);
        assert_eq!(r.tensors[0].type_name, "BF16");
        assert_eq!(r.tensors[0].parameters, 32);
        assert_eq!(r.tensor_types.len(), 1);
        assert_eq!(r.tensor_types[0].type_name, "BF16");
        assert_eq!(r.tensor_types[0].count, 1);
        assert_eq!(r.alignment, DEFAULT_ALIGNMENT);
        assert!(r.parse_warnings.is_empty());
    }

    #[test]
    fn array_value_is_previewed_and_counted() {
        let mut b = Vec::new();
        b.extend_from_slice(&GGUF_MAGIC.to_le_bytes());
        b.extend_from_slice(&3u32.to_le_bytes());
        b.extend_from_slice(&0u64.to_le_bytes()); // tensor_count
        b.extend_from_slice(&1u64.to_le_bytes()); // metadata_kv_count

        // tokenizer.ggml.tokens = ["a", "bb", "ccc"] (string[])
        put_str(&mut b, "tokenizer.ggml.tokens");
        b.extend_from_slice(&VT_ARRAY.to_le_bytes());
        b.extend_from_slice(&VT_STRING.to_le_bytes()); // elem type
        b.extend_from_slice(&3u64.to_le_bytes()); // count
        put_str(&mut b, "a");
        put_str(&mut b, "bb");
        put_str(&mut b, "ccc");

        let r = parse(&b, b.len() as u64).unwrap();
        let entry = &r.metadata[0];
        assert_eq!(entry.key, "tokenizer.ggml.tokens");
        assert_eq!(entry.type_name, "string[]");
        assert_eq!(entry.array_len, Some(3));
        assert_eq!(
            entry.array_preview.as_ref().unwrap(),
            &vec!["a".to_string(), "bb".to_string(), "ccc".to_string()]
        );
    }

    #[test]
    fn rejects_bad_magic() {
        let mut b = Vec::new();
        b.extend_from_slice(&0xDEAD_BEEFu32.to_le_bytes());
        b.extend_from_slice(&3u32.to_le_bytes());
        b.extend_from_slice(&0u64.to_le_bytes());
        b.extend_from_slice(&0u64.to_le_bytes());
        assert!(parse(&b, b.len() as u64).is_err());
    }

    #[test]
    fn rejects_unsupported_version() {
        let mut b = Vec::new();
        b.extend_from_slice(&GGUF_MAGIC.to_le_bytes());
        b.extend_from_slice(&1u32.to_le_bytes()); // v1 unsupported
        b.extend_from_slice(&0u64.to_le_bytes());
        b.extend_from_slice(&0u64.to_le_bytes());
        assert!(parse(&b, b.len() as u64).is_err());
    }

    #[test]
    fn unknown_tensor_type_warns_not_fails() {
        let mut b = Vec::new();
        b.extend_from_slice(&GGUF_MAGIC.to_le_bytes());
        b.extend_from_slice(&3u32.to_le_bytes());
        b.extend_from_slice(&1u64.to_le_bytes()); // tensor_count
        b.extend_from_slice(&0u64.to_le_bytes()); // metadata_kv_count
        put_str(&mut b, "weird.weight");
        b.extend_from_slice(&1u32.to_le_bytes()); // n_dims
        b.extend_from_slice(&2u64.to_le_bytes()); // dim
        b.extend_from_slice(&9999u32.to_le_bytes()); // unknown ggml type
        b.extend_from_slice(&0u64.to_le_bytes()); // offset

        let r = parse(&b, b.len() as u64).unwrap();
        assert_eq!(r.tensors[0].type_name, "Unknown(9999)");
        assert!(!r.parse_warnings.is_empty());
    }

    #[test]
    fn truncated_header_errors() {
        let mut b = minimal_gguf();
        b.truncate(20); // 砍到头部中段
        assert!(parse(&b, 20).is_err());
    }
}
