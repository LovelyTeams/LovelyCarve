//! Who is on the other end: a person at a terminal, a CI job, or a coding
//! agent.
use std::io::IsTerminal;

const FALSE_LIKE: &[&str] = &["", "0", "false", "no", "off"];

fn enabled(value: &str) -> bool {
    !FALSE_LIKE.contains(&value.trim().to_ascii_lowercase().as_str())
}

/// Running under a CI provider. `CI=false` opts out.
pub fn is_ci() -> bool {
    if std::env::var("CI").is_ok_and(|value| value.eq_ignore_ascii_case("false")) {
        return false;
    }
    [
        "CI",
        "CONTINUOUS_INTEGRATION",
        "BUILD_NUMBER",
        "BUILD_ID",
        "RUN_ID",
        "CI_NAME",
        "GITHUB_ACTIONS",
        "GITLAB_CI",
        "TF_BUILD",
        "JENKINS_URL",
        "TEAMCITY_VERSION",
        "BUILDKITE",
        "CIRCLECI",
        "TRAVIS",
        "APPVEYOR",
        "CODEBUILD_BUILD_ID",
        "BITBUCKET_COMMIT",
        "DRONE",
        "SEMAPHORE",
    ]
    .iter()
    .any(|name| std::env::var_os(name).is_some())
}

/// Prompts, confirmations and decoration need a person: a terminal on both
/// stdin and stdout, outside CI and agent mode.
pub fn is_non_interactive_or_ci() -> bool {
    !(std::io::stdin().is_terminal() && std::io::stdout().is_terminal()) || is_ci() || is_agentic()
}

/// Agent mode and `NO_COLOR` disable color; otherwise honor `FORCE_COLOR`
/// before checking whether stdout is a terminal.
pub fn supports_color() -> bool {
    if std::env::var_os("NO_COLOR").is_some() || is_agentic() {
        return false;
    }
    if let Ok(force) = std::env::var("FORCE_COLOR") {
        return force != "0";
    }
    std::io::stdout().is_terminal()
}

/// `LOVELYCARVE_QUIET=1` suppresses the banner, animated progress and terminal progress.
pub fn quiet_env() -> bool {
    std::env::var("LOVELYCARVE_QUIET").is_ok_and(|value| value == "1")
}

/// Extra diagnostics, such as error details, under `DEBUG`.
pub fn debug() -> bool {
    std::env::var_os("DEBUG").is_some_and(|value| !value.is_empty())
}

/// `LOVELYCARVE_AGENT` explicitly selects machine output, including in harnesses
/// without a recognized environment marker. False-like values disable detection.
/// Otherwise use the environment contracts documented by agent harnesses.
pub fn is_agentic() -> bool {
    agentic_from(|name| std::env::var(name).ok())
}

fn agentic_from(environment: impl Fn(&str) -> Option<String>) -> bool {
    if let Some(value) = environment("LOVELYCARVE_AGENT") {
        return enabled(&value);
    }
    let exact = |name, expected| environment(name).is_some_and(|value| value == expected);
    let set = |name| environment(name).filter(|value| enabled(value));
    exact("OPENCODE", "1")
        || set("QWEN_CODE_SESSION_ID").is_some()
        || exact("PI_CODING_AGENT", "true")
        || exact("AI_AGENT", "pi")
        || exact("CURSOR_AGENT", "1")
        || exact("CLAUDECODE", "1")
        || set("CODEX_THREAD_ID").is_some()
        || set("AMP_CURRENT_THREAD_ID").is_some()
        || exact("GEMINI_CLI", "1")
        || exact("AUGMENT_AGENT", "1")
        || exact("CRUSH", "1")
        || exact("AI_AGENT", "crush")
        || exact("AI_AGENT", "github_copilot_vscode_agent")
        || exact("COPILOT_AGENT", "1")
        || set("OZ_RUN_ID").is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_agent_mode_takes_precedence_over_harness_detection() {
        for value in ["1", "true", " TRUE ", "yes"] {
            assert!(agentic_from(
                |name| (name == "LOVELYCARVE_AGENT").then(|| value.into())
            ));
        }
        for value in ["", "0", "false", " FALSE ", "no", "off"] {
            assert!(!agentic_from(|name| match name {
                "LOVELYCARVE_AGENT" => Some(value.into()),
                "CODEX_THREAD_ID" => Some("test-thread".into()),
                _ => None,
            }));
        }
        assert!(agentic_from(
            |name| (name == "CODEX_THREAD_ID").then(|| "test-thread".into())
        ));
        assert!(!agentic_from(|_| None));
    }
}
