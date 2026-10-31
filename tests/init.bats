#!/usr/bin/env bats

load "${BATS_TEST_DIRNAME}/helpers/git_environment.bash"

# Integration tests for `meta init claude`

setup() {
    clear_git_local_env
    isolate_user_env
    META_BIN="$BATS_TEST_DIRNAME/../target/debug/meta"

    if [ ! -f "$META_BIN" ]; then
        cargo build --workspace --quiet
    fi

    TEST_DIR="$(cd "$(mktemp -d)" && pwd -P)"

    # Stub claude to detect unintended global marketplace changes
    STUB_DIR="$(cd "$(mktemp -d)" && pwd -P)"
    printf '#!/bin/sh\necho "$*" >> "%s/claude-calls"\n' "$STUB_DIR" > "$STUB_DIR/claude"
    chmod +x "$STUB_DIR/claude"
    # `agent` is installed unless a test removes it
    printf '#!/bin/sh\nexit 0\n' > "$STUB_DIR/agent"
    chmod +x "$STUB_DIR/agent"
    PATH="$STUB_DIR:$PATH"

    # Create .meta.yaml
    cat > "$TEST_DIR/.meta.yaml" <<'EOF'
{
    "projects": {
        "backend": "git@github.com:org/backend.git"
    }
}
EOF

    cd "$TEST_DIR"
}

teardown() {
    cd /
    rm -rf "$TEST_DIR" "$STUB_DIR"
}

# ============ Fresh install ============

@test "init claude creates skills directories" {
    run "$META_BIN" init claude
    [ "$status" -eq 0 ]
    [ -d ".meta/skills" ]
    [ -d ".claude/skills" ]
    [ -d ".agents/skills" ]
}

@test "init claude creates rules directory" {
    run "$META_BIN" init claude
    [ "$status" -eq 0 ]
    [ -d ".claude/rules" ]
}

@test "init claude creates all skills in .meta/skills and links them for Claude and Codex" {
    run "$META_BIN" init claude
    [ "$status" -eq 0 ]
    for name in meta-workspace meta-git meta-exec meta-plugins meta-worktree meta-safety; do
        [ "$(head -2 ".meta/skills/$name/SKILL.md")" = "$(printf -- '---\nname: %s' "$name")" ]
        for agent in .claude .agents; do
            [ "$(readlink "$agent/skills/$name")" = "../../.meta/skills/$name" ]
        done
        [ ! -e ".claude/skills/$name.md" ]
    done
    [ ! -e ".cursor" ]
}

@test "init claude links into other agent folders that exist" {
    mkdir .cursor
    run "$META_BIN" init claude
    [ "$status" -eq 0 ]
    [ "$(readlink .cursor/skills/meta-git)" = "../../.meta/skills/meta-git" ]
}

@test "init claude stops on a .meta file where its folder goes, writing nothing" {
    printf '{}' > .meta
    run "$META_BIN" init claude
    [ "$status" -ne 0 ]
    [ -f .meta ]
    [ ! -e .claude ]
    [ ! -e .agents ]
}

@test "init claude --help installs nothing" {
    run "$META_BIN" init claude --help
    [ "$status" -eq 0 ]
    [[ "$output" == *"meta init"* ]]
    [ ! -e ".claude" ]
    [ ! -e "$STUB_DIR/claude-calls" ]
}

@test "skill copies in this repo match meta_cli" {
    local root="$BATS_TEST_DIRNAME/.."
    local count=0
    for src in "$root"/meta_cli/.claude/skills/meta-*/SKILL.md; do
        local name
        name="$(basename "$(dirname "$src")")"
        for copy in "$root/.meta/skills/$name/SKILL.md" "$root/claude-plugin/skills/$name/SKILL.md"; do
            cmp "$src" "$copy" || { echo "$copy differs from meta_cli: run 'make skills'"; return 1; }
        done
        for agent in .claude .agents; do
            [ "$(readlink "$root/$agent/skills/$name")" = "../../.meta/skills/$name" ]
        done
        count=$((count + 1))
    done
    [ "$count" -eq 6 ]
    # No copies or links of skills meta_cli no longer has
    [ "$(ls -d "$root"/.meta/skills/meta-*/ "$root"/claude-plugin/skills/meta-*/ | wc -l)" -eq 12 ]
    [ "$(ls -d "$root"/.claude/skills/meta-* "$root"/.agents/skills/meta-* | wc -l)" -eq 12 ]
}

