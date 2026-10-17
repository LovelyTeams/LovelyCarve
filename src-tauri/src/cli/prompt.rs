//! Forms and confirmations: a person at a terminal fills in missing values,
//! while scripts, CI and agents must pass them as options (secrets may also
//! arrive on stdin). Prompts are drawn the way cf's clack prompts are: a gray
//! guide line, a cyan `◆` while active, then a green `◇` with the answer, a
//! red `■` when cancelled, or a yellow `▲` with the problem.
use std::io::{IsTerminal, Read, Write};

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use super::{
    help, interactive,
    output::{self, paint, tone, Role, Tone},
    CliError,
};

/// Raw keyboard input with the cursor hidden, restored when dropped.
struct Terminal;

impl Terminal {
    fn enter() -> Result<Self, CliError> {
        #[cfg(windows)]
        if !crossterm::ansi_support::supports_ansi() {
            return Err(CliError::invalid("终端不支持交互输入；请用选项传入参数"));
        }
        crossterm::terminal::enable_raw_mode()?;
        let mut stderr = std::io::stderr();
        write!(stderr, "\x1b[?25l")?;
        stderr.flush()?;
        Ok(Self)
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        let mut stderr = std::io::stderr();
        if let Err(error) = write!(stderr, "\x1b[?25h").and_then(|()| stderr.flush()) {
            tracing::warn!("show cursor: {error}");
        }
        if let Err(error) = crossterm::terminal::disable_raw_mode() {
            tracing::warn!("restore terminal mode: {error}");
        }
    }
}

fn columns() -> usize {
    crossterm::terminal::size()
        .map(|(columns, _)| usize::from(columns))
        .unwrap_or(80)
        .max(10)
}

fn strip_ansi(text: &str) -> String {
    let mut plain = String::with_capacity(text.len());
    let mut characters = text.chars();
    while let Some(character) = characters.next() {
        if character == '\x1b' {
            if characters.next() == Some('[') {
                for code in characters.by_ref() {
                    if ('@'..='~').contains(&code) {
                        break;
                    }
                }
            }
        } else {
            plain.push(character);
        }
    }
    plain
}

/// Redraws a prompt in place: each frame replaces the rows of the last one.
#[derive(Default)]
struct Screen {
    rows: usize,
}

impl Screen {
    fn draw(&mut self, frame: &str) -> Result<(), CliError> {
        let columns = columns();
        let mut stderr = std::io::stderr().lock();
        if self.rows > 1 {
            write!(stderr, "\x1b[{}A", self.rows - 1)?;
        }
        write!(stderr, "\r\x1b[J{}", frame.replace('\n', "\r\n"))?;
        stderr.flush()?;
        self.rows = frame
            .split('\n')
            .map(|line| {
                output::display_width(&strip_ansi(line))
                    .div_ceil(columns)
                    .max(1)
            })
            .sum();
        Ok(())
    }

    /// The final frame, after which output continues on a new line.
    fn finish(&mut self, frame: &str) -> Result<(), CliError> {
        self.draw(frame)?;
        let mut stderr = std::io::stderr().lock();
        write!(stderr, "\r\n")?;
        stderr.flush()?;
        Ok(())
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    Active,
    Error,
    Submit,
    Cancel,
}

impl State {
    fn symbol(self) -> String {
        match self {
            Self::Active => tone(Tone::Cyan, "◆"),
            Self::Error => tone(Tone::Yellow, "▲"),
            Self::Submit => tone(Tone::Green, "◇"),
            Self::Cancel => tone(Tone::Red, "■"),
        }
    }

    fn bar(self) -> String {
        match self {
            Self::Active => tone(Tone::Cyan, "│"),
            Self::Error => tone(Tone::Yellow, "│"),
            Self::Submit | Self::Cancel => tone(Tone::Gray, "│"),
        }
    }

