//! Help screens drawn from the clap command tree: the usage line, the
//! description, then tables of commands, arguments and options with their
//! defaults and choices, and examples. Descriptions wrap at the terminal
//! width, wide characters counted as two columns. A usage error shows the
//! error and the suggestion, never a whole help screen.
use std::error::Error as _;

use clap::{
    error::{ContextKind, ContextValue, ErrorKind},
    Arg, CommandFactory,
};

use super::{
    cmd::Cli,
    output::{self, Role},
    text::{display_width, terminal_width, wrap},
    CliError,
};

const BINARY: &str = "lovelycarve-cli";

/// The root's commands by task.
pub(super) const SECTIONS: &[(&str, &[&str])] = &[
    (
        "案件与检材",
        &[
            "case",
            "evidence",
            "volumes",
            "encryption",
            "unlock",
            "raid",
            "vss",
        ],
    ),
    (
        "浏览与读取",
        &[
            "ls", "tree", "stat", "cat", "text", "strings", "hash", "identify", "streams",
            "export", "bodyfile",
        ],
    ),
    (
        "查找与解析",
        &["find", "grep", "journal", "inspect", "sqlite", "ocr", "qr"],
    ),
    (
        "分析",
        &[
            "search", "timeline", "carve", "plugin", "analyze", "results",
        ],
    ),
    (
        "任务与主机",
        &["tasks", "index", "mount", "umount", "tz", "system", "disks"],
    ),
    ("程序", &["shell", "batch", "rpc", "schema", "completion"]),
];

/// Examples by command path.
const EXAMPLES: &[(&str, &[(&str, &str)])] = &[
    ("", &[
        ("case new D:\\cases\\nas --name nas --open", "建案并启动案件后台进程"),
        ("-C D:\\cases\\nas evidence add D:\\ev\\nas.E01 --as nas --wait", "登记检材并等待卷索引"),
        ("-C D:\\cases\\nas volumes", "分区、文件系统、加密与索引状态"),
        ("-C D:\\cases\\nas ls /nas/part:0/etc", "列出目录"),
        ("--image D:\\ev\\disk.E01 cat /disk/part:0/etc/passwd", "不建案件，读取一个文件"),
    ]),
    ("case new", &[("case new D:\\cases\\nas --name \"NAS 取证\" --examiner 张三 --open", "建案并启动后台进程")]),
    ("case open", &[("case open D:\\cases\\nas", "启动后台进程；在案件目录中运行可省略目录")]),
    ("evidence add", &[
        ("evidence add D:\\ev\\nas.E01 --as nas --wait", "登记检材并等待卷索引完成"),
        ("evidence add D:\\ev\\disk1.img D:\\ev\\disk2.img", "打开多块成员盘（随后 raid detect）"),
        ("evidence add D:\\ev\\disk.001 --raw-read-mode force", "读取不完整的分段 RAW"),
    ]),
    ("ls", &[
        ("ls /", "列出检材"),
        ("ls /nas", "列出检材的分区与卷"),
        ("ls -l /nas/part:0/Users --sort modified --desc", "按修改时间倒序，显示节点 ID"),
        ("ls /nas/@ntfs:part:0:1234", "按节点 ID 列目录（重名、已删除条目）"),
    ]),
    ("cat", &[
        ("cat /nas/part:0/Windows/System32/config/SAM > SAM", "保存原始字节"),
        ("cat /nas/part:0/a.zip --member docs/readme.txt", "读取 ZIP 成员"),
    ]),
    ("text", &[("text /nas/part:0/etc/passwd", "自动识别编码并输出文本")]),
    ("hash", &[("hash /nas/part:0/a.doc /nas/part:0/b.pdf --algo md5,sha256", "多个文件的哈希")]),
    ("export", &[
        ("export /nas/part:0/Users/alice -o D:\\out\\alice", "导出目录树"),
        ("export /nas/part:0/a.txt /nas/part:0/b.txt -o D:\\out\\picked", "导出选中的多个文件到新目录"),
    ]),
    ("find", &[
        ("find \"auth.log*\" --under /nas/part:0/var/log", "按文件名查找（卷索引）"),
        ("find \"*.db\" --type file --min-size 1M", "全部检材中 1 MiB 以上的 .db 文件"),
    ]),
    ("grep", &[
        ("grep -i password /nas/part:0/home", "在子树中搜索关键词"),
        ("grep -E \"1[3-9][0-9]{9}\" /nas", "正则搜索整份检材"),
    ]),
    ("sqlite query", &[(
        "sqlite query /nas/part:0/data/msg.db \"SELECT chat_id, COUNT(*) FROM messages WHERE sent_at >= ?1 GROUP BY chat_id ORDER BY chat_id\" --param 2026-08-01",
        "只读查询，绑定参数",
    )]),
    ("unlock", &[
        ("unlock /nas/part:2 --recovery-password @key.txt", "BitLocker 恢复密码解锁"),
        ("unlock /vc --scheme veracrypt --password", "VeraCrypt 容器，在终端中输入密码"),
    ]),
    ("raid detect", &[("raid detect", "识别并组装 mdadm / LVM；结果作为新检材出现")]),
    ("carve start", &[("carve start --scope unallocated --types jpeg,png,pdf --fs ntfs", "雕刻未分配空间并恢复 NTFS 记录")]),
    ("analyze run", &[(
        "analyze run org.lovelycarve.linuxinfo/linuxbaseinfo --system linux@part:2 --option items=[\"users\",\"logins\"]",
        "在识别出的 Linux 系统上运行分析器",
    )]),
    ("batch", &[("batch --jobs 4 < requests.jsonl", "逐行执行 {\"id\":…,\"argv\":[…]} 请求")]),
    ("rpc", &[
        ("rpc --list", "列出全部后端命令"),
        ("rpc fs_size_tree --body '{\"imageId\":\"nas\",\"nodeId\":\"part:0\"}'", "按 IPC 名称调用；--body 也可写 @文件"),
    ]),
];

