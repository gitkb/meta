//! MCP Server for meta - exposes multi-repo operations to AI tools.
//!
//! This server implements the Model Context Protocol (MCP) to allow AI assistants
//! like Claude to interact with meta repositories.

use anyhow::{Context, Result};
use meta_cli::dependency_graph::{self, ProjectDependencies};
use meta_cli::git_utils;
use meta_cli::query::{Query, RepoState, WorkspaceState};
use meta_core::config::{self, ProjectInfo};
use serde::{Deserialize, Serialize};
use std::io::{BufRead, Read, Write};
use std::path::PathBuf;
use std::process::Command;

/// MCP Protocol version
const PROTOCOL_VERSION: &str = "2024-11-05";

/// Server information
const SERVER_NAME: &str = "meta-mcp";
const SERVER_VERSION: &str = "0.1.0";

/// The longest request line read; a longer one gets an error reply instead of
/// filling memory.
const MAX_LINE_BYTES: u64 = 10 * 1024 * 1024;

// ============================================================================
// MCP Protocol Types
// ============================================================================

#[derive(Debug, Serialize, Deserialize)]
struct JsonRpcRequest {
    jsonrpc: String,
    /// `None` only when the member is absent (a notification); `"id": null` is `Some(Null)`.
    #[serde(default, deserialize_with = "present")]
    id: Option<serde_json::Value>,
    method: String,
    #[serde(default)]
    params: serde_json::Value,
}

fn present<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<serde_json::Value>, D::Error> {
    serde_json::Value::deserialize(d).map(Some)
}

#[derive(Debug, Serialize)]
struct JsonRpcResponse {
    jsonrpc: String,
    id: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<JsonRpcError>,
}

/// Writes `response` as one line of JSON.
fn write_line(output: &mut impl Write, response: &JsonRpcResponse) -> Result<()> {
    serde_json::to_writer(&mut *output, response)?;
    Ok(output.write_all(b"\n")?)
}

/// JSON-RPC 2.0 ids are strings, numbers or null.
fn valid_id(id: &serde_json::Value) -> bool {
    id.is_null() || id.is_string() || id.is_number()
}

#[derive(Debug, Serialize)]
struct JsonRpcError {
    code: i32,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<serde_json::Value>,
}

#[derive(Debug, Serialize)]
struct ServerInfo {
    name: String,
    version: String,
}

#[derive(Debug, Serialize)]
struct InitializeResult {
    #[serde(rename = "protocolVersion")]
    protocol_version: String,
    capabilities: ServerCapabilities,
    #[serde(rename = "serverInfo")]
    server_info: ServerInfo,
}

#[derive(Debug, Serialize)]
struct ServerCapabilities {
    tools: ToolsCapability,
}

#[derive(Debug, Serialize)]
struct ToolsCapability {
    #[serde(rename = "listChanged")]
    list_changed: bool,
}

#[derive(Debug, Serialize)]
struct Tool {
    name: String,
    description: String,
    #[serde(rename = "inputSchema")]
    input_schema: serde_json::Value,
}

#[derive(Debug, Serialize)]
struct ListToolsResult {
    tools: Vec<Tool>,
}

#[derive(Debug, Serialize)]
struct CallToolResult {
    content: Vec<ToolContent>,
    #[serde(rename = "isError", skip_serializing_if = "Option::is_none")]
    is_error: Option<bool>,
}

#[derive(Debug, Serialize)]
struct ToolContent {
    #[serde(rename = "type")]
    content_type: String,
    text: String,
}

// ============================================================================
// Meta-specific Types
// ============================================================================

// ============================================================================
// MCP Server
// ============================================================================

struct McpServer {
    meta_dir: Option<PathBuf>,
}

impl McpServer {
    fn new() -> Result<Self> {
        // Find .meta.yaml in current directory or parents
        let cwd = std::env::current_dir()?;
        let meta_dir = config::nearest_meta_dir(&cwd)?;
        Ok(Self { meta_dir })
    }

    fn run(&self) -> Result<()> {
        self.serve(std::io::stdin().lock(), std::io::stdout().lock())
    }

    /// Answers each newline-delimited JSON-RPC message from `input`. Bad input
    /// gets an error reply and serving goes on; it stops at end of input or
    /// when `output` fails.
    fn serve(&self, mut input: impl BufRead, mut output: impl Write) -> Result<()> {
        let mut line = Vec::new();
        loop {
            line.clear();
            if (&mut input)
                .take(MAX_LINE_BYTES + 1)
                .read_until(b'\n', &mut line)?
                == 0
            {
                return Ok(());
            }
            if line.len() as u64 > MAX_LINE_BYTES && !line.ends_with(b"\n") {
                input.skip_until(b'\n')?;
                write_line(
                    &mut output,
                    &self.parse_error("Request too large".to_string()),
                )?;
            } else {
                self.handle_line(&line, &mut output)?;
            }
            output.flush()?;
        }
    }

    /// Writes the reply to one line: a request, a notification (no reply), or
    /// a batch, whose replies are written one by one into a JSON array.
    fn handle_line(&self, line: &[u8], output: &mut impl Write) -> Result<()> {
        let Ok(text) = std::str::from_utf8(line) else {
            return write_line(
                output,
                &self.parse_error("Request is not valid UTF-8".to_string()),
            );
        };
        // A blank line holds no message: it is framing, not a request.
        if text.trim().is_empty() {
            return Ok(());
        }
        match serde_json::from_str(text) {
            Err(e) => write_line(output, &self.parse_error(e.to_string())),
            Ok(serde_json::Value::Array(batch)) if batch.is_empty() => write_line(
                output,
                &self.error_response(
                    Some(serde_json::Value::Null),
                    -32600,
                    "Invalid Request: empty batch".to_string(),
                ),
            ),
            Ok(serde_json::Value::Array(batch)) => {
                let mut separator = b"[";
                for response in batch.into_iter().filter_map(|m| self.handle_message(m)) {
                    output.write_all(separator)?;
                    serde_json::to_writer(&mut *output, &response)?;
                    separator = b",";
                }
                if separator == b"," {
                    output.write_all(b"]\n")?;
                }
                Ok(())
            }
            Ok(message) => match self.handle_message(message) {
                Some(response) => write_line(output, &response),
                None => Ok(()),
            },
        }
    }

    /// Replies to one JSON-RPC message, or `None` for a valid notification.
    fn handle_message(&self, message: serde_json::Value) -> Option<JsonRpcResponse> {
        let id = message.get("id").filter(|id| valid_id(id)).cloned();
        let invalid = |reason: &str| {
            Some(self.error_response(
                Some(id.clone().unwrap_or(serde_json::Value::Null)),
                -32600,
                format!("Invalid Request: {reason}"),
            ))
        };
        if !message.is_object() {
            return invalid("not an object");
        }
        if message.get("jsonrpc") != Some(&serde_json::json!("2.0")) {
            return invalid("jsonrpc must be \"2.0\"");
        }
        if message.get("id").is_some_and(|id| !valid_id(id)) {
            return invalid("id must be a string, number or null");
        }
        if message
            .get("params")
            .is_some_and(|params| !(params.is_object() || params.is_array()))
        {
            return invalid("params must be an object or array");
        }
        match serde_json::from_value(message) {
            Ok(request) => self.handle_request(&request),
            Err(e) => invalid(&e.to_string()),
        }
    }

    fn parse_error(&self, message: String) -> JsonRpcResponse {
        self.error_response(
            Some(serde_json::Value::Null),
            -32700,
            format!("Parse error: {message}"),
        )
    }

    /// Returns `None` for a notification (no `id`): JSON-RPC never answers one,
    /// and this server acts on none of them.
    fn handle_request(&self, request: &JsonRpcRequest) -> Option<JsonRpcResponse> {
        let id = request.id.clone()?;
        let result = match request.method.as_str() {
            "initialize" => self.handle_initialize(),
            "tools/list" => self.handle_list_tools(),
            "tools/call" => self.handle_call_tool(&request.params),
            _ => {
                return Some(self.error_response(
                    Some(id),
                    -32601,
                    format!("Method not found: {}", request.method),
                ))
            }
        };

        Some(match result {
            Ok(value) => self.ok_response(Some(id), value),
            Err(e) => self.error_response(Some(id), -32603, e.to_string()),
        })
    }

    fn ok_response(
        &self,
        id: Option<serde_json::Value>,
        result: serde_json::Value,
    ) -> JsonRpcResponse {
        JsonRpcResponse {
            jsonrpc: "2.0".to_string(),
            id,
            result: Some(result),
            error: None,
        }
    }

    fn error_response(
        &self,
        id: Option<serde_json::Value>,
        code: i32,
        message: String,
    ) -> JsonRpcResponse {
        JsonRpcResponse {
            jsonrpc: "2.0".to_string(),
            id,
            result: None,
            error: Some(JsonRpcError {
                code,
                message,
                data: None,
            }),
        }
    }

    fn handle_initialize(&self) -> Result<serde_json::Value> {
        let result = InitializeResult {
            protocol_version: PROTOCOL_VERSION.to_string(),
            capabilities: ServerCapabilities {
                tools: ToolsCapability {
                    list_changed: false,
                },
            },
            server_info: ServerInfo {
                name: SERVER_NAME.to_string(),
                version: SERVER_VERSION.to_string(),
            },
        };
        Ok(serde_json::to_value(result)?)
    }

