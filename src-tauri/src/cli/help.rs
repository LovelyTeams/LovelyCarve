//! Help screens in the layout yargs gives cf: the usage line and the
//! description, then the 命令, 位置参数, 全局选项, 选项 and 示例 tables, with
//! descriptions wrapped to the terminal width (80 columns off a terminal) and
//! each row's notes, such as `[字符串]` or `[默认: false]`, right-aligned.
use std::io::IsTerminal;

use serde_json::Value;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use super::{
    catalog::{Catalog, Command, Kind, Param},
    complete::GLOBAL_OPTIONS,
    interactive,
    output::{self, Role},
};

const AGENT_DISCOVERY: &str = "=== STOP: AGENT COMMAND DISCOVERY ===
AGENTS: Do not explore commands by chaining nested --help calls.
Your first port of call and the best way to discover commands is:
AGENTS: Keep cli search and docs search queries anonymous; describe the action and resource type only.
Never include evidence paths, case names or numbers, file names, hashes, passwords or other identifying values.
  lovelycarve-cli cli search \"<describe the task you want to accomplish>\"
It returns five compact JSON matches with short descriptions. Pick the best match instead of repeating similar searches.
Run `<discovered command> --help` for detailed command help.
For backend request details, replace the result's leading `lovelycarve-cli` with `lovelycarve-cli schema`.
For LovelyCarve documentation, workflows and format limits, use lovelycarve-cli docs search \"<question>\".
It returns ranked JSON results. Read a result's page with lovelycarve-cli docs get <page>.
Run several commands against one opened image in a single `lovelycarve-cli session` (JSONL on stdin).
=== END AGENT COMMAND DISCOVERY ===";

pub const ABOUT: &str = "LovelyCarve 取证命令行：打开检材、浏览与解析文件、搜索、恢复与导出";
const EPILOGUE: &str =
    "更多信息：https://github.com/LovelyTeams/LovelyCarve 与安装包内的 README.md";

pub const TEXT_HELP: &str = "按 UTF-8 解码原始字节后输出";
pub const FORCE_HELP: &str = "跳过确认（用于脚本和 CI）";
pub const BODY_HELP: &str = "原始 JSON 参数对象，键为 API 参数名（见 schema）；可写 @文件路径";
pub const DRY_RUN_HELP: &str = "解析参数并显示将执行的请求，不实际执行";
const SECRET_HELP: &str = "（敏感：不带值时在终端中隐藏输入，否则从标准输入读取；也可写 @文件）";
const JSON_HELP: &str = "（JSON 文本或 @文件路径）";

/// One table row: the label, its description and the notes aligned right.
struct Row {
    label: String,
    /// Extra indentation that lines long-only options up under `-x, --name`.
    indent: usize,
    description: String,
    notes: String,
}

impl Row {
    fn new(label: String, description: String) -> Self {
        Self {
            label,
            indent: 0,
            description,
            notes: String::new(),
        }
    }
}

/// Help for the root, a group or a command, ending with a newline. An agent
/// gets the discovery notice first.
pub fn render(catalog: &Catalog, path: &[String]) -> String {
    compose(catalog, path, terminal_width(), interactive::is_agentic())
}