@test "init claude creates all rule files" {
    run "$META_BIN" init claude
    [ "$status" -eq 0 ]
    [ -f ".claude/rules/meta-workspace-discipline.md" ]
    [ -f ".claude/rules/meta-destructive-commands.md" ]
}

@test "init claude creates settings.json with hooks" {
    run "$META_BIN" init claude
    [ "$status" -eq 0 ]
    [ -f ".claude/settings.json" ]

    # Check for all 3 hooks
    run grep "SessionStart" .claude/settings.json
    [ "$status" -eq 0 ]
    run grep "PreToolUse" .claude/settings.json
    [ "$status" -eq 0 ]
    run grep "PreCompact" .claude/settings.json
    [ "$status" -eq 0 ]
}

@test "init claude settings references meta context" {
    run "$META_BIN" init claude
    [ "$status" -eq 0 ]
    run grep "meta context" .claude/settings.json
    [ "$status" -eq 0 ]
}

@test "init claude settings references agent guard" {
    run "$META_BIN" init claude
    [ "$status" -eq 0 ]
    run grep "agent guard" .claude/settings.json
    [ "$status" -eq 0 ]
    run grep "meta agent guard" .claude/settings.json
    [ "$status" -ne 0 ]
}

@test "init claude PreToolUse has Bash matcher" {
    run "$META_BIN" init claude
    [ "$status" -eq 0 ]
    run grep '"matcher": "Bash"' .claude/settings.json
    [ "$status" -eq 0 ]
}

# ============ Skip existing files ============

@test "init claude skips existing skill files" {
    mkdir -p .meta/skills/meta-workspace .claude/skills/meta-git
    echo "custom content" > .meta/skills/meta-workspace/SKILL.md
    echo "custom content" > .claude/skills/meta-git/SKILL.md

    run "$META_BIN" init claude
    [ "$status" -eq 0 ]

    [ "$(cat .claude/skills/meta-workspace/SKILL.md)" = "custom content" ]
    [ "$(cat .claude/skills/meta-git/SKILL.md)" = "custom content" ]
}

@test "init claude skips existing rule files" {
    mkdir -p .claude/rules
    echo "custom rule" > .claude/rules/meta-workspace-discipline.md

    run "$META_BIN" init claude
    [ "$status" -eq 0 ]

    run cat .claude/rules/meta-workspace-discipline.md
    [ "$output" = "custom rule" ]
}

# ============ Merge settings ============

@test "init claude merges into existing settings.json" {
    mkdir -p .claude
    echo '{"permissions": {"allow": ["Bash(git:*)"]}}' > .claude/settings.json

    run "$META_BIN" init claude
    [ "$status" -eq 0 ]

    # Should preserve permissions
    run grep "permissions" .claude/settings.json
    [ "$status" -eq 0 ]

    # Should add hooks
    run grep "SessionStart" .claude/settings.json
    [ "$status" -eq 0 ]
}

@test "init claude preserves existing hooks in settings.json" {
    mkdir -p .claude
    cat > .claude/settings.json <<'EOF'
{
    "hooks": {
        "Stop": [{"hooks": [{"type": "prompt", "prompt": "custom", "timeout": 30}]}]
    }
}
EOF

    run "$META_BIN" init claude
    [ "$status" -eq 0 ]

    # Should preserve Stop hook
    run grep "Stop" .claude/settings.json
    [ "$status" -eq 0 ]
    run grep "custom" .claude/settings.json
    [ "$status" -eq 0 ]

    # Should add meta hooks
    run grep "SessionStart" .claude/settings.json
    [ "$status" -eq 0 ]
}

@test "init claude does not duplicate existing meta hooks" {
    # First install
    run "$META_BIN" init claude
    [ "$status" -eq 0 ]

    # Second install (should merge without duplicating)
    run "$META_BIN" init claude
    [ "$status" -eq 0 ]

    # Count SessionStart occurrences - should be exactly 1
    count=$(grep -c "SessionStart" .claude/settings.json)
    [ "$count" -eq 1 ]
}

# ============ --update flag ============

@test "init claude --update refreshes skill files" {
    mkdir -p .meta/skills/meta-workspace
    echo "old content" > .meta/skills/meta-workspace/SKILL.md

    run "$META_BIN" init claude --update
    [ "$status" -eq 0 ]

    run grep "Meta Workspace Skill" .meta/skills/meta-workspace/SKILL.md
    [ "$status" -eq 0 ]
}

