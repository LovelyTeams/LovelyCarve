//! What reaches the terminal: results on stdout, everything else on stderr,
//! with the colour roles, the once-per-process banner and the framed error
//! block.
use std::{
    io::{IsTerminal, Write},
    sync::atomic::{AtomicBool, Ordering},
};

use super::{help, interactive, CliError, Json, Origin, Output};

/// Larger JSON is printed without highlighting.
const MAX_HIGHLIGHTED_JSON: usize = 50_000;

/// The semantic palette: mid-luminance colours that read on light and dark
/// backgrounds alike.
#[derive(Clone, Copy)]
pub enum Role {
    Brand,
    Success,
    Error,
    Warning,
    Muted,
    Code,
    JsonKey,
    JsonString,
    JsonNumber,
    JsonBoolean,
}

impl Role {
    fn rgb(self) -> (u8, u8, u8) {
        match self {
            Self::Brand | Self::Warning | Self::JsonNumber => (0xBF, 0x66, 0x00),
            Self::Success | Self::JsonString => (0x3A, 0x8E, 0x52),
            Self::Error => (0xD4, 0x53, 0x50),
            Self::Muted => (0x7F, 0x7F, 0x7F),
            Self::Code | Self::JsonKey => (0x3D, 0x82, 0xBB),
            Self::JsonBoolean => (0x92, 0x6C, 0xC1),
        }
    }
}

/// The terminal's own colours, which prompts and the spinner use.
#[derive(Clone, Copy)]
pub enum Tone {
    Cyan,
    Green,
    Red,
    Yellow,
    Magenta,
    Gray,
    Dim,
    Strike,
}

impl Tone {
    fn codes(self) -> (&'static str, &'static str) {
        match self {
            Self::Cyan => ("36", "39"),
            Self::Green => ("32", "39"),
            Self::Red => ("31", "39"),
            Self::Yellow => ("33", "39"),
            Self::Magenta => ("35", "39"),
            Self::Gray => ("90", "39"),
            Self::Dim => ("2", "22"),
            Self::Strike => ("9", "29"),
        }
    }
}

fn truecolor() -> bool {
    cfg!(windows)
        || std::env::var("COLORTERM")
            .is_ok_and(|value| value.eq_ignore_ascii_case("truecolor") || value == "24bit")
        || std::env::var("TERM_PROGRAM").is_ok_and(|value| {
            ["iTerm.app", "WezTerm", "ghostty", "vscode"].contains(&value.as_str())
        })
}

/// Whether styling is emitted at all; on Windows this also switches the
/// console to virtual terminal processing.
pub fn styled() -> bool {
    static STYLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *STYLED.get_or_init(|| {
        if !interactive::supports_color() {
            return false;
        }
        #[cfg(windows)]
        {
            crossterm::ansi_support::supports_ansi()
        }
        #[cfg(not(windows))]
        {
            true
        }
    })
}

pub fn paint(role: Role, text: &str) -> String {
    if !styled() {
        return text.to_owned();
    }
    if truecolor() {
        let (red, green, blue) = role.rgb();
        return format!("\x1b[38;2;{red};{green};{blue}m{text}\x1b[39m");
    }
    match role {
        Role::Muted => text.to_owned(),
        _ => bold(text),
    }
}

pub fn tone(tone: Tone, text: &str) -> String {
    if !styled() || text.is_empty() {
        return text.to_owned();
    }
    let (open, close) = tone.codes();
    format!("\x1b[{open}m{text}\x1b[{close}m")
}

pub fn bold(text: &str) -> String {
    if styled() {
        format!("\x1b[1m{text}\x1b[22m")
    } else {
        text.to_owned()
    }
}

pub fn italic(text: &str) -> String {
    if styled() {
        format!("\x1b[3m{text}\x1b[23m")
    } else {
        text.to_owned()
    }
}

/// Terminal columns `text` occupies: wide (CJK) characters take two.
pub fn display_width(text: &str) -> usize {
    help::display_width(text)
}

