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
/// - `color.ui=always`, added after the `GIT_CONFIG_*` entries of meta's
///   own environment (see [`with_color`])
pub fn git_env() -> HashMap<String, String> {
    let mut env = HashMap::new();

    // Disable pager for programmatic use
    env.insert("GIT_PAGER".to_string(), "cat".to_string());

    // Disable interactive prompts (fail instead of hanging)
    env.insert("GIT_TERMINAL_PROMPT".to_string(), "0".to_string());

    with_color(&mut env, std::env::var("GIT_CONFIG_COUNT").ok().as_deref());
    env
}

/// Add `color.ui=always` to `env` as the configuration entry after the
/// `count` ones the user's own `GIT_CONFIG_COUNT` holds, so theirs still
/// count. A count git can't read gets nothing added: git reports it.
fn with_color(env: &mut HashMap<String, String>, count: Option<&str>) {
    let Some(n) = count.map_or(Some(0), |count| count.parse::<usize>().ok()) else {
        return;
    };
    env.insert("GIT_CONFIG_COUNT".to_string(), (n + 1).to_string());
    env.insert(format!("GIT_CONFIG_KEY_{n}"), "color.ui".to_string());
    env.insert(format!("GIT_CONFIG_VALUE_{n}"), "always".to_string());
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
    fn color_comes_after_the_users_own_entries() {
        let mut env = HashMap::new();
        with_color(&mut env, Some("2"));
        assert_eq!(env["GIT_CONFIG_COUNT"], "3");
        assert_eq!(env["GIT_CONFIG_KEY_2"], "color.ui");
        assert_eq!(env["GIT_CONFIG_VALUE_2"], "always");
        assert!(!env.contains_key("GIT_CONFIG_KEY_0"));

        let mut env = HashMap::new();
        with_color(&mut env, None);
        assert_eq!(env["GIT_CONFIG_COUNT"], "1");
        assert_eq!(env["GIT_CONFIG_KEY_0"], "color.ui");

        let mut env = HashMap::new();
        with_color(&mut env, Some("many"));
        assert!(env.is_empty());
    }
}