    fn end(self) -> String {
        match self {
            Self::Error => tone(Tone::Yellow, "└"),
            Self::Active | Self::Submit | Self::Cancel => tone(Tone::Cyan, "└"),
        }
    }
}

/// The guide line and the question, wrapped under its symbol.
fn header(message: &str, state: State) -> String {
    let mut lines = vec![tone(Tone::Gray, "│")];
    for (index, line) in help::wrap(message, columns().saturating_sub(3))
        .into_iter()
        .enumerate()
    {
        let prefix = if index == 0 {
            state.symbol()
        } else {
            state.bar()
        };
        lines.push(format!("{prefix}  {line}"));
    }
    lines.join("\n")
}

/// The answer under a finished prompt: dim, or struck through when cancelled.
fn answer(message: &str, state: State, value: &str) -> String {
    let mut frame = header(message, state);
    let shown = match state {
        State::Cancel => tone(Tone::Strike, &tone(Tone::Dim, value)),
        _ => tone(Tone::Dim, value),
    };
    frame.push_str(&format!("\n{}", tone(Tone::Gray, "│")));
    if !value.is_empty() {
        frame.push_str(&format!("  {shown}"));
    }
    if state == State::Cancel && !value.is_empty() {
        frame.push_str(&format!("\n{}", tone(Tone::Gray, "│")));
    }
    frame
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

/// A visible cursor drawn in the input, as clack does while it hides the
/// terminal's own.
fn cursor() -> String {
    if output::styled() {
        "\x1b[7m \x1b[27m".to_owned()
    } else {
        "_".to_owned()
    }
}

/// A single-line text field; `secret` masks the input with `•`.
pub fn text(
    message: &str,
    secret: bool,
    validate: &dyn Fn(&str) -> Option<String>,
) -> Result<String, CliError> {
    let message = output::sanitize(message);
    let _terminal = Terminal::enter()?;
    let mut screen = Screen::default();
    let mut value = String::new();
    let mut problem: Option<String> = None;
    let shown = |value: &str| {
        if secret {
            "•".repeat(value.chars().count())
        } else {
            output::sanitize(value)
        }
    };
    loop {
        let state = if problem.is_some() {
            State::Error
        } else {
            State::Active
        };
        let mut frame = header(&message, state);
        frame.push_str(&format!("\n{}  {}{}", state.bar(), shown(&value), cursor()));
        frame.push_str(&format!("\n{}", state.end()));
        if let Some(problem) = &problem {
            frame.push_str(&format!("  {}", tone(Tone::Yellow, problem)));
        }
        screen.draw(&frame)?;
        let key = next_key()?;
        if cancelled(&key) {
            screen.finish(&answer(&message, State::Cancel, &shown(&value)))?;
            return Err(CliError::cancelled());
        }
        match key.code {
            KeyCode::Enter => match validate(&value) {
                Some(error) => problem = Some(error),
                None => {
                    screen.finish(&answer(&message, State::Submit, &shown(&value)))?;
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
    let choices: Vec<String> = choices
        .iter()
        .map(|choice| output::sanitize(choice))
        .collect();
    if choices.is_empty() {
        return Err(CliError::invalid("没有可选的值"));
    }
    let _terminal = Terminal::enter()?;
    let mut screen = Screen::default();
    let mut cursor = 0usize;
    loop {
        let bar = State::Active.bar();
        let mut frame = header(&message, State::Active);
        for (index, choice) in choices.iter().enumerate() {
            let option = if index == cursor {
                format!("{} {choice}", tone(Tone::Green, "●"))
            } else {
                format!("{} {}", tone(Tone::Dim, "○"), tone(Tone::Dim, choice))
            };
            frame.push_str(&format!("\n{bar}  {option}"));
        }
        frame.push_str(&format!(
            "\n{bar}  {} 切换 • {} 确认\n{}",
            tone(Tone::Dim, "↑/↓"),
            tone(Tone::Dim, "Enter:"),
            State::Active.end()
        ));
        screen.draw(&frame)?;
        let key = next_key()?;
        if cancelled(&key) {
            screen.finish(&answer(&message, State::Cancel, &choices[cursor]))?;
            return Err(CliError::cancelled());
        }
        match key.code {
            KeyCode::Up | KeyCode::Left | KeyCode::Char('k') | KeyCode::Char('h') => {
                cursor = cursor.checked_sub(1).unwrap_or(choices.len() - 1)
            }
            KeyCode::Down
            | KeyCode::Right
            | KeyCode::Tab
            | KeyCode::Char('j')
            | KeyCode::Char('l') => cursor = (cursor + 1) % choices.len(),
            KeyCode::Enter => {
                screen.finish(&answer(&message, State::Submit, &choices[cursor]))?;
                return Ok(choices[cursor].clone());
            }
            _ => {}
        }
    }
}

/// Yes or no, starting at `initial`.
pub fn confirm(message: &str, initial: bool) -> Result<bool, CliError> {
    let message = output::sanitize(message);
    let _terminal = Terminal::enter()?;
    let mut screen = Screen::default();
    let mut accepted = initial;
    let label = |accepted: bool| if accepted { "是" } else { "否" };
    loop {
        let (yes, no) = if accepted {
            (
                format!("{} 是", tone(Tone::Green, "●")),
                format!("{} {}", tone(Tone::Dim, "○"), tone(Tone::Dim, "否")),
            )
        } else {
            (
                format!("{} {}", tone(Tone::Dim, "○"), tone(Tone::Dim, "是")),
                format!("{} 否", tone(Tone::Green, "●")),
            )
        };
        let mut frame = header(&message, State::Active);
        frame.push_str(&format!(
            "\n{}  {yes} {} {no}\n{}",
            State::Active.bar(),
            tone(Tone::Dim, "/"),
            State::Active.end()
        ));
        screen.draw(&frame)?;
        let key = next_key()?;
        if cancelled(&key) {
            screen.finish(&answer(&message, State::Cancel, label(accepted)))?;
            return Err(CliError::cancelled());
        }
        match key.code {
            KeyCode::Left | KeyCode::Right | KeyCode::Up | KeyCode::Down | KeyCode::Tab => {
                accepted = !accepted
            }
            KeyCode::Char('y') | KeyCode::Char('Y') => accepted = true,
            KeyCode::Char('n') | KeyCode::Char('N') => accepted = false,
            KeyCode::Enter => {
                screen.finish(&answer(&message, State::Submit, label(accepted)))?;
                return Ok(accepted);
            }
            _ => {}
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finished_prompts_show_the_answer_like_clack() {
        assert_eq!(
            answer("检材路径", State::Submit, "D:/disk.E01"),
            "│\n◇  检材路径\n│  D:/disk.E01"
        );
        assert_eq!(
            answer("密码", State::Cancel, "•••"),
            "│\n■  密码\n│  •••\n│"
        );
        assert_eq!(answer("密码", State::Cancel, ""), "│\n■  密码\n│");
        assert_eq!(strip_ansi("\x1b[36m◆\x1b[39m  问题"), "◆  问题");
    }
}
