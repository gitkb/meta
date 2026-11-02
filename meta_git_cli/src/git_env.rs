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
/// - `color.ui=always` when meta's output is coloured
///   ([`meta_core::out::color`]), added after the `GIT_CONFIG_*` entries of
///   meta's own environment (see [`with_color`])
pub fn git_env() -> HashMap<String, String> {
    let mut env = HashMap::new();

    // Disable pager for programmatic use
    env.insert("GIT_PAGER".to_string(), "cat".to_string());

    // Disable interactive prompts (fail instead of hanging)
    env.insert("GIT_TERMINAL_PROMPT".to_string(), "0".to_string());

    if meta_core::out::color() {
        with_color(&mut env, |name| std::env::var(name).ok());
    }
    env
}

/// Add `color.ui=always` to `env` as the configuration entry after the
/// ones the user's own `GIT_CONFIG_COUNT` holds (read with `var`), so
/// theirs still count. Nothing is added when their entries set `color.ui`
/// themselves, or when git can't read their count (git reports it).
fn with_color(env: &mut HashMap<String, String>, var: impl Fn(&str) -> Option<String>) {
    let count = var("GIT_CONFIG_COUNT").unwrap_or_default();
    // git reads an empty count as none
    let n = match count.as_str() {
        "" => 0,
        count => match count.parse::<u32>() {
            Ok(n) => n,
            Err(_) => return,
        },
    };
    let Some(next) = n.checked_add(1) else {
        return;
    };
    for i in 0..n {
        match var(&format!("GIT_CONFIG_KEY_{i}")) {
            // their own colour setting stays theirs
            Some(key) if key.eq_ignore_ascii_case("color.ui") => return,
            Some(_) => {}
            // git refuses a count past its keys: nothing to add to
            None => return,
        }
    }
    env.insert("GIT_CONFIG_COUNT".to_string(), next.to_string());
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

    fn with(vars: &[(&str, &str)]) -> HashMap<String, String> {
        let vars: HashMap<String, String> = vars
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        let mut env = HashMap::new();
        with_color(&mut env, |name| vars.get(name).cloned());
        env
    }

    #[test]
    fn color_comes_after_the_users_own_entries() {
        let env = with(&[
            ("GIT_CONFIG_COUNT", "2"),
            ("GIT_CONFIG_KEY_0", "user.name"),
            ("GIT_CONFIG_KEY_1", "status.short"),
        ]);
        assert_eq!(env["GIT_CONFIG_COUNT"], "3");
        assert_eq!(env["GIT_CONFIG_KEY_2"], "color.ui");
        assert_eq!(env["GIT_CONFIG_VALUE_2"], "always");
        assert!(!env.contains_key("GIT_CONFIG_KEY_0"));

        for none in [&[][..], &[("GIT_CONFIG_COUNT", "")][..]] {
            let env = with(none);
            assert_eq!(env["GIT_CONFIG_COUNT"], "1");
            assert_eq!(env["GIT_CONFIG_KEY_0"], "color.ui");
        }
    }

    #[test]
    fn the_users_own_color_setting_or_a_count_git_cant_read_adds_nothing() {
        let theirs = [
            ("GIT_CONFIG_COUNT", "1"),
            ("GIT_CONFIG_KEY_0", "Color.UI"),
            ("GIT_CONFIG_VALUE_0", "never"),
        ];
        assert!(with(&theirs).is_empty());
        assert!(with(&[("GIT_CONFIG_COUNT", "many")]).is_empty());
        assert!(with(&[("GIT_CONFIG_COUNT", "4294967295")]).is_empty());
        // a count past the keys there are is git's to refuse, at once
        assert!(with(&[("GIT_CONFIG_COUNT", "4294967294")]).is_empty());
    }
}