fn compose(catalog: &Catalog, path: &[String], width: usize, agentic: bool) -> String {
    let words: Vec<&str> = path.iter().map(String::as_str).collect();
    let mut blocks = Vec::new();
    let mut tail = None;
    match catalog.find(&words) {
        Some(command) => {
            blocks.push(wrap(&command.usage, width).join("\n"));
            blocks.push(wrap(&command.description, width).join("\n"));
            blocks.extend(option_tables(
                [
                    ("位置参数", positional_rows(command)),
                    ("全局选项", global_rows()),
                    ("选项", option_rows(command)),
                ],
                width,
            ));
            let examples: Vec<Row> = command
                .examples
                .iter()
                .map(|example| Row::new(example.command.clone(), example.description.clone()))
                .collect();
            blocks.push(table_section("示例", &examples, width));
            if !matches!(
                command.native.as_deref(),
                Some("complete" | "session" | "shell")
            ) {
                tail = Some(output::italic(&format!(
                    "如需查看请求参数结构，运行 {}",
                    output::paint(
                        Role::Code,
                        &format!("{} schema {}", catalog.binary, words.join(" "))
                    )
                )));
            }
        }
        None => {
            let (usage, description) = if words.is_empty() {
                (format!("{} <command> [options]", catalog.binary), ABOUT)
            } else {
                (
                    format!("{} {}", catalog.binary, words.join(" ")),
                    catalog.description(&words.join(" ")).unwrap_or_default(),
                )
            };
            blocks.push(wrap(&usage, width).join("\n"));
            blocks.push(wrap(description, width).join("\n"));
            blocks.push(table_section("命令", &command_rows(catalog, &words), width));
            blocks.extend(option_tables([("全局选项", global_rows())], width));
            if words.is_empty() {
                blocks.push(wrap(EPILOGUE, width).join("\n"));
            }
        }
    }
    let mut help = blocks
        .into_iter()
        .filter(|block| !block.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");
    if let Some(tail) = tail {
        help.push_str("\n\n");
        help.push_str(&tail);
    }
    help.push('\n');
    if agentic {
        help = format!("{AGENT_DISCOVERY}\n{help}");
    }
    help
}

/// The stdout terminal's columns, or 80 when stdout is not a terminal.
fn terminal_width() -> usize {
    if !std::io::stdout().is_terminal() {
        return 80;
    }
    crossterm::terminal::size()
        .ok()
        .map(|(columns, _)| usize::from(columns))
        .filter(|columns| *columns > 0)
        .unwrap_or(80)
}

pub fn display_width(text: &str) -> usize {
    UnicodeWidthStr::width(text)
}

fn char_width(character: char) -> usize {
    UnicodeWidthChar::width(character).unwrap_or(0)
}

/// Breakable pieces of a line: words, and every wide character on its own,
/// each marked with whether a space precedes it.
fn pieces(text: &str) -> Vec<(String, bool)> {
    let mut pieces = Vec::new();
    let mut word = String::new();
    let mut word_spaced = false;
    let mut spaced = false;
    for character in text.chars() {
        if character == ' ' {
            if !word.is_empty() {
                pieces.push((std::mem::take(&mut word), word_spaced));
            }
            spaced = true;
        } else if char_width(character) > 1 {
            if !word.is_empty() {
                pieces.push((std::mem::take(&mut word), word_spaced));
            }
            pieces.push((character.to_string(), spaced));
            spaced = false;
        } else {
            if word.is_empty() {
                word_spaced = spaced;
                spaced = false;
            }
            word.push(character);
        }
    }
    if !word.is_empty() {
        pieces.push((word, word_spaced));
    }
    pieces
}

/// `text` wrapped to `width` columns: at spaces and between wide characters,
/// breaking a word only when it is longer than a whole line. A paragraph's
/// indentation is kept, and its continuation lines hang at the same depth.
pub fn wrap(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut lines = Vec::new();
    for paragraph in text.split('\n') {
        let body = paragraph.trim_start_matches(' ');
        let indent = " ".repeat((paragraph.len() - body.len()).min(width.saturating_sub(1)));
        let width = width - indent.len();
        let first = lines.len();
        let mut line = String::new();
        let mut used = 0usize;
        for (piece, spaced) in pieces(body) {
            let piece_width = display_width(&piece);
            let gap = usize::from(spaced && !line.is_empty());
            if !line.is_empty() && used + gap + piece_width > width {
                lines.push(std::mem::take(&mut line));
                used = 0;
            }
            if !line.is_empty() && spaced {
                line.push(' ');
                used += 1;
            }
            if piece_width <= width.saturating_sub(used) {
                line.push_str(&piece);
                used += piece_width;
                continue;
            }
            for character in piece.chars() {
                let character_width = char_width(character);
                if used + character_width > width && !line.is_empty() {
                    lines.push(std::mem::take(&mut line));
                    used = 0;
                }
                line.push(character);
                used += character_width;
            }
        }
        lines.push(line);
        if !indent.is_empty() {
            for line in &mut lines[first..] {
                line.insert_str(0, &indent);
            }
        }
    }
    lines
}

/// The label column: the widest label, at most half the screen, plus padding.
fn column<'a>(rows: impl Iterator<Item = &'a Row>, width: usize) -> usize {
    let widest = rows
        .map(|row| display_width(&row.label) + row.indent)
        .max()
        .unwrap_or(0);
    widest.min(width / 2) + 4
}

/// Rows in two columns with the notes right-aligned on the last line of each
/// row when they fit there, on a line of their own otherwise.
fn table(rows: &[Row], column: usize, width: usize) -> Vec<String> {
    let description_width = width.saturating_sub(column).max(10);
    let mut lines = Vec::new();
    for row in rows {
        let prefix = " ".repeat(2 + row.indent);
        let labels = wrap(&row.label, column.saturating_sub(4 + row.indent).max(1));
        let descriptions = if row.description.is_empty() {
            Vec::new()
        } else {
            wrap(&row.description, description_width)
        };
        let mut rendered = Vec::new();
        for index in 0..labels.len().max(descriptions.len()) {
            let mut line = String::new();
            if let Some(label) = labels.get(index).filter(|label| !label.is_empty()) {
                line.push_str(&prefix);
                line.push_str(label);
            }
            if let Some(description) = descriptions.get(index) {
                let used = display_width(&line);
                line.push_str(&" ".repeat(column.saturating_sub(used).max(1)));
                line.push_str(description);
            }
            rendered.push(line.trim_end().to_owned());
        }
        if !row.notes.is_empty() {
            let notes_width = display_width(&row.notes);
            match rendered.last_mut() {
                Some(last) if display_width(last) + 2 + notes_width <= width => {
                    let used = display_width(last);
                    last.push_str(&" ".repeat(width - notes_width - used));
                    last.push_str(&row.notes);
                }
                _ => rendered.push(format!(
                    "{}{}",
                    " ".repeat(width.saturating_sub(notes_width)),
                    row.notes
                )),
            }
        }
        lines.extend(rendered);
    }
    lines
}

