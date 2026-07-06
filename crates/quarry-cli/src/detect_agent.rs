use std::io;
use std::path::Path;

const DEVIN_LOCAL_PATH: &str = "/opt/.devin";

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct DetectedAgent {
    name: String,
}

impl DetectedAgent {
    fn new(name: impl Into<String>) -> Self {
        Self { name: name.into() }
    }

    pub(crate) fn name(&self) -> &str {
        &self.name
    }
}

// Parity target: @vercel/detect-agent@1.2.3.
pub(crate) fn determine_agent() -> Option<DetectedAgent> {
    determine_agent_with(env_value, Path::try_exists)
}

fn determine_agent_with(
    env_value: impl Fn(&str) -> Option<String>,
    path_exists: impl Fn(&Path) -> io::Result<bool>,
) -> Option<DetectedAgent> {
    if let Some(name) = env_value("AI_AGENT").and_then(normalize_ai_agent) {
        return Some(DetectedAgent::new(name));
    }

    if env_is_present(&env_value, "CURSOR_TRACE_ID") {
        return Some(DetectedAgent::new("cursor"));
    }

    if env_is_present(&env_value, "CURSOR_AGENT")
        || env_value("CURSOR_EXTENSION_HOST_ROLE").as_deref() == Some("agent-exec")
    {
        return Some(DetectedAgent::new("cursor-cli"));
    }

    if env_is_present(&env_value, "GEMINI_CLI") {
        return Some(DetectedAgent::new("gemini"));
    }

    if env_is_present(&env_value, "CODEX_SANDBOX")
        || env_is_present(&env_value, "CODEX_CI")
        || env_is_present(&env_value, "CODEX_THREAD_ID")
    {
        return Some(DetectedAgent::new("codex"));
    }

    if env_is_present(&env_value, "ANTIGRAVITY_AGENT") {
        return Some(DetectedAgent::new("antigravity"));
    }

    if env_is_present(&env_value, "AUGMENT_AGENT") {
        return Some(DetectedAgent::new("augment-cli"));
    }

    if env_is_present(&env_value, "OPENCODE_CLIENT") {
        return Some(DetectedAgent::new("opencode"));
    }

    if env_is_present(&env_value, "CLAUDECODE") || env_is_present(&env_value, "CLAUDE_CODE") {
        let name = if env_is_present(&env_value, "CLAUDE_CODE_IS_COWORK") {
            "cowork"
        } else {
            "claude"
        };
        return Some(DetectedAgent::new(name));
    }

    if env_is_present(&env_value, "REPL_ID") {
        return Some(DetectedAgent::new("replit"));
    }

    if env_is_present(&env_value, "COPILOT_MODEL")
        || env_is_present(&env_value, "COPILOT_ALLOW_ALL")
        || env_is_present(&env_value, "COPILOT_GITHUB_TOKEN")
    {
        return Some(DetectedAgent::new("github-copilot"));
    }

    if path_exists(Path::new(DEVIN_LOCAL_PATH)).unwrap_or(false) {
        return Some(DetectedAgent::new("devin"));
    }

    None
}

fn env_value(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|value| !value.is_empty())
}

fn env_is_present(env_value: impl Fn(&str) -> Option<String>, name: &str) -> bool {
    env_value(name).is_some_and(|value| !value.is_empty())
}

