//! `complete`: shell completion from the command catalogue. `complete <shell>`
//! prints the bootstrap script for an rc file; at Tab the shell runs
//! `complete -- <words…>` and reads one `value<TAB>description` candidate per
//! line.
use super::catalog::{Catalog, Kind};

/// Global options valid on every command.
pub const GLOBAL_OPTIONS: &[(&str, Option<char>, &str, bool)] = &[
    (
        "timezone",
        None,
        "文件时间显示时区：auto、evidence、case、UTC、IANA 名称或 ±HH:MM",
        true,
    ),
    (
        "events",
        None,
        "把命令生命周期与后端进度以 JSONL 写到标准错误",
        false,
    ),
    (
        "session",
        None,
        "使用命名常驻会话（也可设置 LOVELYCARVE_SESSION）",
        true,
    ),
    (
        "image",
        Some('i'),
        "检材 ID 或路径（覆盖 LOVELYCARVE_IMAGE）",
        true,
    ),
    (
        "case",
        None,
        "先打开的案件目录（覆盖 LOVELYCARVE_CASE）",
        true,
    ),
    ("quiet", Some('q'), "不输出非必要信息", false),
    ("help", Some('h'), "显示帮助", false),
    ("version", Some('v'), "显示版本", false),
];

pub fn script(binary: &str, shell: &str) -> Option<String> {
    let function = binary.replace('-', "_");
    let script = match shell {
        "bash" => format!(
            r#"# {binary} bash completion: {binary} complete bash >> ~/.bashrc
_{function}() {{
    local IFS=$'\n'
    local lines line
    lines=$({binary} complete -- "${{COMP_WORDS[@]:1:COMP_CWORD}}" 2>/dev/null) || return
    COMPREPLY=()
    for line in $lines; do
        COMPREPLY+=("${{line%%$'\t'*}}")
    done
}}
complete -o default -F _{function} {binary}
"#
        ),
        "zsh" => format!(
            r#"#compdef {binary}
# {binary} zsh completion: {binary} complete zsh >> ~/.zshrc
_{function}() {{
    local -a candidates
    local line
    for line in ${{(f)"$({binary} complete -- "${{(@)words[2,CURRENT]}}" 2>/dev/null)"}}; do
        candidates+=("${{${{line%%$'\t'*}}//:/\\:}}:${{line#*$'\t'}}")
    done
    _describe '{binary}' candidates
}}
compdef _{function} {binary}
"#
        ),
        "fish" => format!(
            r#"# {binary} fish completion: {binary} complete fish > ~/.config/fish/completions/{binary}.fish
function __{function}_complete
    set -l tokens (commandline -opc) (commandline -ct)
    {binary} complete -- $tokens[2..-1] 2>/dev/null
end
complete -c {binary} -f -a '(__{function}_complete)'
"#
        ),
        "powershell" => format!(
            r#"# {binary} PowerShell completion: {binary} complete powershell >> $PROFILE
Register-ArgumentCompleter -Native -CommandName '{binary}', '{binary}.exe' -ScriptBlock {{
    param($wordToComplete, $commandAst, $cursorPosition)
    $words = @($commandAst.CommandElements | Select-Object -Skip 1 | ForEach-Object {{ $_.ToString() }})
    if ($wordToComplete -eq '') {{ $words += '' }}
    & {binary} complete -- @words 2>$null | ForEach-Object {{
        $value, $description = $_ -split "`t", 2
        if (-not $description) {{ $description = $value }}
        [System.Management.Automation.CompletionResult]::new($value, $value, 'ParameterValue', $description)
    }}
}}
"#
        ),
        _ => return None,
    };
    Some(script)
}

fn takes_value(catalog: &Catalog, path: &[&str], option: &str) -> bool {
    let name = option.trim_start_matches('-');
    if let Some((_, _, _, value)) = GLOBAL_OPTIONS.iter().find(|(long, short, _, _)| {
        *long == name || short.is_some_and(|short| name == short.to_string())
    }) {
        return *value;
    }
    catalog.find(path).is_some_and(|command| {
        command.params.iter().any(|param| {
            (param.option == name || param.short.is_some_and(|short| name == short.to_string()))
                && param.kind != Kind::Boolean
        }) || matches!(name, "body" | "fields")
    })
}

