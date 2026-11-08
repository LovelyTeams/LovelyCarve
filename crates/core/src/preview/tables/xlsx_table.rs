//! Excel（.xlsx/.xlsm/.xls/.xlsb/.ods）→「原类型视图」逐 sheet 表格模型，供前端
//! XlsxViewer 渲染。解析在后端（与 core/csv_table.rs 同位）：镜像内文件经
//! `fs_tree::read_file_limited` 取字节后，用 **calamine**（值 + 合并 + sheet 可见性，
//! 覆盖全部格式）与 **umya-spreadsheet**（富样式：字体/填充/边框/对齐/数字格式、
//! 内置主题色解析，仅 .xlsx/.xlsm）**优势互补**地复合解析成「逐 sheet 的单元格文本
//! 与内联 CSS」模型。前端只负责渲染表格与关键字高亮（与 CsvViewer / SQLite / PE /
//! GGUF 原类型视图同构）。
//!
//! 取证原则同 docx：单元格底色 / 字色 / 边框是文件自带的「证据原貌」，忠实呈现、
//! 绝不令牌化。值/格式/合并/可见性的语义对齐原前端 `src/lib/office/xlsx.ts`
//! （exceljs 移植），故迁到后端后渲染观感保持一致。
//!
//! 安全：umya 的 reader 对「合法 ZIP 但畸形 OOXML」会 `panic`（缺 sheetId/name 等），
//! 故样式叠加包在 `std::panic::catch_unwind` 里——失败即降级为「仅值、空样式」，
//! 绝不让单个坏文件拖垮命令（release 已切 `panic = "unwind"` 使其在发行版也生效）。
//! calamine 走 owned `worksheet_range`（不碰对 Xls/Ods `unimplemented!()` 的零拷贝
//! `*_ref`），其失败均以 `Err` 返回。

mod workbook;
pub use workbook::parse;
mod format;
use format::*;
mod style;
use style::*;

use std::collections::{HashMap, HashSet};
use std::io::Cursor;
use std::panic::{catch_unwind, AssertUnwindSafe};

use calamine::{open_workbook_auto_from_rs, Data, ExcelDateTime, Reader, SheetVisible, Sheets};
use chrono::{Datelike, NaiveDateTime, Timelike};
use serde::Serialize;
use umya_spreadsheet::EnumTrait;

/// 每 sheet 行上限（对齐原前端 DEFAULT_ROW_CAP）。超出截断显示并打标。
pub const ROW_CAP: usize = 2000;
/// 每 sheet 列上限（对齐原前端 DEFAULT_COL_CAP）。兜住 Excel 常见的成百上千「幽灵空列」。
pub const COL_CAP: usize = 200;
/// 跨所有 sheet 的物化单元格总上限。即便每 sheet 在 ROW_CAP×COL_CAP 内，多 sheet
/// 累加仍可能撑爆 IPC payload——达到此界后停止物化后续行（前端原 JS 路径无此约束）。
pub const TOTAL_CELL_CAP: usize = 400_000;

/// 富样式（umya 整本物化）+ 合并（整表 XML 扫描）的「重活预算」：二者皆 O(整表)。仅当
/// 工作簿落在预览窗口内（无 sheet 超 ROW_CAP/COL_CAP）且窗口内非空格总数 ≤ 此界时才做；
/// 对大 / 高表跳过、只给值，保证预览秒级响应（前端提示「已省略样式 / 合并」）。
pub const HEAVY_CELL_BUDGET: usize = 200_000;

/// 一个可见单元格：纯文本 + 已拼好的内联 CSS（颜色/字体/对齐/边框）。被合并覆盖的
/// 单元格不出现在 rows 里（锚点单元格带 rowspan/colspan，浏览器据此铺出合并区）。
/// camelCase 后即 `{ text, style, rowspan?, colspan? }`，与前端原 RawCell 等价。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct XlsxCell {
    /// 单元格显示文本（已按数字/日期格式化；空单元格为空串）。
    pub text: String,
    /// 内联 CSS 字符串（单元格 `style` 属性值）；无样式时为空串（.xls/.xlsb/.ods 恒为空）。
    pub style: String,
    /// 合并锚点的行跨度（>1 时存在；被覆盖单元格已从 rows 移除）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rowspan: Option<u32>,
    /// 合并锚点的列跨度。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub colspan: Option<u32>,
}

/// 一个工作表。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct XlsxSheet {
    /// 工作表名。
    pub name: String,
    /// 隐藏 / 极隐藏（visibility ≠ Visible）——取证向标注，揭示被藏起来的工作表。
    pub hidden: bool,
    /// 极隐藏（VeryHidden，仅 VBA 可设回可见）——比普通隐藏更可疑，前端可加强标注。
    pub very_hidden: bool,
    /// 逐行的可见单元格（已去掉被合并覆盖者；按 col_count 补齐成矩形）。至多 ROW_CAP 行。
    pub rows: Vec<Vec<XlsxCell>>,
    /// 渲染列数（A、B、C… 表头按此生成；已按 COL_CAP 截断）。
    pub col_count: usize,
    /// 真实行数超过 ROW_CAP，rows 只含前 ROW_CAP 行。
    pub truncated_rows: bool,
    /// 真实列数超过 COL_CAP，rows 只含前 COL_CAP 列。
    pub truncated_cols: bool,
}

/// 解析结果（整本工作簿）。字段 camelCase 序列化，对应前端 XlsxTable 接口。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct XlsxTable {
    /// 逐 sheet 模型（工作簿顺序）。
    pub sheets: Vec<XlsxSheet>,
    /// 检测出的格式标签："xlsx" / "xlsm" / "xls" / "xlsb" / "ods"。
    pub format: String,
    /// 本格式不提供富样式（.xls/.xlsb/.ods）——前端据此显示「样式不可用」提示。
    pub styles_unavailable: bool,
    /// 因体量过大（超窗或窗口内格数超预算）跳过了富样式 + 合并解析、仅渲染值以加速——
    /// xlsx/xlsm 本可有样式时为 true，前端据此提示。
    pub heavy_skipped: bool,
    /// 文件大于读取上限，仅解析了前缀字节（截断的 ZIP/复合文档可能解析失败或不完整）。
    pub truncated_bytes: bool,
    /// 因 TOTAL_CELL_CAP 提前停止物化后续行（整本太大）。
    pub truncated_total: bool,
}

/// 一个单元格的 umya 样式叠加信息：内联 CSS + 数字格式串（供 calamine 值格式化）。
struct CellMeta {
    css: String,
    /// numFmt 格式码（如 "#,##0.00"、"yyyy-mm-dd"）；空串表示无格式。
    numfmt: String,
}

#[cfg(test)]
mod tests;
