use super::*;

#[test]
fn argb_drops_alpha_and_uppercases() {
    assert_eq!(argb_to_hex("FFFF0000").as_deref(), Some("#FF0000"));
    assert_eq!(argb_to_hex("ff00ff00").as_deref(), Some("#00FF00"));
    assert_eq!(argb_to_hex("#1a2b3c").as_deref(), Some("#1A2B3C"));
    assert_eq!(argb_to_hex(""), None);
    assert_eq!(argb_to_hex("xyz"), None);
    assert_eq!(argb_to_hex("FFF"), None);
}

#[test]
fn number_plain_and_percent() {
    assert_eq!(format_number(3.0, ""), "3");
    assert_eq!(format_number(3.5, ""), "3.5");
    assert_eq!(format_number(0.5, "0%"), "50%");
    assert_eq!(format_number(0.1234, "0.00%"), "12.34%");
}

#[test]
fn number_decimals_thousands_currency() {
    assert_eq!(format_number(1234.5, "#,##0.00"), "1,234.50");
    assert_eq!(format_number(1234567.0, "#,##0"), "1,234,567");
    assert_eq!(format_number(12.0, "$#,##0.00"), "$12.00");
    assert_eq!(format_number(-1234.0, "#,##0"), "-1,234");
}

#[test]
fn decimal_precision_matches_regex() {
    assert_eq!(decimal_precision("0.00"), 2);
    assert_eq!(decimal_precision("#,##0.000"), 3);
    assert_eq!(decimal_precision("0.00_);(0.00)"), 2);
    assert_eq!(decimal_precision("#,##0"), 0);
}

#[test]
fn frac_digits_percent() {
    assert_eq!(frac_digits_before_percent("0.00%"), Some(2));
    assert_eq!(frac_digits_before_percent("0%"), None);
    assert_eq!(frac_digits_before_percent("0.0%"), Some(1));
}

#[test]
fn thousands_grouping() {
    assert_eq!(group_thousands("1234567"), "1,234,567");
    assert_eq!(group_thousands("-1234.50"), "-1,234.50");
    assert_eq!(group_thousands("100"), "100");
}

#[test]
fn date_formats_by_token() {
    let dt = chrono::NaiveDate::from_ymd_opt(2024, 3, 9)
        .unwrap()
        .and_hms_opt(9, 30, 5)
        .unwrap();
    // 仅时间标记（无 y/d，且无 m）→ 时间。注意 numFmt 的 'm' 同时是「月」标记，这是
    // 从原 JS 移植的歧义，故纯时间格式须不含 m 才走「时间-only」分支。
    assert_eq!(format_date(dt, "hh:ss"), "09:30:05");
    // 含 m（分钟）即被判为「含日期标记」（与原 JS `[ymd]` 正则一致的歧义）→ 日期 + 时间。
    assert_eq!(format_date(dt, "hh:mm:ss"), "2024-03-09 09:30:05");
    // 含日期标记 + 有时间分量 → 日期 + 时间。
    assert_eq!(format_date(dt, "yyyy-mm-dd hh:mm"), "2024-03-09 09:30:05");
    // 仅日期、无时间分量、且 numFmt 不含 h/m/s（"yyyy" 仅年标记）→ 仅日期。
    let d2 = chrono::NaiveDate::from_ymd_opt(2024, 3, 9)
        .unwrap()
        .and_hms_opt(0, 0, 0)
        .unwrap();
    assert_eq!(format_date(d2, "yyyy"), "2024-03-09");
    // 但含 m 的常见日期格式（"yyyy-mm-dd"）因 m 同时是「月/分」标记的歧义（移植自原 JS
    // `[ymd]`/`[hms]` 正则）被判为含时间标记，故附 00:00:00——保持与旧前端渲染一致。
    assert_eq!(format_date(d2, "yyyy-mm-dd"), "2024-03-09 00:00:00");
}

#[test]
fn border_and_align_maps() {
    assert_eq!(border_css("thick"), "3px solid");
    assert_eq!(border_css("double"), "3px double");
    assert_eq!(border_css("hair"), "1px solid");
    assert_eq!(align_h_css("centerContinuous"), Some("center"));
    assert_eq!(align_v_css("center"), Some("middle"));
    assert_eq!(align_h_css("general"), None);
}

