//! `--jq`: a jq program, compiled while the command line is parsed, that
//! filters or reshapes one command's JSON result on stdout. It reads that
//! result and writes only to stdout. The built-ins that would end the process
//! with an exit status of their own (`halt`) or write to stderr (`debug`,
//! `stderr`) are left out, so the exit status and stderr keep reporting the
//! command's own result.
use std::{
    fmt,
    io::{IsTerminal, Write},
    str::FromStr,
    sync::Arc,
};

use jaq_core::{
    compile::Undefined,
    data::JustLut,
    load::{self, Arena, File, Loader},
    Compiler, Ctx, Exn, Vars,
};
use jaq_json::{read, write, Num, Val};

use super::{output, CliError, Json};

type Program = jaq_core::Filter<JustLut<Val>>;

/// A compiled `--jq` program.
#[derive(Clone)]
pub(super) struct Filter(Arc<Program>);

impl fmt::Debug for Filter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Filter").finish_non_exhaustive()
    }
}

/// Why `--jq` leaves out the jq built-in `name`, when it does.
fn left_out(name: &str) -> Option<&'static str> {
    match name {
        "halt" | "halt_error" => Some("退出码由命令结果决定"),
        "debug" | "debug_empty" | "stderr" | "stderr_empty" => Some("标准错误只输出错误与警告"),
        "input" | "inputs" => Some("输入只有当前命令的结果"),
        _ => None,
    }
}

impl FromStr for Filter {
    type Err = CliError;

    fn from_str(code: &str) -> Result<Self, Self::Err> {
        let defs = jaq_core::defs()
            .chain(jaq_std::defs())
            .chain(jaq_json::defs())
            .filter(|def| left_out(def.name).is_none());
        let funs = jaq_core::funs()
            .chain(jaq_std::funs())
            .chain(jaq_json::funs())
            .filter(|(name, ..)| left_out(name).is_none());
        let arena = Arena::default();
        let modules = Loader::new(defs)
            .load(&arena, File { code, path: () })
            .map_err(syntax_error)?;
        // The program reads the command's result only: no data files.
        load::import(&modules, |_| Err(String::new()))
            .map_err(|_| CliError::new("usage", "不能导入数据文件"))?;
        let program = Compiler::default()
            .with_funs(funs)
            .compile(modules)
            .map_err(|errors| {
                let message = errors
                    .iter()
                    .find_map(|(_, names)| names.first())
                    .map_or_else(|| "表达式无法编译".to_owned(), undefined);
                CliError::new("usage", message)
            })?;
        Ok(Self(Arc::new(program)))
    }
}

// The messages below follow `--jq <表达式>：` in the usage error.

/// The first syntax error: what jaq expected, and where.
fn syntax_error(errors: load::Errors<&str, ()>) -> CliError {
    let message = errors.first().and_then(|(_, error)| match error {
        load::Error::Io(errors) => errors
            .first()
            .map(|(path, _)| format!("不能导入模块 {path}")),
        load::Error::Lex(errors) => errors.first().map(|(expected, rest)| {
            let expected = expected.as_str();
            match rest.lines().next().unwrap_or_default() {
                "" => format!("表达式不完整，缺少 {expected}"),
                rest => {
                    let at: String = rest.chars().take(16).collect();
                    format!("表达式在「{at}」处需要 {expected}")
                }
            }
        }),
        load::Error::Parse(errors) => errors.first().map(|(expected, found)| {
            let expected = expected.as_str();
            if found.is_empty() {
                format!("表达式末尾需要 {expected}")
            } else {
                format!("表达式需要 {expected}，遇到「{found}」")
            }
        }),
    });
    CliError::new("usage", message.unwrap_or_else(|| "表达式无效".to_owned()))
}

/// A name the program uses but nothing defines.
fn undefined((name, kind): &(&str, Undefined)) -> String {
    if let Some(reason) = left_out(name) {
        return format!("不提供 {name}，{reason}");
    }
    let kind = match kind {
        Undefined::Filter(_) => "函数",
        Undefined::Var => "变量",
        Undefined::Label => "标签",
        Undefined::Mod => "模块",
        // `Undefined` is non-exhaustive.
        _ => "名称",
    };
    format!("{kind} {name} 未定义")
}

/// A failure while the program runs, with the message `error` raised.
fn failure(exception: Exn<'_, Val>) -> CliError {
    match exception.get_err() {
        Ok(error) => {
            let message = match error.into_val() {
                Val::TStr(text) => String::from_utf8_lossy(&text).into_owned(),
                value => value.to_string(),
            };
            CliError::invalid(format!("jq 求值失败：{message}"))
        }
        // `halt` is left out, and nothing else halts.
        Err(_) => super::internal_error("jq 表达式意外终止"),
    }
}

impl Filter {
    /// Print what the program yields for `input`: a string as it is, any
    /// other value as `--json` prints it, each followed by a newline. Values
    /// printed before a failure stay printed.
    pub(super) fn print(&self, input: &Json) -> Result<(), CliError> {
        let terminal = std::io::stdout().is_terminal();
        let mut stdout = std::io::BufWriter::new(std::io::stdout().lock());
        let printed = self.write(input, &mut stdout, terminal);
        let flushed = stdout.flush();
        printed?;
        flushed?;
        Ok(())
    }