    fn handle_list_tools(&self) -> Result<serde_json::Value> {
        let tools = vec![
            // ================================================================
            // Core Tools
            // ================================================================
            Tool {
                name: "meta_list_projects".to_string(),
                description: "List all projects in the meta repository".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "tag": {
                            "type": "string",
                            "description": "Filter projects by tag"
                        }
                    }
                }),
            },
            Tool {
                name: "meta_exec".to_string(),
                description: "Execute a command across all meta projects (meta exec)".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "command": {
                            "type": "array",
                            "items": { "type": "string" },
                            "description": "The program and its arguments, one per item, e.g. [\"git\", \"log\", \"--format=%h %s\"]. They run as given, with no shell; for a shell, ask for one: [\"sh\", \"-c\", \"...\"]"
                        },
                        "tag": {
                            "type": "string",
                            "description": "Filter projects by tag"
                        }
                    },
                    "required": ["command"]
                }),
            },
            Tool {
                name: "meta_get_config".to_string(),
                description: "Get the meta repository configuration including all projects, tags, and settings".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {}
                }),
            },
            Tool {
                name: "meta_get_project_path".to_string(),
                description: "Get the absolute path for a specific project".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "project": {
                            "type": "string",
                            "description": "Name of the project"
                        }
                    },
                    "required": ["project"]
                }),
            },
            // ================================================================
            // Multi-Repo Git Tools (Phase 5.1)
            // ================================================================
            Tool {
                name: "meta_git_status".to_string(),
                description: "Get git status for all projects with structured output showing dirty/clean state, branch, and changes".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "project": {
                            "type": "string",
                            "description": "Specific project to check (optional, defaults to all)"
                        },
                        "tag": {
                            "type": "string",
                            "description": "Filter projects by tag"
                        }
                    }
                }),
            },
            Tool {
                name: "meta_git_pull".to_string(),
                description: "Pull changes from remote for all projects or filtered by tag".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "tag": {
                            "type": "string",
                            "description": "Filter projects by tag"
                        },
                        "rebase": {
                            "type": "boolean",
                            "description": "Use rebase instead of merge (default: false)"
                        }
                    }
                }),
            },
            Tool {
                name: "meta_git_push".to_string(),
                description: "Push commits to remote for all projects or filtered by tag".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "tag": {
                            "type": "string",
                            "description": "Filter projects by tag"
                        }
                    }
                }),
            },
            Tool {
                name: "meta_git_fetch".to_string(),
                description: "Fetch from remotes for all projects in parallel".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "tag": {
                            "type": "string",
                            "description": "Filter projects by tag"
                        }
                    }
                }),
            },
            Tool {
                name: "meta_git_diff".to_string(),
                description: "Get diffs across repositories showing what has changed".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "project": {
                            "type": "string",
                            "description": "Specific project to diff (optional, defaults to all)"
                        },
                        "staged": {
                            "type": "boolean",
                            "description": "Show only staged changes (default: false shows unstaged)"
                        },
                        "tag": {
                            "type": "string",
                            "description": "Filter projects by tag"
                        }
                    }
                }),
            },
            Tool {
                name: "meta_git_branch".to_string(),
                description: "Get branch information for all projects including current branch, tracking branch, and ahead/behind status".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "tag": {
                            "type": "string",
                            "description": "Filter projects by tag"
                        }
                    }
                }),
            },
            Tool {
                name: "meta_git_add".to_string(),
                description: "Stage files across repositories".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "project": {
                            "type": "string",
                            "description": "Specific project to add files in (optional, defaults to all)"
                        },
                        "files": {
                            "type": "array",
                            "items": { "type": "string" },
                            "description": "Paths to add, one per item, passed to git as given (default: [\".\"] for all changed files)"
                        },
                        "tag": {
                            "type": "string",
                            "description": "Filter projects by tag"
                        }
                    }
                }),
            },
            Tool {
                name: "meta_git_commit".to_string(),
                description: "Commit staged changes across repositories with a shared message".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "message": {
                            "type": "string",
                            "description": "Commit message"
                        },
                        "project": {
                            "type": "string",
                            "description": "Specific project to commit in (optional, defaults to all with staged changes)"
                        },
                        "tag": {
                            "type": "string",
                            "description": "Filter projects by tag"
                        }
                    },
                    "required": ["message"]
                }),
            },
            Tool {
                name: "meta_git_checkout".to_string(),
                description: "Checkout a branch across repositories".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "branch": {
                            "type": "string",
                            "description": "Branch name to checkout"
                        },
                        "create": {
                            "type": "boolean",
                            "description": "Create the branch if it doesn't exist (default: false)"
                        },
                        "tag": {
                            "type": "string",
                            "description": "Filter projects by tag"
                        }
                    },
                    "required": ["branch"]
                }),
            },
            Tool {
                name: "meta_git_multi_commit".to_string(),
                description: "Create commits with different messages for each repository. Allows tailored commit messages per project.".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "commits": {
                            "type": "array",
                            "description": "Array of commit objects, each specifying a project and message",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "project": {
                                        "type": "string",
                                        "description": "Project name (use '.' for root repo)"
                                    },
                                    "message": {
                                        "type": "string",
                                        "description": "Commit message for this project"
                                    }
                                },
                                "required": ["project", "message"]
                            }
                        }
                    },
                    "required": ["commits"]
                }),
            },
            // ================================================================
            // Build/Test Orchestration Tools (Phase 5.2)
            // ================================================================
            Tool {
                name: "meta_detect_build_systems".to_string(),
                description: "Detect build systems (Cargo, npm, make, etc.) for each project".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "tag": {
                            "type": "string",
                            "description": "Filter projects by tag"
                        }
                    }
                }),
            },
            Tool {
                name: "meta_run_tests".to_string(),
                description: "Run tests across all projects using detected build systems".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "tag": {
                            "type": "string",
                            "description": "Filter projects by tag"
                        },
                        "project": {
                            "type": "string",
                            "description": "Specific project to test"
                        }
                    }
                }),
            },
            Tool {
                name: "meta_build".to_string(),
                description: "Build all projects using detected build systems".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "tag": {
                            "type": "string",
                            "description": "Filter projects by tag"
                        },
                        "release": {
                            "type": "boolean",
                            "description": "Build in release mode (default: false)"
                        }
                    }
                }),
            },
            Tool {
                name: "meta_clean".to_string(),
                description: "Clean build artifacts across all projects".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "tag": {
                            "type": "string",
                            "description": "Filter projects by tag"
                        }
                    }
                }),
            },
            // ================================================================
            // Project Discovery & Analysis Tools (Phase 5.3)
            // ================================================================
            Tool {
                name: "meta_search_code".to_string(),
                description: "Search for patterns across all repositories using grep".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "pattern": {
                            "type": "string",
                            "description": "Search pattern (regex supported)"
                        },
                        "file_pattern": {
                            "type": "string",
                            "description": "File glob pattern to filter (e.g., '*.rs', '*.ts')"
                        },
                        "tag": {
                            "type": "string",
                            "description": "Filter projects by tag"
                        }
                    },
                    "required": ["pattern"]
                }),
            },
            Tool {
                name: "meta_get_file_tree".to_string(),
                description: "Get file tree structure for projects".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "project": {
                            "type": "string",
                            "description": "Specific project to get tree for (optional, defaults to all)"
                        },
                        "depth": {
                            "type": "integer",
                            "description": "Maximum depth to traverse (default: 3)"
                        },
                        "tag": {
                            "type": "string",
                            "description": "Filter projects by tag"
                        }
                    }
                }),
            },
            Tool {
                name: "meta_list_plugins".to_string(),
                description: "List all installed meta plugins and their commands".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {}
                }),
            },
            // ================================================================
            // AI-Dominance Tools (Phase 9)
            // ================================================================
            Tool {
                name: "meta_query_repos".to_string(),
                description: "Query repositories by state/criteria using a simple DSL. Examples: 'dirty:true', 'tag:backend', 'dirty:true AND branch:main', 'modified_in:24h'".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "query": {
                            "type": "string",
                            "description": "Query string using DSL (e.g., 'dirty:true AND tag:backend')"
                        }
                    },
                    "required": ["query"]
                }),
            },
            Tool {
                name: "meta_workspace_state".to_string(),
                description: "Get a summary of the entire workspace state including dirty/clean counts, branches, and tags".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {}
                }),
            },
            Tool {
                name: "meta_analyze_impact".to_string(),
                description: "Analyze what would be affected if a project changes. Returns direct and transitive dependents.".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "project": {
                            "type": "string",
                            "description": "Project name to analyze impact for"
                        }
                    },
                    "required": ["project"]
                }),
            },
            Tool {
                name: "meta_execution_order".to_string(),
                description: "Get topological execution order respecting dependencies. Dependencies come before dependents.".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "tag": {
                            "type": "string",
                            "description": "Filter by tag (optional)"
                        }
                    }
                }),
            },
            Tool {
                name: "meta_batch_execute".to_string(),
                description: "Execute a command across projects, optionally rolling back the projects it ran in if it fails".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "command": {
                            "type": "array",
                            "items": { "type": "string" },
                            "description": "The program and its arguments to run in each project, one per item. They run as given, with no shell; for a shell, ask for one: [\"sh\", \"-c\", \"...\"]"
                        },
                        "tag": {
                            "type": "string",
                            "description": "Filter projects by tag (optional)"
                        },
                        "atomic": {
                            "type": "boolean",
                            "description": "If true, refuse to start unless every target project is the root of a clean git repository; on the first failure, stop and put each project the command reached back on its branch and commit from before. Its uncommitted changes and untracked files there are saved in a git stash, and the commits HEAD and the branch were on are reported, all by id in the result. Ignored files, submodules, other refs and effects outside the repositories are not undone. Projects it never reached are not touched (default: false)"
                        }
                    },
                    "required": ["command"]
                }),
            },
        ];

        let result = ListToolsResult { tools };
        Ok(serde_json::to_value(result)?)
    }

    fn handle_call_tool(&self, params: &serde_json::Value) -> Result<serde_json::Value> {
        let name = params
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing tool name"))?;

        let arguments = params
            .get("arguments")
            .cloned()
            .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));

        let result = match name {
            // Core tools
            "meta_list_projects" => self.tool_list_projects(&arguments),
            "meta_exec" => self.tool_exec(&arguments),
            "meta_get_config" => self.tool_get_config(&arguments),
            "meta_get_project_path" => self.tool_get_project_path(&arguments),
            // Git tools
            "meta_git_status" => self.tool_git_status(&arguments),
            "meta_git_pull" => self.tool_git_pull(&arguments),
            "meta_git_push" => self.tool_git_push(&arguments),
            "meta_git_fetch" => self.tool_git_fetch(&arguments),
            "meta_git_diff" => self.tool_git_diff(&arguments),
            "meta_git_branch" => self.tool_git_branch(&arguments),
            "meta_git_add" => self.tool_git_add(&arguments),
            "meta_git_commit" => self.tool_git_commit(&arguments),
            "meta_git_checkout" => self.tool_git_checkout(&arguments),
            "meta_git_multi_commit" => self.tool_git_multi_commit(&arguments),
            // Build/test tools
            "meta_detect_build_systems" => self.tool_detect_build_systems(&arguments),
            "meta_run_tests" => self.tool_run_tests(&arguments),
            "meta_build" => self.tool_build(&arguments),
            "meta_clean" => self.tool_clean(&arguments),
            // Discovery tools
            "meta_search_code" => self.tool_search_code(&arguments),
            "meta_get_file_tree" => self.tool_get_file_tree(&arguments),
            "meta_list_plugins" => self.tool_list_plugins(&arguments),
            // AI-Dominance tools
            "meta_query_repos" => self.tool_query_repos(&arguments),
            "meta_workspace_state" => self.tool_workspace_state(&arguments),
            "meta_analyze_impact" => self.tool_analyze_impact(&arguments),
            "meta_execution_order" => self.tool_execution_order(&arguments),
            "meta_batch_execute" => self.tool_batch_execute(&arguments),
            _ => Err(anyhow::anyhow!("Unknown tool: {name}")),
        };

        match result {
            Ok(text) => {
                let call_result = CallToolResult {
                    content: vec![ToolContent {
                        content_type: "text".to_string(),
                        text,
                    }],
                    is_error: None,
                };
                Ok(serde_json::to_value(call_result)?)
            }
            Err(e) => {
                let call_result = CallToolResult {
                    content: vec![ToolContent {
                        content_type: "text".to_string(),
                        text: e.to_string(),
                    }],
                    is_error: Some(true),
                };
                Ok(serde_json::to_value(call_result)?)
            }
        }
    }

    fn tool_list_projects(&self, args: &serde_json::Value) -> Result<String> {
        let meta_dir = self
            .meta_dir
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("No meta repository found"))?;

        let projects = self.load_projects(meta_dir)?;

        // Filter by tag if specified
        let tag_filter = args.get("tag").and_then(|v| v.as_str());
        let filtered: Vec<&ProjectInfo> = if let Some(tag) = tag_filter {
            projects
                .iter()
                .filter(|p| p.tags.contains(&tag.to_string()))
                .collect()
        } else {
            projects.iter().collect()
        };

        let mut output = format!(
            "Found {} project(s) in {}\n\n",
            filtered.len(),
            meta_dir.display()
        );
        for project in filtered {
            output.push_str(&format!("- {} ({})\n", project.name, project.path));
            if !project.tags.is_empty() {
                output.push_str(&format!("  Tags: {}\n", project.tags.join(", ")));
            }
        }

        Ok(output)
    }

    fn tool_git_status(&self, args: &serde_json::Value) -> Result<String> {
        let meta_dir = self
            .meta_dir
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("No meta repository found"))?;

        let project_filter = args.get("project").and_then(|v| v.as_str());

        // Run meta git status with --json for structured output
        let mut cmd = Command::new("meta");
        cmd.arg("--json").arg("git").arg("status");
        cmd.current_dir(meta_dir);

        let output = cmd.output().context("Failed to execute meta git status")?;

        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);

            // If project filter specified, parse JSON and filter
            if let Some(project) = project_filter {
                if let Ok(json) = serde_json::from_str::<serde_json::Value>(&stdout) {
                    if let Some(results) = json.get("results").and_then(|r| r.as_array()) {
                        for result in results {
                            if result.get("project").and_then(|p| p.as_str()) == Some(project) {
                                return Ok(serde_json::to_string_pretty(result)?);
                            }
                        }
                        return Err(anyhow::anyhow!("Project '{project}' not found"));
                    }
                }
            }

            Ok(stdout.to_string())
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            Err(anyhow::anyhow!("meta git status failed: {stderr}"))
        }
    }

    fn tool_exec(&self, args: &serde_json::Value) -> Result<String> {
        let meta_dir = self
            .meta_dir
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("No meta repository found"))?;

        let command = string_list(args, "command")?
            .filter(|argv| !argv.is_empty())
            .ok_or_else(|| anyhow::anyhow!("Missing 'command' argument"))?;

        let mut cmd = Command::new("meta");
        cmd.arg("--json");

        if let Some(tag) = args.get("tag").and_then(|v| v.as_str()) {
            cmd.arg("--tag").arg(tag);
        }

        cmd.arg("exec").arg("--").args(&command);

        cmd.current_dir(meta_dir);

        let output = cmd.output().context("Failed to execute meta command")?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        if output.status.success() {
            Ok(stdout.to_string())
        } else {
            Err(anyhow::anyhow!("Command failed:\n{stdout}\n{stderr}"))
        }
    }

    // ========================================================================
    // Core Tools
    // ========================================================================

    fn tool_get_config(&self, _args: &serde_json::Value) -> Result<String> {
        let meta_dir = self
            .meta_dir
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("No meta repository found"))?;

        // Find and return the raw config
        let path = config::find_meta_config_in(meta_dir)?
            .ok_or_else(|| anyhow::anyhow!("No meta config found"))?;
        let content = std::fs::read_to_string(&path)?;
        Ok(format!("Config file: {}\n\n{}", path.display(), content))
    }

    fn tool_get_project_path(&self, args: &serde_json::Value) -> Result<String> {
        let meta_dir = self
            .meta_dir
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("No meta repository found"))?;

        let project_name = args
            .get("project")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing 'project' argument"))?;

        let projects = self.load_projects(meta_dir)?;

        for project in projects {
            if project.name == project_name {
                let full_path = meta_dir.join(&project.path);
                return Ok(serde_json::json!({
                    "project": project.name,
                    "path": full_path.display().to_string(),
                    "exists": full_path.exists()
                })
                .to_string());
            }
        }

        Err(anyhow::anyhow!("Project '{project_name}' not found"))
    }

    // ========================================================================
    // Git Tools
    // ========================================================================

    fn tool_git_pull(&self, args: &serde_json::Value) -> Result<String> {
        let meta_dir = self
            .meta_dir
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("No meta repository found"))?;

        let mut cmd = Command::new("meta");
        cmd.arg("--json");

        if let Some(tag) = args.get("tag").and_then(|v| v.as_str()) {
            cmd.arg("--tag").arg(tag);
        }

        cmd.arg("git").arg("pull");

        if args
            .get("rebase")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
        {
            cmd.arg("--rebase");
        }

        cmd.current_dir(meta_dir);

        let output = cmd.output().context("Failed to execute meta git pull")?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        if output.status.success() {
            Ok(stdout.to_string())
        } else {
            Err(anyhow::anyhow!("git pull failed:\n{stdout}\n{stderr}"))
        }
    }

    fn tool_git_push(&self, args: &serde_json::Value) -> Result<String> {
        let meta_dir = self
            .meta_dir
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("No meta repository found"))?;

        let mut cmd = Command::new("meta");
        cmd.arg("--json");

        if let Some(tag) = args.get("tag").and_then(|v| v.as_str()) {
            cmd.arg("--tag").arg(tag);
        }

        cmd.arg("git").arg("push");
        cmd.current_dir(meta_dir);

        let output = cmd.output().context("Failed to execute meta git push")?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        if output.status.success() {
            Ok(stdout.to_string())
        } else {
            Err(anyhow::anyhow!("git push failed:\n{stdout}\n{stderr}"))
        }
    }

    fn tool_git_fetch(&self, args: &serde_json::Value) -> Result<String> {
        let meta_dir = self
            .meta_dir
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("No meta repository found"))?;

        let mut cmd = Command::new("meta");
        cmd.arg("--json");

        if let Some(tag) = args.get("tag").and_then(|v| v.as_str()) {
            cmd.arg("--tag").arg(tag);
        }

        cmd.arg("git").arg("fetch");
        cmd.current_dir(meta_dir);

        let output = cmd.output().context("Failed to execute meta git fetch")?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        if output.status.success() {
            Ok(stdout.to_string())
        } else {
            Err(anyhow::anyhow!("git fetch failed:\n{stdout}\n{stderr}"))
        }
    }

    fn tool_git_diff(&self, args: &serde_json::Value) -> Result<String> {
        let meta_dir = self
            .meta_dir
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("No meta repository found"))?;

        let projects = self.load_projects(meta_dir)?;
        let project_filter = args.get("project").and_then(|v| v.as_str());
        let tag_filter = args.get("tag").and_then(|v| v.as_str());
        let staged = args
            .get("staged")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let filtered: Vec<&ProjectInfo> = projects
            .iter()
            .filter(|p| {
                if let Some(project) = project_filter {
                    return p.name == project;
                }
                if let Some(tag) = tag_filter {
                    return p.tags.contains(&tag.to_string());
                }
                true
            })
            .collect();

        let mut results = Vec::new();

        for project in filtered {
            let project_path = meta_dir.join(&project.path);
            if !project_path.exists() {
                continue;
            }

            let mut cmd = Command::new("git");
            cmd.arg("diff");
            if staged {
                cmd.arg("--staged");
            }
            cmd.current_dir(&project_path);

            let output = cmd.output()?;
            let diff = String::from_utf8_lossy(&output.stdout);

            if !diff.is_empty() {
                results.push(serde_json::json!({
                    "project": project.name,
                    "diff": diff.to_string()
                }));
            }
        }

        Ok(serde_json::to_string_pretty(&results)?)
    }

    fn tool_git_branch(&self, args: &serde_json::Value) -> Result<String> {
        let meta_dir = self
            .meta_dir
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("No meta repository found"))?;

        let projects = self.load_projects(meta_dir)?;
        let tag_filter = args.get("tag").and_then(|v| v.as_str());

        let filtered: Vec<&ProjectInfo> = if let Some(tag) = tag_filter {
            projects
                .iter()
                .filter(|p| p.tags.contains(&tag.to_string()))
                .collect()
        } else {
            projects.iter().collect()
        };

        let mut results = Vec::new();

        for project in filtered {
            let project_path = meta_dir.join(&project.path);
            if !project_path.exists() {
                continue;
            }

            // Get current branch
            let current_branch =
                git_utils::current_branch(&project_path).unwrap_or_else(|| "HEAD".to_string());

            // Get tracking branch info
            let tracking_output = Command::new("git")
                .args([
                    "for-each-ref",
                    "--format=%(upstream:short)",
                    &format!("refs/heads/{current_branch}"),
                ])
                .current_dir(&project_path)
                .output()?;
            let tracking_branch = String::from_utf8_lossy(&tracking_output.stdout)
                .trim()
                .to_string();

            // Get ahead/behind counts
            let mut ahead = 0;
            let mut behind = 0;
            if !tracking_branch.is_empty() {
                let ahead_behind = Command::new("git")
                    .args([
                        "rev-list",
                        "--left-right",
                        "--count",
                        &format!("{current_branch}...{tracking_branch}"),
                    ])
                    .current_dir(&project_path)
                    .output()?;
                let counts = String::from_utf8_lossy(&ahead_behind.stdout);
                let parts: Vec<&str> = counts.trim().split('\t').collect();
                if parts.len() == 2 {
                    ahead = parts[0].parse().unwrap_or(0);
                    behind = parts[1].parse().unwrap_or(0);
                }
            }

            results.push(serde_json::json!({
                "project": project.name,
                "branch": current_branch,
                "tracking": if tracking_branch.is_empty() { None } else { Some(&tracking_branch) },
                "ahead": ahead,
                "behind": behind
            }));
        }

        Ok(serde_json::to_string_pretty(&results)?)
    }

    fn tool_git_add(&self, args: &serde_json::Value) -> Result<String> {
        let meta_dir = self
            .meta_dir
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("No meta repository found"))?;

        let files = match string_list(args, "files")? {
            None => vec![".".to_string()],
            Some(files) if files.is_empty() => {
                anyhow::bail!("'files' is empty: name the paths to add")
            }
            Some(files) => files,
        };

        let mut cmd = Command::new("meta");
        cmd.arg("--json");

        if let Some(tag) = args.get("tag").and_then(|v| v.as_str()) {
            cmd.arg("--tag").arg(tag);
        }

        cmd.arg("git").arg("add").arg("--").args(&files);
        cmd.current_dir(meta_dir);

        let output = cmd.output().context("Failed to execute meta git add")?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        if output.status.success() {
            Ok(format!("Staged files: {}\n{stdout}", files.join(", ")))
        } else {
            Err(anyhow::anyhow!("git add failed:\n{stdout}\n{stderr}"))
        }
    }

    fn tool_git_commit(&self, args: &serde_json::Value) -> Result<String> {
        let meta_dir = self
            .meta_dir
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("No meta repository found"))?;

        let message = args
            .get("message")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing 'message' argument"))?;

        let mut cmd = Command::new("meta");
        cmd.arg("--json");

        if let Some(tag) = args.get("tag").and_then(|v| v.as_str()) {
            cmd.arg("--tag").arg(tag);
        }

        cmd.arg("git").arg("commit").arg("-m").arg(message);
        cmd.current_dir(meta_dir);

        let output = cmd.output().context("Failed to execute meta git commit")?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        if output.status.success() {
            Ok(stdout.to_string())
        } else {
            Err(anyhow::anyhow!("git commit failed:\n{stdout}\n{stderr}"))
        }
    }

    fn tool_git_multi_commit(&self, args: &serde_json::Value) -> Result<String> {
        let meta_dir = self
            .meta_dir
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("No meta repository found"))?;

        let commits = args
            .get("commits")
            .and_then(|v| v.as_array())
            .ok_or_else(|| anyhow::anyhow!("Missing 'commits' argument"))?;

        #[derive(Debug, Serialize)]
        struct CommitResult {
            project: String,
            success: bool,
            message: String,
            #[serde(skip_serializing_if = "Option::is_none")]
            error: Option<String>,
        }

        let mut results: Vec<CommitResult> = Vec::new();

        for commit_obj in commits {
            let project = commit_obj
                .get("project")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow::anyhow!("Missing 'project' in commit entry"))?;

            let message = commit_obj
                .get("message")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow::anyhow!("Missing 'message' in commit entry"))?;

            // Determine the path for this project
            let project_path = if project == "." {
                meta_dir.clone()
            } else {
                meta_dir.join(project)
            };

            if !project_path.exists() {
                results.push(CommitResult {
                    project: project.to_string(),
                    success: false,
                    message: message.to_string(),
                    error: Some(format!(
                        "Project path does not exist: {}",
                        project_path.display()
                    )),
                });
                continue;
            }

            // Execute git commit for this project
            let output = Command::new("git")
                .arg("-C")
                .arg(&project_path)
                .arg("commit")
                .arg("-m")
                .arg(message)
                .output();

            match output {
                Ok(out) => {
                    if out.status.success() {
                        results.push(CommitResult {
                            project: project.to_string(),
                            success: true,
                            message: message.to_string(),
                            error: None,
                        });
                    } else {
                        let stderr = String::from_utf8_lossy(&out.stderr);
                        results.push(CommitResult {
                            project: project.to_string(),
                            success: false,
                            message: message.to_string(),
                            error: Some(stderr.trim().to_string()),
                        });
                    }
                }
                Err(e) => {
                    results.push(CommitResult {
                        project: project.to_string(),
                        success: false,
                        message: message.to_string(),
                        error: Some(e.to_string()),
                    });
                }
            }
        }

        let succeeded = results.iter().filter(|r| r.success).count();
        let failed = results.iter().filter(|r| !r.success).count();

        let output = serde_json::json!({
            "results": results,
            "summary": {
                "total": results.len(),
                "succeeded": succeeded,
                "failed": failed
            }
        });

        Ok(serde_json::to_string_pretty(&output)?)
    }

    fn tool_git_checkout(&self, args: &serde_json::Value) -> Result<String> {
        let meta_dir = self
            .meta_dir
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("No meta repository found"))?;

        let branch = args
            .get("branch")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing 'branch' argument"))?;

        let create = args
            .get("create")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let mut cmd = Command::new("meta");
        cmd.arg("--json");

        if let Some(tag) = args.get("tag").and_then(|v| v.as_str()) {
            cmd.arg("--tag").arg(tag);
        }

        cmd.arg("git").arg("checkout");
        if create {
            cmd.arg("-b");
        }
        cmd.arg(branch);
        cmd.current_dir(meta_dir);

        let output = cmd
            .output()
            .context("Failed to execute meta git checkout")?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        if output.status.success() {
            Ok(stdout.to_string())
        } else {
            Err(anyhow::anyhow!("git checkout failed:\n{stdout}\n{stderr}"))
        }
    }

    // ========================================================================
    // Build/Test Tools
    // ========================================================================

    fn tool_detect_build_systems(&self, args: &serde_json::Value) -> Result<String> {
        let meta_dir = self
            .meta_dir
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("No meta repository found"))?;

        let projects = self.load_projects(meta_dir)?;
        let tag_filter = args.get("tag").and_then(|v| v.as_str());

        let filtered: Vec<&ProjectInfo> = if let Some(tag) = tag_filter {
            projects
                .iter()
                .filter(|p| p.tags.contains(&tag.to_string()))
                .collect()
        } else {
            projects.iter().collect()
        };

        let mut results = Vec::new();

        for project in filtered {
            let project_path = meta_dir.join(&project.path);
            if !project_path.exists() {
                continue;
            }

            let mut build_systems = Vec::new();

            // Check for various build systems
            if project_path.join("Cargo.toml").exists() {
                build_systems.push("cargo");
            }
            if project_path.join("package.json").exists() {
                build_systems.push("npm");
            }
            if project_path.join("Makefile").exists() || project_path.join("makefile").exists() {
                build_systems.push("make");
            }
            if project_path.join("go.mod").exists() {
                build_systems.push("go");
            }
            if project_path.join("pom.xml").exists() {
                build_systems.push("maven");
            }
            if project_path.join("build.gradle").exists()
                || project_path.join("build.gradle.kts").exists()
            {
                build_systems.push("gradle");
            }
            if project_path.join("pyproject.toml").exists()
                || project_path.join("setup.py").exists()
            {
                build_systems.push("python");
            }

            results.push(serde_json::json!({
                "project": project.name,
                "path": project.path,
                "build_systems": build_systems
            }));
        }

        Ok(serde_json::to_string_pretty(&results)?)
    }

    fn tool_run_tests(&self, args: &serde_json::Value) -> Result<String> {
        let meta_dir = self
            .meta_dir
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("No meta repository found"))?;

        let projects = self.load_projects(meta_dir)?;
        let project_filter = args.get("project").and_then(|v| v.as_str());
        let tag_filter = args.get("tag").and_then(|v| v.as_str());

        let filtered: Vec<&ProjectInfo> = projects
            .iter()
            .filter(|p| {
                if let Some(project) = project_filter {
                    return p.name == project;
                }
                if let Some(tag) = tag_filter {
                    return p.tags.contains(&tag.to_string());
                }
                true
            })
            .collect();

        let mut results = Vec::new();

        for project in filtered {
            let project_path = meta_dir.join(&project.path);
            if !project_path.exists() {
                continue;
            }

            let Some((cmd_name, cmd_args)) = repo_command(&project_path, RepoAction::Test) else {
                continue;
            };

            let output = Command::new(cmd_name)
                .args(&cmd_args)
                .current_dir(&project_path)
                .output();

            match output {
                Ok(out) => {
                    results.push(serde_json::json!({
                        "project": project.name,
                        "command": format!("{} {}", cmd_name, cmd_args.join(" ")),
                        "success": out.status.success(),
                        "stdout": String::from_utf8_lossy(&out.stdout).to_string(),
                        "stderr": String::from_utf8_lossy(&out.stderr).to_string()
                    }));
                }
                Err(e) => {
                    results.push(serde_json::json!({
                        "project": project.name,
                        "error": e.to_string()
                    }));
                }
            }
        }

        Ok(serde_json::to_string_pretty(&results)?)
    }

    fn tool_build(&self, args: &serde_json::Value) -> Result<String> {
        let meta_dir = self
            .meta_dir
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("No meta repository found"))?;

        let release = args
            .get("release")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let projects = self.load_projects(meta_dir)?;
        let tag_filter = args.get("tag").and_then(|v| v.as_str());
        // Root builds remain included even when a tag filter selects child repos.
        let mut repos = vec![(".".to_string(), meta_dir.clone())];
        repos.extend(
            projects
                .into_iter()
                .filter(|p| tag_filter.is_none_or(|tags| has_any_tag(tags, &p.tags)))
                .map(|p| (p.name, meta_dir.join(p.path))),
        );
        let mut results = Vec::new();
        let mut failed = false;
        for (name, path) in repos {
            let Some((program, command_args)) = repo_command(&path, RepoAction::Build { release })
            else {
                continue;
            };
            match Command::new(program)
                .args(&command_args)
                .current_dir(&path)
                .output()
            {
                Ok(out) => {
                    failed |= !out.status.success();
                    results.push(serde_json::json!({
                        "project": name,
                        "command": format!("{} {}", program, command_args.join(" ")).trim_end(),
                        "success": out.status.success(),
                        "stdout": String::from_utf8_lossy(&out.stdout),
                        "stderr": String::from_utf8_lossy(&out.stderr)
                    }));
                }
                Err(e) => {
                    failed = true;
                    results.push(serde_json::json!({"project": name, "error": e.to_string()}));
                }
            }
        }
        let output = serde_json::to_string_pretty(&results)?;
        if failed {
            anyhow::bail!("build failed:\n{output}");
        }
        Ok(output)
    }

    fn tool_clean(&self, args: &serde_json::Value) -> Result<String> {
        let meta_dir = self
            .meta_dir
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("No meta repository found"))?;

        let projects = self.load_projects(meta_dir)?;
        let tag_filter = args.get("tag").and_then(|v| v.as_str());

        let filtered: Vec<&ProjectInfo> = if let Some(tag) = tag_filter {
            projects
                .iter()
                .filter(|p| p.tags.contains(&tag.to_string()))
                .collect()
        } else {
            projects.iter().collect()
        };

        let mut results = Vec::new();

        for project in filtered {
            let project_path = meta_dir.join(&project.path);
            if !project_path.exists() {
                continue;
            }

            let (cmd_name, cmd_args): (&str, Vec<&str>) =
                if project_path.join("Cargo.toml").exists() {
                    ("cargo", vec!["clean"])
                } else if project_path.join("package.json").exists() {
                    // For npm projects, we'd typically remove node_modules
                    continue; // Skip npm for now as rm -rf is dangerous
                } else if project_path.join("go.mod").exists() {
                    ("go", vec!["clean"])
                } else if project_path.join("Makefile").exists() {
                    ("make", vec!["clean"])
                } else {
                    continue;
                };

            let output = Command::new(cmd_name)
                .args(&cmd_args)
                .current_dir(&project_path)
                .output();

            match output {
                Ok(out) => {
                    results.push(serde_json::json!({
                        "project": project.name,
                        "command": format!("{} {}", cmd_name, cmd_args.join(" ")),
                        "success": out.status.success()
                    }));
                }
                Err(e) => {
                    results.push(serde_json::json!({
                        "project": project.name,
                        "error": e.to_string()
                    }));
                }
            }
        }

        Ok(serde_json::to_string_pretty(&results)?)
    }

    // ========================================================================
    // Discovery Tools
    // ========================================================================

    fn tool_search_code(&self, args: &serde_json::Value) -> Result<String> {
        let meta_dir = self
            .meta_dir
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("No meta repository found"))?;

        let pattern = args
            .get("pattern")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing 'pattern' argument"))?;

        let file_pattern = args.get("file_pattern").and_then(|v| v.as_str());

        let projects = self.load_projects(meta_dir)?;
        let tag_filter = args.get("tag").and_then(|v| v.as_str());

        let filtered: Vec<&ProjectInfo> = if let Some(tag) = tag_filter {
            projects
                .iter()
                .filter(|p| p.tags.contains(&tag.to_string()))
                .collect()
        } else {
            projects.iter().collect()
        };

        let mut results = Vec::new();

        for project in filtered {
            let project_path = meta_dir.join(&project.path);
            if !project_path.exists() {
                continue;
            }

            let mut cmd = Command::new("grep");
            cmd.args(["-r", "-n", "-I"]); // recursive, line numbers, skip binary

            if let Some(fp) = file_pattern {
                cmd.args(["--include", fp]);
            }

            cmd.arg(pattern);
            cmd.current_dir(&project_path);

            let output = cmd.output();

            match output {
                Ok(out) => {
                    let matches = String::from_utf8_lossy(&out.stdout);
                    if !matches.is_empty() {
                        results.push(serde_json::json!({
                            "project": project.name,
                            "matches": matches.lines().take(50).collect::<Vec<_>>()
                        }));
                    }
                }
                Err(_) => continue,
            }
        }

        Ok(serde_json::to_string_pretty(&results)?)
    }

    fn tool_get_file_tree(&self, args: &serde_json::Value) -> Result<String> {
        let meta_dir = self
            .meta_dir
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("No meta repository found"))?;

        let depth = args.get("depth").and_then(|v| v.as_i64()).unwrap_or(3) as usize;

        let projects = self.load_projects(meta_dir)?;
        let project_filter = args.get("project").and_then(|v| v.as_str());
        let tag_filter = args.get("tag").and_then(|v| v.as_str());

        let filtered: Vec<&ProjectInfo> = projects
            .iter()
            .filter(|p| {
                if let Some(project) = project_filter {
                    return p.name == project;
                }
                if let Some(tag) = tag_filter {
                    return p.tags.contains(&tag.to_string());
                }
                true
            })
            .collect();

        let mut results = Vec::new();

        for project in filtered {
            let project_path = meta_dir.join(&project.path);
            if !project_path.exists() {
                continue;
            }

            let tree = Self::build_file_tree(&project_path, depth, 0)?;
            results.push(serde_json::json!({
                "project": project.name,
                "tree": tree
            }));
        }

        Ok(serde_json::to_string_pretty(&results)?)
    }

    fn build_file_tree(
        path: &std::path::Path,
        max_depth: usize,
        current_depth: usize,
    ) -> Result<serde_json::Value> {
        if current_depth >= max_depth {
            return Ok(serde_json::json!(null));
        }

        if path.is_file() {
            return Ok(serde_json::json!({
                "type": "file",
                "name": path.file_name().unwrap_or_default().to_string_lossy()
            }));
        }

        let mut children = Vec::new();

        if let Ok(entries) = std::fs::read_dir(path) {
            for entry in entries.flatten() {
                let entry_path = entry.path();
                let name = entry_path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string();

                // Skip hidden files and common ignored directories
                if name.starts_with('.') || name == "node_modules" || name == "target" {
                    continue;
                }

                if entry_path.is_dir() {
                    children.push(serde_json::json!({
                        "type": "directory",
                        "name": name,
                        "children": Self::build_file_tree(&entry_path, max_depth, current_depth + 1)?
                    }));
                } else {
                    children.push(serde_json::json!({
                        "type": "file",
                        "name": name
                    }));
                }
            }
        }

        Ok(serde_json::Value::Array(children))
    }

    fn tool_list_plugins(&self, _args: &serde_json::Value) -> Result<String> {
        self.list_plugins_with(Command::new("meta"))
    }

    fn list_plugins_with(&self, mut cmd: Command) -> Result<String> {
        let meta_dir = self
            .meta_dir
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("No meta repository found"))?;

        cmd.args(["plugin", "list", "--json"]);
        cmd.current_dir(meta_dir);

        let output = cmd.output().context("Failed to list plugins")?;

        let stdout = String::from_utf8_lossy(&output.stdout);

        if output.status.success() {
            Ok(stdout.to_string())
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            Err(anyhow::anyhow!(
                "Failed to list plugins:\n{stdout}\n{stderr}"
            ))
        }
    }

    // ========================================================================
    // AI-Dominance Tools (Phase 9)
    // ========================================================================

    fn tool_query_repos(&self, args: &serde_json::Value) -> Result<String> {
        let meta_dir = self
            .meta_dir
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("No meta repository found"))?;

        let query_str = args
            .get("query")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing 'query' argument"))?;

        let query = Query::parse(query_str)?;
        let projects = self.load_projects(meta_dir)?;
        let mut matching = Vec::new();

        for project in &projects {
            let project_path = meta_dir.join(&project.path);
            if !project_path.exists() {
                continue;
            }

            let state = RepoState::collect(&project.name, &project_path, &project.tags)?;
            if state.matches(&query) {
                matching.push(state);
            }
        }

        Ok(serde_json::to_string_pretty(&serde_json::json!({
            "query": query_str,
            "matches": matching.len(),
            "projects": matching
        }))?)
    }

    fn tool_workspace_state(&self, _args: &serde_json::Value) -> Result<String> {
        let meta_dir = self
            .meta_dir
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("No meta repository found"))?;

        let projects = self.load_projects(meta_dir)?;
        let mut repo_states = Vec::new();

        for project in &projects {
            let project_path = meta_dir.join(&project.path);
            if !project_path.exists() {
                continue;
            }
            if let Ok(state) = RepoState::collect(&project.name, &project_path, &project.tags) {
                repo_states.push(state);
            }
        }

        let workspace_state = WorkspaceState::from_repos(&repo_states);
        Ok(serde_json::to_string_pretty(&workspace_state)?)
    }

    fn tool_analyze_impact(&self, args: &serde_json::Value) -> Result<String> {
        let meta_dir = self
            .meta_dir
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("No meta repository found"))?;

        let project_name = args
            .get("project")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing 'project' argument"))?;

        let projects = self.load_project_dependencies(meta_dir)?;
        let graph = dependency_graph::DependencyGraph::build(projects)?;
        let impact = graph.analyze_impact(project_name);

        Ok(serde_json::to_string_pretty(&impact)?)
    }

    fn tool_execution_order(&self, args: &serde_json::Value) -> Result<String> {
        let meta_dir = self
            .meta_dir
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("No meta repository found"))?;

        let tag_filter = args.get("tag").and_then(|v| v.as_str());

        let projects = self.load_project_dependencies(meta_dir)?;
        let graph = dependency_graph::DependencyGraph::build(projects)?;
        let tags: Vec<String> = tag_filter.map(|t| vec![t.to_string()]).unwrap_or_default();
        let order = graph.execution_order_filtered(&tags)?;

        Ok(serde_json::to_string_pretty(&serde_json::json!({
            "execution_order": order,
            "count": order.len(),
            "tag_filter": tag_filter
        }))?)
    }

    fn tool_batch_execute(&self, args: &serde_json::Value) -> Result<String> {
        let meta_dir = self
            .meta_dir
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("No meta repository found"))?;

        let command = string_list(args, "command")?
            .filter(|argv| !argv.is_empty())
            .ok_or_else(|| anyhow::anyhow!("Missing 'command' argument"))?;
        let (program, program_args) = command
            .split_first()
            .ok_or_else(|| anyhow::anyhow!("Missing 'command' argument"))?;

        let tag_filter = args.get("tag").and_then(|v| v.as_str());
        let atomic = args
            .get("atomic")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let projects = self.load_projects(meta_dir)?;

        // Filter by tag
        let filtered: Vec<&ProjectInfo> = if let Some(tag) = tag_filter {
            projects
                .iter()
                .filter(|p| p.tags.contains(&tag.to_string()))
                .collect()
        } else {
            projects.iter().collect()
        };

        // Atomic: every target must be a clean git repository before anything
        // runs, so a rollback discards only what the command changed.
        let mut before = Vec::new();
        if atomic {
            let mut refused = Vec::new();
            for project in &filtered {
                let path = meta_dir.join(&project.path);
                // The branch is empty for a detached HEAD.
                let head = self
                    .repo_git(&path, &["rev-parse", "--verify", "HEAD"])
                    .and_then(|commit| {
                        let branch = self.repo_git(&path, &["branch", "--show-current"])?;
                        Ok(((!branch.is_empty()).then_some(branch), commit))
                    });
                match (head, self.is_clean(&path)) {
                    (Ok(head), Ok(true)) => before.push(head),
                    (Ok(_), Ok(false)) => refused.push(format!(
                        "{}: has uncommitted changes or untracked files",
                        project.name
                    )),
                    _ => refused.push(format!(
                        "{}: not the root of a git repository with a commit",
                        project.name
                    )),
                }
            }
            if !refused.is_empty() {
                return Err(anyhow::anyhow!(
                    "Atomic batch refused, nothing was run: {}",
                    refused.join("; ")
                ));
            }
        }

        let mut results = Vec::new();
        let mut has_failure = false;

        for project in &filtered {
            let project_path = meta_dir.join(&project.path);
            if !project_path.exists() {
                results.push(serde_json::json!({
                    "project": project.name,
                    "success": false,
                    "error": "Path does not exist"
                }));
                has_failure = true;
                if atomic {
                    break;
                }
                continue;
            }

            let output = Command::new(program)
                .args(program_args)
                .current_dir(&project_path)
                .output();

            match output {
                Ok(out) => {
                    let success = out.status.success();
                    if !success {
                        has_failure = true;
                    }
                    results.push(serde_json::json!({
                        "project": project.name,
                        "success": success,
                        "stdout": String::from_utf8_lossy(&out.stdout).to_string(),
                        "stderr": String::from_utf8_lossy(&out.stderr).to_string()
                    }));
                }
                Err(e) => {
                    has_failure = true;
                    results.push(serde_json::json!({
                        "project": project.name,
                        "success": false,
                        "error": e.to_string()
                    }));
                }
            }

            // If atomic and failure, stop and rollback
            if atomic && has_failure {
                break;
            }
        }

        // Roll back only the projects the command reached: the first
        // `results.len()` targets. The ones after them are untouched.
        let rollback = (atomic && has_failure).then(|| {
            let mut restored = Vec::new();
            let mut failed = Vec::new();
            for (project, (branch, commit)) in filtered.iter().zip(&before).take(results.len()) {
                let path = meta_dir.join(&project.path);
                match self.restore_repo(&path, branch.as_deref(), commit) {
                    Ok((stash, replaced)) => restored.push(serde_json::json!({
                        "project": project.name,
                        "stash": stash,
                        "replaced_commits": replaced
                    })),
                    Err(e) => failed.push(serde_json::json!({
                        "project": project.name,
                        "error": format!("{e:#}")
                    })),
                }
            }
            serde_json::json!({
                "status": if failed.is_empty() { "success" } else { "failed" },
                "restored": restored,
                "failed": failed
            })
        });

        Ok(serde_json::to_string_pretty(&serde_json::json!({
            "command": command,
            "results": results,
            "has_failure": has_failure,
            "rollback": rollback
        }))?)
    }

    /// Runs git on the repository whose root is `path` (its `.git` is
    /// named, so git never falls back to an enclosing repository, and it
    /// never recurses into submodules, which are repositories of their own)
    /// and returns the trimmed stdout, or an error with git's stderr.
    fn repo_git(&self, path: &std::path::Path, args: &[&str]) -> Result<String> {
        let mut git_dir = std::ffi::OsString::from("--git-dir=");
        git_dir.push(path.join(".git"));
        let mut work_tree = std::ffi::OsString::from("--work-tree=");
        work_tree.push(path);
        let output = Command::new("git")
            .args(["-c", "submodule.recurse=false"])
            .arg(git_dir)
            .arg(work_tree)
            .args(args)
            .current_dir(path)
            .output()
            .with_context(|| format!("Failed to run git {args:?} in {}", path.display()))?;
        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
        } else {
            Err(anyhow::anyhow!(
                "git {} failed in {}: {}",
                args.join(" "),
                path.display(),
                String::from_utf8_lossy(&output.stderr).trim()
            ))
        }
    }

    /// Whether the repository at `path` has no uncommitted changes and no
    /// untracked files, whatever `status.showUntrackedFiles` says.
    fn is_clean(&self, path: &std::path::Path) -> Result<bool> {
        let status = self.repo_git(path, &["status", "--porcelain", "--untracked-files=normal"])?;
        Ok(status.is_empty())
    }

    /// Puts the repository at `path` back on `branch` (detached when `None`)
    /// at `commit`, recreating the branch if it is gone. Uncommitted changes
    /// and untracked files are first saved in a stash entry; ignored files are
    /// not. Returns that stash's commit id and the commits HEAD and the branch
    /// were on, other than `commit`, so all of them can be recovered.
    fn restore_repo(
        &self,
        path: &std::path::Path,
        branch: Option<&str>,
        commit: &str,
    ) -> Result<(Option<String>, Vec<String>)> {
        let commit = self.repo_git(
            path,
            &["rev-parse", "--verify", &format!("{commit}^{{commit}}")],
        )?;
        let mut replaced = vec![self.repo_git(path, &["rev-parse", "--verify", "HEAD"])?];
        if let Some(branch) = branch {
            // Empty when the branch is gone.
            let tip = format!("refs/heads/{branch}");
            replaced.push(self.repo_git(path, &["for-each-ref", "--format=%(objectname)", &tip])?);
        }
        replaced.retain(|replaced| !replaced.is_empty() && *replaced != commit);
        replaced.dedup();

        // `stash list` is empty, not an error, when there is no stash.
        let stash_head = || self.repo_git(path, &["stash", "list", "-n1", "--format=%H"]);
        let old_stash = stash_head()?;
        self.repo_git(
            path,
            &[
                "stash",
                "push",
                "--include-untracked",
                "--quiet",
                "-m",
                "meta-restore-backup",
            ],
        )?;
        let stash = stash_head()
            .context("uncommitted changes may be in a new stash entry, see `git stash list`")?;
        let stash = (stash != old_stash).then_some(stash);
        let checkout = match branch {
            Some(branch) => ["checkout", "--quiet", "-B", branch, &commit, "--"].to_vec(),
            None => ["checkout", "--quiet", "--detach", &commit, "--"].to_vec(),
        };
        self.repo_git(path, &checkout)
            .with_context(|| match &stash {
                Some(stash) => format!("uncommitted changes are saved in stash {stash}"),
                None => "there were no uncommitted changes to save".to_string(),
            })?;
        Ok((stash, replaced))
    }

    // ========================================================================
    // Dependency Graph Helpers
    // ========================================================================

    fn load_project_dependencies(
        &self,
        meta_dir: &std::path::Path,
    ) -> Result<Vec<ProjectDependencies>> {
        let projects = self.load_projects(meta_dir)?;
        Ok(projects.into_iter().map(|p| p.into()).collect())
    }

    // ========================================================================
    // Helper Functions
    // ========================================================================

    fn load_projects(&self, meta_dir: &std::path::Path) -> Result<Vec<ProjectInfo>> {
        let config_path = config::find_meta_config(meta_dir, None)?
            .ok_or_else(|| anyhow::anyhow!("No meta config found"))?;
        let (projects, _ignore) = config::parse_meta_config(&config_path)?;
        Ok(projects)
    }
}