/// Global options that take a separate value.
const VALUED_GLOBALS: &[&str] = &["-C", "--case", "--image", "--tz"];

/// The subcommand path the words name, stopping at the first word that is
/// not a subcommand.
pub fn path_of(words: &[String]) -> Vec<String> {
    let root = Cli::command();
    let mut command = &root;
    let mut path = Vec::new();
    let mut skip = false;
    for word in words {
        if skip {
            skip = false;
            continue;
        }
        if word == "--" {
            break;
        }
        if word.starts_with('-') {
            skip = VALUED_GLOBALS.contains(&word.as_str());
            continue;
        }
        match command.find_subcommand(word) {
            Some(child) => {
                path.push(child.get_name().to_owned());
                command = child;
            }
            None => break,
        }
    }
    path
}

/// `-h` / `--help` and `-V` / `--version` anywhere before `--`.
pub fn intercept(args: &[String]) -> Option<i32> {
    let options = args.iter().take_while(|word| *word != "--");
    let mut help = false;
    let mut version = false;
    for word in options {
        match word.as_str() {
            "-h" | "--help" => help = true,
            "-V" | "--version" => version = true,
            _ => {}
        }
    }
    if help {
        output::open_session(false);
        print!("{}", render(&path_of(args)));
        return Some(0);
    }
    if version {
        println!("{}", env!("CARGO_PKG_VERSION"));
        return Some(0);
    }
    None
}