/// Characters of the Unicode format category (Cf), by range.
const FORMAT_CHARACTERS: &[(u32, u32)] = &[
    (0x00AD, 0x00AD),
    (0x0600, 0x0605),
    (0x061C, 0x061C),
    (0x06DD, 0x06DD),
    (0x070F, 0x070F),
    (0x0890, 0x0891),
    (0x08E2, 0x08E2),
    (0x180E, 0x180E),
    (0x200B, 0x200F),
    (0x202A, 0x202E),
    (0x2060, 0x2064),
    (0x2066, 0x206F),
    (0xFEFF, 0xFEFF),
    (0xFFF9, 0xFFFB),
    (0x110BD, 0x110BD),
    (0x110CD, 0x110CD),
    (0x13430, 0x1343F),
    (0x1BCA0, 0x1BCA3),
    (0x1D173, 0x1D17A),
    (0xE0001, 0xE0001),
    (0xE0020, 0xE007F),
];

/// Replace characters that could alter or control terminal display: the
/// control, format, line separator and paragraph separator categories.
pub fn sanitize(text: &str) -> String {
    text.chars()
        .map(|character| {
            let code = character as u32;
            let format = FORMAT_CHARACTERS
                .iter()
                .any(|(first, last)| (*first..=*last).contains(&code));
            if character.is_control() || format || matches!(character, '\u{2028}' | '\u{2029}') {
                ' '
            } else {
                character
            }
        })
        .collect()
}

static BANNER_PRINTED: AtomicBool = AtomicBool::new(false);
static BANNER_SUPPRESSED: AtomicBool = AtomicBool::new(false);

/// `lovelycarve-cli · v…` and an underline, once per process, on stderr; never
/// when quiet (`--quiet` or `LOVELYCARVE_QUIET=1`) or when stderr is not a terminal.
pub fn open_session(quiet: bool) {
    if BANNER_PRINTED.load(Ordering::SeqCst) || BANNER_SUPPRESSED.load(Ordering::SeqCst) {
        return;
    }
    if quiet || interactive::quiet_env() || !std::io::stderr().is_terminal() {
        BANNER_SUPPRESSED.store(true, Ordering::SeqCst);
        return;
    }
    let mut stderr = std::io::stderr().lock();
    if writeln!(stderr, "{}", headline()).is_ok() {
        BANNER_PRINTED.store(true, Ordering::SeqCst);
    }
}

pub fn headline() -> String {
    let version = format!("v{}", env!("CARGO_PKG_VERSION"));
    let text = format!(
        "{} {} {}",
        paint(Role::Brand, &bold("lovelycarve-cli")),
        paint(Role::Muted, "·"),
        paint(Role::Muted, &version)
    );
    let width = display_width("lovelycarve-cli · ") + display_width(&version);
    format!("{text}\n{}", paint(Role::Brand, &"─".repeat(width)))
}

/// One JSON token of compact or pretty JSON text.
enum Token<'a> {
    Open(char),
    Close(char),
    Colon,
    Comma,
    Text(&'a str),
    Scalar(&'a str),
}

fn tokens(text: &str) -> Vec<Token<'_>> {
    let mut tokens = Vec::new();
    let mut characters = text.char_indices().peekable();
    while let Some((start, character)) = characters.next() {
        match character {
            '{' | '[' => tokens.push(Token::Open(character)),
            '}' | ']' => tokens.push(Token::Close(character)),
            ':' => tokens.push(Token::Colon),
            ',' => tokens.push(Token::Comma),
            '"' => {
                let mut end = text.len();
                let mut escaped = false;
                for (index, inner) in characters.by_ref() {
                    if escaped {
                        escaped = false;
                    } else if inner == '\\' {
                        escaped = true;
                    } else if inner == '"' {
                        end = index + 1;
                        break;
                    }
                }
                tokens.push(Token::Text(&text[start..end]));
            }
            _ if character.is_whitespace() => {}
            _ => {
                let mut end = start + character.len_utf8();
                while let Some((index, next)) = characters.peek().copied() {
                    if matches!(next, ',' | ':' | '{' | '}' | '[' | ']') || next.is_whitespace() {
                        break;
                    }
                    end = index + next.len_utf8();
                    characters.next();
                }
                tokens.push(Token::Scalar(&text[start..end]));
            }
        }
    }
    tokens
}

