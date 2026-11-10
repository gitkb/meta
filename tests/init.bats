#!/usr/bin/env bats

load "${BATS_TEST_DIRNAME}/helpers/git_environment.bash"

# Integration tests for `meta init skills`

SKILL_NAMES="meta-workspace meta-git meta-exec meta-plugins meta-worktree meta-safety"
OLD_AGENT_DIRS=".cursor .opencode .pi .omp .grok"

setup() {
    clear_git_local_env
    isolate_user_env
    META_BIN="$META_BIN_DIR/meta"

    TEST_DIR="$(cd "$(mktemp -d)" && pwd -P)"

    # Stub claude to detect unintended global marketplace changes
    STUB_DIR="$(cd "$(mktemp -d)" && pwd -P)"
    printf '#!/bin/sh\necho "$*" >> "%s/claude-calls"\n' "$STUB_DIR" > "$STUB_DIR/claude"
    chmod +x "$STUB_DIR/claude"
    # `agent` is installed unless a test removes it
    printf '#!/bin/sh\n[ "$1" = --help ] && echo "Claude Code agent toolkit"\nexit 0\n' > "$STUB_DIR/agent"
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

@test "init skills creates skills directories" {
    run "$META_BIN" init skills
    [ "$status" -eq 0 ]
    [ -d ".meta/skills" ]
    [ -d ".claude/skills" ]
    [ -d ".agents/skills" ]
}

@test "init skills writes six skills, links them into .agents and .claude only, and no rules" {
    for dir in $OLD_AGENT_DIRS; do mkdir "$dir"; done
    run "$META_BIN" init skills
    [ "$status" -eq 0 ]
    for name in $SKILL_NAMES; do
        [ -f ".meta/skills/$name/SKILL.md" ] && [ ! -L ".meta/skills/$name/SKILL.md" ]
        [ "$(head -2 ".meta/skills/$name/SKILL.md")" = "$(printf -- '---\nname: %s' "$name")" ]
        for agent in .agents .claude; do
            [ "$(readlink "$agent/skills/$name")" = "../../.meta/skills/$name" ]
            [ -f "$agent/skills/$name/SKILL.md" ]
        done
        [ ! -e ".claude/skills/$name.md" ]
    done
    for dir in $OLD_AGENT_DIRS; do [ -z "$(ls -A "$dir")" ]; done
    [ ! -e .claude/rules ]
}

@test "init skills --update removes what an older meta installed for other agents, and only that" {
    run "$META_BIN" init skills
    [ "$status" -eq 0 ]
    # as an older meta left it: links in five more agent folders, two rules
    for dir in $OLD_AGENT_DIRS; do
        mkdir -p "$dir/skills"
        for name in $SKILL_NAMES; do ln -s "../../.meta/skills/$name" "$dir/skills/$name"; done
    done
    mkdir -p .claude/rules
    cp "$BATS_TEST_DIRNAME/../meta_cli/tests/fixtures/meta-destructive-commands.md" \
        "$BATS_TEST_DIRNAME/../meta_cli/tests/fixtures/meta-workspace-discipline.md" .claude/rules/
    # the user's own, in each of those folders, and an edited skill and rule
    for dir in $OLD_AGENT_DIRS; do
        mkdir -p "$dir/skills/mine" && echo mine > "$dir/skills/mine/SKILL.md"
    done
    rm .grok/skills/meta-git && mkdir .grok/skills/meta-git && echo mine > .grok/skills/meta-git/SKILL.md
    echo mine > .claude/rules/mine.md
    echo "my edit" >> .claude/rules/meta-workspace-discipline.md
    echo "my edit" >> .meta/skills/meta-exec/SKILL.md

    run "$META_BIN" init skills --update
    [ "$status" -eq 0 ]
    for dir in $OLD_AGENT_DIRS; do
        [ "$(ls "$dir/skills" | tr '\n' ' ')" = "$([ "$dir" = .grok ] && echo "meta-git mine " || echo "mine ")" ]
    done
    [ ! -e .claude/rules/meta-destructive-commands.md ]
    [ "$(tail -1 .claude/rules/meta-workspace-discipline.md)" = "my edit" ]
    [[ "$output" == *"Kept"*"meta-workspace-discipline.md"* ]]
    [ "$(cat .claude/rules/mine.md)" = mine ]
    [ "$(tail -1 .meta/skills/meta-exec/SKILL.md)" = "my edit" ]
    [[ "$output" == *"Kept"*"meta-exec/SKILL.md"* ]]

    # with no links left in it, a folder meta made goes
    rm -r .cursor/skills/mine
    for name in $SKILL_NAMES; do ln -s "../../.meta/skills/$name" ".cursor/skills/$name"; done
    run "$META_BIN" init skills --force
    [ "$status" -eq 0 ]
    [ ! -e .cursor/skills ] && [ -d .cursor ]
}

@test "a skill of the user's in .agents/skills or .claude/skills survives install and update" {
    for agent in .agents .claude; do
        mkdir -p "$agent/skills/mine" && echo mine > "$agent/skills/mine/SKILL.md"
    done
    for flag in '' --update --force; do
        run "$META_BIN" init skills $flag
        [ "$status" -eq 0 ]
        for agent in .agents .claude; do [ "$(cat "$agent/skills/mine/SKILL.md")" = mine ]; done
    done
}

@test "the old meta init claude is a usage error naming meta init skills" {
    run "$META_BIN" init claude
    [ "$status" -eq 2 ]
    [[ "$output" == *"meta init skills"* ]]
    [ ! -e .meta ] && [ ! -e .claude ]
}

@test "init skills stops on a .meta file where its folder goes, writing nothing" {
    printf '{}' > .meta
    run "$META_BIN" init skills
    [ "$status" -ne 0 ]
    [ -f .meta ]
    [ ! -e .claude ]
    [ ! -e .agents ]
}

@test "init skills --help installs nothing" {
    run "$META_BIN" init skills --help
    [ "$status" -eq 0 ]
    [[ "$output" == *"meta init"* ]]
    [ ! -e ".claude" ]
    [ ! -e "$STUB_DIR/claude-calls" ]
}

@test "init skills installs the skills of claude-plugin/skills, their one source" {
    local root="$BATS_TEST_DIRNAME/.."
    run "$META_BIN" init skills
    [ "$status" -eq 0 ]
    local count=0
    for src in "$root"/claude-plugin/skills/meta-*/SKILL.md; do
        local name
        name="$(basename "$(dirname "$src")")"
        cmp "$src" ".meta/skills/$name/SKILL.md"
        # this repo's own agents read the same files
        [ "$(readlink "$root/.claude/skills/$name")" = "../../claude-plugin/skills/$name" ]
        [ "$(readlink "$root/.agents/skills/$name")" = "../../claude-plugin/skills/$name" ]
        count=$((count + 1))
    done
    [ "$count" -eq 6 ]
    [ "$(ls -d .meta/skills/meta-*/ | wc -l)" -eq 6 ]
}

@test "init skills creates settings.json with hooks" {
    run "$META_BIN" init skills
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

@test "init skills settings references meta context" {
    run "$META_BIN" init skills
    [ "$status" -eq 0 ]
    run grep "meta -r context" .claude/settings.json
    [ "$status" -eq 0 ]
}

@test "init skills settings references agent guard" {
    run "$META_BIN" init skills
    [ "$status" -eq 0 ]
    run grep "agent guard" .claude/settings.json
    [ "$status" -eq 0 ]
    run grep "meta agent guard" .claude/settings.json
    [ "$status" -ne 0 ]
}

@test "init skills --update lists an older meta's guard hook among the hooks that run" {
    "$META_BIN" init skills
    python3 - <<'PY'
import json
p = '.claude/settings.json'
s = json.load(open(p))
s['hooks']['PreToolUse'][0]['hooks'][0]['command'] = 'agent guard'
json.dump(s, open(p, 'w'))
PY
    run "$META_BIN" init skills --update
    [ "$status" -eq 0 ]
    [[ "$output" == *"Its PreToolUse hook runs any \`agent\` on PATH"* ]]
    [[ "$output" == *"Hooks:   .claude/settings.json (PreCompact, PreToolUse, SessionStart; Claude Code only)"* ]]
}

@test "init skills adds no guard hook for an agent that isn't gitkb's" {
    printf '#!/bin/sh\necho "some other agent"\nexit 0\n' > "$STUB_DIR/agent"
    run "$META_BIN" init skills
    [ "$status" -eq 0 ]
    [[ "$output" == *"Skipped the PreToolUse hook"* ]]
    run grep -c PreToolUse .claude/settings.json
    [ "$output" = 0 ]
}

@test "init skills PreToolUse has Bash matcher" {
    run "$META_BIN" init skills
    [ "$status" -eq 0 ]
    run grep '"matcher": "Bash"' .claude/settings.json
    [ "$status" -eq 0 ]
}

# ============ Skip existing files ============

@test "init skills skips existing skill files" {
    mkdir -p .meta/skills/meta-workspace .claude/skills/meta-git
    echo "custom content" > .meta/skills/meta-workspace/SKILL.md
    echo "custom content" > .claude/skills/meta-git/SKILL.md

    run "$META_BIN" init skills
    [ "$status" -eq 0 ]

    [ "$(cat .claude/skills/meta-workspace/SKILL.md)" = "custom content" ]
    [ "$(cat .claude/skills/meta-git/SKILL.md)" = "custom content" ]
}

# ============ Merge settings ============

@test "init skills merges into existing settings.json" {
    mkdir -p .claude
    echo '{"permissions": {"allow": ["Bash(git:*)"]}}' > .claude/settings.json

    run "$META_BIN" init skills
    [ "$status" -eq 0 ]

    # Should preserve permissions
    run grep "permissions" .claude/settings.json
    [ "$status" -eq 0 ]

    # Should add hooks
    run grep "SessionStart" .claude/settings.json
    [ "$status" -eq 0 ]
}

@test "init skills preserves existing hooks in settings.json" {
    mkdir -p .claude
    cat > .claude/settings.json <<'EOF'
{
    "hooks": {
        "Stop": [{"hooks": [{"type": "prompt", "prompt": "custom", "timeout": 30}]}]
    }
}
EOF

    run "$META_BIN" init skills
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

@test "init skills does not duplicate existing meta hooks" {
    # First install
    run "$META_BIN" init skills
    [ "$status" -eq 0 ]

    # Second install (should merge without duplicating)
    run "$META_BIN" init skills
    [ "$status" -eq 0 ]

    # Count SessionStart occurrences - should be exactly 1
    count=$(grep -c "SessionStart" .claude/settings.json)
    [ "$count" -eq 1 ]
}

# ============ --update flag ============

@test "init skills --update refreshes skill files meta wrote, and keeps one the user edited" {
    local root="$BATS_TEST_DIRNAME/.."
    mkdir -p .meta/skills/meta-workspace .meta/skills/meta-git
    # as an older meta wrote it
    cp "$root/meta_cli/tests/fixtures/meta-workspace-SKILL.md" .meta/skills/meta-workspace/SKILL.md
    echo "my own notes" > .meta/skills/meta-git/SKILL.md

    run "$META_BIN" init skills --update
    [ "$status" -eq 0 ]
    cmp "$root/claude-plugin/skills/meta-workspace/SKILL.md" .meta/skills/meta-workspace/SKILL.md
    [ "$(cat .meta/skills/meta-git/SKILL.md)" = "my own notes" ]
    [[ "$output" == *"Kept $TEST_DIR/.meta/skills/meta-git/SKILL.md (changed since meta wrote it"* ]]
}

@test "init skills --update keeps a skill folder meta did not mark" {
    mkdir -p .claude/skills/meta-git
    echo "another tool" > .claude/skills/meta-git/SKILL.md

    run "$META_BIN" init skills --update
    [ "$status" -eq 0 ]

    [ "$(cat .claude/skills/meta-git/SKILL.md)" = "another tool" ]
    [[ "$output" == *"not created by meta"* ]]
}

@test "init skills --update leaves other tools' links alone" {
    mkdir -p .other/meta-git .claude/skills
    echo "other tool" > .other/meta-git/SKILL.md
    ln -s ../../.other/meta-git .claude/skills/meta-git

    run "$META_BIN" init skills --update
    [ "$status" -eq 0 ]

    [ "$(readlink .claude/skills/meta-git)" = "../../.other/meta-git" ]
    [ "$(cat .other/meta-git/SKILL.md)" = "other tool" ]
    [[ "$output" == *"not created by meta"* ]]
}

@test "init skills --update skips settings.json" {
    mkdir -p .claude
    echo '{"custom": true}' > .claude/settings.json

    run "$META_BIN" init skills --update
    [ "$status" -eq 0 ]

    # Settings should be untouched
    run grep "custom" .claude/settings.json
    [ "$status" -eq 0 ]
    run grep "SessionStart" .claude/settings.json
    [ "$status" -eq 1 ]  # Should NOT have SessionStart
}

# ============ --force flag ============

@test "init skills --force overwrites skill files meta wrote, and keeps one the user edited" {
    local root="$BATS_TEST_DIRNAME/.."
    mkdir -p .meta/skills/meta-workspace .meta/skills/meta-git
    cp "$root/meta_cli/tests/fixtures/meta-workspace-SKILL.md" .meta/skills/meta-workspace/SKILL.md
    echo "custom content" > .meta/skills/meta-git/SKILL.md

    run "$META_BIN" init skills --force
    [ "$status" -eq 0 ]
    cmp "$root/claude-plugin/skills/meta-workspace/SKILL.md" .meta/skills/meta-workspace/SKILL.md
    [ "$(cat .meta/skills/meta-git/SKILL.md)" = "custom content" ]
}

@test "init skills --force merges settings.json" {
    mkdir -p .claude
    echo '{"custom": true}' > .claude/settings.json

    run "$META_BIN" init skills --force
    [ "$status" -eq 0 ]

    # Custom key is preserved even with --force
    run grep "custom" .claude/settings.json
    [ "$status" -eq 0 ]

    # Meta hooks should be present
    run grep "SessionStart" .claude/settings.json
    [ "$status" -eq 0 ]
}

# ============ settings.local.json ============

@test "init skills does not touch settings.local.json" {
    mkdir -p .claude
    echo '{"permissions": {"allow": ["Bash(git:*)"]}}' > .claude/settings.local.json

    run "$META_BIN" init skills
    [ "$status" -eq 0 ]

    run cat .claude/settings.local.json
    [[ "$output" == *"permissions"* ]]
    [[ "$output" != *"hooks"* ]]
}

@test "init skills --force does not touch settings.local.json" {
    mkdir -p .claude
    echo '{"permissions": {"allow": ["Bash(git:*)"]}}' > .claude/settings.local.json

    run "$META_BIN" init skills --force
    [ "$status" -eq 0 ]

    run cat .claude/settings.local.json
    [[ "$output" == *"permissions"* ]]
    [[ "$output" != *"hooks"* ]]
}

# ============ Output messages ============

@test "init skills shows correct file counts" {
    run "$META_BIN" init skills
    [ "$status" -eq 0 ]
    [[ "$output" == *"7 file(s)"* ]]  # 6 skills + 1 settings
}

@test "init skills shows skills count" {
    run "$META_BIN" init skills
    [ "$status" -eq 0 ]
    [[ "$output" == *"6 skills"* ]]
}

@test "init skills shows hook names" {
    run "$META_BIN" init skills
    [ "$status" -eq 0 ]
    [[ "$output" == *"SessionStart"* ]]
    [[ "$output" == *"PreToolUse"* ]]
    [[ "$output" == *"PreCompact"* ]]
}

# ============ Where it installs ============

@test "init skills refuses outside a meta workspace" {
    rm .meta.yaml

    run "$META_BIN" init skills
    [ "$status" -ne 0 ]
    [[ "$output" == *"No .meta.yaml in $TEST_DIR or any folder above it"* ]]
    [ ! -e .claude ]
    [ ! -e .meta ]
}

@test "init skills from a child repo installs once, in the workspace" {
    mkdir -p backend/src
    git -C backend init --quiet
    cd backend/src
    run "$META_BIN" init skills
    [ "$status" -eq 0 ]
    [[ "$output" == *"Installing in the workspace at $TEST_DIR (the nearest .meta.yaml)"* ]]
    [ -f "$TEST_DIR/.claude/settings.json" ]
    [ ! -e "$TEST_DIR/backend/.claude" ]
    [ ! -e "$TEST_DIR/backend/src/.claude" ]
}

# ============ The agent guard hook ============

@test "init skills adds no second agent guard after a hand edit" {
    "$META_BIN" init skills
    python3 - <<'PY'
import json
p = '.claude/settings.json'
s = json.load(open(p))
s['hooks']['PreToolUse'][0]['hooks'].append({'type': 'command', 'command': 'my-own-check'})
json.dump(s, open(p, 'w'))
PY
    run "$META_BIN" init skills
    [ "$status" -eq 0 ]
    [[ "$output" == *"Already has meta's hooks"* ]]
    python3 - <<'PY'
import json
groups = json.load(open('.claude/settings.json'))['hooks']['PreToolUse']
assert len(groups) == 1, groups
commands = [h['command'] for h in groups[0]['hooks']]
assert commands == ["agent --help 2>/dev/null | grep -q 'Claude Code agent toolkit' && agent guard; exit 0", 'my-own-check'], commands
PY
}

@test "without agent, init skills skips the guard hook and adds it once agent is there" {
    # Only the stubs on meta's PATH, so an agent installed on this machine isn't found
    init_skills() { PATH="$STUB_DIR" "$META_BIN" init skills; }
    rm "$STUB_DIR/agent"
    run init_skills
    [ "$status" -eq 0 ]
    [[ "$output" == *"Skipped the PreToolUse hook \`agent guard\`"* ]]
    [[ "$output" == *"run \`meta init skills\` again"* ]]
    run grep "agent guard" .claude/settings.json
    [ "$status" -ne 0 ]

    printf '#!/bin/sh\n[ "$1" = --help ] && echo "Claude Code agent toolkit"\nexit 0\n' > "$STUB_DIR/agent"
    chmod +x "$STUB_DIR/agent"
    run init_skills
    [ "$status" -eq 0 ]
    [[ "$output" == *"Added PreToolUse hooks to"* ]]
    [ "$(grep -c "agent guard" .claude/settings.json)" -eq 1 ]

    # A guard already there is never taken out
    rm "$STUB_DIR/agent"
    run init_skills
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
        run "$META_BIN" init skills $flag
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
    init_skills() { PATH="$STUB_DIR" "$META_BIN" init skills; }
    run init_skills
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
    run "$META_BIN" init skills
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
    run "$META_BIN" init skills
    [ "$status" -eq 0 ]
    [[ "$output" == *"Added PreToolUse hooks to "* ]]
    [[ "$output" == *"Hooks:   .claude/settings.json (PreCompact, PreToolUse, SessionStart; Claude Code only)"* ]]
    python3 - <<'PY'
import json
s = json.load(open('.claude/settings.json'))
assert [g['matcher'] for g in s['hooks']['PreToolUse']] == ['Read', 'Bash']
PY
    touch -t 200001010000 .claude/settings.json
    run "$META_BIN" init skills
    [ "$status" -eq 0 ]
    [[ "$output" == *"Already has meta's hooks (no write): "* ]]
    python3 - <<'PY'
import os
assert os.stat('.claude/settings.json').st_mtime < 1000000000
PY
}

@test "init prints marketplace hint without calling claude" {
    run "$META_BIN" init skills
    [ "$status" -eq 0 ]
    [ ! -e "$STUB_DIR/claude-calls" ]
    [[ "$output" == *"For global Claude plugin access, run:"* ]]
    [[ "$output" == *"  claude plugin marketplace add gitkb/claude-plugins"* ]]
    [[ "$output" == *"  claude plugin install meta@gitkb"* ]]
}

@test "init update leaves unsupported settings alone and does not claim hooks" {
    mkdir .claude
    echo '[]' > .claude/settings.json
    run "$META_BIN" init skills --update
    [ "$status" -eq 0 ]
    [ "$(cat .claude/settings.json)" = '[]' ]
    [[ "$output" != *"Hooks:"* ]]
    [[ "$output" == *"--update preserves settings"* ]]
}

@test "init skills merges into a settings.json reached through a link, keeping the link" {
    # a stowed settings.json, elsewhere
    mkdir -p other .claude
    echo '{"permissions": {"allow": ["Bash(ls)"]}}' > other/settings.json
    ln -s ../other/settings.json .claude/settings.json

    run "$META_BIN" init skills
    [ "$status" -eq 0 ]
    [ "$(readlink .claude/settings.json)" = "../other/settings.json" ]
    grep -q SessionStart other/settings.json
    grep -q 'Bash(ls)' other/settings.json
}

@test "init skills offers --force and --update only for what they refresh" {
    run "$META_BIN" init skills
    [ "$status" -eq 0 ]
    [[ "$output" != *"--force or --update"* ]]

    run "$META_BIN" init skills --update
    [ "$status" -eq 0 ]
    [[ "$output" != *"Skipped "*" existing file(s)"* ]]
}