@test "init claude --update keeps a skill folder meta did not mark" {
    mkdir -p .claude/skills/meta-git
    echo "another tool" > .claude/skills/meta-git/SKILL.md

    run "$META_BIN" init claude --update
    [ "$status" -eq 0 ]

    [ "$(cat .claude/skills/meta-git/SKILL.md)" = "another tool" ]
    [[ "$output" == *"not created by meta"* ]]
}

@test "init claude --update leaves other tools' links alone" {
    mkdir -p .other/meta-git .claude/skills
    echo "other tool" > .other/meta-git/SKILL.md
    ln -s ../../.other/meta-git .claude/skills/meta-git

    run "$META_BIN" init claude --update
    [ "$status" -eq 0 ]

    [ "$(readlink .claude/skills/meta-git)" = "../../.other/meta-git" ]
    [ "$(cat .other/meta-git/SKILL.md)" = "other tool" ]
    [[ "$output" == *"not created by meta"* ]]
}

@test "init claude --update refreshes rule files" {
    mkdir -p .claude/rules
    echo "old content" > .claude/rules/meta-workspace-discipline.md

    run "$META_BIN" init claude --update
    [ "$status" -eq 0 ]

    run grep "Meta Workspace Discipline" .claude/rules/meta-workspace-discipline.md
    [ "$status" -eq 0 ]
}

@test "init claude --update skips settings.json" {
    mkdir -p .claude
    echo '{"custom": true}' > .claude/settings.json

    run "$META_BIN" init claude --update
    [ "$status" -eq 0 ]

    # Settings should be untouched
    run grep "custom" .claude/settings.json
    [ "$status" -eq 0 ]
    run grep "SessionStart" .claude/settings.json
    [ "$status" -eq 1 ]  # Should NOT have SessionStart
}

# ============ --force flag ============

@test "init claude --force overwrites skill files" {
    mkdir -p .meta/skills/meta-workspace
    echo "custom content" > .meta/skills/meta-workspace/SKILL.md

    run "$META_BIN" init claude --force
    [ "$status" -eq 0 ]

    run grep "Meta Workspace Skill" .meta/skills/meta-workspace/SKILL.md
    [ "$status" -eq 0 ]
}

@test "init claude --force overwrites rule files" {
    mkdir -p .claude/rules
    echo "custom rule" > .claude/rules/meta-workspace-discipline.md

    run "$META_BIN" init claude --force
    [ "$status" -eq 0 ]

    run grep "Meta Workspace Discipline" .claude/rules/meta-workspace-discipline.md
    [ "$status" -eq 0 ]
}

@test "init claude --force merges settings.json" {
    mkdir -p .claude
    echo '{"custom": true}' > .claude/settings.json

    run "$META_BIN" init claude --force
    [ "$status" -eq 0 ]

    # Custom key is preserved even with --force
    run grep "custom" .claude/settings.json
    [ "$status" -eq 0 ]

    # Meta hooks should be present
    run grep "SessionStart" .claude/settings.json
    [ "$status" -eq 0 ]
}

# ============ settings.local.json ============

@test "init claude does not touch settings.local.json" {
    mkdir -p .claude
    echo '{"permissions": {"allow": ["Bash(git:*)"]}}' > .claude/settings.local.json

    run "$META_BIN" init claude
    [ "$status" -eq 0 ]

    run cat .claude/settings.local.json
    [[ "$output" == *"permissions"* ]]
    [[ "$output" != *"hooks"* ]]
}

@test "init claude --force does not touch settings.local.json" {
    mkdir -p .claude
    echo '{"permissions": {"allow": ["Bash(git:*)"]}}' > .claude/settings.local.json

    run "$META_BIN" init claude --force
    [ "$status" -eq 0 ]

    run cat .claude/settings.local.json
    [[ "$output" == *"permissions"* ]]
    [[ "$output" != *"hooks"* ]]
}

# ============ Output messages ============

@test "init claude shows correct file counts" {
    run "$META_BIN" init claude
    [ "$status" -eq 0 ]
    [[ "$output" == *"9 file(s)"* ]]  # 6 skills + 2 rules + 1 settings
}

@test "init claude shows skills count" {
    run "$META_BIN" init claude
    [ "$status" -eq 0 ]
    [[ "$output" == *"6 skills"* ]]
}

