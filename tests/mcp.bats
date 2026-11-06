#!/usr/bin/env bats

load "${BATS_TEST_DIRNAME}/helpers/git_environment.bash"

# meta-mcp's tools that run meta, with the real binaries

setup() {
    sandbox
    META_MCP_BIN="$META_BIN_DIR/meta-mcp"
    # meta-mcp runs the meta, and meta the plugins, found here
    export PATH="$META_BIN_DIR:$PATH"

    # A workspace repo with r1 (tag odd, pushed to a bare remote) and r2
    # (tag even, at a path other than its name)
    WS="$TEST_DIR/ws"
    git init --quiet --bare "$TEST_DIR/r1.git"
    for repo in "$WS" "$WS/r1" "$WS/second"; do
        mkdir -p "$repo"
        git -C "$repo" init --quiet -b main
        echo "# $repo" > "$repo/README"
        git -C "$repo" add README
        git -C "$repo" commit --quiet -m init
    done
    git -C "$WS/r1" remote add origin "$TEST_DIR/r1.git"
    git -C "$WS/r1" push --quiet -u origin main
    cat > "$WS/.meta.yaml" <<EOF
projects:
  r1: {repo: "$TEST_DIR/r1.git", tags: [odd]}
  r2: {repo: "$TEST_DIR/r2.git", path: second, tags: [even]}
EOF
    printf 'r1\nsecond\n' > "$WS/.gitignore"
    git -C "$WS" add .meta.yaml .gitignore
    git -C "$WS" commit --quiet -m ws
    cd "$WS"
}

# call TOOL ARGUMENTS: "isError=<true|null>", then the tool's text
call() {
    printf '{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"%s","arguments":%s}}\n' "$1" "$2" |
        "$META_MCP_BIN" | jq -r '"isError=\(.result.isError)", .result.content[0].text'
}

@test "meta-mcp meta_git_add and meta_git_commit act only in the project named" {
    echo x >> r1/README
    echo y >> second/README
    run call meta_git_add '{"project":"r1","files":["README"]}'
    [[ "$output" == "isError=null"* ]]
    [ "$(git -C r1 diff --cached --name-only)" = "README" ]
    [ -z "$(git -C second diff --cached --name-only)" ]

    # by the configured name, at the project's own path
    run call meta_git_add '{"project":"r2"}'
    [ "$(git -C second diff --cached --name-only)" = "README" ]
    git -C second reset --quiet

    run call meta_git_add '{"project":"no-such"}'
    [[ "$output" == "isError=true"*"not found"* ]]
    [ -z "$(git -C second diff --cached --name-only)" ]

    git -C second add README
    run call meta_git_commit '{"project":"r1","message":"only r1"}'
    [[ "$output" == "isError=null"* ]]
    [ "$(git -C r1 log -1 --format=%s)" = "only r1" ]
    [ "$(git -C second log -1 --format=%s)" = "init" ]
    [ "$(git log -1 --format=%s)" = "ws" ]
}

@test "meta-mcp meta_git_status answers for the project named, and . for the root" {
    echo x >> r1/README
    run call meta_git_status '{"project":"r1"}'
    [[ "$output" == "isError=null"* ]]
    [ "$(sed 1d <<<"$output" | jq -r '[.results[].path] | join(" ")')" = "$WS/r1" ]
    [[ "$output" == *"modified:"* ]]
    run call meta_git_status '{"project":"."}'
    [ "$(sed 1d <<<"$output" | jq -r '[.results[].path] | join(" ")')" = "$WS" ]
}

@test "meta-mcp meta_git_checkout refuses a branch git would read as an option, and never restores a file" {
    run call meta_git_checkout '{"branch":"--orphan=evil"}'
    [[ "$output" == "isError=true"*"not a valid branch name"* ]]
    for repo in . r1 second; do
        [ "$(git -C "$repo" branch --show-current)" = main ]
        [ -z "$(git -C "$repo" diff --cached --name-only)" ]
    done

    # no branch is called README: git must not take it for the file
    echo "my edit" >> r1/README
    run call meta_git_checkout '{"branch":"README"}'
    [[ "$output" == "isError=true"* ]]
    grep -q "my edit" r1/README

    run call meta_git_checkout '{"branch":"topic","create":true,"tag":"odd"}'
    [ "$(git -C r1 branch --show-current)" = topic ]
    [ "$(git -C second branch --show-current)" = main ]
}

