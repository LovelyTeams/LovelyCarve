//! Workbook parsing, bounded sheet projection and merge metadata.
use super::*;

/// 解析镜像内 Excel 文件字节为逐 sheet 表格 + 内联样式。格式由内容自动检测（看穿
/// 改后缀伪装）。`truncated_bytes` 由调用方传入（文件是否已被读取上限截断）。
pub fn parse(data: &[u8], truncated_bytes: bool) -> anyhow::Result<XlsxTable> {
    // calamine 自动探测格式（按内容魔数，不信扩展名），覆盖 xlsx/xlsm/xls/xlsb/ods。
    // Cursor<&[u8]> 满足 Read+Seek+Clone（&[u8] 为 Copy），零拷贝。
    let mut wb = open_workbook_auto_from_rs(Cursor::new(data)).map_err(|err| {
        anyhow::anyhow!("无法识别为 Excel 工作簿（仅支持 .xlsx/.xlsm/.xls/.xlsb/.ods）：{err}")
    })?;

    // 格式标签 + 是否提供富样式。Sheets::Xlsx 同时覆盖 .xlsx 与 .xlsm（同一 reader），
    // 故 .xlsm 另按 vbaProject.bin 细分（仅用于显示）。
    let (format, styles_unavailable) = match &wb {
        Sheets::Xlsx(_) => (
            if has_vba_project(data) {
                "xlsm"
            } else {
                "xlsx"
            }
            .to_string(),
            false,
        ),
        Sheets::Xls(_) => ("xls".to_string(), true),
        Sheets::Xlsb(_) => ("xlsb".to_string(), true),
        Sheets::Ods(_) => ("ods".to_string(), true),
    };

    // sheet 名 + 可见性（先收成 owned，后续要 &mut 借用 wb 读 range）。
    let sheets_meta: Vec<(String, bool, bool)> = wb
        .sheets_metadata()
        .iter()
        .map(|s| {
            (
                s.name.clone(),
                !matches!(s.visible, SheetVisible::Visible),
                matches!(s.visible, SheetVisible::VeryHidden),
            )
        })
        .collect();

    // ── Phase 1：流式读各 sheet 窗口内的原始值（行升序越过 ROW_CAP 即 break，绝不读完
    // 超高 sheet），同时探测工作簿是否落在预览窗口内。富样式（umya 整本物化）与合并
    // （整表 XML 扫描）都是 O(整表) 的重活，仅对落在窗口内的中小工作簿才做，大/高表只给
    // 值以保证秒级响应。此处先收原始 Data（延迟渲染——numFmt 要等 umya 才知道）。
    struct SheetVals {
        cells: HashMap<(u32, u32), Data>,
        max_row: u32,
        max_col: u32,
        read_ok: bool,
    }
    let mut per_sheet: Vec<SheetVals> = Vec::with_capacity(sheets_meta.len());
    let mut window_overflow = false;
    let mut in_window_cells = 0usize;
    for (name, _h, _vh) in &sheets_meta {
        let mut cells: HashMap<(u32, u32), Data> = HashMap::new();
        let mut max_row = 0u32;
        let mut max_col = 0u32;
        let mut read_ok = true;
        // **安全关键**：calamine 的 worksheet_range 会按使用范围包围盒物化稠密 Vec
        // （vec![Empty; rows*cols]）——仅 A1+XFD1048576 两格即逼出约 170 亿格分配 → abort。
        // 故 OOXML（xlsx/xlsm/xlsb）用流式 cell reader 逐格读、超窗即弃、绝不稠密化，且行
        // 升序越过 ROW_CAP 即 break（性能：不读完超高 sheet）；.xls/.ods 无流式 reader，
        // 退回 worksheet_range（网格远小，放大有限）。
        macro_rules! drain_cells {
            ($reader:expr) => {{
                match $reader {
                    Ok(mut rdr) => {
                        while let Ok(Some(cell)) = rdr.next_cell() {
                            let (r0, c0) = cell.get_position();
                            if r0 as usize >= ROW_CAP {
                                max_row = max_row.max(r0.saturating_add(1));
                                break; // 行升序：越过行上限即停（已标溢出）。
                            }
                            max_row = max_row.max(r0 + 1);
                            max_col = max_col.max(c0.saturating_add(1));
                            if c0 as usize >= COL_CAP {
                                continue;
                            }
                            let data: Data = cell.get_value().clone().into();
                            if matches!(data, Data::Empty) {
                                continue;
                            }
                            cells.insert((r0 + 1, c0 + 1), data);
                        }
                    }
                    Err(_) => read_ok = false,
                }
            }};
        }
        match &mut wb {
            Sheets::Xlsx(x) => drain_cells!(x.worksheet_cells_reader(name)),
            Sheets::Xlsb(x) => drain_cells!(x.worksheet_cells_reader(name)),
            other => match other.worksheet_range(name) {
                Ok(range) => {
                    let (sr, sc) = range.start().unwrap_or((0, 0));
                    let h = range.height() as u32;
                    let w = range.width() as u32;
                    max_row = if h == 0 { 0 } else { sr.saturating_add(h) };
                    max_col = if w == 0 { 0 } else { sc.saturating_add(w) };
                    for (i, row) in range.rows().enumerate() {
                        let r = sr + i as u32 + 1;
                        if r as usize > ROW_CAP {
                            break;
                        }
                        for (j, cell) in row.iter().enumerate() {
                            if matches!(cell, Data::Empty) {
                                continue;
                            }
                            let c = sc + j as u32 + 1;
                            if c as usize > COL_CAP {
                                continue;
                            }
                            cells.insert((r, c), cell.clone());
                        }
                    }
                }
                Err(_) => read_ok = false,
            },
        }
        if max_row as usize > ROW_CAP || max_col as usize > COL_CAP {
            window_overflow = true;
        }
        in_window_cells += cells.len();
        per_sheet.push(SheetVals {
            cells,
            max_row,
            max_col,
            read_ok,
        });
    }

    // 决定是否做重活：工作簿落在窗口内且窗口内非空格总数有界才做（否则只给值、加速）。
    let do_heavy = !window_overflow && in_window_cells <= HEAVY_CELL_BUDGET;

    // 合并区（owned，按 sheet 名）。load_merged_regions 要扫整表 XML，故仅 do_heavy 时做。
    // 仅 xlsx/xls 暴露；xlsb/ods 无合并 API → 空。元组 0-based 闭区间 (tr,tc,br,bc)。
    let merges = if do_heavy {
        collect_merges(&mut wb, &sheets_meta)
    } else {
        HashMap::new()
    };

    // 富样式仅 xlsx/xlsm 且 do_heavy：umya 物化整本（O(整表)）。reader 对畸形 OOXML 会
    // panic，故包在 catch_unwind 里，失败即 None（降级为仅值、空样式）。
    let umya_book = if do_heavy && !styles_unavailable {
        catch_unwind(AssertUnwindSafe(|| {
            umya_spreadsheet::reader::xlsx::read_reader(Cursor::new(data), true)
        }))
        .ok()
        .and_then(|r| r.ok())
    } else {
        None
    };

    // 因体量跳过了富样式 + 合并（xlsx/xlsm 本可有样式）——前端据此提示「已省略样式/合并」。
    let heavy_skipped = !do_heavy && !styles_unavailable;

    // ── Phase 3：逐 sheet 物化（用 Phase 1 的原始 Data + umya 样式 + 合并；此时才渲染文本）。
    let mut sheets = Vec::with_capacity(sheets_meta.len());
    let mut emitted_total = 0usize;
    let mut truncated_total = false;

    for ((name, hidden, very_hidden), vals) in sheets_meta.iter().zip(per_sheet) {
        // 整 sheet 读失败（坏页等）→ 空表，不影响其他 sheet。
        if !vals.read_ok {
            sheets.push(empty_sheet(name, *hidden, *very_hidden));
            continue;
        }

        // umya 样式/numFmt 图（仅 do_heavy 且 xlsx/xlsm，否则空），键为 1-based (row, col)。
        let (style_map, style_max_row, style_max_col) = match umya_book.as_ref() {
            Some(book) => umya_sheet_styles(book, name),
            None => (HashMap::new(), 0, 0),
        };

        // 合并 → 锚点跨度 + 被覆盖集 + 合并最大行列。
        let mut anchor: HashMap<(u32, u32), (u32, u32)> = HashMap::new();
        let mut covered: HashSet<(u32, u32)> = HashSet::new();
        let mut merge_max_row = 0u32;
        let mut merge_max_col = 0u32;
        if let Some(regions) = merges.get(name) {
            for &(r0, c0, r1, c1) in regions {
                // 规范化端点（容错恶意/反向 ref 如 "B1:A1"，避免 u32 减法下溢 panic）。
                let (tr, br) = (r0.min(r1), r0.max(r1));
                let (tc, bc) = (c0.min(c1), c0.max(c1));
                merge_max_row = merge_max_row.max(br.saturating_add(1));
                merge_max_col = merge_max_col.max(bc.saturating_add(1));
                let rowspan = br - tr + 1;
                let colspan = bc - tc + 1;
                // 锚点只在窗口内才会被物化；窗口外的锚点无意义，跳过。
                if (rowspan > 1 || colspan > 1)
                    && (tr as usize) < ROW_CAP
                    && (tc as usize) < COL_CAP
                {
                    anchor.insert((tr + 1, tc + 1), (rowspan, colspan));
                }
                // **安全关键**：被覆盖集只需覆盖渲染窗口（1..=ROW_CAP × 1..=COL_CAP）。
                // 整表合并（如 A1:XFD1048576）若按原始矩形铺会插入上百亿项 → OOM，
                // 故先把矩形夹到窗口再铺（夹后若 start>end 则区间为空，不迭代）。
                let r_hi = br.min(ROW_CAP as u32 - 1);
                let c_hi = bc.min(COL_CAP as u32 - 1);
                for r in tr..=r_hi {
                    for c in tc..=c_hi {
                        if r == tr && c == tc {
                            continue;
                        }
                        covered.insert((r + 1, c + 1));
                    }
                }
            }
        }

        let max_row = vals.max_row.max(style_max_row).max(merge_max_row) as usize;
        let max_col = vals.max_col.max(style_max_col).max(merge_max_col) as usize;
        let mut truncated_rows = max_row > ROW_CAP;
        let truncated_cols = max_col > COL_CAP;
        let row_count = max_row.min(ROW_CAP);
        let col_count = max_col.min(COL_CAP);

        // 总单元格预算用尽：本 sheet 起即放弃物化（空 rows + 标 truncated）。
        if emitted_total >= TOTAL_CELL_CAP {
            truncated_total = true;
            sheets.push(XlsxSheet {
                name: name.clone(),
                hidden: *hidden,
                very_hidden: *very_hidden,
                rows: Vec::new(),
                col_count,
                truncated_rows: true,
                truncated_cols,
            });
            continue;
        }

        // 物化为矩形（逐行去掉被合并覆盖者；锚点带 span；空格保留以维持列对齐）。
        let mut rows = Vec::with_capacity(row_count);
        for r in 1..=row_count as u32 {
            let mut row_cells = Vec::new();
            for c in 1..=col_count as u32 {
                let key = (r, c);
                if covered.contains(&key) {
                    continue;
                }
                let (rowspan, colspan) = match anchor.get(&key) {
                    Some(&(rs, cs)) => (Some(rs), Some(cs)),
                    None => (None, None),
                };
                let numfmt = style_map.get(&key).map(|m| m.numfmt.as_str()).unwrap_or("");
                let text = match vals.cells.get(&key) {
                    Some(data) => render_cell(data, numfmt),
                    None => String::new(),
                };
                let style = style_map
                    .get(&key)
                    .map(|m| m.css.clone())
                    .unwrap_or_default();
                row_cells.push(XlsxCell {
                    text,
                    style,
                    rowspan,
                    colspan,
                });
                emitted_total += 1;
            }
            rows.push(row_cells);
            // 物化途中越过总预算：截断本 sheet 余下行（若还有行）。
            if emitted_total >= TOTAL_CELL_CAP && (r as usize) < row_count {
                truncated_total = true;
                truncated_rows = true;
                break;
            }
        }

        sheets.push(XlsxSheet {
            name: name.clone(),
            hidden: *hidden,
            very_hidden: *very_hidden,
            rows,
            col_count,
            truncated_rows,
            truncated_cols,
        });
    }

    Ok(XlsxTable {
        sheets,
        format,
        styles_unavailable,
        heavy_skipped,
        truncated_bytes,
        truncated_total,
    })
}