/// A tool argument that is a list of strings, such as a command's argv or a
/// list of paths; None when it is absent.
fn string_list(args: &serde_json::Value, key: &str) -> Result<Option<Vec<String>>> {
    match args.get(key).filter(|value| !value.is_null()) {
        Some(value) => serde_json::from_value(value.clone())
            .map(Some)
            .with_context(|| format!("'{key}' must be a list of strings")),
        None => Ok(None),
    }
}

/// Whether `project_tags` holds any of the comma-separated `tags`, as
/// `meta --tag` selects.
fn has_any_tag(tags: &str, project_tags: &[String]) -> bool {
    tags.split(',')
        .map(str::trim)
        .any(|tag| !tag.is_empty() && project_tags.iter().any(|t| t == tag))
}

/// npm is a `.cmd` script on Windows, which `Command` doesn't find by its bare name.
const NPM: &str = if cfg!(windows) { "npm.cmd" } else { "npm" };

/// What to run in a repo: build it (`release` for cargo) or test it.
#[derive(Clone, Copy)]
enum RepoAction {
    Build { release: bool },
    Test,
}

/// Choose build/test commands from the same per-repo build-system precedence.
fn repo_command(
    path: &std::path::Path,
    action: RepoAction,
) -> Option<(&'static str, Vec<&'static str>)> {
    let build = matches!(action, RepoAction::Build { .. });
    if path.join("Cargo.toml").is_file() {
        Some(match action {
            RepoAction::Build { release: true } => ("cargo", vec!["build", "--release"]),
            RepoAction::Build { release: false } => ("cargo", vec!["build"]),
            RepoAction::Test => ("cargo", vec!["test"]),
        })
    } else if path.join("package.json").is_file() {
        Some((
            NPM,
            if build {
                vec!["run", "build"]
            } else {
                vec!["test"]
            },
        ))
    } else if path.join("go.mod").is_file() {
        Some(("go", vec![if build { "build" } else { "test" }, "./..."]))
    } else if path.join("Makefile").is_file() {
        Some(("make", if build { vec![] } else { vec!["test"] }))
    } else {
        None
    }
}