@test "meta-mcp meta_git_push reports each repo, and isn't an error when only some failed" {
    echo x >> r1/README
    git -C r1 commit --quiet -am ahead
    # the root has no remote to push to
    run call meta_git_push '{}'
    [[ "$output" == "isError=null"* ]]
    [ "$(git -C "$TEST_DIR/r1.git" log -1 --format=%s main)" = "ahead" ]
    [ "$(sed 1d <<<"$output" | jq '[.results[] | select(.error)] | length')" -ge 1 ]
}

@test "meta-mcp meta_exec is an error only when every repo failed" {
    touch r1/marker
    run call meta_exec '{"command":["test","-f","marker"]}'
    [[ "$output" == "isError=null"* ]]
    run call meta_exec '{"command":["test","-f","no-such-file"]}'
    [[ "$output" == "isError=true"* ]]
    # its output, not only an exit status
    [ "$(sed 1d <<<"$output" | jq '.results | length')" -eq 3 ]
}

@test "meta-mcp meta_list_plugins lists the installed plugins" {
    run call meta_list_plugins '{}'
    [[ "$output" == "isError=null"* ]]
    sed 1d <<<"$output" | jq -e '.success and ([.results[].data.name] | index("git") != null)'
}

@test "meta-mcp meta_git_diff, meta_git_multi_commit and meta_git_branch read each project's own repository" {
    echo x >> second/README
    run call meta_git_diff '{"project":"r2"}'
    [[ "$output" == "isError=null"* ]]
    sed 1d <<<"$output" | jq -e 'length == 1 and .[0].project == "r2" and (.[0].diff | contains("+x"))'

    git -C second add README
    run call meta_git_multi_commit '{"commits":[{"project":"r2","message":"in r2"},{"project":"r1","message":"m"}]}'
    [[ "$output" == "isError=null"* ]]
    [ "$(git -C second log -1 --format=%s)" = "in r2" ]
    # nothing staged in r1: its commit failed, and says why
    sed 1d <<<"$output" | jq -e '.results[1].success == false and (.results[1].error | length > 0)'
    [ "$(git -C r1 log -1 --format=%s)" = init ]

    echo y >> r1/README
    git -C r1 commit --quiet -am ahead
    git -C second checkout --quiet --detach
    run call meta_git_branch '{}'
    sed 1d <<<"$output" | jq -e '
        (.[] | select(.project == "r1")) == {"project":"r1","branch":"main","tracking":"origin/main","ahead":1,"behind":0}
        and (.[] | select(.project == "r2")) == {"project":"r2","branch":"HEAD","tracking":null,"ahead":0,"behind":0}'

    # a plain folder: never the workspace repository around it
    mkdir plain
    echo '  plain: {repo: x}' >> .meta.yaml
    echo "root work" > staged
    git add staged
    run call meta_git_diff '{"project":"plain","staged":true}'
    [[ "$output" == "isError=true"*"not a git repository"* ]]
    [[ "$output" != *"root work"* ]]
    run call meta_git_multi_commit '{"commits":[{"project":"plain","message":"m"}]}'
    [[ "$output" == "isError=true"* ]]
    [ "$(git log -1 --format=%s)" = ws ]
    run call meta_git_branch '{}'
    sed 1d <<<"$output" | jq -e '.[] | select(.project == "plain") | .branch == null and .ahead == null and (.error | length > 0)'
}

@test "meta-mcp meta_search_code searches a repository and a plain folder each on its own, at most 50 matches" {
    # the workspace repository ignores the plain folder; its own .gitignore counts
    mkdir plain
    echo hit > plain/f
    echo hit > plain/ignored
    echo ignored > plain/.gitignore
    echo plain >> .gitignore
    echo '  plain: {repo: x}' >> .meta.yaml
    for i in $(seq 60); do echo hit; done > r1/many
    run call meta_search_code '{"pattern":"hit"}'
    [[ "$output" == "isError=null"* ]]
    sed 1d <<<"$output" | jq -e '
        (.[] | select(.project == "r1") | (.matches | length) == 50 and .truncated)
        and (.[] | select(.project == "plain") | .matches == ["f:1:hit"])'

    # no match is an answer; a search git refuses everywhere is a failure
    run call meta_search_code '{"pattern":"absent"}'
    [[ "$output" == "isError=null"* ]]
    run call meta_search_code '{"pattern":"["}'
    [[ "$output" == "isError=true"* ]]
}