/// The screen with no arguments.
pub fn splash() {
    let lines = [
        output::headline(),
        String::new(),
        "LovelyCarve 取证命令行：打开检材、浏览与解析文件、搜索、恢复与导出。".to_owned(),
        "检材内的路径都从工作区根开始：/<检材别名>/<分区或卷>/<路径>。".to_owned(),
        String::new(),
        output::bold("开始使用："),
        example_line("case new <案件目录> --open", "建案并启动案件后台进程"),
        example_line("-C <案件目录> evidence add <检材路径> --wait", "登记检材"),
        example_line("-C <案件目录> ls /", "浏览：/ 列出检材"),
        example_line(
            "--image <检材路径> ls /<别名>",
            "不建案件，直接浏览一份检材",
        ),
        example_line("--help", "按任务列出全部命令"),
        example_line("<命令> --help", "查看单个命令"),
        String::new(),
    ];
    println!("{}", lines.join("\n"));
}

fn example_line(command: &str, description: &str) -> String {
    let text = format!("{BINARY} {command}");
    let padding = " ".repeat(52usize.saturating_sub(display_width(&text)).max(1));
    format!(
        "  {}{padding} {}",
        output::paint(Role::Code, &text),
        output::paint(Role::Muted, description)
    )
}

/// One table row: the label, its description and its notes.
struct Row {
    label: String,
    description: String,
    notes: String,
}

fn table(title: &str, rows: &[Row], width: usize) -> String {
    if rows.is_empty() {
        return String::new();
    }
    let column = rows
        .iter()
        .map(|row| display_width(&row.label))
        .max()
        .unwrap_or(0)
        .min(width / 2)
        + 4;
    let description_width = width.saturating_sub(column).max(16);
    let mut lines = vec![output::bold(title)];
    for row in rows {
        let mut text = row.description.clone();
        if !row.notes.is_empty() {
            if !text.is_empty() {
                text.push(' ');
            }
            text.push_str(&row.notes);
        }
        let wrapped = wrap(&text, description_width);
        let label_width = display_width(&row.label) + 2;
        let mut first = true;
        for line in wrapped {
            if first {
                let padding = " ".repeat(column.saturating_sub(label_width).max(1));
                lines.push(format!(
                    "  {}{padding}{line}",
                    output::paint(Role::Code, &row.label)
                ));
                first = false;
            } else {
                lines.push(format!("{}{line}", " ".repeat(column)));
            }
        }
        if first {
            lines.push(format!("  {}", output::paint(Role::Code, &row.label)));
        }
    }
    lines.join("\n")
}

fn about(command: &clap::Command) -> String {
    command
        .get_long_about()
        .or_else(|| command.get_about())
        .map(ToString::to_string)
        .unwrap_or_default()
}

fn notes(arg: &Arg) -> String {
    let mut notes = Vec::new();
    let defaults: Vec<String> = arg
        .get_default_values()
        .iter()
        .map(|value| value.to_string_lossy().into_owned())
        .collect();
    let values: Vec<String> = arg
        .get_possible_values()
        .iter()
        .filter(|value| !value.is_hide_set())
        .map(|value| value.get_name().to_owned())
        .collect();
    if !values.is_empty() && arg.get_action().takes_values() {
        notes.push(format!("[可选：{}]", values.join(", ")));
    }
    if !defaults.is_empty() {
        notes.push(format!("[默认：{}]", defaults.join(",")));
    }
    if arg.is_required_set() {
        notes.push("[必填]".to_owned());
    }
    output::paint(Role::Muted, &notes.join(" "))
}

fn option_label(arg: &Arg) -> String {
    let mut label = String::new();
    match (arg.get_short(), arg.get_long()) {
        (Some(short), Some(long)) => label.push_str(&format!("-{short}, --{long}")),
        (Some(short), None) => label.push_str(&format!("-{short}")),
        (None, Some(long)) => label.push_str(&format!("    --{long}")),
        (None, None) => label.push_str(arg.get_id().as_str()),
    }
    if arg.get_action().takes_values()
        && arg
            .get_num_args()
            .is_none_or(|range| range.max_values() > 0)
    {
        let name = arg
            .get_value_names()
            .and_then(|names| names.first().map(ToString::to_string))
            .unwrap_or_else(|| "值".to_owned());
        // clap names unnamed values after the field, in capitals.
        let name = if name
            .chars()
            .all(|character| character.is_ascii_uppercase() || character == '_')
        {
            name.to_lowercase().replace('_', "-")
        } else {
            name
        };
        let optional = arg
            .get_num_args()
            .is_some_and(|range| range.min_values() == 0);
        if optional {
            label.push_str(&format!("[=<{name}>]"));
        } else {
            label.push_str(&format!(" <{name}>"));
        }
    }
    label
}