/// JSON text laid out as `serde_json::to_string_pretty` would (two-space
/// indent, `": "`, empty `[]` and `{}`), keeping the key order, with each
/// token painted when `highlight` is set.
fn pretty(text: &str, highlight: bool) -> String {
    let paint_if = |role: Role, token: &str| {
        if highlight {
            paint(role, token)
        } else {
            token.to_owned()
        }
    };
    let tokens = tokens(text);
    let mut out = String::with_capacity(text.len() * 2);
    let mut depth = 0usize;
    let newline = |out: &mut String, depth: usize| {
        out.push('\n');
        out.push_str(&"  ".repeat(depth));
    };
    let mut index = 0;
    while index < tokens.len() {
        match &tokens[index] {
            Token::Open(open) => {
                out.push(*open);
                if let Some(Token::Close(close)) = tokens.get(index + 1) {
                    out.push(*close);
                    index += 1;
                } else {
                    depth += 1;
                    newline(&mut out, depth);
                }
            }
            Token::Close(close) => {
                depth = depth.saturating_sub(1);
                newline(&mut out, depth);
                out.push(*close);
            }
            Token::Colon => out.push_str(": "),
            Token::Comma => {
                out.push(',');
                newline(&mut out, depth);
            }
            Token::Text(token) => {
                let role = if matches!(tokens.get(index + 1), Some(Token::Colon)) {
                    Role::JsonKey
                } else {
                    Role::JsonString
                };
                out.push_str(&paint_if(role, token));
            }
            Token::Scalar(token) => {
                let role = match *token {
                    "true" | "false" => Role::JsonBoolean,
                    "null" => Role::Muted,
                    _ => Role::JsonNumber,
                };
                out.push_str(&paint_if(role, token));
            }
        }
        index += 1;
    }
    out
}

/// Compact JSON for pipes and agents; indented, optionally highlighted JSON
/// for people at a terminal.
pub fn format_json(json: &Json) -> String {
    let text = json.text();
    let highlight =
        !interactive::is_non_interactive_or_ci() && styled() && text.len() <= MAX_HIGHLIGHTED_JSON;
    if !std::io::stdout().is_terminal() || interactive::is_agentic() {
        text.into_owned()
    } else {
        pretty(&text, highlight)
    }
}

/// Indented JSON without colour, for files.
pub fn plain_json(json: &Json) -> String {
    pretty(&json.text(), false)
}

/// Print a result. Structured data is JSON; a command that returned nothing
/// leaves stdout empty and, at a terminal, confirms on stderr; raw content
/// goes to stdout unchanged (or decoded as UTF-8 with `--text`).
pub fn print_output(
    output: Output,
    success_label: &str,
    quiet: bool,
    text: bool,
) -> Result<(), CliError> {
    match output {
        Output::Json(json) if json.is_null() => {
            if !quiet && std::io::stderr().is_terminal() {
                eprintln!("{} {success_label}", paint(Role::Success, "✓"));
            }
        }
        Output::Json(json) => {
            let mut stdout = std::io::stdout().lock();
            writeln!(stdout, "{}", format_json(&json))?;
            stdout.flush()?;
        }
        Output::Bytes(bytes) => {
            let mut stdout = std::io::stdout().lock();
            if text {
                stdout.write_all(String::from_utf8_lossy(&bytes).as_bytes())?;
            } else {
                stdout.write_all(&bytes)?;
            }
            stdout.flush()?;
        }
        Output::Written => {}
    }
    Ok(())
}

/// A block's content: either plain text that the block wraps and paints, or
/// lines the caller already styled.
pub enum Content<'a> {
    Plain(&'a str),
    Styled(Vec<String>),
}

/// The framed error block: a blank line, `┌ Title`, the message, optional
/// muted details after a blank gutter line, `└`.
pub fn error_block(title: &str, content: Content<'_>, details: Option<&str>) -> String {
    let width = crossterm::terminal::size()
        .map(|(columns, _)| usize::from(columns))
        .unwrap_or(80)
        .saturating_sub(2)
        .max(20);
    let bar = paint(Role::Error, "│");
    let mut lines = vec![
        String::new(),
        format!(
            "{} {}",
            paint(Role::Error, "┌"),
            paint(Role::Error, &bold(title))
        ),
    ];
    let body = match content {
        Content::Plain(message) => help::wrap(message, width)
            .into_iter()
            .map(|line| {
                if line.is_empty() {
                    line
                } else {
                    paint(Role::Error, &bold(&line))
                }
            })
            .collect(),
        Content::Styled(lines) => lines,
    };
    for line in body {
        lines.push(if line.is_empty() {
            bar.clone()
        } else {
            format!("{bar} {line}")
        });
    }
    if let Some(details) = details {
        lines.push(bar.clone());
        for line in help::wrap(details, width) {
            lines.push(if line.is_empty() {
                bar.clone()
            } else {
                format!("{bar} {}", paint(Role::Muted, &line))
            });
        }
    }
    lines.push(paint(Role::Error, "└"));
    lines.join("\n")
}

