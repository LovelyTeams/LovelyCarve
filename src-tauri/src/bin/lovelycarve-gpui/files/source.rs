//! Name-based source type hints shared by file icons and the code preview.

#[derive(Clone, Copy)]
pub struct SourceLanguage {
    pub grammar: &'static str,
    pub label: &'static str,
}

pub fn source_language(name: &str, suffix: &str) -> Option<SourceLanguage> {
    let stem = name.trim_start_matches('.');
    let (grammar, label) = if ["bashrc", "bash_profile", "profile", "zshrc"]
        .iter()
        .any(|alias| stem.eq_ignore_ascii_case(alias))
    {
        ("bash", "Shell")
    } else if [
        "Makefile",
        "CMakeLists.txt",
        "Dockerfile",
        "Containerfile",
        "Justfile",
        "gitignore",
        "gitattributes",
        "editorconfig",
        "env",
    ]
    .iter()
    .any(|alias| stem.eq_ignore_ascii_case(alias))
    {
        ("plain", "纯文本")
    } else {
        match suffix {
            "rs" => ("rust", "Rust"),
            "py" | "pyi" => ("python", "Python"),
            "js" | "mjs" | "cjs" | "jsx" => ("javascript", "JavaScript"),
            "ts" | "mts" | "cts" => ("typescript", "TypeScript"),
            "tsx" => ("tsx", "TSX"),
            "c" | "h" => ("c", "C"),
            "cc" | "cpp" | "cxx" | "hpp" | "hxx" => ("cpp", "C++"),
            "cs" => ("csharp", "C#"),
            "go" => ("go", "Go"),
            "java" => ("java", "Java"),
            "lua" => ("lua", "Lua"),
            "sh" | "bash" | "zsh" => ("bash", "Shell"),
            "html" | "htm" => ("html", "HTML"),
            "css" => ("css", "CSS"),
            "json" | "jsonc" => ("json", "JSON"),
            "toml" => ("toml", "TOML"),
            "yaml" | "yml" => ("yaml", "YAML"),
            "conf" | "cfg" | "ini" | "env" | "properties" | "json5" | "xml" | "plist" | "vue"
            | "svelte" | "rb" | "php" | "pl" | "r" | "kt" | "kts" | "swift" | "m" | "mm"
            | "scala" | "clj" | "ex" | "exs" | "erl" | "hs" | "dart" | "fish" | "ps1" | "psm1"
            | "bat" | "cmd" | "sql" | "graphql" | "gql" | "proto" | "scss" | "sass" | "less"
            | "styl" | "dockerfile" | "makefile" | "cmake" | "gradle" | "gitignore"
            | "gitattributes" | "editorconfig" => ("plain", "纯文本"),
            _ => return None,
        }
    };
    Some(SourceLanguage { grammar, label })
}