fn positional_label(arg: &Arg) -> String {
    let name = arg
        .get_value_names()
        .and_then(|names| names.first().map(ToString::to_string))
        .unwrap_or_else(|| arg.get_id().to_string());
    let many = arg
        .get_num_args()
        .is_some_and(|range| range.max_values() > 1);
    let name = if many { format!("{name}…") } else { name };
    if arg.is_required_set() {
        format!("<{name}>")
    } else {
        format!("[{name}]")
    }
}

/// Help for the root or a subcommand path, ending with a newline.
pub fn render(path: &[String]) -> String {
    let width = terminal_width().min(120);
    let root = Cli::command();
    let mut command = &root;
    for word in path {
        match command.find_subcommand(word) {
            Some(child) => command = child,
            None => break,
        }
    }
    let joined = path.join(" ");
    let mut blocks = Vec::new();
    let positionals: Vec<&Arg> = command
        .get_positionals()
        .filter(|arg| !arg.is_hide_set())
        .collect();
    let mut usage = vec![BINARY.to_owned()];
    if !joined.is_empty() {
        usage.push(joined.clone());
    }
    let subcommands: Vec<&clap::Command> = command
        .get_subcommands()
        .filter(|child| !child.is_hide_set())
        .collect();
    if !subcommands.is_empty() {
        usage.push(
            if command.is_subcommand_required_set() || positionals.is_empty() && path.is_empty() {
                "<命令>".to_owned()
            } else {
                "[命令]".to_owned()
            },
        );
    }
    usage.extend(positionals.iter().map(|arg| positional_label(arg)));
    usage.push("[选项]".to_owned());
    blocks.push(format!("{}{}", output::bold("用法："), usage.join(" ")));
    let description = if path.is_empty() {
        "LovelyCarve 取证命令行。检材内的路径从工作区根开始：/<检材别名>/<分区或卷>/<路径>；\
         --json 输出 JSON，-C <案件目录> 指定案件。"
            .to_owned()
    } else {
        about(command)
    };
    if !description.is_empty() {
        blocks.push(wrap(&description, width).join("\n"));
    }
    if path.is_empty() {
        for (title, names) in SECTIONS {
            let rows: Vec<Row> = names
                .iter()
                .filter_map(|name| command.find_subcommand(name))
                .map(|child| Row {
                    label: child.get_name().to_owned(),
                    description: about(child),
                    notes: String::new(),
                })
                .collect();
            blocks.push(table(title, &rows, width));
        }
    } else if !subcommands.is_empty() {
        let rows: Vec<Row> = subcommands
            .iter()
            .map(|child| Row {
                label: child.get_name().to_owned(),
                description: about(child),
                notes: String::new(),
            })
            .collect();
        blocks.push(table("命令", &rows, width));
    }
    let arguments: Vec<Row> = positionals
        .iter()
        .map(|arg| Row {
            label: positional_label(arg),
            description: arg.get_help().map(ToString::to_string).unwrap_or_default(),
            notes: notes(arg),
        })
        .collect();
    blocks.push(table("参数", &arguments, width));
    let options: Vec<Row> = command
        .get_opts()
        .chain(
            command
                .get_arguments()
                .filter(|arg| !arg.is_positional() && !arg.get_action().takes_values()),
        )
        .filter(|arg| !arg.is_hide_set() && !arg.is_global_set())
        .map(|arg| Row {
            label: option_label(arg),
            description: arg.get_help().map(ToString::to_string).unwrap_or_default(),
            notes: notes(arg),
        })
        .collect();
    blocks.push(table("选项", &dedup(options), width));
    let globals: Vec<Row> = root
        .get_arguments()
        .filter(|arg| arg.is_global_set() && !arg.is_hide_set())
        .map(|arg| Row {
            label: option_label(arg),
            description: arg.get_help().map(ToString::to_string).unwrap_or_default(),
            notes: String::new(),
        })
        .collect();
    blocks.push(table("全局选项", &globals, width));
    if let Some((_, examples)) = EXAMPLES.iter().find(|(name, _)| *name == joined) {
        let rows: Vec<Row> = examples
            .iter()
            .map(|(example, description)| Row {
                label: format!("{BINARY} {example}"),
                description: (*description).to_owned(),
                notes: String::new(),
            })
            .collect();
        let mut lines = vec![output::bold("示例")];
        for row in rows {
            lines.push(format!("  {}", output::paint(Role::Code, &row.label)));
            lines.push(format!(
                "      {}",
                output::paint(Role::Muted, &row.description)
            ));
        }
        blocks.push(lines.join("\n"));
    }
    if !path.is_empty() && subcommands.is_empty() {
        blocks.push(output::paint(
            Role::Muted,
            &format!("参数与输出的 JSON 结构：{BINARY} schema {joined}"),
        ));
    }
    let mut help = blocks
        .into_iter()
        .filter(|block| !block.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");
    help.push('\n');
    help
}

/// Rows with the same label once.
fn dedup(rows: Vec<Row>) -> Vec<Row> {
    let mut seen = std::collections::HashSet::new();
    rows.into_iter()
        .filter(|row| seen.insert(row.label.clone()))
        .collect()
}

fn context_text(error: &clap::Error, kind: ContextKind) -> Option<String> {
    match error.get(kind)? {
        ContextValue::String(text) => Some(text.clone()),
        ContextValue::Strings(texts) => Some(texts.join(", ")),
        ContextValue::StyledStr(text) => Some(text.to_string()),
        ContextValue::StyledStrs(texts) => Some(
            texts
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", "),
        ),
        ContextValue::Number(number) => Some(number.to_string()),
        ContextValue::Bool(flag) => Some(flag.to_string()),
        ContextValue::None => None,
        _ => None,
    }
}