/// 空表（sheet 读取失败 / 无内容）。
pub(super) fn empty_sheet(name: &str, hidden: bool, very_hidden: bool) -> XlsxSheet {
    XlsxSheet {
        name: name.to_string(),
        hidden,
        very_hidden,
        rows: Vec::new(),
        col_count: 0,
        truncated_rows: false,
        truncated_cols: false,
    }
}

/// ZIP 容器内是否含 `xl/vbaProject.bin`（用 OOXML 项目自带的 zip v9 读，仅区分 xlsx/xlsm 标签）。
pub(super) fn has_vba_project(data: &[u8]) -> bool {
    match zip::ZipArchive::new(Cursor::new(data)) {
        Ok(mut z) => z.by_name("xl/vbaProject.bin").is_ok(),
        Err(_) => false,
    }
}

/// 收集合并区（owned）。仅 xlsx/xls 暴露合并 API（且在具体 reader 上，非 Reader trait /
/// Sheets 枚举），xlsb/ods 返回空。元组为 0-based 闭区间 (top, left, bottom, right)。
pub(super) fn collect_merges(
    wb: &mut Sheets<Cursor<&[u8]>>,
    sheets_meta: &[(String, bool, bool)],
) -> HashMap<String, Vec<(u32, u32, u32, u32)>> {
    let mut merges: HashMap<String, Vec<(u32, u32, u32, u32)>> = HashMap::new();
    match wb {
        Sheets::Xlsx(xlsx) => {
            for (name, _hidden, _very_hidden) in sheets_meta {
                match xlsx.merge_cells_by_sheet_name(name) {
                    Ok(dimensions) => {
                        merges.insert(
                            name.clone(),
                            dimensions
                                .into_iter()
                                .map(|dimension| {
                                    (
                                        dimension.start.0,
                                        dimension.start.1,
                                        dimension.end.0,
                                        dimension.end.1,
                                    )
                                })
                                .collect(),
                        );
                    }
                    Err(error) => tracing::warn!("读取工作表 {name} 的合并区域失败：{error}"),
                }
            }
        }
        Sheets::Xls(xls) => {
            for (name, _hidden, _very_hidden) in sheets_meta {
                match xls.merge_cells_by_sheet_name(name) {
                    Ok(dimensions) => {
                        merges.insert(
                            name.clone(),
                            dimensions
                                .into_iter()
                                .map(|dimension| {
                                    (
                                        dimension.start.0,
                                        dimension.start.1,
                                        dimension.end.0,
                                        dimension.end.1,
                                    )
                                })
                                .collect(),
                        );
                    }
                    Err(error) => tracing::warn!("读取工作表 {name} 的合并区域失败：{error}"),
                }
            }
        }
        Sheets::Xlsb(_) | Sheets::Ods(_) => {}
    }
    merges
}

