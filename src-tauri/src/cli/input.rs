use clap::{Arg, ArgAction, ArgMatches, Command};
use serde_json::{Map, Value};

use super::{catalog, CliError};

pub(super) fn parser() -> Result<Command, CliError> {
    let mut parser = Command::new("lovelycarve-cli")
        .version(env!("CARGO_PKG_VERSION"))
        .about("LovelyCarve 无界面取证终端：交互浏览、案件分析与全功能脚本接口")
        .arg(
            Arg::new("json")
                .long("json")
                .global(true)
                .action(ArgAction::SetTrue)
                .help("结果输出 JSON"),
        )
        .arg(
            Arg::new("events")
                .long("events")
                .global(true)
                .action(ArgAction::SetTrue)
                .help("将后台事件以 JSONL 输出到 stderr"),
        )
        .arg(
            Arg::new("image")
                .long("image")
                .global(true)
                .help("执行前打开检材"),
        )
        .arg(
            Arg::new("case")
                .long("case")
                .global(true)
                .help("执行前打开案件目录"),
        )
        .subcommand(Command::new("shell").about("交互终端（默认）；help 查看命令，exit 退出"))
        .subcommand(Command::new("capabilities").about("查看本构建启用的功能与运行平台"))
        .subcommand(
            Command::new("session")
                .about("stdin/stdout JSONL 有状态协议；EOF 等待任务后退出")
                .arg(
                    Arg::new("jobs")
                        .long("jobs")
                        .short('j')
                        .default_value("1")
                        .value_parser(clap::value_parser!(u32).range(1..=64))
                        .help("并发查询数（1–64）；上下文与修改操作顺序执行"),
                ),
        )
        .subcommand(
            Command::new("run")
                .about("顺序执行命令脚本，出错即停止")
                .arg(Arg::new("script").required(true)),
        )
        .subcommand(
            Command::new("commands")
                .about("列出或查看完整后端命令与参数")
                .arg(Arg::new("name")),
        )
        .subcommand(
            Command::new("call")
                .about("按原始后端名称执行命令")
                .arg(Arg::new("command").required(true))
                .arg(Arg::new("args").long("args").conflicts_with("args-file"))
                .arg(
                    Arg::new("args-file")
                        .long("args-file")
                        .help("UTF-8 JSON 参数文件（适合复杂参数与敏感值）"),
                )
                .arg(
                    Arg::new("output")
                        .long("output")
                        .help("保存原始二进制或 JSON，拒绝覆盖已有文件"),
                ),
        );
    for (name, about, positional) in [
        ("open", "打开检材并设为当前上下文", Some("path")),
        ("use", "切换已加载的检材", Some("image-id")),
        ("select-partition", "选择分区（分区 ID）", Some("id")),
        ("cd", "切换镜像内目录，支持 /、.. 与相对路径", Some("path")),
        ("info", "当前检材信息", None),
        ("list-partitions", "当前检材分区表", None),
        ("list-devices", "枚举物理磁盘", None),
        ("pwd", "显示镜像内工作目录", None),
        ("tasks", "列出当前后台任务", None),
        ("wait", "等待后台任务和插件调度任务结束", None),
    ] {
        let mut command = Command::new(name).about(about);
        if let Some(positional) = positional {
            command = command.arg(Arg::new(positional).required(true));
        }
        parser = parser.subcommand(command);
    }
    for name in ["ls", "tree", "find"] {
        parser = parser.subcommand(
            Command::new(name)
                .about("通过卷索引分页浏览或搜索文件树；未覆盖目录沿用后端回退")
                .arg(Arg::new("path").default_value("."))
                .arg(
                    Arg::new("name")
                        .long("name")
                        .help("名称包含的文本（忽略大小写）"),
                )
                .arg(
                    Arg::new("limit")
                        .long("limit")
                        .default_value("10000")
                        .value_parser(clap::value_parser!(u32).range(1..=100000)),
                )
                .arg(
                    Arg::new("depth")
                        .long("depth")
                        .default_value("32")
                        .value_parser(clap::value_parser!(u32).range(1..=128)),
                ),
        );
    }
    parser = parser.subcommand(
        Command::new("grep")
            .about("复用全局搜索检索文件内容（需要案件）；默认搜索当前目录子树")
            .arg(Arg::new("pattern").required(true))
            .arg(Arg::new("path").default_value("."))
            .arg(
                Arg::new("regex")
                    .short('E')
                    .long("regex")
                    .action(ArgAction::SetTrue)
                    .help("将模式作为正则表达式"),
            )
            .arg(
                Arg::new("ignore-case")
                    .short('i')
                    .long("ignore-case")
                    .action(ArgAction::SetTrue),
            )
            .arg(
                Arg::new("exclude-deleted")
                    .long("exclude-deleted")
                    .action(ArgAction::SetTrue),
            )
            .arg(
                Arg::new("codepage")
                    .long("codepage")
                    .action(ArgAction::Append)
                    .help("编码页，可重复；省略使用全局搜索的全部编码"),
            )
            .arg(
                Arg::new("search-id")
                    .long("search-id")
                    .help("搜索任务 ID；省略自动生成"),
            )
            .arg(
                Arg::new("limit")
                    .long("limit")
                    .default_value("10000")
                    .value_parser(clap::value_parser!(u32).range(1..=100000))
                    .help("最多显示的命中记录数；完整结果保存在案件中"),
            ),
    );
    for name in ["stat", "file", "md5sum", "sha1sum", "sha256sum", "hash"] {
        parser = parser.subcommand(
            Command::new(name)
                .about(match name {
                    "stat" => "从目录索引读取节点属性、时间和删除状态（可离线使用）",
                    "file" => "使用 Magika 识别文件内容类型、MIME 和置信度",
                    _ => "流式计算文件哈希",
                })
                .arg(Arg::new("path").required(true)),
        );
    }
    parser = parser.subcommand(
        Command::new("strings")
            .about("流式提取文件内可打印字符串，显示字节偏移；需要原检材")
            .arg(Arg::new("path").required(true))
            .arg(
                Arg::new("encoding")
                    .long("encoding")
                    .short('e')
                    .default_value("ascii")
                    .value_parser(clap::value_parser!(super::strings::Encoding)),
            )
            .arg(
                Arg::new("min-length")
                    .long("min-length")
                    .short('n')
                    .default_value("4")
                    .value_parser(clap::value_parser!(u32).range(1..=4096))
                    .help("最少字符数"),
            )
            .arg(
                Arg::new("offset")
                    .long("offset")
                    .default_value("0")
                    .value_parser(clap::value_parser!(u64)),
            )
            .arg(
                Arg::new("length")
                    .long("length")
                    .value_parser(clap::value_parser!(u64).range(1..))
                    .help("扫描字节数；省略扫描到文件末尾"),
            )
            .arg(
                Arg::new("limit")
                    .long("limit")
                    .default_value("10000")
                    .value_parser(clap::value_parser!(u32).range(1..=100000))
                    .help("最多返回的字符串数"),
            ),
    );
    for name in ["cat", "hex"] {
        parser = parser.subcommand(
            Command::new(name)
                .about("有界文件读取；使用 export 导出完整文件")
                .arg(Arg::new("path").required(true))
                .arg(
                    Arg::new("offset")
                        .long("offset")
                        .default_value("0")
                        .value_parser(clap::value_parser!(u64)),
                )
                .arg(
                    Arg::new("length")
                        .long("length")
                        .default_value("4096")
                        .value_parser(clap::value_parser!(u32).range(1..=16777216)),
                ),
        );
    }
    parser = parser.subcommand(
        Command::new("export")
            .about("流式导出单文件，拒绝覆盖；目录/ZIP 使用 fs-export-dir")
            .arg(Arg::new("path").required(true))
            .arg(Arg::new("destination").required(true)),
    );
    for command in catalog::commands()? {
        let mut entry = Command::new(command.name.replace('_', "-"))
            .alias(command.name.clone())
            .about(if command.description.is_empty() {
                format!("{} / {}", command.group, command.name)
            } else {
                command.description
            });
        entry = match command.name.as_str() {
            "carve_start" => entry.visible_alias("carve"),
            "carve_results_page" => entry.visible_alias("carve-list"),
            "carve_recover" => entry.visible_alias("carve-export"),
            "global_search_start" => entry.visible_alias("rawsearch"),
            "timeline_start" => entry.visible_alias("timeline"),
            "fs_mount_readonly" => entry.visible_alias("mount"),
            "fs_unmount_readonly" => entry.visible_alias("unmount"),
            _ => entry,
        };
        for parameter in command.parameters {
            entry = entry.arg(Arg::new(parameter.name).long(parameter.flag).help(format!(
                "{}{}",
                parameter.r#type,
                if parameter.required {
                    "（必填；imageId 可用当前检材）"
                } else {
                    "（可选）"
                }
            )));
        }
        parser = parser.subcommand(entry);
    }
    Ok(parser)
}