    fn write(&self, input: &Json, out: &mut impl Write, terminal: bool) -> Result<(), CliError> {
        let input = read::parse_single(input.text().as_bytes())
            .map_err(|error| super::internal_error(format!("jq 无法读取命令结果：{error}")))?;
        let context = Ctx::<JustLut<Val>>::new(&self.0.lut, Vars::new([]));
        for value in self.0.id.run((context, input)) {
            let value = value.map_err(failure)?;
            if let Val::TStr(text) = &value {
                let text = std::str::from_utf8(text)
                    .map_err(|_| CliError::invalid("jq 结果中的字符串不是有效的 UTF-8"))?;
                if terminal {
                    out.write_all(shown(text).as_bytes())?;
                } else {
                    out.write_all(text.as_bytes())?;
                }
            } else {
                let text = compact(&value)?;
                if terminal {
                    out.write_all(output::indent_json(&text).as_bytes())?;
                } else {
                    out.write_all(text.as_bytes())?;
                }
            }
            out.write_all(b"\n")?;
            if terminal {
                out.flush()?;
            }
        }
        Ok(())
    }
}

/// A string as the terminal shows it: line breaks and tabs stay, other
/// control and format characters become spaces.
fn shown(text: &str) -> String {
    text.split('\n')
        .map(|line| {
            line.split('\t')
                .map(output::sanitize)
                .collect::<Vec<_>>()
                .join("\t")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// A value as compact JSON text. jaq also has byte strings, non-string
/// keys and non-finite numbers, which JSON cannot hold.
fn compact(value: &Val) -> Result<String, CliError> {
    if !is_json(value) {
        let preview: String = value.to_string().chars().take(60).collect();
        return Err(CliError::invalid(format!(
            "jq 结果无法表示为 JSON：{preview}"
        )));
    }
    let mut text = Vec::new();
    write::write(&mut text, &write::Pp::default(), 0, value)?;
    String::from_utf8(text).map_err(super::internal_error)
}

fn is_json(value: &Val) -> bool {
    match value {
        Val::TStr(text) => std::str::from_utf8(text).is_ok(),
        Val::BStr(_) => false,
        Val::Num(Num::Float(number)) => number.is_finite(),
        Val::Arr(values) => values.iter().all(is_json),
        Val::Obj(entries) => entries
            .iter()
            .all(|(key, value)| matches!(key, Val::TStr(_)) && is_json(key) && is_json(value)),
        Val::Null | Val::Bool(_) | Val::Num(_) => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(program: &str, input: &str) -> (String, Result<(), CliError>) {
        let filter: Filter = program.parse().unwrap();
        let mut out = Vec::new();
        let result = filter.write(&Json::from_text(input.to_owned()), &mut out, false);
        (String::from_utf8(out).unwrap(), result)
    }

    fn compile_error(program: &str) -> String {
        let error = program.parse::<Filter>().unwrap_err();
        assert_eq!(error.code, "usage", "{program}: {}", error.message);
        error.message
    }

    #[test]
    fn values_print_one_per_line_with_strings_raw() {
        let input = r#"{"z":1,"a":[1.5,"x\ty"],"big":18446744073709551616}"#;
        let (out, result) = run(".a[], .z, ., .big + 1, empty", input);
        result.unwrap();
        assert_eq!(
            out,
            format!("1.5\nx\ty\n1\n{input}\n18446744073709551617\n")
        );
        let (out, result) = run(".items[] | select(.size > 9)", r#"{"items":[{"size":1}]}"#);
        result.unwrap();
        assert_eq!(out, "");
    }

    #[test]
    fn failures_keep_the_values_printed_before_them() {
        let (out, result) = run(r#"1, error("bad filter"), 2"#, "null");
        let error = result.unwrap_err();
        assert_eq!(
            (out.as_str(), error.code.as_str(), error.message.as_str()),
            ("1\n", "invalidInput", "jq 求值失败：bad filter")
        );
        for program in ["nan", "infinite", "{(1): 2}", r#""x" | tobytes"#] {
            let (out, result) = run(program, "null");
            let error = result.unwrap_err();
            assert_eq!(out, "", "{program}");
            assert!(
                error.message.starts_with("jq 结果无法表示为 JSON"),
                "{program}: {}",
                error.message
            );
        }
    }

    #[test]
    fn strings_on_a_terminal_keep_only_line_breaks_and_tabs() {
        assert_eq!(shown("a\u{1b}[31mb\tc\r\nd\u{202e}"), "a [31mb\tc \nd ");
    }

    #[test]
    fn left_out_built_ins_and_imports_fail_before_anything_runs() {
        for (program, name) in [
            ("halt", "halt"),
            ("halt(3)", "halt"),
            ("halt_error", "halt_error"),
            (r#""x" | halt_error(1)"#, "halt_error"),
            ("debug", "debug"),
            (r#"debug("x")"#, "debug"),
            ("stderr", "stderr"),
            ("stderr_empty", "stderr_empty"),
            ("input", "input"),
            ("[inputs]", "inputs"),
        ] {
            let message = compile_error(program);
            assert!(
                message.starts_with(&format!("不提供 {name}，")),
                "{program}: {message}"
            );
        }
        for (program, message) in [
            ("unknown_filter", "函数 unknown_filter 未定义"),
            ("$x", "变量 $x 未定义"),
            (r#"include "lib"; ."#, "不能导入模块 lib"),
            (r#"import "external" as $data; $data"#, "不能导入数据文件"),
            (".[", "表达式不完整，缺少 closing bracket"),
            (".a)", "表达式在「)」处需要 token"),
            ("1 +", "表达式末尾需要 term"),
            (".a | | .b", "表达式需要 term，遇到「|」"),
        ] {
            assert_eq!(compile_error(program), message, "{program}");
        }
    }

    #[test]
    fn the_standard_library_stays_available() {
        let (out, result) = run(
            r#"[.[] | select(test("^b"; "i"))] | sort | map(ascii_upcase) | join(",")"#,
            r#"["beta","alpha","Bravo"]"#,
        );
        result.unwrap();
        assert_eq!(out, "BRAVO,BETA\n");
    }
}