/// 构建一个 sheet 的 umya 样式/numFmt 图（键 1-based (row, col)）+ 真实最大行列
/// （含仅有样式无值的单元格，对齐 exceljs eachCell 的网格扩展）。
#[allow(deprecated)] // umya 3.0 将 get_* 标记为 deprecated（推荐去前缀名）；get_* 仍是稳定可用面，统一沿用。
pub(super) fn umya_sheet_styles(
    book: &umya_spreadsheet::Workbook,
    name: &str,
) -> (HashMap<(u32, u32), CellMeta>, u32, u32) {
    let mut map: HashMap<(u32, u32), CellMeta> = HashMap::new();
    let mut max_row = 0u32;
    let mut max_col = 0u32;
    let Some(ws) = book
        .get_sheet_collection()
        .iter()
        .find(|w| w.get_name() == name)
    else {
        return (map, 0, 0);
    };
    let theme = book.get_theme();
    for cell in ws.get_cells() {
        let coord = cell.get_coordinate();
        let col = coord.get_col_num();
        let row = coord.get_row_num();
        max_row = max_row.max(row);
        max_col = max_col.max(col);
        if row as usize > ROW_CAP || col as usize > COL_CAP {
            continue;
        }
        let style = cell.get_style();
        let css = cell_style(style, theme);
        let numfmt = style
            .get_number_format()
            .map(|nf| nf.get_format_code().to_string())
            .unwrap_or_default();
        if !css.is_empty() || !numfmt.is_empty() {
            map.insert((row, col), CellMeta { css, numfmt });
        }
    }
    (map, max_row, max_col)
}
