//! Git-specific environment variable configuration.
//!
//! This module centralizes all git-specific env var knowledge so that
//! loop_lib can remain a generic execution engine without tool-specific
//! knowledge.

use std::collections::HashMap;

/// Build git-specific environment variables.
///
/// This returns a HashMap suitable for use in `PlannedCommand.env` that
/// configures git for non-interactive, programmatic use.
///
/// Includes:
/// - `GIT_PAGER=cat` - Disable pager to prevent blocking
/// - `GIT_TERMINAL_PROMPT=0` - Fail instead of prompting for credentials
/// - `GIT_CONFIG_*` - Force color output (always set; loop_lib handles TTY detection)
///
/// Note: Color vars are always included because this function may be called
/// from a subprocess (e.g., plugin protocol) where stdout is piped. The actual
/// TTY detection happens at execution time in loop_lib.
pub fn git_env() -> HashMap<String, String> {
    let mut env = HashMap::new();

    // Disable pager for programmatic use
    env.insert("GIT_PAGER".to_string(), "cat".to_string());

    // Disable interactive prompts (fail instead of hanging)
    env.insert("GIT_TERMINAL_PROMPT".to_string(), "0".to_string());

    // Force git colors - always include these since this function may be called
    // from a subprocess context. loop_lib will handle TTY detection for the
    // actual command execution.
    env.insert("GIT_CONFIG_COUNT".to_string(), "1".to_string());
    env.insert("GIT_CONFIG_KEY_0".to_string(), "color.ui".to_string());
    env.insert("GIT_CONFIG_VALUE_0".to_string(), "always".to_string());

    env
}

/// Build git env with optional SSH config overrides.
///
/// Use this when SSH configuration from `.meta.yaml` should be applied.
#[allow(dead_code)] // Future extension point for SSH config support
pub fn git_env_with_ssh(ssh_command: Option<&str>) -> HashMap<String, String> {
    let mut env = git_env();

    if let Some(cmd) = ssh_command {
        env.insert("GIT_SSH_COMMAND".to_string(), cmd.to_string());
    }

    env
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn git_env_includes_pager_setting() {
        let env = git_env();
        assert_eq!(env.get("GIT_PAGER"), Some(&"cat".to_string()));
    }

    #[test]
    fn git_env_includes_terminal_prompt_setting() {
        let env = git_env();
        assert_eq!(env.get("GIT_TERMINAL_PROMPT"), Some(&"0".to_string()));
    }

    #[test]
    fn git_env_with_ssh_includes_ssh_command() {
        let env = git_env_with_ssh(Some("ssh -o StrictHostKeyChecking=no"));
        assert_eq!(
            env.get("GIT_SSH_COMMAND"),
            Some(&"ssh -o StrictHostKeyChecking=no".to_string())
        );
    }

    #[test]
    fn git_env_with_ssh_none_has_no_ssh_command() {
        let env = git_env_with_ssh(None);
        assert!(!env.contains_key("GIT_SSH_COMMAND"));
    }

    #[test]
    fn git_env_with_ssh_preserves_base_env() {
        let env = git_env_with_ssh(Some("ssh"));
        assert_eq!(env.get("GIT_PAGER"), Some(&"cat".to_string()));
        assert_eq!(env.get("GIT_TERMINAL_PROMPT"), Some(&"0".to_string()));
    }
}
