//! Who is on the other end: a person at a terminal or a script. Only
//! presentation depends on it (colour, prompts, progress); what a command
//! returns never does.
use std::io::IsTerminal;

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

/// Prompts and confirmations need a person: a terminal on both stdin and
/// stdout, outside CI.
pub fn is_non_interactive_or_ci() -> bool {
    !(std::io::stdin().is_terminal() && std::io::stdout().is_terminal()) || is_ci()
}

/// `NO_COLOR` disables color; otherwise honor `FORCE_COLOR` before checking
/// whether stdout is a terminal.
pub fn supports_color() -> bool {
    if std::env::var_os("NO_COLOR").is_some() {
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
