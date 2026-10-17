//! Forms and confirmations: a person at a terminal fills in missing values,
//! while scripts, CI and agents must pass them as options (secrets may also
//! arrive on stdin).
use std::io::{IsTerminal, Read, Write};

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use super::{
    interactive,
    output::{self, paint, Role},
    CliError,
};

struct RawMode;

impl RawMode {
    fn enable() -> Result<Self, CliError> {
        crossterm::terminal::enable_raw_mode()?;
        Ok(Self)
    }
}

impl Drop for RawMode {
    fn drop(&mut self) {
        if let Err(error) = crossterm::terminal::disable_raw_mode() {
            tracing::warn!("restore terminal mode: {error}");
        }
    }
}

fn next_key() -> Result<KeyEvent, CliError> {
    loop {
        if let Event::Key(key) = event::read()? {
            if key.kind != KeyEventKind::Release {
                return Ok(key);
            }
        }
    }
}

fn cancelled(key: &KeyEvent) -> bool {
    key.code == KeyCode::Esc
        || (key.modifiers.contains(KeyModifiers::CONTROL)
            && matches!(key.code, KeyCode::Char('c') | KeyCode::Char('d')))
}

fn finish(message: &str, value: &str, rows: usize) -> Result<(), CliError> {
    let mut stderr = std::io::stderr().lock();
    // Back to the header row, then replace the active prompt with its result.
    write!(stderr, "\r\x1b[{rows}A\x1b[J")?;
    write!(
        stderr,
        "{}  {}\r\n{}  {}\r\n",
        paint(Role::Muted, "◇"),
        message,
        paint(Role::Muted, "│"),
        paint(Role::Muted, value)
    )?;
    stderr.flush()?;
    Ok(())
}

fn abort(message: &str, rows: usize) -> CliError {
    let mut stderr = std::io::stderr().lock();
    let rendered = write!(
        stderr,
        "\r\x1b[{rows}A\x1b[J{}  {}\r\n",
        paint(Role::Error, "■"),
        message
    )
    .and_then(|()| stderr.flush());
    if let Err(error) = rendered {
        tracing::debug!("render cancelled prompt: {error}");
    }
    CliError::cancelled()
}

/// A single-line text field; `secret` masks the input with `•`.
pub fn text(
    message: &str,
    secret: bool,
    validate: &dyn Fn(&str) -> Option<String>,
) -> Result<String, CliError> {
    let message = output::sanitize(message);
    let mut stderr = std::io::stderr();
    write!(
        stderr,
        "{}  {}\n{}  \n{}\x1b[1A\r",
        paint(Role::Brand, "◆"),
        message,
        paint(Role::Brand, "│"),
        paint(Role::Brand, "└")
    )?;
    let _raw = RawMode::enable()?;
    let mut value = String::new();
    let mut problem: Option<String> = None;
    loop {
        let shown = if secret {
            "•".repeat(value.chars().count())
        } else {
            value.clone()
        };
        let footer = problem
            .as_deref()
            .map(|text| paint(Role::Warning, text))
            .unwrap_or_default();
        write!(
            stderr,
            "\r\x1b[2K{}  {shown}\x1b[1B\r\x1b[2K{}  {footer}\x1b[1A\r\x1b[{}C",
            paint(Role::Brand, "│"),
            paint(Role::Brand, "└"),
            3 + output::display_width(&shown)
        )?;
        stderr.flush()?;
        let key = next_key()?;
        if cancelled(&key) {
            return Err(abort(&message, 1));
        }
        match key.code {
            KeyCode::Enter => match validate(&value) {
                Some(error) => problem = Some(error),
                None => {
                    let shown = if secret {
                        "•".repeat(8)
                    } else {
                        value.clone()
                    };
                    finish(&message, &shown, 1)?;
                    return Ok(value);
                }
            },
            KeyCode::Backspace => {
                value.pop();
                problem = None;
            }
            KeyCode::Char(character) => {
                value.push(character);
                problem = None;
            }
            _ => {}
        }
    }
}

/// Pick one of `choices` with the arrow keys.
pub fn select(message: &str, choices: &[String]) -> Result<String, CliError> {
    let message = output::sanitize(message);
    let mut stderr = std::io::stderr();
    let _raw = RawMode::enable()?;
    let mut cursor = 0usize;
    let rows = choices.len() + 1;
    write!(stderr, "{}  {}\r\n", paint(Role::Brand, "◆"), message)?;
    loop {
        for (index, choice) in choices.iter().enumerate() {
            let marker = if index == cursor {
                paint(Role::Success, "●")
            } else {
                paint(Role::Muted, "○")
            };
            write!(
                stderr,
                "\r\x1b[2K{}  {marker} {}\r\n",
                paint(Role::Brand, "│"),
                output::sanitize(choice)
            )?;
        }
        write!(stderr, "\r\x1b[2K{}", paint(Role::Brand, "└"))?;
        stderr.flush()?;
        let key = next_key()?;
        if cancelled(&key) {
            return Err(abort(&message, rows));
        }
        match key.code {
            KeyCode::Up | KeyCode::Left => {
                cursor = cursor.checked_sub(1).unwrap_or(choices.len() - 1)
            }
            KeyCode::Down | KeyCode::Right | KeyCode::Tab => cursor = (cursor + 1) % choices.len(),
            KeyCode::Enter => {
                let choice = choices[cursor].clone();
                finish(&message, &output::sanitize(&choice), rows)?;
                return Ok(choice);
            }
            _ => {}
        }
        write!(stderr, "\r\x1b[{}A", choices.len())?;
    }
}