/// 端到端：用 umya 写一本含值/样式/合并/隐藏 sheet/数字格式的 xlsx 到临时文件，
/// 读回字节后过 `parse`，核对值、合并锚点/覆盖、隐藏标注与格式化文本。
#[test]
#[allow(deprecated)]
fn roundtrip_xlsx_values_styles_merge_hidden() {
    use umya_spreadsheet as umya;

    let mut book = umya::new_file();
    // 默认已有 "Sheet1"。
    let ws = book.get_sheet_mut(&0).unwrap();
    ws.get_cell_mut("A1").set_value("Name");
    ws.get_cell_mut("B1").set_value("Amount");
    ws.get_cell_mut("A2").set_value("Alice");
    ws.get_cell_mut("B2").set_value_number(1234.5);
    // 合并 A1:B1。
    ws.add_merge_cells("A1:B1");

    // 再加一个隐藏 sheet。注意：set_sheet_state 写的是另一个字段，writer 不读它；
    // 要让写出的 workbook.xml 带 state="hidden"，须用 set_state(枚举)。
    let hidden = book.new_sheet("Secret").unwrap();
    hidden.set_state(umya::SheetStateValues::Hidden);
    hidden.get_cell_mut("A1").set_value("hush");

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("rt.xlsx");
    umya::writer::xlsx::write(&book, &path).unwrap();
    let bytes = std::fs::read(&path).unwrap();

    let table = parse(&bytes, false).expect("parse xlsx");
    assert_eq!(table.format, "xlsx");
    assert!(!table.styles_unavailable);
    assert_eq!(table.sheets.len(), 2);

    let s0 = &table.sheets[0];
    assert_eq!(s0.name, "Sheet1");
    assert!(!s0.hidden);
    // A1 是合并锚点，跨 2 列。
    let a1 = &s0.rows[0][0];
    assert_eq!(a1.text, "Name");
    assert_eq!(a1.colspan, Some(2));
    // B1 被合并覆盖，本行物化后只剩 1 个单元格。
    assert_eq!(s0.rows[0].len(), 1);
    // A2 / B2。
    assert_eq!(s0.rows[1][0].text, "Alice");
    assert_eq!(s0.rows[1][1].text, "1234.5");

    let s1 = &table.sheets[1];
    assert_eq!(s1.name, "Secret");
    assert!(s1.hidden);
}

/// 安全回归：远角单元格（A1 + XFD1048576）绝不能触发 calamine 的稠密化
/// （vec![Empty; 1048576*16384] ≈ 170 亿格 → 进程 abort）。流式 reader 应只读两格、
/// 超窗即弃，秒级完成并标截断；窗口内的 A1 仍在。无修复时本测试会 OOM/挂死。
#[test]
#[allow(deprecated)]
fn far_corner_cell_does_not_densify() {
    use umya_spreadsheet as umya;
    let mut book = umya::new_file();
    let ws = book.get_sheet_mut(&0).unwrap();
    ws.get_cell_mut("A1").set_value("near");
    ws.get_cell_mut("XFD1048576").set_value("far");
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("corner.xlsx");
    umya::writer::xlsx::write(&book, &path).unwrap();
    let bytes = std::fs::read(&path).unwrap();

    let table = parse(&bytes, false).expect("parse far-corner xlsx");
    let s0 = &table.sheets[0];
    // 远角格在行上限之外：流式读到 A1 后行升序越过 ROW_CAP 即停（根本不读到远角，
    // 故绝不稠密化、不 OOM）。标 truncated_rows，窗口内的 A1 保留；超窗 → 跳过重活。
    assert!(s0.truncated_rows);
    assert!(s0.rows.len() <= ROW_CAP);
    assert_eq!(s0.rows[0][0].text, "near"); // 窗口内的格仍忠实呈现
    assert!(table.heavy_skipped);
}

/// 安全回归：整表大合并（A1:XFD1048576）的被覆盖集必须夹到渲染窗口，否则按原始
/// 矩形铺会插入约 170 亿项 → OOM。锚点仍带完整跨度（浏览器再夹）。无修复时会 OOM。
#[test]
#[allow(deprecated)]
fn whole_sheet_merge_is_window_bounded() {
    use umya_spreadsheet as umya;
    let mut book = umya::new_file();
    let ws = book.get_sheet_mut(&0).unwrap();
    ws.get_cell_mut("A1").set_value("top");
    ws.add_merge_cells("A1:XFD1048576");
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("merge.xlsx");
    umya::writer::xlsx::write(&book, &path).unwrap();
    let bytes = std::fs::read(&path).unwrap();

    let table = parse(&bytes, false).expect("parse whole-sheet-merge xlsx");
    let s0 = &table.sheets[0];
    let anchor = &s0.rows[0][0];
    assert_eq!(anchor.text, "top");
    assert_eq!(anchor.rowspan, Some(1_048_576));
    assert_eq!(anchor.colspan, Some(16_384));
    assert!(s0.truncated_rows && s0.truncated_cols);
}

/// 性能门控回归：超高 sheet（> ROW_CAP 行）应触发「跳过重活」——只给值、秒级响应，
/// heavy_skipped=true（格式本支持样式，仅因体量被跳过）；流式读越过行上限即停。
#[test]
#[allow(deprecated)]
fn tall_sheet_skips_heavy_styling() {
    use umya_spreadsheet as umya;
    let mut book = umya::new_file();
    let ws = book.get_sheet_mut(&0).unwrap();
    ws.get_cell_mut("A1").set_value("head");
    ws.get_cell_mut("A3000").set_value("tail"); // 行 3000 > ROW_CAP(2000) → 超窗
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("tall.xlsx");
    umya::writer::xlsx::write(&book, &path).unwrap();
    let bytes = std::fs::read(&path).unwrap();

    let table = parse(&bytes, false).expect("parse tall xlsx");
    assert!(table.heavy_skipped); // 因体量跳过了样式/合并
    assert!(!table.styles_unavailable); // 格式本支持样式，只是被跳过
    let s0 = &table.sheets[0];
    assert!(s0.truncated_rows);
    assert_eq!(s0.rows[0][0].text, "head"); // 窗口内的值仍忠实呈现
}

/// 非 Excel 输入 → Err（前端报错态），不 panic。
#[test]
fn rejects_non_spreadsheet() {
    assert!(parse(b"not a spreadsheet at all", false).is_err());
    assert!(parse(&[], false).is_err());
}
