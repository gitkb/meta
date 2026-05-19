#!/usr/bin/env bats

# Integration tests for `meta init claude`

setup() {
    META_BIN="$BATS_TEST_DIRNAME/../target/debug/meta"

    if [ ! -f "$META_BIN" ]; then
        cargo build --workspace --quiet
    fi

    TEST_DIR="$(mktemp -d)"

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
    rm -rf "$TEST_DIR"
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

@test "init claude creates all skill files" {
    run "$META_BIN" init claude
    [ "$status" -eq 0 ]
    [ -f ".claude/skills/meta-workspace.md" ]
    [ -f ".claude/skills/meta-git.md" ]
    [ -f ".claude/skills/meta-exec.md" ]
    [ -f ".claude/skills/meta-plugins.md" ]
    [ -f ".claude/skills/meta-worktree.md" ]
    [ -f ".claude/skills/meta-safety.md" ]
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
}

@test "init claude PreToolUse has Bash matcher" {
    run "$META_BIN" init claude
    [ "$status" -eq 0 ]
    run grep '"matcher": "Bash"' .claude/settings.json
    [ "$status" -eq 0 ]
}

# ============ Skip existing files ============

@test "init claude skips existing skill files" {
    mkdir -p .claude/skills
    echo "custom content" > .claude/skills/meta-workspace.md

    run "$META_BIN" init claude
    [ "$status" -eq 0 ]

    run cat .claude/skills/meta-workspace.md
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
    mkdir -p .claude/skills
    echo "old content" > .claude/skills/meta-workspace.md

    run "$META_BIN" init claude --update
    [ "$status" -eq 0 ]

    run grep "Meta Workspace Skill" .claude/skills/meta-workspace.md
    [ "$status" -eq 0 ]
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
    mkdir -p .claude/skills
    echo "custom content" > .claude/skills/meta-workspace.md

    run "$META_BIN" init claude --force
    [ "$status" -eq 0 ]

    run grep "Meta Workspace Skill" .claude/skills/meta-workspace.md
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
    [[ "$output" == *"6 skill files"* ]]
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