fn section(title: &str, lines: Vec<String>) -> String {
    let mut block = output::paint(Role::Brand, title);
    for line in lines {
        block.push('\n');
        block.push_str(&line);
    }
    block
}

fn table_section(title: &str, rows: &[Row], width: usize) -> String {
    if rows.is_empty() {
        return String::new();
    }
    section(title, table(rows, column(rows.iter(), width), width))
}

/// Positionals, global and command options share one label column.
fn option_tables<const N: usize>(sections: [(&str, Vec<Row>); N], width: usize) -> Vec<String> {
    let column = column(sections.iter().flat_map(|(_, rows)| rows), width);
    sections
        .iter()
        .filter(|(_, rows)| !rows.is_empty())
        .map(|(title, rows)| section(title, table(rows, column, width)))
        .collect()
}

fn command_rows(catalog: &Catalog, words: &[&str]) -> Vec<Row> {
    catalog
        .children(words)
        .into_iter()
        .map(|(name, description)| {
            let mut path = words.to_vec();
            path.push(&name);
            let mut label = format!("{} {}", catalog.binary, path.join(" "));
            if let Some(command) = catalog.find(&path) {
                for param in command.positionals() {
                    if param.visible() {
                        label.push(' ');
                        label.push_str(&placeholder(param));
                    }
                }
            }
            Row::new(label, description)
        })
        .collect()
}

fn placeholder(param: &Param) -> String {
    if param.required {
        format!("<{}>", param.option)
    } else {
        format!("[{}]", param.option)
    }
}

fn global_rows() -> Vec<Row> {
    GLOBAL_OPTIONS
        .iter()
        .map(|(long, short, help, takes_value)| Row {
            label: match short {
                Some(short) => format!("-{short}, --{long}"),
                None => format!("--{long}"),
            },
            indent: if short.is_some() { 0 } else { 4 },
            description: (*help).to_owned(),
            notes: match (*takes_value, *long) {
                (true, _) => "[字符串]",
                (false, "quiet") => "[布尔] [默认: false]",
                (false, _) => "[布尔]",
            }
            .to_owned(),
        })
        .collect()
}

fn kind_note(kind: Kind) -> &'static str {
    match kind {
        Kind::String | Kind::Enum => "[字符串]",
        Kind::Number => "[数字]",
        Kind::Boolean => "[布尔]",
        Kind::Array => "[数组]",
        Kind::Json => "[JSON]",
    }
}

fn default_text(value: &Value) -> String {
    match value {
        Value::String(text) => format!("\"{text}\""),
        other => other.to_string(),
    }
}

fn notes(param: &Param) -> String {
    let mut notes = vec![kind_note(param.kind).to_owned()];
    if param.required {
        notes.push("[必填]".to_owned());
    }
    if let Some(values) = &param.values {
        let values: Vec<String> = values.iter().map(|value| format!("\"{value}\"")).collect();
        notes.push(format!("[可选值: {}]", values.join(", ")));
    }
    if let Some(default) = &param.default {
        notes.push(format!("[默认: {}]", default_text(default)));
    }
    notes.join(" ")
}

fn describe(param: &Param) -> String {
    let mut description = param.description.clone();
    if param.secret {
        description.push_str(SECRET_HELP);
    }
    if param.kind == Kind::Json {
        description.push_str(JSON_HELP);
    }
    description
}

fn positional_rows(command: &Command) -> Vec<Row> {
    command
        .positionals()
        .into_iter()
        .filter(|param| param.visible())
        .map(|param| Row {
            notes: notes(param),
            ..Row::new(param.option.clone(), describe(param))
        })
        .collect()
}

fn flag(long: &str, short: Option<char>, description: &str, notes: &str) -> Row {
    Row {
        label: match short {
            Some(short) => format!("-{short}, --{long}"),
            None => format!("--{long}"),
        },
        indent: if short.is_some() { 0 } else { 4 },
        description: description.to_owned(),
        notes: notes.to_owned(),
    }
}