fn main() -> Result<()> {
    McpServer::new()?.run()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Create a server with a known meta_dir for tests that need one.
    fn server_with_meta_dir() -> (McpServer, tempfile::TempDir) {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join(".meta.yaml"), "{}").unwrap();
        let server = McpServer {
            meta_dir: Some(tmp.path().to_path_buf()),
        };
        (server, tmp)
    }

    #[test]
    #[cfg(unix)]
    fn build_uses_make_for_root_and_child_and_reports_failures() {
        let (server, tmp) = server_with_meta_dir();
        let child = tmp.path().join("child");
        std::fs::create_dir(&child).unwrap();
        std::fs::write(
            tmp.path().join(".meta.yaml"),
            r#"{"projects":{"child":{"tags":["selected"]}}}"#,
        )
        .unwrap();
        for path in [tmp.path(), child.as_path()] {
            std::fs::write(path.join("Makefile"), "all:\n\t@echo made > built\n").unwrap();
        }
        let output = server
            .tool_build(&json!({"tag":"selected", "release":true}))
            .unwrap();
        let results: serde_json::Value = serde_json::from_str(&output).unwrap();
        assert_eq!(results.as_array().unwrap().len(), 2);
        assert!(results
            .as_array()
            .unwrap()
            .iter()
            .all(|r| r["command"] == "make" && r["success"] == true));
        assert!(tmp.path().join("built").exists());
        assert!(child.join("built").exists());
        std::fs::write(
            child.join("Makefile"),
            "all:\n\t@echo build-failed >&2\n\t@exit 1\n",
        )
        .unwrap();
        let err = server.tool_build(&json!({})).unwrap_err().to_string();
        assert!(err.contains("build failed:"), "{err}");
        assert!(err.contains("build-failed"), "{err}");
        assert!(err.contains("child"), "{err}");
    }

    #[test]
    fn build_tags_select_like_meta_tag() {
        let tags = ["red".to_string()];
        assert!(has_any_tag("red", &tags));
        assert!(has_any_tag("blue, red", &tags));
        assert!(!has_any_tag("blue", &tags));
    }

    #[test]
    fn build_and_test_share_detection_and_keep_release() {
        let tmp = tempfile::tempdir().unwrap();
        for (file, program, build, test) in [
            ("Makefile", "make", vec![], vec!["test"]),
            (
                "go.mod",
                "go",
                vec!["build", "./..."],
                vec!["test", "./..."],
            ),
            (
                "package.json",
                if cfg!(windows) { "npm.cmd" } else { "npm" },
                vec!["run", "build"],
                vec!["test"],
            ),
            (
                "Cargo.toml",
                "cargo",
                vec!["build", "--release"],
                vec!["test"],
            ),
        ] {
            std::fs::write(tmp.path().join(file), "").unwrap();
            assert_eq!(
                repo_command(tmp.path(), RepoAction::Build { release: true }),
                Some((program, build))
            );
            assert_eq!(
                repo_command(tmp.path(), RepoAction::Test),
                Some((program, test))
            );
        }
    }

    #[test]
    #[cfg(unix)]
    fn list_plugins_succeeds_with_supported_cli_arguments() {
        use std::os::unix::fs::PermissionsExt;
        let (server, tmp) = server_with_meta_dir();
        let meta = tmp.path().join("meta");
        std::fs::write(
            &meta,
            "#!/bin/sh\n[ \"$*\" = 'plugin list --json' ] || exit 2\nprintf '[]\\n'\n",
        )
        .unwrap();
        std::fs::set_permissions(&meta, std::fs::Permissions::from_mode(0o700)).unwrap();
        // Through sh: running a just-written file can fail with "text file
        // busy" while another test thread forks
        let mut sh = Command::new("/bin/sh");
        sh.arg(&meta);
        let result = server.list_plugins_with(sh).unwrap();
        assert_eq!(result.trim(), "[]");
    }

    #[test]
    fn test_get_config_ignores_meta_directory() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join(".meta/skills")).unwrap();
        std::fs::write(tmp.path().join(".meta.yaml"), "projects: {}\n").unwrap();
        let server = McpServer {
            meta_dir: Some(tmp.path().to_path_buf()),
        };
        let result = server.tool_get_config(&serde_json::json!({})).unwrap();
        assert!(result.contains(".meta.yaml"));
        assert!(result.contains("projects: {}"));
    }

    #[test]
    fn test_notifications_get_no_reply() {
        let server = McpServer { meta_dir: None };
        let request = |line: &str| serde_json::from_str::<JsonRpcRequest>(line).unwrap();

        for line in [
            r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
            r#"{"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":1}}"#,
            r#"{"jsonrpc":"2.0","method":"no/such/method"}"#,
        ] {
            assert!(server.handle_request(&request(line)).is_none(), "{line}");
        }

        let unknown = server
            .handle_request(&request(
                r#"{"jsonrpc":"2.0","id":null,"method":"no/such/method"}"#,
            ))
            .unwrap();
        assert_eq!(unknown.id, Some(serde_json::Value::Null));
        assert_eq!(unknown.error.unwrap().code, -32601);
    }

    /// Feeds `input` to the server and returns each reply line as JSON.
    fn serve(input: &[u8]) -> Vec<serde_json::Value> {
        let server = McpServer { meta_dir: None };
        let mut output = Vec::new();
        server.serve(input, &mut output).unwrap();
        String::from_utf8(output)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    fn error_code(reply: &serde_json::Value) -> i64 {
        reply["error"]["code"].as_i64().unwrap()
    }

    #[test]
    fn test_bad_input_gets_an_error_reply_and_serving_goes_on() {
        let initialize = br#"{"jsonrpc":"2.0","id":99,"method":"initialize","params":{}}"#;
        let too_large = vec![b' '; MAX_LINE_BYTES as usize + 1];
        for (line, code, id) in [
            (
                &br#"{"jsonrpc":"2.0","id":7,"method":"tools/list""#[..],
                -32700,
                json!(null),
            ),
            (b"\xff\xfe", -32700, json!(null)),
            (&too_large, -32700, json!(null)),
            (b"[]", -32600, json!(null)),
            (b"42", -32600, json!(null)),
            (br#"{"jsonrpc":"2.0","id":8}"#, -32600, json!(8)),
            (
                br#"{"jsonrpc":"1.0","id":"x","method":"tools/list"}"#,
                -32600,
                json!("x"),
            ),
            (
                br#"{"jsonrpc":"2.0","id":{},"method":"tools/list"}"#,
                -32600,
                json!(null),
            ),
            (
                br#"{"jsonrpc":"2.0","id":9,"method":"initialize","params":3}"#,
                -32600,
                json!(9),
            ),
            (
                br#"{"jsonrpc":"2.0","id":9,"method":"initialize","params":null}"#,
                -32600,
                json!(9),
            ),
            (br#"{"jsonrpc":"2.0","method":7}"#, -32600, json!(null)),
            (
                br#"{"jsonrpc":"2.0","id":10,"method":"no/such/method"}"#,
                -32601,
                json!(10),
            ),
        ] {
            let input = [line, b"\n", initialize, b"\n"].concat();
            let replies = serve(&input);
            let shown = String::from_utf8_lossy(&line[..line.len().min(80)]);
            assert_eq!(replies.len(), 2, "{shown}");
            assert_eq!(error_code(&replies[0]), code, "{shown}");
            assert_eq!(replies[0]["id"], id, "{shown}");
            assert_eq!(replies[1]["id"], 99, "{shown}");
            assert!(replies[1]["result"].is_object(), "{shown}");
        }
    }

    #[test]
    fn test_batch_gets_an_array_of_replies() {
        let replies = serve(
            br#"[{"jsonrpc":"2.0","id":1,"method":"tools/list"},{"jsonrpc":"2.0","method":"notifications/initialized"},["2.0",2,"tools/list",{}]]
[{"jsonrpc":"2.0","method":"notifications/initialized"}]
"#,
        );
        assert_eq!(replies.len(), 1, "an all-notification batch gets no reply");
        let batch = replies[0].as_array().unwrap();
        assert_eq!(batch.len(), 2);
        assert_eq!(batch[0]["id"], 1);
        assert!(batch[0]["result"]["tools"].is_array());
        assert_eq!(error_code(&batch[1]), -32600);
    }

    fn git(dir: &std::path::Path, args: &[&str]) -> String {
        let out = Command::new("git")
            .args(["-c", "user.name=t", "-c", "user.email=t@example.com"])
            .args(args)
            .current_dir(dir)
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}: {out:?}");
        String::from_utf8(out.stdout).unwrap()
    }

    /// A meta workspace with projects `a` (tag `x`) and `b`, each a git repo
    /// with one commit of `README`.
    fn workspace() -> (McpServer, tempfile::TempDir) {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join(".meta.yaml"),
            r#"{"projects":{"a":{"repo":"x","tags":["x"]},"b":"x"}}"#,
        )
        .unwrap();
        for name in ["a", "b"] {
            init_repo(&tmp.path().join(name));
        }
        let server = McpServer {
            meta_dir: Some(tmp.path().to_path_buf()),
        };
        (server, tmp)
    }

    /// A git repo at `dir` with one commit of `README`.
    fn init_repo(dir: &std::path::Path) {
        std::fs::create_dir_all(dir).unwrap();
        git(dir, &["init", "-q", "-b", "main"]);
        let name = dir.file_name().unwrap().to_str().unwrap();
        std::fs::write(dir.join("README"), name).unwrap();
        git(dir, &["add", "README"]);
        git(dir, &["commit", "-qm", "init"]);
    }

    /// `meta_batch_execute`; a string `command` is a script, run with
    /// `sh -c` the way a user asks for a shell.
    fn batch(server: &McpServer, mut args: serde_json::Value) -> Result<serde_json::Value> {
        if let Some(script) = args["command"].as_str().map(str::to_string) {
            args["command"] = json!(["sh", "-c", script]);
        }
        Ok(serde_json::from_str(&server.tool_batch_execute(&args)?)?)
    }

    #[test]
    fn test_batch_runs_its_argv_with_no_shell() {
        let (server, tmp) = workspace();
        let args = json!({"command": ["git", "rev-parse", "--sq-quote", "$(touch PWNED)", "a b"]});
        let result: serde_json::Value =
            serde_json::from_str(&server.tool_batch_execute(&args).unwrap()).unwrap();
        assert_eq!(
            result["results"][0]["stdout"], " '$(touch PWNED)' 'a b'\n",
            "{result}"
        );
        for dir in [
            tmp.path().to_path_buf(),
            tmp.path().join("a"),
            tmp.path().join("b"),
        ] {
            assert!(!dir.join("PWNED").exists());
        }
    }

    #[test]
    fn test_atomic_batch_never_touches_projects_it_does_not_target() {
        let (server, tmp) = workspace();
        let (a, b) = (tmp.path().join("a"), tmp.path().join("b"));
        std::fs::write(b.join("README"), "my edit").unwrap();
        let head = git(&a, &["rev-parse", "HEAD"]);

        let result = batch(
            &server,
            json!({
                "command": "echo changed > README && git -c user.name=t -c user.email=t@example.com commit -qam c && touch new && false",
                "tag": "x",
                "atomic": true
            }),
        )
        .unwrap();

        assert_eq!(result["rollback"]["status"], "success", "{result}");
        let stash = result["rollback"]["restored"][0]["stash"].as_str().unwrap();
        assert_eq!(git(&a, &["show", &format!("{stash}^3:new")]), "");
        let replaced = &result["rollback"]["restored"][0]["replaced_commits"];
        assert_eq!(replaced.as_array().unwrap().len(), 1, "{replaced}");
        let replaced = replaced[0].as_str().unwrap();
        assert_eq!(
            git(&a, &["show", &format!("{replaced}:README")]),
            "changed\n"
        );
        assert!(!a.join("new").exists());
        assert_eq!(git(&a, &["rev-parse", "HEAD"]), head);
        assert_eq!(git(&a, &["branch", "--show-current"]), "main\n");
        assert_eq!(std::fs::read_to_string(a.join("README")).unwrap(), "a");
        assert_eq!(
            std::fs::read_to_string(b.join("README")).unwrap(),
            "my edit"
        );
        assert_eq!(git(&b, &["stash", "list"]), "");
    }

    #[test]
    fn test_atomic_batch_refuses_a_dirty_target_and_runs_nothing() {
        let (server, tmp) = workspace();
        std::fs::write(tmp.path().join("b/README"), "my edit").unwrap();

        let err = batch(&server, json!({"command": "touch ran", "atomic": true})).unwrap_err();

        assert!(
            err.to_string().contains("b: has uncommitted changes"),
            "{err}"
        );
        assert!(!tmp.path().join("a/ran").exists());
        assert_eq!(git(&tmp.path().join("b"), &["stash", "list"]), "");
    }

    #[test]
    fn test_atomic_batch_refuses_hidden_untracked_files_and_non_repos() {
        let (server, tmp) = workspace();
        let a = tmp.path().join("a");
        git(&a, &["config", "status.showUntrackedFiles", "no"]);
        std::fs::write(a.join("mine"), "my file").unwrap();
        // `c` is a plain directory inside the workspace root's repository,
        // not a repository of its own. (A project nested inside another
        // project, such as `a/sub`, is refused when .meta.yaml is parsed.)
        init_repo(tmp.path());
        std::fs::create_dir(tmp.path().join("c")).unwrap();
        std::fs::write(
            tmp.path().join(".meta.yaml"),
            r#"{"projects":{"a":"x","c":"x"}}"#,
        )
        .unwrap();

        let err = batch(&server, json!({"command": "touch ran", "atomic": true})).unwrap_err();

        assert!(
            err.to_string().contains("a: has uncommitted changes"),
            "{err}"
        );
        assert!(
            err.to_string()
                .contains("c: not the root of a git repository"),
            "{err}"
        );
        assert!(!a.join("ran").exists());
    }

    #[test]
    fn test_atomic_batch_reports_a_failed_rollback() {
        let (server, tmp) = workspace();
        // The workspace root is a repository too, with work of its own.
        init_repo(tmp.path());
        std::fs::write(tmp.path().join("README"), "root edit").unwrap();

        let result = batch(
            &server,
            json!({"command": "rm -rf .git && false", "atomic": true}),
        )
        .unwrap();

        assert_eq!(result["rollback"]["status"], "failed", "{result}");
        assert_eq!(result["rollback"]["failed"][0]["project"], "a");
        assert_eq!(result["rollback"]["restored"], json!([]));
        assert_eq!(result["results"].as_array().unwrap().len(), 1);
        assert_eq!(git(tmp.path(), &["stash", "list"]), "");
        assert_eq!(
            std::fs::read_to_string(tmp.path().join("README")).unwrap(),
            "root edit"
        );
    }

    #[test]
    fn test_atomic_batch_stops_at_a_project_that_disappeared() {
        let (server, tmp) = workspace();
        init_repo(&tmp.path().join("c"));
        std::fs::write(
            tmp.path().join(".meta.yaml"),
            r#"{"projects":{"a":"x","b":"x","c":"x"}}"#,
        )
        .unwrap();

        let result = batch(
            &server,
            json!({"command": "rm -rf ../b && touch ran", "atomic": true}),
        )
        .unwrap();

        assert_eq!(result["results"].as_array().unwrap().len(), 2, "{result}");
        assert!(!tmp.path().join("c/ran").exists());
        assert_eq!(result["rollback"]["status"], "failed");
        assert_eq!(result["rollback"]["restored"][0]["project"], "a");
        assert_eq!(result["rollback"]["failed"][0]["project"], "b");
        assert!(!tmp.path().join("a/ran").exists());
    }

    #[test]
    fn test_rollback_reports_the_branch_tip_and_recreates_a_deleted_branch() {
        let (server, tmp) = workspace();
        let a = tmp.path().join("a");
        std::fs::write(tmp.path().join(".meta.yaml"), r#"{"projects":{"a":"x"}}"#).unwrap();
        let head = git(&a, &["rev-parse", "HEAD"]);
        let commit =
            "echo x > README && git -c user.name=t -c user.email=t@example.com commit -qam x";

        // HEAD is back on the old commit, but the branch tip has moved on.
        let result = batch(
            &server,
            json!({"command": format!("{commit} && git checkout -q HEAD~1 && false"), "atomic": true}),
        )
        .unwrap();
        let replaced = result["rollback"]["restored"][0]["replaced_commits"][0]
            .as_str()
            .unwrap();
        assert_eq!(git(&a, &["show", &format!("{replaced}:README")]), "x\n");
        assert_eq!(git(&a, &["branch", "--show-current"]), "main\n");
        assert_eq!(git(&a, &["rev-parse", "HEAD"]), head);

        let result = batch(
            &server,
            json!({"command": "git checkout -q -b tmp && git branch -q -D main && false", "atomic": true}),
        )
        .unwrap();
        assert_eq!(result["rollback"]["status"], "success", "{result}");
        assert_eq!(git(&a, &["branch", "--show-current"]), "main\n");
        assert_eq!(git(&a, &["rev-parse", "HEAD"]), head);
    }

    #[test]
    fn test_rollback_never_moves_a_submodule() {
        let (server, tmp) = workspace();
        let a = tmp.path().join("a");
        let b = tmp.path().join("b");
        let b = b.to_str().unwrap();
        git(
            &a,
            &[
                "-c",
                "protocol.file.allow=always",
                "submodule",
                "add",
                "-q",
                b,
                "sub",
            ],
        );
        git(&a, &["commit", "-qm", "sub"]);
        git(&a, &["config", "submodule.recurse", "true"]);
        std::fs::write(tmp.path().join(".meta.yaml"), r#"{"projects":{"a":"x"}}"#).unwrap();

        // The command moves the submodule on and commits that in `a`.
        let id = "git -c user.name=t -c user.email=t@example.com";
        let command = format!(
            "{id} -C sub commit -q --allow-empty -m y && git add sub && {id} commit -qm bump && false"
        );
        let result = batch(&server, json!({"command": command, "atomic": true})).unwrap();

        assert_eq!(result["rollback"]["status"], "success", "{result}");
        let sub = a.join("sub");
        assert_eq!(git(&sub, &["log", "-1", "--format=%s"]), "y\n");
        assert_eq!(git(&sub, &["branch", "--show-current"]), "main\n");
    }

    #[test]
    fn test_server_creation() {
        let server = McpServer::new().unwrap();
        // Server should be created (meta_dir may or may not be set depending on cwd)
        // We just verify it doesn't panic and returns a valid server
        drop(server);
    }

    #[test]
    fn test_initialize_response() {
        let server = McpServer::new().unwrap();
        let result = server.handle_initialize().unwrap();

        let result_obj = result.as_object().unwrap();
        assert_eq!(result_obj.get("protocolVersion").unwrap(), PROTOCOL_VERSION);
        assert!(result_obj.get("capabilities").is_some());
        assert!(result_obj.get("serverInfo").is_some());
    }

    #[test]
    fn test_list_tools_response() {
        let server = McpServer::new().unwrap();
        let result = server.handle_list_tools().unwrap();

        let result_obj = result.as_object().unwrap();
        let tools = result_obj.get("tools").unwrap().as_array().unwrap();
        assert!(!tools.is_empty());

        // Check that expected tools are present
        let tool_names: Vec<&str> = tools
            .iter()
            .filter_map(|t| t.get("name").and_then(|n| n.as_str()))
            .collect();

        // Core tools
        assert!(tool_names.contains(&"meta_list_projects"));
        assert!(tool_names.contains(&"meta_exec"));
        assert!(tool_names.contains(&"meta_get_config"));
        assert!(tool_names.contains(&"meta_get_project_path"));

        // Git tools
        assert!(tool_names.contains(&"meta_git_status"));
        assert!(tool_names.contains(&"meta_git_pull"));
        assert!(tool_names.contains(&"meta_git_push"));
        assert!(tool_names.contains(&"meta_git_fetch"));
        assert!(tool_names.contains(&"meta_git_diff"));
        assert!(tool_names.contains(&"meta_git_branch"));
        assert!(tool_names.contains(&"meta_git_add"));
        assert!(tool_names.contains(&"meta_git_commit"));
        assert!(tool_names.contains(&"meta_git_checkout"));
        assert!(tool_names.contains(&"meta_git_multi_commit"));

        // Build/test tools
        assert!(tool_names.contains(&"meta_detect_build_systems"));
        assert!(tool_names.contains(&"meta_run_tests"));
        assert!(tool_names.contains(&"meta_build"));
        assert!(tool_names.contains(&"meta_clean"));

        // Discovery tools
        assert!(tool_names.contains(&"meta_search_code"));
        assert!(tool_names.contains(&"meta_get_file_tree"));
        assert!(tool_names.contains(&"meta_list_plugins"));

        // AI-Dominance tools
        assert!(tool_names.contains(&"meta_query_repos"));
        assert!(tool_names.contains(&"meta_workspace_state"));
        assert!(tool_names.contains(&"meta_analyze_impact"));
        assert!(tool_names.contains(&"meta_execution_order"));
        assert!(tool_names.contains(&"meta_batch_execute"));

        // Verify total count (4 core + 10 git + 4 build + 3 discovery + 5 AI = 26)
        assert_eq!(tool_names.len(), 26);
    }

    #[test]
    fn test_ok_response() {
        let server = McpServer::new().unwrap();
        let response = server.ok_response(
            Some(serde_json::json!(1)),
            serde_json::json!({"test": "value"}),
        );

        assert_eq!(response.jsonrpc, "2.0");
        assert!(response.result.is_some());
        assert!(response.error.is_none());
    }

    #[test]
    fn test_error_response() {
        let server = McpServer::new().unwrap();
        let response = server.error_response(
            Some(serde_json::json!(1)),
            -32600,
            "Invalid Request".to_string(),
        );

        assert_eq!(response.jsonrpc, "2.0");
        assert!(response.result.is_none());
        assert!(response.error.is_some());

        let error = response.error.unwrap();
        assert_eq!(error.code, -32600);
        assert_eq!(error.message, "Invalid Request");
    }

    #[test]
    fn test_multi_commit_tool_schema() {
        let server = McpServer::new().unwrap();
        let result = server.handle_list_tools().unwrap();

        let result_obj = result.as_object().unwrap();
        let tools = result_obj.get("tools").unwrap().as_array().unwrap();

        // Find meta_git_multi_commit tool
        let multi_commit_tool = tools
            .iter()
            .find(|t| t.get("name").and_then(|n| n.as_str()) == Some("meta_git_multi_commit"))
            .expect("meta_git_multi_commit tool should exist");

        // Verify schema structure
        let schema = multi_commit_tool.get("inputSchema").unwrap();
        let props = schema.get("properties").unwrap().as_object().unwrap();

        // Should have a commits property
        assert!(props.contains_key("commits"));

        let commits_prop = props.get("commits").unwrap();
        assert_eq!(commits_prop.get("type").unwrap(), "array");

        // Verify items schema
        let items = commits_prop.get("items").unwrap();
        let item_props = items.get("properties").unwrap().as_object().unwrap();
        assert!(item_props.contains_key("project"));
        assert!(item_props.contains_key("message"));

        // Verify required fields in items
        let item_required = items.get("required").unwrap().as_array().unwrap();
        let required_fields: Vec<&str> = item_required.iter().filter_map(|v| v.as_str()).collect();
        assert!(required_fields.contains(&"project"));
        assert!(required_fields.contains(&"message"));
    }

    #[test]
    fn test_multi_commit_missing_commits_arg() {
        let (server, _tmp) = server_with_meta_dir();

        // Test with empty args - should fail with missing commits
        let result = server.tool_git_multi_commit(&serde_json::json!({}));
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("commits"));
    }

    #[test]
    fn test_multi_commit_invalid_commits_format() {
        let (server, _tmp) = server_with_meta_dir();

        // Test with commits as string instead of array
        let result = server.tool_git_multi_commit(&serde_json::json!({
            "commits": "not an array"
        }));
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("commits"));
    }

    #[test]
    fn test_multi_commit_missing_project_in_entry() {
        let (server, _tmp) = server_with_meta_dir();

        // Test with commit entry missing project
        let result = server.tool_git_multi_commit(&serde_json::json!({
            "commits": [
                {"message": "Test commit"}
            ]
        }));
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("project"));
    }

    #[test]
    fn test_multi_commit_missing_message_in_entry() {
        let (server, _tmp) = server_with_meta_dir();

        // Test with commit entry missing message
        let result = server.tool_git_multi_commit(&serde_json::json!({
            "commits": [
                {"project": "test-repo"}
            ]
        }));
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("message"));
    }
}