fn normalize_ai_agent(name: String) -> Option<String> {
    let name = name.trim();
    if name.is_empty() {
        return None;
    }

    if matches!(name, "github-copilot" | "github-copilot-cli") {
        return Some("github-copilot".to_string());
    }

    Some(name.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    type Env<'a> = &'a [(&'a str, &'a str)];

    #[test]
    fn detects_ai_agent_first_and_preserves_custom_name() {
        let agent = detect(&[
            ("AI_AGENT", "my-custom-agent@v1.0"),
            ("CURSOR_TRACE_ID", "trace"),
        ]);

        assert_agent_name(agent, "my-custom-agent@v1.0");
    }

    #[test]
    fn trims_ai_agent_name() {
        let agent = detect(&[("AI_AGENT", "  v0  ")]);

        assert_agent_name(agent, "v0");
    }

    #[test]
    fn normalizes_github_copilot_ai_agent_names() {
        assert_agent_name(detect(&[("AI_AGENT", "github-copilot")]), "github-copilot");
        assert_agent_name(
            detect(&[("AI_AGENT", "github-copilot-cli")]),
            "github-copilot",
        );
    }

    #[test]
    fn treats_empty_and_whitespace_ai_agent_as_absent() {
        assert_agent_name(
            detect(&[("AI_AGENT", ""), ("CURSOR_TRACE_ID", "trace")]),
            "cursor",
        );
        assert_agent_name(
            detect(&[("AI_AGENT", "   "), ("CURSOR_TRACE_ID", "trace")]),
            "cursor",
        );
    }

    #[test]
    fn detects_cursor_trace_before_cursor_cli_markers() {
        let agent = detect(&[("CURSOR_TRACE_ID", "trace"), ("CURSOR_AGENT", "1")]);

        assert_agent_name(agent, "cursor");
    }

    #[test]
    fn detects_cursor_cli_from_agent_marker() {
        assert_agent_name(detect(&[("CURSOR_AGENT", "1")]), "cursor-cli");
    }

    #[test]
    fn detects_cursor_cli_from_extension_host_role() {
        assert_agent_name(
            detect(&[("CURSOR_EXTENSION_HOST_ROLE", "agent-exec")]),
            "cursor-cli",
        );
    }

    #[test]
    fn ignores_other_cursor_extension_host_roles() {
        let agent = detect(&[("CURSOR_EXTENSION_HOST_ROLE", "worker")]);

        assert_eq!(agent, None);
    }

    #[test]
    fn detects_gemini_codex_antigravity_augment_and_opencode() {
        assert_agent_name(detect(&[("GEMINI_CLI", "1")]), "gemini");
        assert_agent_name(detect(&[("CODEX_SANDBOX", "1")]), "codex");
        assert_agent_name(detect(&[("CODEX_CI", "1")]), "codex");
        assert_agent_name(detect(&[("CODEX_THREAD_ID", "thread")]), "codex");
        assert_agent_name(detect(&[("ANTIGRAVITY_AGENT", "1")]), "antigravity");
        assert_agent_name(detect(&[("AUGMENT_AGENT", "1")]), "augment-cli");
        assert_agent_name(detect(&[("OPENCODE_CLIENT", "1")]), "opencode");
    }

    #[test]
    fn detects_claude_and_claude_code() {
        assert_agent_name(detect(&[("CLAUDECODE", "1")]), "claude");
        assert_agent_name(detect(&[("CLAUDE_CODE", "1")]), "claude");
    }

    #[test]
    fn detects_cowork_only_with_claude_marker() {
        assert_agent_name(
            detect(&[("CLAUDECODE", "1"), ("CLAUDE_CODE_IS_COWORK", "1")]),
            "cowork",
        );
        assert_agent_name(
            detect(&[("CLAUDE_CODE", "1"), ("CLAUDE_CODE_IS_COWORK", "1")]),
            "cowork",
        );
        assert_eq!(detect(&[("CLAUDE_CODE_IS_COWORK", "1")]), None);
    }

    #[test]
    fn detects_replit_and_copilot_fallback_markers() {
        assert_agent_name(detect(&[("REPL_ID", "repl")]), "replit");
        assert_agent_name(detect(&[("COPILOT_MODEL", "gpt")]), "github-copilot");
        assert_agent_name(detect(&[("COPILOT_ALLOW_ALL", "1")]), "github-copilot");
        assert_agent_name(
            detect(&[("COPILOT_GITHUB_TOKEN", "token")]),
            "github-copilot",
        );
    }

    #[test]
    fn treats_empty_env_values_as_absent() {
        assert_eq!(detect(&[("GEMINI_CLI", "")]), None);
        assert_agent_name(detect(&[("GEMINI_CLI", ""), ("CODEX_CI", "1")]), "codex");
    }

    #[test]
    fn preserves_upstream_priority_order() {
        let agent = detect(&[
            ("CURSOR_AGENT", "1"),
            ("GEMINI_CLI", "1"),
            ("CODEX_CI", "1"),
            ("ANTIGRAVITY_AGENT", "1"),
            ("AUGMENT_AGENT", "1"),
            ("OPENCODE_CLIENT", "1"),
            ("CLAUDECODE", "1"),
            ("REPL_ID", "repl"),
            ("COPILOT_MODEL", "gpt"),
        ]);

        assert_agent_name(agent, "cursor-cli");
    }

    #[test]
    fn detects_devin_when_path_exists() {
        let agent = determine_agent_with(test_env(&[]), |path| {
            assert_eq!(path, Path::new(DEVIN_LOCAL_PATH));
            Ok(true)
        });

        assert_agent_name(agent, "devin");
    }

    #[test]
    fn ignores_devin_when_path_is_absent_or_errors() {
        let absent = determine_agent_with(test_env(&[]), |_path| Ok(false));
        let error = determine_agent_with(test_env(&[]), |_path| {
            Err(io::Error::new(io::ErrorKind::PermissionDenied, "denied"))
        });

        assert_eq!(absent, None);
        assert_eq!(error, None);
    }

    #[test]
    fn env_markers_take_priority_over_devin_path() {
        let agent = determine_agent_with(test_env(&[("COPILOT_MODEL", "gpt")]), |_path| Ok(true));

        assert_agent_name(agent, "github-copilot");
    }

    fn detect(env: Env<'_>) -> Option<DetectedAgent> {
        determine_agent_with(test_env(env), |_path| Ok(false))
    }

    fn test_env(env: Env<'_>) -> impl Fn(&str) -> Option<String> + '_ {
        |name| {
            env.iter()
                .find_map(|(key, value)| (*key == name).then(|| (*value).to_string()))
                .filter(|value| !value.is_empty())
        }
    }

    fn assert_agent_name(agent: Option<DetectedAgent>, name: &str) {
        match agent {
            Some(agent) => assert_eq!(agent.name(), name),
            None => panic!("agent should be detected"),
        }
    }
}