/// Candidates for the last of `words`, the word being completed.
pub fn candidates(catalog: &Catalog, words: &[String]) -> Vec<(String, String)> {
    let Some((current, before)) = words.split_last() else {
        return Vec::new();
    };
    let mut path: Vec<&str> = Vec::new();
    let mut expecting: Option<&str> = None;
    let mut positionals = 0usize;
    for word in before {
        // The value of the option before it.
        if expecting.take().is_some() {
            continue;
        }
        if word.starts_with('-') {
            if !word.contains('=') && takes_value(catalog, &path, word) {
                expecting = Some(word);
            }
            continue;
        }
        let mut next = path.clone();
        next.push(word);
        let at_leaf = catalog.find(&path).is_some();
        let listed = !catalog.is_hidden(&next);
        if listed && (catalog.is_group(&next) || (!at_leaf && catalog.find(&next).is_some())) {
            path = next;
        } else {
            positionals += 1;
        }
    }
    let mut found: Vec<(String, String)> = Vec::new();
    if let Some(option) = expecting {
        let name = option.trim_start_matches('-');
        if let Some(command) = catalog.find(&path) {
            if let Some(values) = command
                .params
                .iter()
                .find(|param| param.option == name)
                .and_then(|param| param.values.clone())
            {
                found.extend(values.into_iter().map(|value| (value.clone(), value)));
            }
        }
    } else if let Some(command) = catalog.find(&path) {
        if current.starts_with('-') {
            for param in command
                .params
                .iter()
                .filter(|param| param.visible() && param.position.is_none() && !param.scope)
            {
                found.push((format!("--{}", param.option), param.description.clone()));
            }
            for option in &command.options {
                let Some(name) = option["name"].as_str() else {
                    continue;
                };
                let description = option["description"].as_str().unwrap_or_default();
                let flag = format!("--{name}");
                if !found.iter().any(|(existing, _)| existing == &flag) {
                    found.push((flag, description.to_owned()));
                }
                if let Some(alias) = option["alias"].as_str() {
                    found.push((format!("-{alias}"), description.to_owned()));
                }
            }
        } else if let Some(values) = command
            .positionals()
            .get(positionals)
            .and_then(|param| param.values.clone())
        {
            found.extend(values.into_iter().map(|value| (value.clone(), value)));
        }
    } else if !current.starts_with('-') {
        found.extend(catalog.children(&path));
    }
    if current.starts_with('-') && expecting.is_none() {
        for (long, short, description, _) in GLOBAL_OPTIONS {
            found.push((format!("--{long}"), (*description).to_owned()));
            if let Some(short) = short {
                found.push((format!("-{short}"), format!("--{long} 的简写")));
            }
        }
    }
    found.retain(|(value, _)| value.starts_with(current.as_str()));
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::catalog::catalog;

    fn complete(words: &[&str]) -> Vec<String> {
        let words: Vec<String> = words.iter().map(|word| (*word).to_owned()).collect();
        candidates(catalog().unwrap(), &words)
            .into_iter()
            .map(|(value, _)| value)
            .collect()
    }

    #[test]
    fn completes_groups_commands_options_and_values() {
        assert!(complete(&["ima"]).contains(&"images".to_owned()));
        assert!(complete(&["images", ""]).contains(&"open".to_owned()));
        assert!(complete(&["images", "export", "--"]).contains(&"--destination".to_owned()));
        assert!(complete(&["images", "export", "--"]).contains(&"--image".to_owned()));
        assert_eq!(
            complete(&["images", "export", "--mode", ""]),
            ["source", "unlockedPartitions"]
        );
        assert_eq!(complete(&["complete", "p"]), ["powershell"]);
        assert!(complete(&["session", "-"]).contains(&"-j".to_owned()));
        assert!(complete(&["cases", "delete", "x", "-"]).contains(&"-f".to_owned()));
        assert!(complete(&["files", "list", "-"]).contains(&"-q".to_owned()));
        assert!(
            !complete(&["t"]).contains(&"tools".to_owned()),
            "hidden commands are not offered"
        );
        assert!(complete(&["docs", ""]).contains(&"search".to_owned()));
        assert!(script("lovelycarve-cli", "bash")
            .unwrap()
            .contains("complete -o default -F _lovelycarve_cli lovelycarve-cli"));
        assert!(script("lovelycarve-cli", "cmd").is_none());
    }
}