fn option_rows(command: &Command) -> Vec<Row> {
    let mut rows: Vec<Row> = command
        .params
        .iter()
        .filter(|param| param.visible() && param.position.is_none() && !param.scope)
        .map(|param| flag(&param.option, param.short, &describe(param), &notes(param)))
        .collect();
    if command.raw_output {
        rows.push(flag("text", None, TEXT_HELP, "[布尔] [默认: false]"));
    }
    if command.destructive {
        rows.push(flag("force", Some('f'), FORCE_HELP, "[布尔] [默认: false]"));
    }
    if command.takes_body() {
        rows.push(flag("body", None, BODY_HELP, "[JSON]"));
    }
    rows.push(flag("dry-run", None, DRY_RUN_HELP, "[布尔] [默认: false]"));
    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::catalog::catalog;

    #[test]
    fn wrapping_breaks_at_spaces_and_between_wide_characters() {
        assert_eq!(wrap("alpha beta gamma", 10), ["alpha beta", "gamma"]);
        assert_eq!(wrap("检材路径与镜像", 6), ["检材路", "径与镜", "像"]);
        assert_eq!(wrap("打开 E01 检材", 8), ["打开 E01", "检材"]);
        assert_eq!(wrap("abcdefghij", 4), ["abcd", "efgh", "ij"]);
        assert_eq!(wrap("", 10), [""]);
        assert_eq!(wrap("a\n\nb", 10), ["a", "", "b"]);
        assert_eq!(wrap("  1. 第一项说明", 8), ["  1. 第", "  一项说", "  明"]);
    }

    #[test]
    fn rows_align_descriptions_and_right_align_notes() {
        let rows = [
            Row {
                notes: "[布尔] [默认: false]".into(),
                ..Row::new("-q, --quiet".into(), "不输出非必要信息".into())
            },
            Row {
                indent: 4,
                notes: "[字符串]".into(),
                ..Row::new(
                    "--case".into(),
                    "先打开的案件目录（覆盖 LOVELYCARVE_CASE）".into(),
                )
            },
        ];
        let lines = table(&rows, column(rows.iter(), 60), 60);
        assert_eq!(
            lines,
            [
                format!(
                    "  -q, --quiet  不输出非必要信息{}[布尔] [默认: false]",
                    " ".repeat(9)
                ),
                "      --case   先打开的案件目录（覆盖 LOVELYCARVE_CASE）".to_owned(),
                format!("{}[字符串]", " ".repeat(52)),
            ]
        );
        assert!(lines.iter().all(|line| display_width(line) <= 60));
    }

    #[test]
    fn help_lists_sections_in_yargs_order() {
        let catalog = catalog().unwrap();
        let path = |words: &[&str]| {
            words
                .iter()
                .map(|word| (*word).to_owned())
                .collect::<Vec<_>>()
        };
        let help = compose(catalog, &path(&["images", "export"]), 80, false);
        let positions: Vec<usize> = [
            "\n位置参数\n",
            "\n全局选项\n",
            "\n选项\n",
            "如需查看请求参数结构",
        ]
        .iter()
        .map(|title| {
            help.find(title)
                .unwrap_or_else(|| panic!("{title}: {help}"))
        })
        .collect();
        assert!(positions.windows(2).all(|pair| pair[0] < pair[1]), "{help}");
        assert!(help.starts_with("lovelycarve-cli images export [image] [options]\n\n"));
        assert!(help.contains("[可选值: \"source\", \"unlockedPartitions\"]"));
        assert!(help.lines().all(|line| display_width(line) <= 80), "{help}");
        let root = compose(catalog, &[], 80, false);
        assert!(root.contains("lovelycarve-cli images"));
        assert!(
            !root.contains("lovelycarve-cli tools"),
            "hidden commands stay out of help"
        );
        assert!(root.trim_end().ends_with("README.md"));
        let group = compose(catalog, &path(&["images"]), 80, false);
        assert!(
            group.contains("lovelycarve-cli images open <path>"),
            "{group}"
        );
        // Examples wider than half the screen wrap inside their column, as in yargs.
        let complete = compose(catalog, &path(&["complete"]), 80, false);
        assert!(complete.contains("\n示例\n") && !complete.contains("schema complete"));
        assert!(
            complete.lines().all(|line| display_width(line) <= 80),
            "{complete}"
        );
        let wide = compose(catalog, &path(&["complete"]), 160, false);
        assert!(
            wide.contains("  lovelycarve-cli complete bash >> ~/.bashrc  "),
            "{wide}"
        );
        let agent = compose(catalog, &path(&["files", "list"]), 80, true);
        assert!(agent.starts_with("=== STOP: AGENT COMMAND DISCOVERY ===\n"));
        assert!(agent.contains("docs search"));
    }
}