/// Yes or no, starting at `initial`.
pub fn confirm(message: &str, initial: bool) -> Result<bool, CliError> {
    let message = output::sanitize(message);
    let mut stderr = std::io::stderr();
    let _raw = RawMode::enable()?;
    let mut accepted = initial;
    write!(stderr, "{}  {}\r\n", paint(Role::Brand, "◆"), message)?;
    loop {
        let (yes, no) = if accepted {
            (paint(Role::Success, "● 是"), paint(Role::Muted, "○ 否"))
        } else {
            (paint(Role::Muted, "○ 是"), paint(Role::Success, "● 否"))
        };
        write!(
            stderr,
            "\r\x1b[2K{}  {yes} / {no}\r\n\r\x1b[2K{}",
            paint(Role::Brand, "│"),
            paint(Role::Brand, "└")
        )?;
        stderr.flush()?;
        let key = next_key()?;
        if cancelled(&key) {
            return Err(abort(&message, 2));
        }
        match key.code {
            KeyCode::Left | KeyCode::Right | KeyCode::Up | KeyCode::Down | KeyCode::Tab => {
                accepted = !accepted
            }
            KeyCode::Char('y') | KeyCode::Char('Y') => accepted = true,
            KeyCode::Char('n') | KeyCode::Char('N') => accepted = false,
            KeyCode::Enter => {
                finish(&message, if accepted { "是" } else { "否" }, 2)?;
                return Ok(accepted);
            }
            _ => {}
        }
        write!(stderr, "\r\x1b[1A")?;
    }
}

/// All of stdin with one trailing newline removed, so `echo secret | …`
/// works the obvious way. Never reads a terminal.
pub fn read_piped_stdin() -> Result<String, CliError> {
    let mut stdin = std::io::stdin();
    if stdin.is_terminal() {
        return Ok(String::new());
    }
    let mut text = String::new();
    stdin.read_to_string(&mut text)?;
    if let Some(stripped) = text.strip_suffix('\n') {
        text = stripped.strip_suffix('\r').unwrap_or(stripped).to_owned();
    }
    Ok(text)
}

/// A value the command needs and the caller did not pass.
pub fn required_field(flag: &str, description: &str, secret: bool) -> Result<String, CliError> {
    if interactive::is_non_interactive_or_ci() {
        if secret {
            let piped = read_piped_stdin()?;
            if !piped.is_empty() {
                return Ok(piped);
            }
            return Err(CliError::invalid(format!(
                "缺少 --{flag}。可从标准输入传入、使用 --{flag} <值>，或在终端中交互运行。"
            )));
        }
        return Err(CliError::invalid(format!(
            "缺少 --{flag}。请使用 --{flag} <值>，或在终端中交互运行。"
        )));
    }
    output::open_session(false);
    let message = if description.is_empty() {
        format!("输入 --{flag} 的值")
    } else {
        description.to_owned()
    };
    text(&message, secret, &|value| {
        value.is_empty().then(|| "值不能为空".to_owned())
    })
}

/// One of a fixed set of values the caller did not pass.
pub fn required_choice(
    flag: &str,
    description: &str,
    choices: &[String],
) -> Result<String, CliError> {
    if interactive::is_non_interactive_or_ci() {
        return Err(CliError::invalid(format!(
            "缺少 --{flag}（可选值：{}）。请使用 --{flag} <值>，或在终端中交互运行。",
            choices.join(", ")
        )));
    }
    output::open_session(false);
    let message = if description.is_empty() {
        format!("选择 --{flag} 的值")
    } else {
        description.to_owned()
    };
    select(&message, choices)
}

/// Ask before a destructive command. `--force` is the only way past this in
/// scripts: without a person to ask, it prints the question and `Aborted.`
/// and fails. An interactive decline returns `false`.
pub fn confirm_delete(force: bool, message: Option<&str>) -> Result<bool, CliError> {
    if force {
        return Ok(true);
    }
    let question = match message {
        Some(message) => format!("{message}是否继续？"),
        None => "此操作会永久删除该资源。是否继续？".to_owned(),
    };
    if interactive::is_non_interactive_or_ci() {
        eprint!(
            "{} {question}\n  {}\nAborted.\n",
            paint(Role::Warning, "?"),
            paint(Role::Muted, "（非交互环境：传入 --force 以确认）")
        );
        return Err(CliError::exit(1));
    }
    output::open_session(false);
    confirm(&question, false)
}
