# Plugin Development Guide

This guide provides a comprehensive overview of developing plugins for the `meta` CLI platform, including the plugin protocol, best practices, and examples.

## Table of Contents

- [Introduction](#introduction)
- [Plugin Types](#plugin-types)
- [Plugin Discovery](#plugin-discovery)
- [Plugin Protocol](#plugin-protocol)
- [Creating a Plugin](#creating-a-plugin)
- [Plugin Help System](#plugin-help-system)
- [Best Practices](#best-practices)
- [Testing Plugins](#testing-plugins)
- [Publishing Plugins](#publishing-plugins)
- [Troubleshooting](#troubleshooting)
- [See Also](#see-also)

---

## Introduction

Plugins extend the functionality of `meta` by adding new commands. They are discovered automatically and communicate with meta via a JSON protocol over stdin/stdout.

**Important:** Only plugin commands and `meta exec` are supported. Bare commands like `meta npm install` do **not** work—there is no automatic fallback to loop. Users must either use a plugin command (`meta git status`) or explicitly use `meta exec -- npm install`.

Key benefits:
- **Language agnostic** - Write in any language that can read/write JSON
- **Isolated** - Plugins run as subprocesses
- **Discoverable** - Auto-found from standard locations
- **Command ownership** - Plugins fully own their command namespace

## Plugin Types

### Rust Binary Plugins

Build plugins as Rust crates for performance and native integration:

```rust
// Cargo.toml
[package]
name = "meta-docker"
version = "0.1.0"

[[bin]]
name = "meta-docker"
```

### External Executables

Any executable named `meta-<name>` in your PATH or a plugins directory (see [Plugin Discovery](#plugin-discovery)):

```bash
#!/bin/bash
# meta-hello plugin
```

### Script Plugins

Shell scripts, Python, Node.js, etc.:

```python
#!/usr/bin/env python3
# meta-python plugin
import json
import sys
```

## Plugin Discovery

Meta discovers plugins from these locations (in order):

1. `.meta/plugins/` next to the nearest meta config at or above the current directory, then next to each parent meta config that lists that directory as a project (the same workspace), nearest first. A parent that doesn't list it is another workspace and is not read. Inside a worktree set meta made, a checkout's own `.meta/plugins/` comes first, then those of the checkout it stands for (the one it was made from) and its workspace. `meta plugin install <name> --local` installs into the nearest one.
2. `~/.meta/plugins/` (moved by `META_DATA_DIR`), where `meta plugin install <name>` installs.
3. Executables named `meta-*` on PATH.

The first file found with a given name wins. For `meta <word> ...`, meta starts only the file `meta-<word>`; if it doesn't answer `--meta-plugin-info` within 5 seconds, the command fails naming it. `meta --help` and `meta plugin list` start every plugin; there a plugins folder meta can't read, and a plugin that doesn't answer within 5 seconds, are skipped with a warning naming them; one that answers with anything but its info is left out. Project-local plugins are binaries: `--local` writes `.meta/plugins/.gitignore` (`*`) so they stay out of git as untracked files.

### Naming Convention

A plugin is an executable named `meta-<word>` (on Windows, `meta-<word>.exe`; `.cmd` and `.bat` files aren't plugins), and it serves `meta <word> ...`: `meta-docker` serves `meta docker build`. The file name decides this, not the `name` or `commands` the plugin reports, so a plugin serving two top-level words ships two files.

The `commands` a plugin lists start with its word (`docker build`, `docker push`). For `meta docker build --tag x`, the longest listed command that matches the typed words whole is the request's `command` (`docker build`) and the rest are its `args` (`["--tag", "x"]`); with none matching, `command` is `docker` and every later word is an arg. A bare `meta docker` prints the plugin's help. A word meta has as a built-in command (`exec`, `plugin`, ...) never reaches a plugin.

## Plugin Protocol

Plugins communicate with meta via JSON over stdin/stdout.

### Info Request

Meta queries plugin capabilities:

```bash
meta-docker --meta-plugin-info
```

**Response:**
```json
{
  "name": "docker",
  "version": "0.1.0",
  "description": "Docker operations for meta repositories",
  "commands": ["build", "push", "compose"],
  "help": {
    "build": "Build Docker images across repos",
    "push": "Push images to registry",
    "compose": "Run docker-compose operations"
  }
}
```

### Execution Request

Meta invokes plugin execution:

```bash
echo '<request>' | meta-docker --meta-plugin-exec
```

**Request Format:**
```json
{
  "command": "build",
  "args": ["--tag", "latest"],
  "projects": [
    {
      "name": "api",
      "path": "./api",
      "tags": ["backend"],
      "repo": "git@github.com:org/api.git"
    }
  ],
  "filters": {
    "tags": ["backend"],
    "include": [],
    "exclude": []
  },
  "options": {
    "parallel": false,
    "dry_run": false,
    "json_output": false
  }
}
```

**Response Format:** a plugin prints text (meta shows it as is), or an execution plan that meta runs in each directory:
```json
{
  "plan": {
    "commands": [
      {"dir": "/ws/api", "argv": ["docker", "build", "-t", "api:latest", "."]}
    ],
    "parallel": true
  }
}
```
Each `argv` is a program and its arguments. Meta runs it as given, with no shell, so the plugin needs no quoting; a plugin that needs a shell plans `["sh", "-c", "..."]`. `projects` in the request lists the repos `--include`/`--exclude` selected; a plugin that finds repos itself must refuse those filters rather than ignore them.

### Error Response

```json
{
  "success": false,
  "error": "Unknown command: foo",
  "results": []
}
```

## Creating a Plugin

### Rust Plugin Example

**Cargo.toml:**
```toml
[package]
name = "meta-docker"
version = "0.1.0"
edition = "2021"

[dependencies]
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"
clap = { version = "4.0", features = ["derive"] }
```

**src/main.rs:**
```rust
use serde::{Deserialize, Serialize};
use std::io::{self, Read};

#[derive(Deserialize)]
struct PluginRequest {
    command: String,
    args: Vec<String>,
    projects: Vec<Project>,
    #[serde(default)]
    options: Options,
}

#[derive(Deserialize)]
struct Project {
    name: String,
    path: String,
    tags: Option<Vec<String>>,
}

#[derive(Deserialize, Default)]
struct Options {
    parallel: bool,
    dry_run: bool,
}

#[derive(Serialize)]
struct PluginInfo {
    name: &'static str,
    version: &'static str,
    description: &'static str,
    commands: Vec<&'static str>,
}

#[derive(Serialize)]
struct PluginResponse {
    success: bool,
    results: Vec<ProjectResult>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

#[derive(Serialize)]
struct ProjectResult {
    project: String,
    success: bool,
    output: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

fn main() {
    let args: Vec<String> = std::env::args().collect();

    if args.contains(&"--meta-plugin-info".to_string()) {
        let info = PluginInfo {
            name: "docker",
            version: "0.1.0",
            description: "Docker operations for meta repos",
            commands: vec!["build", "push"],
        };
        println!("{}", serde_json::to_string(&info).unwrap());
        return;
    }

    if args.contains(&"--meta-plugin-exec".to_string()) {
        let mut input = String::new();
        io::stdin().read_to_string(&mut input).unwrap();

        let request: PluginRequest = serde_json::from_str(&input).unwrap();
        let response = execute(request);

        println!("{}", serde_json::to_string(&response).unwrap());
        return;
    }

    // Fallback: show help
    eprintln!("meta-docker: Docker plugin for meta");
    eprintln!("Commands: build, push");
}

fn execute(request: PluginRequest) -> PluginResponse {
    let results: Vec<ProjectResult> = request.projects.iter().map(|p| {
        // Your plugin logic here
        ProjectResult {
            project: p.name.clone(),
            success: true,
            output: format!("Processed {}", p.name),
            error: None,
        }
    }).collect();

    let success = results.iter().all(|r| r.success);

    PluginResponse {
        success,
        results,
        error: None,
    }
}
```

### Shell Script Example

**meta-hello:**
```bash
#!/bin/bash

if [[ "$1" == "--meta-plugin-info" ]]; then
    cat <<EOF
{
  "name": "hello",
  "version": "0.1.0",
  "description": "A friendly greeting plugin",
  "commands": ["world", "there"]
}
EOF
    exit 0
fi

if [[ "$1" == "--meta-plugin-exec" ]]; then
    # Read request from stdin
    REQUEST=$(cat)
    COMMAND=$(echo "$REQUEST" | jq -r '.command')

    # Process and respond
    cat <<EOF
{
  "success": true,
  "results": [
    {"project": ".", "success": true, "output": "Hello, $COMMAND!"}
  ]
}
EOF
    exit 0
fi

echo "meta-hello: Greeting plugin"
echo "Commands: world, there"
```

### Python Example

**meta-py:**
```python
#!/usr/bin/env python3
import json
import sys

def get_info():
    return {
        "name": "py",
        "version": "0.1.0",
        "description": "Python operations plugin",
        "commands": ["lint", "format"]
    }

def execute(request):
    results = []
    for project in request.get("projects", []):
        results.append({
            "project": project["name"],
            "success": True,
            "output": f"Processed {project['name']}"
        })
    return {
        "success": all(r["success"] for r in results),
        "results": results
    }

if __name__ == "__main__":
    if "--meta-plugin-info" in sys.argv:
        print(json.dumps(get_info()))
    elif "--meta-plugin-exec" in sys.argv:
        request = json.loads(sys.stdin.read())
        response = execute(request)
        print(json.dumps(response))
    else:
        print("meta-py: Python plugin for meta")
```

## Plugin Help System

Plugins can provide structured help via the `--meta-plugin-info` response:

```json
{
  "name": "docker",
  "version": "0.1.0",
  "description": "Docker operations for meta repositories",
  "commands": ["build", "push", "compose"],
  "help": {
    "build": "Build Docker images\n\nUsage: meta docker build [OPTIONS]\n\nOptions:\n  --tag TAG  Image tag",
    "push": "Push images to registry",
    "compose": "Run docker-compose commands"
  }
}
```

When users run `meta docker --help`, meta displays this information.

## Best Practices

### 1. Follow the Protocol

Always implement both `--meta-plugin-info` and `--meta-plugin-exec` flags.

### 2. Handle Errors Gracefully

```rust
PluginResponse {
    success: false,
    results: vec![],
    error: Some("Failed to connect to Docker daemon".to_string()),
}
```

### 3. Respect Filters

Use the provided `filters` to only operate on requested projects:

```rust
let filtered_projects: Vec<_> = request.projects
    .iter()
    .filter(|p| {
        if let Some(tags) = &p.tags {
            request.filters.tags.iter().any(|t| tags.contains(t))
        } else {
            request.filters.tags.is_empty()
        }
    })
    .collect();
```

### 4. Support Dry Run

When `options.dry_run` is true, show what would happen without executing:

```rust
if request.options.dry_run {
    return ProjectResult {
        project: p.name.clone(),
        success: true,
        output: format!("[DRY RUN] Would build {}", p.name),
        error: None,
    };
}
```

### 5. Provide JSON Output

When `options.json_output` is true, ensure structured output.

### 6. Cross-Platform Compatibility

Avoid OS-specific code when possible. Use standard paths and commands.

## Testing Plugins

### Manual Testing

```bash
# Test info response
./meta-docker --meta-plugin-info | jq .

# Test execution
echo '{"command":"build","args":[],"projects":[{"name":"test","path":"./test"}],"options":{}}' \
  | ./meta-docker --meta-plugin-exec | jq .
```

### Integration Testing

```bash
# Place plugin in discovery path
cp meta-docker "${META_DATA_DIR:-$HOME/.meta}/plugins/"

# Test via meta
meta docker build --dry-run
```

### Automated Tests

```rust
#[test]
fn test_plugin_info() {
    let output = Command::new("./target/debug/meta-docker")
        .arg("--meta-plugin-info")
        .output()
        .unwrap();

    let info: PluginInfo = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(info.name, "docker");
}
```

## Publishing Plugins

### Via Registry

```bash
# Search for plugins
meta plugin search docker

# Install from registry
meta plugin install meta-docker
```

### Manual Distribution

1. Build for target platforms
2. Distribute binaries or packages
3. Users place in `$META_DATA_DIR/plugins/` (default `~/.meta/plugins/`) or PATH

## Troubleshooting

### Plugin Not Detected

- Ensure executable permissions: `chmod +x meta-plugin`
- Check naming: must start with `meta-`
- Verify location: `.meta/plugins/` next to a meta config of this workspace, `$META_DATA_DIR/plugins/` (default `~/.meta/plugins/`), or PATH
- Make sure it answers `--meta-plugin-info` within 5 seconds
- Use `META_DEBUG=1` for debug output

### Protocol Errors

- Validate JSON format with `jq`
- Check for proper stdout/stderr separation
- Ensure newline after JSON output

### Permission Issues

- Check file permissions
- Verify PATH includes plugin directory

## See Also

- [Architecture Overview](architecture_overview.md)
- [Advanced Usage Guide](advanced_usage.md)
- [Plugin Help & Clone Design](plugin_help_and_clone_design.md)
- [FAQ / Troubleshooting](faq_troubleshooting.md)
