//! MCP Server for meta - exposes multi-repo operations to AI tools.
//!
//! This server implements the Model Context Protocol (MCP) to allow AI assistants
//! like Claude to interact with meta repositories.

mod batch;
mod run;
mod tools;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::io::{BufRead, Read, Write};
use std::path::PathBuf;
use std::sync::mpsc::Sender;
use std::sync::Mutex;

/// MCP Protocol version
const PROTOCOL_VERSION: &str = "2024-11-05";

/// Server information
const SERVER_NAME: &str = "meta-mcp";
const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");

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
// MCP Server
// ============================================================================

struct McpServer {
    meta_dir: Option<PathBuf>,
    calls: run::Calls,
}

/// What one line gets back: one reply, or a batch's replies.
enum Answer {
    One(JsonRpcResponse),
    Batch(Vec<JsonRpcResponse>),
}

/// What one line of input holds.
enum Line {
    Message(serde_json::Value),
    /// Not JSON: why
    Bad(String),
    Blank,
}

/// The `notifications/cancelled` parameters this server reads.
#[derive(Deserialize)]
struct Cancelled {
    #[serde(rename = "requestId")]
    request_id: serde_json::Value,
}

impl McpServer {
    fn new() -> Result<Self> {
        // Find .meta.yaml in current directory or parents
        let cwd = std::env::current_dir()?;
        let meta_dir = meta_core::config::nearest_meta_dir(&cwd)?;
        Ok(Self::in_dir(meta_dir))
    }

    fn in_dir(meta_dir: Option<PathBuf>) -> Self {
        Self {
            meta_dir,
            calls: run::Calls::default(),
        }
    }

    /// Serves stdin and stdout. On Unix a signal that ends the server
    /// (SIGTERM, SIGINT, SIGHUP) first stops the running tool's programs,
    /// which run in a process group of their own and wouldn't get it.
    fn run(&self) -> Result<()> {
        #[cfg(unix)]
        {
            use signal_hook::consts::{SIGHUP, SIGINT, SIGTERM};
            let mut signals = signal_hook::iterator::Signals::new([SIGTERM, SIGINT, SIGHUP])?;
            let handle = signals.handle();
            std::thread::scope(|scope| {
                // the first lets the running call end, rollback included; a
                // second ends the server at once
                scope.spawn(move || {
                    let mut stopping = false;
                    for signal in signals.forever() {
                        if stopping {
                            std::process::exit(128 + signal);
                        }
                        stopping = true;
                        scope.spawn(move || {
                            self.calls.stop_running();
                            std::process::exit(128 + signal);
                        });
                    }
                });
                let served = self.serve(std::io::stdin().lock(), std::io::stdout());
                handle.close();
                served
            })
        }
        #[cfg(not(unix))]
        self.serve(std::io::stdin().lock(), std::io::stdout())
    }

    /// Answers each newline-delimited JSON-RPC message from `input`. Bad input
    /// gets an error reply and serving goes on; it stops at end of input or
    /// when `output` fails. Tool calls run one at a time, in order, beside the
    /// reading, so `ping`, `tools/list` and a cancellation are answered while
    /// a tool runs.
    fn serve(&self, input: impl BufRead, output: impl Write + Send) -> Result<()> {
        let output = Mutex::new(output);
        let (calls, queue) = std::sync::mpsc::channel::<serde_json::Value>();
        std::thread::scope(|scope| {
            let worker = scope.spawn(|| {
                for message in queue {
                    self.calls.answering(|| {
                        let Some(answer) = self.answer(message) else {
                            return;
                        };
                        // No more answers can go out, and the reader may wait
                        // for input that never comes: the server ends here,
                        // with the answer on stderr (an atomic batch's
                        // rollback is in it)
                        if let Err(e) = write_answer(&output, &answer) {
                            self.calls.cancel_all();
                            let _ = writeln!(std::io::stderr(), "Error: {e:#}; the reply was:");
                            let _ = write_answer(&Mutex::new(std::io::stderr()), &answer);
                            std::process::exit(1);
                        }
                    });
                }
                Ok(())
            });
            let read = self.read(input, &output, calls);
            let work = worker
                .join()
                .unwrap_or_else(|_| Err(anyhow::anyhow!("A tool call panicked")));
            read.and(work)
        })
    }