/// A clap failure in this CLI's words: what was wrong, the closest command
/// or option, and where the help is.
pub fn usage_error(words: &[String], error: &clap::Error) -> CliError {
    let path = path_of(words);
    let invalid = context_text(error, ContextKind::InvalidArg);
    let value = context_text(error, ContextKind::InvalidValue);
    let mut message = match error.kind() {
        ErrorKind::InvalidSubcommand => format!(
            "未知命令：{}",
            context_text(error, ContextKind::InvalidSubcommand).unwrap_or_default()
        ),
        ErrorKind::UnknownArgument => match invalid.as_deref() {
            Some(word) if !word.starts_with('-') => format!("多余的参数：{word}"),
            Some(word) => format!("未知选项：{word}"),
            None => "未知选项".to_owned(),
        },
        ErrorKind::InvalidValue | ErrorKind::ValueValidation => match (invalid, value) {
            (Some(arg), Some(value)) => match error.source() {
                Some(reason) => format!("{arg}：{reason}"),
                None => format!("{arg} 的值无效：{value}"),
            },
            (Some(arg), None) => format!("{arg} 的值无效"),
            _ => error
                .to_string()
                .lines()
                .next()
                .unwrap_or_default()
                .trim_start_matches("error: ")
                .to_owned(),
        },
        ErrorKind::MissingRequiredArgument => format!("缺少参数：{}", invalid.unwrap_or_default()),
        ErrorKind::MissingSubcommand | ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand => {
            format!("{} 需要子命令", path.join(" "))
        }
        ErrorKind::ArgumentConflict => format!(
            "{} 不能与 {} 同时使用",
            invalid.unwrap_or_default(),
            context_text(error, ContextKind::PriorArg).unwrap_or_default()
        ),
        ErrorKind::NoEquals => format!("{} 需要写成 {0}=值", invalid.unwrap_or_default()),
        ErrorKind::TooManyValues | ErrorKind::WrongNumberOfValues | ErrorKind::TooFewValues => {
            format!("{} 的值个数不对", invalid.unwrap_or_default())
        }
        _ => error
            .to_string()
            .lines()
            .next()
            .unwrap_or_default()
            .trim_start_matches("error: ")
            .to_owned(),
    };
    if let Some(valid) = context_text(error, ContextKind::ValidValue) {
        message.push_str(&format!("（可选：{valid}）"));
    }
    let prefix = if path.is_empty() {
        String::new()
    } else {
        format!("{} ", path.join(" "))
    };
    if let Some(suggestion) = context_text(error, ContextKind::SuggestedSubcommand) {
        let best = suggestion
            .split(", ")
            .last()
            .unwrap_or(&suggestion)
            .to_owned();
        message.push_str(&format!("\n你是否要找：{BINARY} {prefix}{best}？"));
    }
    if let Some(suggestion) = context_text(error, ContextKind::SuggestedArg) {
        message.push_str(&format!("\n你是否要找：{suggestion}？"));
    }
    message.push_str(&format!("\n用 {BINARY} {prefix}--help 查看用法"));
    CliError::new("usage", message)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(text: &str) -> Vec<String> {
        text.split_whitespace().map(str::to_owned).collect()
    }

    #[test]
    fn help_paths_skip_options_and_stop_at_arguments() {
        assert_eq!(
            path_of(&words("-C D:/case sqlite query /x/y.db --help")),
            ["sqlite", "query"]
        );
        assert_eq!(path_of(&words("ls /nas --limit 5")), ["ls"]);
        assert!(path_of(&words("--json")).is_empty());
    }

    #[test]
    fn every_root_command_is_in_a_help_section() {
        let root = Cli::command();
        for child in root.get_subcommands().filter(|child| !child.is_hide_set()) {
            assert!(
                SECTIONS
                    .iter()
                    .any(|(_, names)| names.contains(&child.get_name())),
                "{} is in no help section",
                child.get_name()
            );
        }
        for (_, names) in SECTIONS {
            for name in *names {
                assert!(
                    root.find_subcommand(name).is_some(),
                    "{name} is not a command"
                );
            }
        }
    }

    #[test]
    fn examples_parse_as_commands() {
        for (path, examples) in EXAMPLES {
            for (example, _) in *examples {
                let words = crate::cli::shell::tokenize(example).unwrap();
                let words: Vec<String> = words
                    .into_iter()
                    .take_while(|word| word != "<" && word != ">")
                    .collect();
                assert!(
                    crate::cli::runtime::parse(&words).is_ok(),
                    "{path}: {example}"
                );
            }
        }
    }

    #[test]
    fn usage_errors_name_the_problem_and_the_help() {
        let error = crate::cli::runtime::parse(&words("ls /nas --limt 5")).unwrap_err();
        assert_eq!(error.code, "usage");
        assert!(error.message.contains("--limt"), "{}", error.message);
        assert!(error.message.contains("--limit"), "{}", error.message);
        assert!(
            error
                .message
                .ends_with("用 lovelycarve-cli ls --help 查看用法"),
            "{}",
            error.message
        );
        let error = crate::cli::runtime::parse(&words("lss /")).unwrap_err();
        assert!(error.message.contains("未知命令：lss"), "{}", error.message);
        assert!(render(&["ls".into()]).contains("--limit"));
        assert!(render(&[]).contains("浏览与读取"));
    }
}