/// `→ message`, muted.
pub fn hint(message: &str) -> String {
    format!(
        "{} {}",
        paint(Role::Muted, "→"),
        paint(Role::Muted, message)
    )
}

fn sanitize_lines(text: &str) -> String {
    text.split('\n')
        .map(sanitize)
        .collect::<Vec<_>>()
        .join("\n")
}

/// The error as rendered: a plain block for the CLI's own failures, and for a
/// backend command's failure `[code] message` with the operation underneath.
pub fn render_error(error: &CliError, debug: bool) -> String {
    let details = if debug {
        error
            .details
            .as_ref()
            .and_then(|details| serde_json::to_string_pretty(details).ok())
    } else {
        None
    };
    let message = sanitize_lines(&error.message);
    match &error.origin {
        Origin::Cli => error_block("错误", Content::Plain(&message), details.as_deref()),
        Origin::Backend(operation) => {
            let mut lines: Vec<String> = message.split('\n').map(bold).collect();
            if let Some(first) = lines.first_mut() {
                let code = paint(Role::Error, &bold(&format!("[{}]", sanitize(&error.code))));
                *first = format!("{code} {first}");
            }
            if let Some(operation) = operation {
                lines.push(paint(Role::Muted, &format!("IPC {operation}")));
            }
            error_block("后端错误", Content::Styled(lines), details.as_deref())
        }
    }
}

/// Render an error once, on stderr.
pub fn report_error(error: &CliError) {
    if error.reported {
        return;
    }
    open_session(false);
    eprintln!("{}", render_error(error, interactive::debug()));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pretty_matches_serde_layout_and_keeps_key_order() {
        let value = serde_json::json!({"a": [1, {"b": null}], "c": "x\"y\\", "d": {}, "e": [], "f": -1.5e3, "g": true});
        assert_eq!(
            pretty(&value.to_string(), false),
            serde_json::to_string_pretty(&value).unwrap()
        );
        assert_eq!(
            pretty(r#"{"z":1,"a":{"m":"中文,:{}","b":[true,false]}}"#, false),
            "{\n  \"z\": 1,\n  \"a\": {\n    \"m\": \"中文,:{}\",\n    \"b\": [\n      true,\n      false\n    ]\n  }\n}"
        );
        assert_eq!(pretty("null", false), "null");
        assert_eq!(pretty("[]", false), "[]");
    }

    #[test]
    fn error_blocks_distinguish_cli_and_backend_failures() {
        let cli = render_error(&CliError::invalid("缺少 --path"), false);
        let lines: Vec<&str> = cli.lines().collect();
        assert_eq!(lines, ["", "┌ 错误", "│ 缺少 --path", "└"]);
        let backend = CliError {
            details: Some(
                serde_json::json!({"code": "notFound", "message": "找不到检材", "path": "D:/x"}),
            ),
            ..super::super::command_error(
                serde_json::json!({"code": "notFound", "message": "找不到检材"}),
            )
            .in_operation("image_info")
        };
        let lines: Vec<String> = render_error(&backend, false)
            .lines()
            .map(str::to_owned)
            .collect();
        assert_eq!(
            lines,
            [
                "",
                "┌ 后端错误",
                "│ [notFound] 找不到检材",
                "│ IPC image_info",
                "└"
            ]
        );
        let debug = render_error(&backend, true);
        assert!(debug.contains("│ ") && debug.contains("\"path\": \"D:/x\""));
    }

    #[test]
    fn sanitize_replaces_control_and_format_characters() {
        assert_eq!(
            sanitize("a\u{1b}[31mb\u{202e}c\u{00ad}d\u{2029}e\u{e0041}f\u{200d}"),
            "a [31mb c d e f "
        );
        assert_eq!(sanitize("检材 E01"), "检材 E01");
    }
}