pub(super) fn typed_value(raw: &str, kind: &str) -> Result<Value, CliError> {
    if kind == "String" || kind == "PathBuf" || kind.starts_with("Option < String") {
        Ok(Value::String(raw.to_string()))
    } else {
        serde_json::from_str(raw)
            .map_err(|error| CliError::invalid(format!("参数要求 {kind}，请传 JSON 值: {error}")))
    }
}

pub(super) fn object(raw: &str) -> Result<Map<String, Value>, CliError> {
    match serde_json::from_str(raw.trim_start_matches('\u{feff}'))? {
        Value::Object(args) => Ok(args),
        _ => Err(CliError::invalid("参数必须是 JSON 对象")),
    }
}

/// JSONL shell commands use the same argument parser as interactive commands.
/// Build argv directly: evidence paths and patterns are never shell-expanded.
pub(super) fn shell_matches(
    command: &str,
    args: &Map<String, Value>,
) -> Result<Option<ArgMatches>, CliError> {
    let command = command.replace('_', "-");
    if !matches!(
        command.as_str(),
        "open"
            | "use"
            | "select-partition"
            | "cd"
            | "pwd"
            | "ls"
            | "tree"
            | "find"
            | "stat"
            | "hex"
            | "strings"
            | "file"
            | "cat"
            | "grep"
            | "hash"
            | "md5sum"
            | "sha1sum"
            | "sha256sum"
            | "info"
            | "list-partitions"
            | "list-devices"
            | "tasks"
    ) {
        return Ok(None);
    }
    let parser = parser()?;
    let descriptor = parser
        .find_subcommand(&command)
        .ok_or_else(|| CliError::invalid("未知终端命令"))?;
    let mut normalized = Map::new();
    for (key, value) in args {
        let key = key.replace('_', "-");
        if normalized.insert(key.clone(), value.clone()).is_some() {
            return Err(CliError::invalid(format!("重复参数: {key}")));
        }
        if !descriptor
            .get_arguments()
            .any(|argument| argument.get_id().as_str() == key)
        {
            return Err(CliError::invalid(format!("未知终端参数: {key}")));
        }
    }
    let mut words = vec!["lovelycarve-cli".to_owned(), command.clone()];
    let mut positional = Vec::new();
    for argument in descriptor.get_arguments() {
        let Some(value) = normalized.get(argument.get_id().as_str()) else {
            continue;
        };
        let flag = argument.get_long().map(|name| format!("--{name}"));
        if matches!(argument.get_action(), ArgAction::SetTrue) {
            match value.as_bool() {
                Some(true) => words.push(flag.ok_or_else(|| CliError::invalid("开关缺少名称"))?),
                Some(false) => {}
                None => {
                    return Err(CliError::invalid(format!(
                        "{} 要求布尔值",
                        argument.get_id()
                    )))
                }
            }
            continue;
        }
        let values = match value {
            Value::Array(values) if matches!(argument.get_action(), ArgAction::Append) => {
                values.as_slice()
            }
            _ => std::slice::from_ref(value),
        };
        for value in values {
            let text = match value {
                Value::String(text) => text.clone(),
                Value::Number(number) => number.to_string(),
                Value::Null | Value::Bool(_) | Value::Array(_) | Value::Object(_) => {
                    return Err(CliError::invalid(format!(
                        "{} 要求文本或数值",
                        argument.get_id()
                    )))
                }
            };
            match &flag {
                Some(flag) => words.push(format!("{flag}={text}")),
                None => positional.push(text),
            }
        }
    }
    words.push("--".into());
    words.extend(positional);
    parser
        .try_get_matches_from(words)
        .map(Some)
        .map_err(|error| CliError::invalid(error.to_string()))
}