    /// Reads `input` to its end: answers each message, but passes the ones
    /// that call a tool to `calls`.
    fn read(
        &self,
        mut input: impl BufRead,
        output: &Mutex<impl Write>,
        calls: Sender<serde_json::Value>,
    ) -> Result<()> {
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
            let answer = if line.len() as u64 > MAX_LINE_BYTES && !line.ends_with(b"\n") {
                input.skip_until(b'\n')?;
                Some(Answer::One(
                    self.parse_error("Request too large".to_string()),
                ))
            } else {
                match self.parse_line(&line) {
                    Line::Message(message) if self.queue_calls(&message) => {
                        calls
                            .send(message)
                            .map_err(|_| anyhow::anyhow!("Tool calls have stopped"))?;
                        None
                    }
                    Line::Message(message) => self.answer(message),
                    Line::Bad(error) => Some(Answer::One(self.parse_error(error))),
                    Line::Blank => None,
                }
            };
            if let Some(answer) = answer {
                // no one reads what a tool would answer: stop the running
                // one (a rollback still finishes) and start none
                write_answer(output, &answer).inspect_err(|_| self.calls.cancel_all())?;
            }
        }
    }

    /// One line's message.
    fn parse_line(&self, line: &[u8]) -> Line {
        let Ok(text) = std::str::from_utf8(line) else {
            return Line::Bad("Request is not valid UTF-8".to_string());
        };
        // A blank line holds no message: it is framing, not a request.
        if text.trim().is_empty() {
            return Line::Blank;
        }
        serde_json::from_str(text).map_or_else(|e| Line::Bad(e.to_string()), Line::Message)
    }

    /// Whether `message` (or a message of its batch) calls a tool; the ids
    /// of those calls are noted, so they can be cancelled while they wait,
    /// and the batch's cancellations act at once, not after its calls.
    fn queue_calls(&self, message: &serde_json::Value) -> bool {
        let messages = match message {
            serde_json::Value::Array(batch) => batch.as_slice(),
            message => std::slice::from_ref(message),
        };
        let method = |message: &serde_json::Value, name: &str| {
            message.get("method").and_then(|m| m.as_str()) == Some(name)
        };
        let calls: Vec<_> = messages
            .iter()
            .filter(|m| method(m, "tools/call"))
            .collect();
        for id in calls.iter().filter_map(|call| call.get("id")) {
            if valid_id(id) {
                self.calls.queue(id.to_string());
            }
        }
        if !calls.is_empty() && messages.len() > 1 {
            // only a well-formed notification: anything else is refused
            // when its turn comes
            let notification = |m: &serde_json::Value| {
                m.get("jsonrpc") == Some(&serde_json::json!("2.0")) && m.get("id").is_none()
            };
            for message in messages
                .iter()
                .filter(|m| method(m, "notifications/cancelled") && notification(m))
            {
                self.cancel(message.get("params"));
            }
        }
        !calls.is_empty()
    }

    /// Acts on the parameters of `notifications/cancelled`: an object, as
    /// MCP has them (serde would also read a list as one); others are ignored.
    fn cancel(&self, params: Option<&serde_json::Value>) {
        if let Some(Ok(cancelled)) = params
            .filter(|params| params.is_object())
            .map(|params| serde_json::from_value::<Cancelled>(params.clone()))
        {
            self.calls.cancel(&cancelled.request_id.to_string());
        }
    }

    /// The answer to a message or a batch: `None` when it holds only
    /// notifications.
    fn answer(&self, message: serde_json::Value) -> Option<Answer> {
        match message {
            serde_json::Value::Array(batch) if batch.is_empty() => {
                Some(Answer::One(self.error_response(
                    Some(serde_json::Value::Null),
                    -32600,
                    "Invalid Request: empty batch".to_string(),
                )))
            }
            serde_json::Value::Array(batch) => {
                let replies: Vec<_> = batch
                    .into_iter()
                    .filter_map(|m| self.handle_message(m))
                    .collect();
                (!replies.is_empty()).then_some(Answer::Batch(replies))
            }
            message => self.handle_message(message).map(Answer::One),
        }
    }

    /// Replies to one JSON-RPC message, or `None` for a valid notification.
    fn handle_message(&self, message: serde_json::Value) -> Option<JsonRpcResponse> {
        let id = message.get("id").filter(|id| valid_id(id)).cloned();
        let invalid = |reason: &str| {
            // a tool call refused here never starts
            if let Some(id) = &id {
                self.calls.forget(&id.to_string());
            }
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

    /// Returns `None` for a notification (no `id`): JSON-RPC never answers
    /// one; this server acts only on a cancellation. A cancelled tool call
    /// gets no reply either, as MCP asks.
    fn handle_request(&self, request: &JsonRpcRequest) -> Option<JsonRpcResponse> {
        // only as a notification: with an id it is a request for a method
        // that doesn't exist, and changes nothing
        if request.method == "notifications/cancelled" && request.id.is_none() {
            self.cancel(Some(&request.params));
        }
        let id = request.id.clone()?;
        let result = match request.method.as_str() {
            "initialize" => self.handle_initialize(),
            "ping" => Ok(serde_json::json!({})),
            "tools/list" => self.handle_list_tools(),
            "tools/call" => {
                if !self.calls.start(id.to_string()) {
                    return None;
                }
                let result = self.handle_call_tool(&request.params);
                self.calls.finish(|cancelled| {
                    if !cancelled {
                        return Some(result);
                    }
                    // No reply, as MCP asks, but what it did is kept: an
                    // atomic batch's rollback names the stash it made
                    if let Ok(result) = &result {
                        let _ = writeln!(
                            std::io::stderr(),
                            "meta-mcp: request {id} was cancelled; its result: {result}"
                        );
                    }
                    None
                })?
            }
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
            Err(e) => self.error_response(Some(id), -32603, format!("{e:#}")),
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
                // it may repeat what the client sent
                message: meta_core::git::redact_urls(&message).into_owned(),
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
        let tools: Vec<_> = tools::TOOLS
            .iter()
            .map(|tool| {
                serde_json::json!({
                    "name": tool.name,
                    "description": tool.description,
                    "inputSchema": tool.input_schema,
                })
            })
            .collect();
        Ok(serde_json::json!({ "tools": tools }))
    }

    fn handle_call_tool(&self, params: &serde_json::Value) -> Result<serde_json::Value> {
        let name = params
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing tool name"))?;
        // absent or null: no arguments
        let arguments = params
            .get("arguments")
            .filter(|arguments| !arguments.is_null())
            .cloned()
            .unwrap_or_else(|| serde_json::json!({}));

        let result = match tools::TOOLS.iter().find(|tool| tool.name == name) {
            Some(tool) => tool.call(self, arguments),
            None => Err(anyhow::anyhow!("Unknown tool: {name}")),
        };
        let (text, is_error) = match result {
            Ok(reply) => (reply.text, reply.is_error),
            // it may repeat a URL the client passed; a reply's command
            // output passes through
            Err(e) => (
                meta_core::git::redact_urls(&format!("{e:#}")).into_owned(),
                true,
            ),
        };
        Ok(serde_json::to_value(CallToolResult {
            content: vec![ToolContent {
                content_type: "text".to_string(),
                text,
            }],
            is_error: is_error.then_some(true),
        })?)
    }
}

/// Writes `answer` as one line of JSON, whole, whichever thread writes.
fn write_answer(output: &Mutex<impl Write>, answer: &Answer) -> Result<()> {
    let mut output = output
        .lock()
        .map_err(|_| anyhow::anyhow!("Output failed"))?;
    match answer {
        Answer::One(response) => write_line(&mut *output, response)?,
        Answer::Batch(responses) => {
            serde_json::to_writer(&mut *output, responses)?;
            output.write_all(b"\n")?;
        }
    }
    Ok(output.flush()?)
}

/// MCP server for meta: serves the Model Context Protocol on stdin and
/// stdout, for the meta workspace around the current directory.
#[derive(clap::Parser)]
#[command(version)]
struct Cli {}

fn main() -> Result<()> {
    meta_core::args::parse::<Cli>();
    McpServer::new()?.run()
}

#[cfg(test)]
mod test_support;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_notifications_get_no_reply() {
        let server = McpServer::in_dir(None);
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
        let server = McpServer::in_dir(None);
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

    #[test]
    fn ping_gets_an_empty_result() {
        let replies = serve(br#"{"jsonrpc":"2.0","id":"p","method":"ping"}"#);
        assert_eq!(
            replies,
            [json!({"jsonrpc": "2.0", "id": "p", "result": {}})]
        );
    }

    #[test]
    fn a_call_cancelled_before_it_starts_never_runs_or_answers() {
        let (server, tmp) = test_support::workspace();
        let call = json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {
            "name": "meta_batch_execute", "arguments": {"command": ["touch", "ran"]}}});
        let cancel = json!({"jsonrpc": "2.0", "method": "notifications/cancelled",
            "params": {"requestId": 2}});
        let ping = json!({"jsonrpc": "2.0", "id": 3, "method": "ping"});
        let mut output = Vec::new();
        let input = format!("{}\n", json!([call, cancel, ping]));
        server.serve(input.as_bytes(), &mut output).unwrap();
        let replies: serde_json::Value = serde_json::from_slice(&output).unwrap();
        assert_eq!(replies, json!([{"jsonrpc": "2.0", "id": 3, "result": {}}]));
        assert!(!tmp.path().join("a/ran").exists());
    }

    /// A stopping server waits until the reply to a call that ended is
    /// written, however slowly the client reads it.
    #[test]
    #[cfg(unix)]
    fn a_stopping_server_writes_the_reply_of_a_call_that_ended() {
        use std::sync::atomic::{AtomicBool, Ordering};
        /// Output whose first write waits until it is let through.
        struct Gate<'a> {
            writing: &'a AtomicBool,
            open: &'a AtomicBool,
            written: &'a Mutex<Vec<u8>>,
        }
        impl Write for Gate<'_> {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                self.writing.store(true, Ordering::SeqCst);
                while !self.open.load(Ordering::SeqCst) {
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
                self.written.lock().unwrap().extend_from_slice(bytes);
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let (server, _tmp) = test_support::server("projects: {}\n");
        let (writing, open) = (AtomicBool::new(false), AtomicBool::new(false));
        let (stopped, written) = (AtomicBool::new(false), Mutex::new(Vec::new()));
        let call = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
            "params": {"name": "meta_get_config"}});
        let input = format!("{call}\n");
        std::thread::scope(|scope| {
            let output = Gate {
                writing: &writing,
                open: &open,
                written: &written,
            };
            scope.spawn(|| server.serve(input.as_bytes(), output));
            test_support::wait_for("the reply to be written", || writing.load(Ordering::SeqCst));
            scope.spawn(|| {
                server.calls.stop_running();
                stopped.store(true, Ordering::SeqCst);
            });
            std::thread::sleep(std::time::Duration::from_millis(200));
            let early = stopped.load(Ordering::SeqCst);
            // let the reply out before checking, so a failure can't hang
            open.store(true, Ordering::SeqCst);
            assert!(!early, "stopped before the reply was out");
        });
        let reply: serde_json::Value = serde_json::from_slice(&written.lock().unwrap()).unwrap();
        assert_eq!(reply["id"], 1, "{reply}");
    }

    #[test]
    fn a_malformed_cancellation_in_a_batch_cancels_nothing() {
        let (server, tmp) = test_support::workspace();
        let call = json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {
            "name": "meta_batch_execute", "arguments": {"command": ["git", "init", "-q", "ran"]}}});
        let bad_version = json!({"jsonrpc": "1.0", "method": "notifications/cancelled",
            "params": {"requestId": 2}});
        // params as a list, which MCP doesn't have
        let listed = json!({"jsonrpc": "2.0", "method": "notifications/cancelled",
            "params": [2]});
        // with an id: a request for a method that doesn't exist
        let with_id = json!({"jsonrpc": "2.0", "id": 7, "method": "notifications/cancelled",
            "params": {"requestId": 2}});
        let mut output = Vec::new();
        let input = format!("{}\n", json!([with_id, listed, call, bad_version]));
        server.serve(input.as_bytes(), &mut output).unwrap();
        let replies: serde_json::Value = serde_json::from_slice(&output).unwrap();
        assert_eq!(replies[0]["error"]["code"], -32601, "{replies}");
        assert_eq!(replies[1]["id"], 2, "{replies}");
        assert_eq!(replies[2]["error"]["code"], -32600, "{replies}");
        assert!(tmp.path().join("a/ran").exists());
    }

    /// The reply to a `tools/call` of `name` with `arguments`, as JSON text.
    fn call_text(server: &McpServer, name: &str, arguments: serde_json::Value) -> String {
        let reply = server.handle_call_tool(&json!({"name": name, "arguments": arguments}));
        serde_json::to_string(&reply.unwrap()).unwrap()
    }

    #[test]
    fn replies_echo_no_credentials_and_pass_output_through() {
        let (server, _tmp) = test_support::workspace();
        let secret = "https://alice:ghp_SECRET@h/r";
        // output with a URL, a newline and an e-mail address in it comes
        // back whole (a scan of the reply's JSON text read it as userinfo)
        let reply: serde_json::Value = serde_json::from_str(&call_text(
            &server,
            "meta_batch_execute",
            // printf makes the newline: Windows (msys) splits an argument
            // at one
            json!({"command": ["printf", r"homepage=https://example.com\nauthor=bob@example.com"]}),
        ))
        .unwrap();
        let text = reply["content"][0]["text"].as_str().unwrap();
        let result: serde_json::Value = serde_json::from_str(text).unwrap();
        assert_eq!(result["has_failure"], false, "{result}");
        assert_eq!(
            result["results"][0]["stdout"], "homepage=https://example.com\nauthor=bob@example.com",
            "{result}"
        );
        // in a reply or in an error
        for (name, arguments) in [
            ("meta_batch_execute", json!({"command": ["true", secret]})),
            // a program that can't start
            ("meta_batch_execute", json!({"command": [secret]})),
            // a helper gets its URL as typed, a space in it too
            (
                "meta_batch_execute",
                json!({"command": ["true", "foo::https://alice:ghp_SECRET w@h/r"]}),
            ),
            (
                "meta_query_repos",
                json!({"query": format!("branch:{secret}")}),
            ),
            ("meta_analyze_impact", json!({"project": secret})),
            (
                "meta_get_project_path",
                json!({"project": format!("{secret}.git")}),
            ),
            ("meta_execution_order", json!({"tag": secret})),
            (
                "meta_git_multi_commit",
                json!({"commits": [{"project": "a", "message": format!("see {secret}")}]}),
            ),
        ] {
            let reply = call_text(&server, name, arguments);
            assert!(reply.contains("https://***@h/r"), "{name}: {reply}");
            assert!(!reply.contains("ghp_SECRET"), "{name}: {reply}");
        }
    }

    #[test]
    fn protocol_errors_hide_credentials() {
        let (server, _tmp) = test_support::server("projects: {}\n");
        let request: JsonRpcRequest = serde_json::from_value(json!({
            "jsonrpc": "2.0", "id": 1, "method": "https://alice:ghp_SECRET@h/r"
        }))
        .unwrap();
        let reply = serde_json::to_string(&server.handle_request(&request)).unwrap();
        assert!(reply.contains("https://***@h/r"), "{reply}");
        assert!(!reply.contains("ghp_SECRET"), "{reply}");
    }

    #[test]
    fn a_tool_error_keeps_its_cause() {
        let (server, _tmp) = test_support::server("unknown_top: 1\n");
        let result = server
            .handle_call_tool(&json!({"name": "meta_list_projects"}))
            .unwrap();
        assert_eq!(result["isError"], true);
        let text = result["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("unknown field `unknown_top`"), "{text}");
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
}
