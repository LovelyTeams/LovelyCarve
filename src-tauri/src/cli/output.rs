//! What reaches the terminal: results on stdout, everything else on stderr,
//! with the colour roles, the once-per-process banner and the framed error
//! block.
use std::{
    io::{IsTerminal, Write},
    sync::atomic::{AtomicBool, Ordering},
};

use serde_json::Value;

use super::{interactive, CliError, Output};

/// Larger JSON is printed without highlighting.
const MAX_HIGHLIGHTED_JSON: usize = 50_000;

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
    text.chars()
        .map(|character| if (character as u32) >= 0x1100 { 2 } else { 1 })
        .sum()
}

/// Replace characters that could alter or control terminal display.
pub fn sanitize(text: &str) -> String {
    text.chars()
        .map(|character| {
            if character.is_control()
                || matches!(character, '\u{200b}'..='\u{200f}' | '\u{2028}'..='\u{202e}' | '\u{2060}'..='\u{206f}' | '\u{feff}')
            {
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
    let width = "lovelycarve-cli · ".chars().count() + version.chars().count();
    format!("{text}\n{}", paint(Role::Brand, &"─".repeat(width)))
}

fn write_json(value: &Value, indent: usize, out: &mut String) {
    let pad = "  ".repeat(indent);
    let inner = "  ".repeat(indent + 1);
    match value {
        Value::Null => out.push_str(&paint(Role::Muted, "null")),
        Value::Bool(flag) => out.push_str(&paint(Role::JsonBoolean, &flag.to_string())),
        Value::Number(number) => out.push_str(&paint(Role::JsonNumber, &number.to_string())),
        Value::String(text) => out.push_str(&paint(
            Role::JsonString,
            &serde_json::to_string(text).unwrap_or_default(),
        )),
        Value::Array(items) if items.is_empty() => out.push_str("[]"),
        Value::Array(items) => {
            out.push_str("[\n");
            for (index, item) in items.iter().enumerate() {
                out.push_str(&inner);
                write_json(item, indent + 1, out);
                out.push_str(if index + 1 < items.len() { ",\n" } else { "\n" });
            }
            out.push_str(&pad);
            out.push(']');
        }
        Value::Object(map) if map.is_empty() => out.push_str("{}"),
        Value::Object(map) => {
            out.push_str("{\n");
            for (index, (key, item)) in map.iter().enumerate() {
                out.push_str(&inner);
                out.push_str(&paint(
                    Role::JsonKey,
                    &serde_json::to_string(key).unwrap_or_default(),
                ));
                out.push_str(": ");
                write_json(item, indent + 1, out);
                out.push_str(if index + 1 < map.len() { ",\n" } else { "\n" });
            }
            out.push_str(&pad);
            out.push('}');
        }
    }
}

/// Indented JSON, highlighted only for a person at a color terminal.
pub fn format_json(value: &Value) -> Result<String, CliError> {
    let plain = serde_json::to_string_pretty(value)?;
    if interactive::is_non_interactive_or_ci() || !styled() || plain.len() > MAX_HIGHLIGHTED_JSON {
        return Ok(plain);
    }
    let mut highlighted = String::with_capacity(plain.len() * 2);
    write_json(value, 0, &mut highlighted);
    Ok(highlighted)
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
        Output::Json(Value::Null) => {
            if !quiet && std::io::stderr().is_terminal() {
                eprintln!("{} {success_label}", paint(Role::Success, "✓"));
            }
        }
        Output::Json(value) => {
            let mut stdout = std::io::stdout().lock();
            writeln!(stdout, "{}", format_json(&value)?)?;
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

fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    for paragraph in text.split('\n') {
        if paragraph.chars().count() <= width {
            lines.push(paragraph.to_owned());
            continue;
        }
        let mut line = String::new();
        for character in paragraph.chars() {
            if line.chars().count() >= width {
                lines.push(std::mem::take(&mut line));
            }
            line.push(character);
        }
        if !line.is_empty() {
            lines.push(line);
        }
    }
    lines
}

/// The framed error block: a blank line, `┌ Title`, the message, optional
/// muted details, `└`.
pub fn error_block(title: &str, message: &str, details: Option<&str>) -> String {
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
    for line in wrap(message, width) {
        lines.push(if line.is_empty() {
            bar.clone()
        } else {
            format!("{bar} {}", paint(Role::Error, &bold(&line)))
        });
    }
    if let Some(details) = details {
        lines.push(bar.clone());
        for line in wrap(details, width) {
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

/// Render an error once, on stderr.
pub fn report_error(error: &CliError) {
    if error.reported {
        return;
    }
    open_session(false);
    let message = if error.code.is_empty() || matches!(error.code.as_str(), "usage" | "io") {
        sanitize_lines(&error.message)
    } else {
        format!("[{}] {}", error.code, sanitize_lines(&error.message))
    };
    let details = if interactive::debug() {
        error
            .details
            .as_ref()
            .and_then(|details| serde_json::to_string_pretty(details).ok())
    } else {
        None
    };
    eprintln!("{}", error_block("错误", &message, details.as_deref()));
}

fn sanitize_lines(text: &str) -> String {
    text.split('\n')
        .map(sanitize)
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn highlighted_json_keeps_the_pretty_layout() {
        let value = serde_json::json!({"a": [1, {"b": null}], "c": "x\"y", "d": {}, "e": []});
        let mut highlighted = String::new();
        write_json(&value, 0, &mut highlighted);
        let stripped = highlighted
            .split('\x1b')
            .enumerate()
            .map(|(index, part)| {
                if index == 0 {
                    part
                } else {
                    part.split_once('m').map_or("", |(_, rest)| rest)
                }
            })
            .collect::<String>();
        assert_eq!(stripped, serde_json::to_string_pretty(&value).unwrap());
    }

    #[test]
    fn error_block_frames_the_message() {
        let block = error_block("错误", "first\nsecond", Some("detail"));
        let lines: Vec<&str> = block.lines().collect();
        assert_eq!(lines.len(), 7);
        assert!(lines[1].contains("┌"));
        assert!(lines[6].contains("└"));
    }
}