@test "init claude shows rules count" {
    run "$META_BIN" init claude
    [ "$status" -eq 0 ]
    [[ "$output" == *"2 rule files"* ]]
}

@test "init claude shows hook names" {
    run "$META_BIN" init claude
    [ "$status" -eq 0 ]
    [[ "$output" == *"SessionStart"* ]]
    [[ "$output" == *"PreToolUse"* ]]
    [[ "$output" == *"PreCompact"* ]]
}

# ============ Where it installs ============

@test "init claude refuses outside a meta workspace" {
    rm .meta.yaml

    run "$META_BIN" init claude
    [ "$status" -ne 0 ]
    [[ "$output" == *"No .meta.yaml in $TEST_DIR or any folder above it"* ]]
    [ ! -e .claude ]
    [ ! -e .meta ]
}

@test "init claude from a child repo installs once, in the workspace" {
    mkdir -p backend/src
    git -C backend init --quiet
    cd backend/src
    run "$META_BIN" init claude
    [ "$status" -eq 0 ]
    [[ "$output" == *"Installing in the workspace at $TEST_DIR (the nearest .meta.yaml)"* ]]
    [ -f "$TEST_DIR/.claude/settings.json" ]
    [ ! -e "$TEST_DIR/backend/.claude" ]
    [ ! -e "$TEST_DIR/backend/src/.claude" ]
}

# ============ The agent guard hook ============

@test "init claude adds no second agent guard after a hand edit" {
    "$META_BIN" init claude
    python3 - <<'PY'
import json
p = '.claude/settings.json'
s = json.load(open(p))
s['hooks']['PreToolUse'][0]['hooks'].append({'type': 'command', 'command': 'my-own-check'})
json.dump(s, open(p, 'w'))
PY
    run "$META_BIN" init claude
    [ "$status" -eq 0 ]
    [[ "$output" == *"Already has meta's hooks"* ]]
    python3 - <<'PY'
import json
groups = json.load(open('.claude/settings.json'))['hooks']['PreToolUse']
assert len(groups) == 1, groups
commands = [h['command'] for h in groups[0]['hooks']]
assert commands == ['agent guard', 'my-own-check'], commands
PY
}

@test "without agent, init claude skips the guard hook and adds it once agent is there" {
    # Only the stubs on meta's PATH, so an agent installed on this machine isn't found
    init_claude() { PATH="$STUB_DIR" "$META_BIN" init claude; }
    rm "$STUB_DIR/agent"
    run init_claude
    [ "$status" -eq 0 ]
    [[ "$output" == *"Skipped the PreToolUse hook \`agent guard\`"* ]]
    [[ "$output" == *"run \`meta init claude\` again"* ]]
    run grep "agent guard" .claude/settings.json
    [ "$status" -ne 0 ]

    printf '#!/bin/sh\nexit 0\n' > "$STUB_DIR/agent"
    chmod +x "$STUB_DIR/agent"
    run init_claude
    [ "$status" -eq 0 ]
    [[ "$output" == *"Added PreToolUse hooks to"* ]]
    [ "$(grep -c "agent guard" .claude/settings.json)" -eq 1 ]

    # A guard already there is never taken out
    rm "$STUB_DIR/agent"
    run init_claude
    [ "$status" -eq 0 ]
    [[ "$output" != *"Skipped the PreToolUse hook"* ]]
    [ "$(grep -c "agent guard" .claude/settings.json)" -eq 1 ]
}

assert_settings_refused_without_changes() {
    local content="$1" key="$2"
    mkdir -p .claude
    printf '%s' "$content" > .claude/settings.json
    local before
    before="$(find . -print | sort)"
    for flag in '' '--force'; do
        run "$META_BIN" init claude $flag
        [ "$status" -ne 0 ]
        [[ "$output" == *".claude/settings.json"* ]]
        [[ "$output" == *"$key"* ]]
        [[ "$output" != *"Installed"* ]]
        [[ "$output" != *"Hooks:"* ]]
        [ "$(cat .claude/settings.json)" = "$content" ]
        [ "$(find . -print | sort)" = "$before" ]
    done
}

@test "init refuses settings array before writing any files" {
    assert_settings_refused_without_changes '[{"user":"data"}]' "must hold a JSON object"
}