/// Deliberately not a system shell: backslashes in Windows paths remain literal;
/// single/double quotes group arguments, with no expansion or command execution.
pub(super) fn tokenize(line: &str) -> Result<Vec<String>, CliError> {
    let mut result = Vec::new();
    let mut token = String::new();
    let mut quote = None;
    let mut started = false;
    for character in line.chars() {
        if let Some(delimiter) = quote {
            if character == delimiter {
                quote = None;
            } else {
                token.push(character);
            }
        } else if character == '\'' || character == '"' {
            quote = Some(character);
            started = true;
        } else if character.is_whitespace() {
            if started {
                result.push(std::mem::take(&mut token));
                started = false;
            }
        } else {
            token.push(character);
            started = true;
        }
    }
    if quote.is_some() {
        return Err(CliError::invalid("引号未闭合"));
    }
    if started {
        result.push(token);
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn shell_preserves_windows_paths_and_json() {
        assert_eq!(
            tokenize(r#"open "D:\Evidence Files\disk.raw""#).unwrap(),
            ["open", "D:\\Evidence Files\\disk.raw"]
        );
        assert_eq!(
            tokenize(r#"call image_info --args '{"imageId":"abc"}'"#).unwrap(),
            ["call", "image_info", "--args", r#"{"imageId":"abc"}"#]
        );
        assert!(tokenize("open 'missing").is_err());
        assert_eq!(tokenize("cd ''").unwrap(), ["cd", ""]);
    }
    #[test]
    fn rejects_non_objects_and_parses_typed_flags() {
        assert!(object("[]").is_err());
        assert_eq!(typed_value("false", "bool").unwrap(), Value::Bool(false));
        assert_eq!(
            typed_value("null", "String").unwrap(),
            Value::String("null".into())
        );
    }
    #[test]
    fn generated_clap_commands_do_not_collide() {
        parser().unwrap().debug_assert();
    }

    #[test]
    fn session_jobs_are_bounded_and_default_to_sequential() {
        let matches = parser()
            .unwrap()
            .try_get_matches_from(["lovelycarve-cli", "session"])
            .unwrap();
        assert_eq!(
            matches
                .subcommand_matches("session")
                .unwrap()
                .get_one::<u32>("jobs"),
            Some(&1)
        );
        for value in ["0", "65", "no"] {
            assert!(parser()
                .unwrap()
                .try_get_matches_from(["lovelycarve-cli", "session", "--jobs", value])
                .is_err());
        }
    }

    #[test]
    fn jsonl_shell_commands_preserve_literals_and_validate_options() {
        let matches = shell_matches("grep", &object(r#"{"pattern":"--literal","path":"/folder with spaces/a.txt","ignore_case":true,"codepage":["UTF-8","UTF-16LE"]}"#).unwrap()).unwrap().unwrap();
        let args = matches.subcommand_matches("grep").unwrap();
        assert_eq!(args.get_one::<String>("pattern").unwrap(), "--literal");
        assert_eq!(
            args.get_one::<String>("path").unwrap(),
            "/folder with spaces/a.txt"
        );
        assert!(args.get_flag("ignore-case"));
        assert_eq!(
            args.get_many::<String>("codepage")
                .unwrap()
                .cloned()
                .collect::<Vec<_>>(),
            ["UTF-8", "UTF-16LE"]
        );
        assert!(shell_matches("pwd", &object(r#"{"typo":true}"#).unwrap()).is_err());
        assert!(shell_matches(
            "grep",
            &object(r#"{"pattern":"text","ignore_case":"true"}"#).unwrap()
        )
        .is_err());
        assert!(shell_matches(
            "strings",
            &object(r#"{"path":"a.txt","min_length":0}"#).unwrap()
        )
        .is_err());
        assert!(shell_matches("image_load", &Map::new()).unwrap().is_none());
    }
}
