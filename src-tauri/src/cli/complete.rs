//! Shell completion from the clap command tree. `completion <shell>` prints
//! the script for an rc file; at Tab the shell runs `__complete <words…>` and
//! reads one `value<TAB>description` candidate per line.
use clap::{Arg, CommandFactory};

use super::cmd::{system::Shell, Cli};

const BINARY: &str = "lovelycarve-cli";

pub fn script(shell: Shell) -> String {
    let function = BINARY.replace('-', "_");
    match shell {
        Shell::Bash => format!(
            r#"# {BINARY} bash completion: {BINARY} completion bash >> ~/.bashrc
_{function}() {{
    local IFS=$'\n'
    local lines line
    lines=$({BINARY} __complete "${{COMP_WORDS[@]:1:COMP_CWORD}}" 2>/dev/null) || return
    COMPREPLY=()
    for line in $lines; do
        COMPREPLY+=("${{line%%$'\t'*}}")
    done
}}
complete -o default -F _{function} {BINARY}
"#
        ),
        Shell::Zsh => format!(
            r#"#compdef {BINARY}
# {BINARY} zsh completion: {BINARY} completion zsh >> ~/.zshrc
_{function}() {{
    local -a candidates
    local line
    for line in ${{(f)"$({BINARY} __complete "${{(@)words[2,CURRENT]}}" 2>/dev/null)"}}; do
        candidates+=("${{${{line%%$'\t'*}}//:/\\:}}:${{line#*$'\t'}}")
    done
    _describe '{BINARY}' candidates
}}
compdef _{function} {BINARY}
"#
        ),
        Shell::Fish => format!(
            r#"# {BINARY} fish completion: {BINARY} completion fish > ~/.config/fish/completions/{BINARY}.fish
function __{function}_complete
    set -l tokens (commandline -opc) (commandline -ct)
    {BINARY} __complete $tokens[2..-1] 2>/dev/null
end
complete -c {BINARY} -f -a '(__{function}_complete)'
"#
        ),
        Shell::Powershell => format!(
            r#"# {BINARY} PowerShell completion: {BINARY} completion powershell >> $PROFILE
Register-ArgumentCompleter -Native -CommandName '{BINARY}', '{BINARY}.exe' -ScriptBlock {{
    param($wordToComplete, $commandAst, $cursorPosition)
    $words = @($commandAst.CommandElements | Select-Object -Skip 1 | ForEach-Object {{ $_.ToString() }})
    if ($wordToComplete -eq '') {{ $words += '' }}
    & {BINARY} __complete @words 2>$null | ForEach-Object {{
        $value, $description = $_ -split "`t", 2
        if (-not $description) {{ $description = $value }}
        [System.Management.Automation.CompletionResult]::new($value, $value, 'ParameterValue', $description)
    }}
}}
"#
        ),
    }
}

fn help(arg: &Arg) -> String {
    arg.get_help().map(ToString::to_string).unwrap_or_default()
}

fn takes_value(arg: &Arg) -> bool {
    arg.get_action().takes_values()
        && arg
            .get_num_args()
            .is_none_or(|range| range.min_values() > 0)
        && !arg.is_require_equals_set()
}

/// The option `word` names among `options`.
fn option<'a>(options: &[&'a Arg], word: &str) -> Option<&'a Arg> {
    options.iter().copied().find(|arg| {
        word.strip_prefix("--")
            .map(|long| arg.get_long() == Some(long))
            .unwrap_or_else(|| {
                word.strip_prefix('-')
                    .and_then(|short| short.chars().next())
                    .is_some_and(|short| arg.get_short() == Some(short) && word.len() == 2)
            })
    })
}

/// Candidates for the last of `words`, the word being completed.
pub fn candidates(words: &[String]) -> Vec<(String, String)> {
    let Some((current, before)) = words.split_last() else {
        return Vec::new();
    };
    let root = Cli::command();
    let globals: Vec<&Arg> = root
        .get_arguments()
        .filter(|arg| arg.is_global_set())
        .collect();
    let mut command = &root;
    let mut expecting: Option<&Arg> = None;
    let mut positionals = 0usize;
    for word in before {
        if expecting.take().is_some() {
            continue;
        }
        let options: Vec<&Arg> = command
            .get_arguments()
            .chain(globals.iter().copied())
            .collect();
        if word.starts_with('-') {
            if !word.contains('=') {
                expecting = option(&options, word).filter(|arg| takes_value(arg));
            }
            continue;
        }
        match command
            .find_subcommand(word)
            .filter(|child| !child.is_hide_set())
        {
            Some(child) if positionals == 0 => command = child,
            _ => positionals += 1,
        }
    }
    let mut found: Vec<(String, String)> = Vec::new();
    let options: Vec<&Arg> = command
        .get_arguments()
        .chain(globals.iter().copied())
        .filter(|arg| !arg.is_hide_set())
        .collect();
    if let Some(arg) = expecting {
        found.extend(
            arg.get_possible_values()
                .iter()
                .map(|value| (value.get_name().to_owned(), help(arg))),
        );
    } else if current.starts_with('-') {
        let mut seen = std::collections::HashSet::new();
        for arg in options.iter().filter(|arg| !arg.is_positional()) {
            if let Some(long) = arg.get_long() {
                if seen.insert(long.to_owned()) {
                    found.push((format!("--{long}"), help(arg)));
                }
            }
            if let Some(short) = arg.get_short() {
                found.push((format!("-{short}"), help(arg)));
            }
        }
    } else {
        if positionals == 0 {
            found.extend(
                command
                    .get_subcommands()
                    .filter(|child| !child.is_hide_set())
                    .map(|child| {
                        (
                            child.get_name().to_owned(),
                            child
                                .get_about()
                                .map(ToString::to_string)
                                .unwrap_or_default(),
                        )
                    }),
            );
        }
        if let Some(arg) = command.get_positionals().nth(positionals) {
            found.extend(
                arg.get_possible_values()
                    .iter()
                    .map(|value| (value.get_name().to_owned(), help(arg))),
            );
        }
    }
    found.retain(|(value, _)| value.starts_with(current.as_str()));
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    fn complete(words: &[&str]) -> Vec<String> {
        let words: Vec<String> = words.iter().map(|word| (*word).to_owned()).collect();
        candidates(&words)
            .into_iter()
            .map(|(value, _)| value)
            .collect()
    }

    #[test]
    fn completes_commands_options_and_values() {
        assert!(complete(&["evi"]).contains(&"evidence".to_owned()));
        assert!(complete(&["evidence", ""]).contains(&"add".to_owned()));
        assert!(complete(&["sqlite", ""]).contains(&"query".to_owned()));
        assert!(complete(&["ls", "--"]).contains(&"--limit".to_owned()));
        assert!(complete(&["ls", "--"]).contains(&"--json".to_owned()));
        assert_eq!(complete(&["ls", "--sort", "mod"]), ["modified"]);
        assert_eq!(complete(&["completion", "p"]), ["powershell"]);
        assert!(complete(&["batch", "-"]).contains(&"-j".to_owned()));
        assert!(
            !complete(&["case", ""]).contains(&"serve".to_owned()),
            "hidden commands are not offered"
        );
        assert!(!complete(&[""]).contains(&"__complete".to_owned()));
        assert!(
            script(Shell::Bash).contains("complete -o default -F _lovelycarve_cli lovelycarve-cli")
        );
    }
}