@test "init refuses hooks array before writing any files" {
    assert_settings_refused_without_changes '{"hooks":[{"mine":1}],"theme":"dark"}' "key 'hooks'"
    [[ "$output" == *"must be an object"* ]]
}

@test "init refuses a PreToolUse object without agent too" {
    rm "$STUB_DIR/agent"
    mkdir -p .claude
    printf '%s' '{"hooks":{"PreToolUse":{}}}' > .claude/settings.json
    init_claude() { PATH="$STUB_DIR" "$META_BIN" init claude; }
    run init_claude
    [ "$status" -ne 0 ]
    [[ "$output" == *"key 'hooks.PreToolUse'"* ]]
    [ ! -e .meta ]
    [ "$(cat .claude/settings.json)" = '{"hooks":{"PreToolUse":{}}}' ]
}

@test "init refuses PreToolUse object before writing any files" {
    assert_settings_refused_without_changes '{"hooks":{"PreToolUse":{"matcher":"Bash"}}}' "key 'hooks.PreToolUse'"
    [[ "$output" == *"must be an array"* ]]
}

@test "init refuses invalid JSON before writing any files" {
    assert_settings_refused_without_changes '{invalid json' "Invalid JSON"
}

@test "init compares whole hook groups including matcher and reports only added hooks" {
    run "$META_BIN" init claude
    [ "$status" -eq 0 ]
    [[ "$output" == *"Created "*".claude/settings.json"* ]]
    python3 - <<'PY'
import json
from pathlib import Path
p = Path('.claude/settings.json')
s = json.loads(p.read_text())
s['hooks']['PreToolUse'][0]['matcher'] = 'Read'
p.write_text(json.dumps(s))
PY
    run "$META_BIN" init claude
    [ "$status" -eq 0 ]
    [[ "$output" == *"Added PreToolUse hooks to "* ]]
    [[ "$output" == *"Hooks:   .claude/settings.json (PreCompact, PreToolUse, SessionStart)"* ]]
    python3 - <<'PY'
import json
s = json.load(open('.claude/settings.json'))
assert [g['matcher'] for g in s['hooks']['PreToolUse']] == ['Read', 'Bash']
PY
    touch -t 200001010000 .claude/settings.json
    run "$META_BIN" init claude
    [ "$status" -eq 0 ]
    [[ "$output" == *"Already has meta's hooks (no write): "* ]]
    python3 - <<'PY'
import os
assert os.stat('.claude/settings.json').st_mtime < 1000000000
PY
}

@test "init prints marketplace hint without calling claude" {
    run "$META_BIN" init claude
    [ "$status" -eq 0 ]
    [ ! -e "$STUB_DIR/claude-calls" ]
    [[ "$output" == *"For global Claude plugin access, run:"* ]]
    [[ "$output" == *"  claude plugin marketplace add gitkb/claude-plugins"* ]]
    [[ "$output" == *"  claude plugin install meta@gitkb"* ]]
}

@test "init update leaves unsupported settings alone and does not claim hooks" {
    mkdir .claude
    echo '[]' > .claude/settings.json
    run "$META_BIN" init claude --update
    [ "$status" -eq 0 ]
    [ "$(cat .claude/settings.json)" = '[]' ]
    [[ "$output" != *"Hooks:"* ]]
    [[ "$output" == *"--update preserves settings"* ]]
}

@test "init claude does not read a settings.json reached through a link" {
    # A settings.json holding meta's hooks, elsewhere
    mkdir other
    (cd other && echo 'projects: {}' > .meta.yaml && "$META_BIN" init claude >/dev/null)
    grep -q SessionStart other/.claude/settings.json
    mkdir -p .claude
    ln -s ../other/.claude/settings.json .claude/settings.json

    run "$META_BIN" init claude
    [ "$status" -eq 0 ]
    [[ "$output" == *"reached through the symlink"* ]]
    [[ "$output" != *"Hooks:"* ]]
}

@test "init claude offers --force and --update only for what they refresh" {
    mkdir -p other/rules .claude
    ln -s ../other/rules .claude/rules
    run "$META_BIN" init claude
    [ "$status" -eq 0 ]
    [[ "$output" == *"reached through the symlink"* ]]
    [[ "$output" != *"--force or --update"* ]]

    run "$META_BIN" init claude --update
    [ "$status" -eq 0 ]
    [[ "$output" != *"Skipped "*" existing file(s)"* ]]
}
