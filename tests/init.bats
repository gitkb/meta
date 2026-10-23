#!/usr/bin/env bats

load "${BATS_TEST_DIRNAME}/helpers/git_environment.bash"

# Integration tests for `meta init claude`

setup() {
    clear_git_local_env
    META_BIN="$BATS_TEST_DIRNAME/../target/debug/meta"

    if [ ! -f "$META_BIN" ]; then
        cargo build --workspace --quiet
    fi

    TEST_DIR="$(mktemp -d)"

    # Stub claude: init registers a plugin marketplace through it
    STUB_DIR="$(mktemp -d)"
    printf '#!/bin/sh\necho "$*" >> "%s/claude-calls"\n' "$STUB_DIR" > "$STUB_DIR/claude"
    chmod +x "$STUB_DIR/claude"
    PATH="$STUB_DIR:$PATH"

    # Create .meta config
    cat > "$TEST_DIR/.meta.json" <<'EOF'
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

@test "init claude creates skills directory" {
    run "$META_BIN" init claude
    [ "$status" -eq 0 ]
    [ -d ".claude/skills" ]
}

@test "init claude creates rules directory" {
    run "$META_BIN" init claude
    [ "$status" -eq 0 ]
    [ -d ".claude/rules" ]
}

@test "init claude creates all skills as SKILL.md folders with frontmatter" {
    run "$META_BIN" init claude
    [ "$status" -eq 0 ]
    for name in meta-workspace meta-git meta-exec meta-plugins meta-worktree meta-safety; do
        [ "$(head -2 ".claude/skills/$name/SKILL.md")" = "$(printf -- '---\nname: %s' "$name")" ]
        [ ! -e ".claude/skills/$name.md" ]
    done
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
        for copy in "$root/.claude/skills/$name/SKILL.md" "$root/claude-plugin/skills/$name/SKILL.md"; do
            cmp "$src" "$copy" || { echo "$copy differs from meta_cli: run 'make skills'"; return 1; }
        done
        count=$((count + 1))
    done
    [ "$count" -eq 6 ]
    # No copies of skills meta_cli no longer has
    [ "$(ls -d "$root"/.claude/skills/meta-*/ "$root"/claude-plugin/skills/meta-*/ | wc -l)" -eq 12 ]
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
    mkdir -p .claude/skills/meta-workspace
    echo "custom content" > .claude/skills/meta-workspace/SKILL.md

    run "$META_BIN" init claude
    [ "$status" -eq 0 ]

    run cat .claude/skills/meta-workspace/SKILL.md
    [ "$output" = "custom content" ]
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
    mkdir -p .claude/skills/meta-workspace
    echo "old content" > .claude/skills/meta-workspace/SKILL.md

    run "$META_BIN" init claude --update
    [ "$status" -eq 0 ]

    run grep "Meta Workspace Skill" .claude/skills/meta-workspace/SKILL.md
    [ "$status" -eq 0 ]
}

@test "init claude keeps old flat skill files without --update" {
    mkdir -p .claude/skills
    echo "custom content" > .claude/skills/meta-git.md

    run "$META_BIN" init claude
    [ "$status" -eq 0 ]

    [ "$(cat .claude/skills/meta-git.md)" = "custom content" ]
    [ -f ".claude/skills/meta-git/SKILL.md" ]
}

@test "init claude --update replaces old flat skill files" {
    mkdir -p .claude/skills
    for name in meta-workspace meta-git meta-exec meta-plugins meta-worktree meta-safety; do
        echo "old content" > ".claude/skills/$name.md"
    done
    echo "mine" > .claude/skills/my-skill.md

    run "$META_BIN" init claude --update
    [ "$status" -eq 0 ]

    for name in meta-workspace meta-git meta-exec meta-plugins meta-worktree meta-safety; do
        [ ! -e ".claude/skills/$name.md" ]
        [ -f ".claude/skills/$name/SKILL.md" ]
    done
    [ "$(cat .claude/skills/my-skill.md)" = "mine" ]
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
    mkdir -p .claude/skills/meta-workspace
    echo "custom content" > .claude/skills/meta-workspace/SKILL.md

    run "$META_BIN" init claude --force
    [ "$status" -eq 0 ]

    run grep "Meta Workspace Skill" .claude/skills/meta-workspace/SKILL.md
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

@test "init claude --force overwrites settings.json" {
    mkdir -p .claude
    echo '{"custom": true}' > .claude/settings.json

    run "$META_BIN" init claude --force
    [ "$status" -eq 0 ]

    # Custom key should be gone
    run grep "custom" .claude/settings.json
    [ "$status" -eq 1 ]

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

# ============ Warning without .meta config ============

@test "init claude warns when not in meta repo" {
    rm .meta.json

    run "$META_BIN" init claude
    [ "$status" -eq 0 ]
    [[ "$output" == *"Warning"* ]]
    [[ "$output" == *"No .meta config"* ]]
}
