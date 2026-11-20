//! The WebSocket MCP server that a `claude` CLI connects to for IDE tools,
//! found through a lock file in `~/.claude/ide/`. The wire format follows the
//! Claude Code IDE protocol: JSON-RPC 2.0 MCP messages over a WebSocket on
//! 127.0.0.1, authorized by the `x-claude-code-ide-authorization` header.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context as _, Result};
use async_tungstenite::tungstenite::Message;
use async_tungstenite::tungstenite::handshake::server::{ErrorResponse, Request, Response};
use async_tungstenite::tungstenite::http::StatusCode;
use futures::channel::{mpsc, oneshot};
use futures::{FutureExt as _, StreamExt as _};
use gpui::{BackgroundExecutor, Task};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub const AUTH_HEADER: &str = "x-claude-code-ide-authorization";
const SUBPROTOCOL_HEADER: &str = "sec-websocket-protocol";
const SUBPROTOCOL: &str = "mcp";
pub const IDE_NAME: &str = "Vireo";
pub const PROTOCOL_VERSION: &str = "2025-06-18";

/// The lock file that lets the CLI find this server, at `~/.claude/ide/<port>.lock`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LockFile {
    pub pid: u32,
    pub workspace_folders: Vec<String>,
    pub ide_name: String,
    pub transport: String,
    pub running_in_windows: bool,
    pub auth_token: String,
}

impl LockFile {
    pub fn new(workspace_folders: Vec<String>, auth_token: String) -> Self {
        Self {
            pid: std::process::id(),
            workspace_folders,
            ide_name: IDE_NAME.to_string(),
            transport: "ws".to_string(),
            running_in_windows: cfg!(windows),
            auth_token,
        }
    }

    pub fn path(ide_dir: &Path, port: u16) -> PathBuf {
        ide_dir.join(format!("{port}.lock"))
    }

    pub fn write(&self, ide_dir: &Path, port: u16) -> Result<PathBuf> {
        std::fs::create_dir_all(ide_dir)
            .with_context(|| format!("failed to create {}", ide_dir.display()))?;
        let path = Self::path(ide_dir, port);
        let contents = serde_json::to_vec(self)?;
        // The file holds the token that authorizes tool calls: keep it private.
        #[cfg(unix)]
        {
            use std::io::Write as _;
            use std::os::unix::fs::OpenOptionsExt as _;
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .mode(0o600)
                .open(&path)
                .with_context(|| format!("failed to write {}", path.display()))?;
            file.write_all(&contents)?;
        }
        #[cfg(not(unix))]
        std::fs::write(&path, contents)
            .with_context(|| format!("failed to write {}", path.display()))?;
        Ok(path)
    }
}

pub fn generate_auth_token() -> String {
    let bytes: [u8; 16] = rand::random();
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// A tool call forwarded to the foreground, where the workspace lives.
pub struct ToolCallRequest {
    pub name: String,
    pub arguments: Value,
    pub reply: oneshot::Sender<ToolResult>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ToolResult {
    pub content: Vec<Value>,
    pub is_error: bool,
}

impl ToolResult {
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            content: vec![json!({"type": "text", "text": text.into()})],
            is_error: false,
        }
    }

    pub fn texts(texts: impl IntoIterator<Item = String>) -> Self {
        Self {
            content: texts
                .into_iter()
                .map(|text| json!({"type": "text", "text": text}))
                .collect(),
            is_error: false,
        }
    }

    pub fn json(value: &Value) -> Self {
        Self::text(serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string()))
    }

    pub fn error(text: impl Into<String>) -> Self {
        Self {
            is_error: true,
            ..Self::text(text)
        }
    }

    pub fn to_json(&self) -> Value {
        let mut result = json!({"content": self.content});
        if self.is_error {
            result["isError"] = Value::Bool(true);
        }
        result
    }
}

/// What to do with one incoming JSON-RPC message.
#[derive(Debug, PartialEq)]
pub enum Route {
    Respond(Value),
    CallTool {
        id: Value,
        name: String,
        arguments: Value,
    },
    Connected {
        pid: Option<u64>,
    },
    Ignore,
}

pub fn route_message(message: &Value, server_version: &str) -> Route {
    let method = message.get("method").and_then(Value::as_str);
    let id = message.get("id").cloned();
    let params = message.get("params").cloned().unwrap_or(Value::Null);
    let Some(method) = method else {
        return Route::Ignore;
    };
    let Some(id) = id else {
        return match method {
            "ide_connected" => Route::Connected {
                pid: params.get("pid").and_then(Value::as_u64),
            },
            _ => Route::Ignore,
        };
    };
    match method {
        "initialize" => Route::Respond(json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": {
                "protocolVersion": params
                    .get("protocolVersion")
                    .and_then(Value::as_str)
                    .unwrap_or(PROTOCOL_VERSION),
                "capabilities": {"tools": {"listChanged": true}},
                "serverInfo": {"name": "Vireo Claude Code MCP", "version": server_version},
            }
        })),
        "ping" => Route::Respond(json!({"jsonrpc": "2.0", "id": id, "result": {}})),
        "tools/list" => Route::Respond(json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": {"tools": tool_definitions()}
        })),
        "tools/call" => match params.get("name").and_then(Value::as_str) {
            Some(name) => Route::CallTool {
                id,
                name: name.to_string(),
                arguments: params.get("arguments").cloned().unwrap_or(json!({})),
            },
            None => Route::Respond(error_response(id, -32602, "missing tool name")),
        },
        "resources/list" => {
            Route::Respond(json!({"jsonrpc": "2.0", "id": id, "result": {"resources": []}}))
        }
        "prompts/list" => {
            Route::Respond(json!({"jsonrpc": "2.0", "id": id, "result": {"prompts": []}}))
        }
        other => Route::Respond(error_response(
            id,
            -32601,
            &format!("Method not found: {other}"),
        )),
    }
}

/// The tools a panel session gets through its in-process MCP server: the
/// others (diffs, tabs, selections) are handled by the panel itself.
pub const SDK_TOOLS: &[&str] = &["getDiagnostics", "executeCode"];

/// Routes a message for the panel's in-process MCP server, which serves only
/// [`SDK_TOOLS`].
pub fn route_sdk_message(message: &Value, server_version: &str) -> Route {
    match route_message(message, server_version) {
        Route::Respond(mut response)
            if message.get("method").and_then(Value::as_str) == Some("tools/list") =>
        {
            let tools: Vec<Value> = tool_definitions()
                .as_array()
                .into_iter()
                .flatten()
                .filter(|tool| {
                    tool.get("name")
                        .and_then(Value::as_str)
                        .is_some_and(|name| SDK_TOOLS.contains(&name))
                })
                .cloned()
                .collect();
            response["result"]["tools"] = Value::Array(tools);
            Route::Respond(response)
        }
        Route::CallTool { id, name, .. } if !SDK_TOOLS.contains(&name.as_str()) => {
            Route::Respond(error_response(id, -32601, &format!("Unknown tool: {name}")))
        }
        route => route,
    }
}

pub fn error_response(id: Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

pub fn tool_response(id: Value, result: &ToolResult) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "result": result.to_json()})
}

pub fn notification(method: &str, params: Value) -> Value {
    json!({"jsonrpc": "2.0", "method": method, "params": params})
}

/// The tools served to the CLI: the five the VS Code extension serves today,
/// then the ones earlier CLI versions still call.
pub fn tool_definitions() -> Value {
    let schema = "http://json-schema.org/draft-07/schema#";
    let path_tool = |name: &str, description: &str| {
        json!({
            "name": name,
            "description": description,
            "inputSchema": {
                "type": "object",
                "properties": {"filePath": {"type": "string", "description": "Path to the file"}},
                "required": ["filePath"],
                "additionalProperties": false,
                "$schema": schema,
            }
        })
    };
    let no_input = |name: &str, description: &str| {
        json!({
            "name": name,
            "description": description,
            "inputSchema": {"type": "object", "properties": {}, "$schema": schema}
        })
    };
    json!([
        {
            "name": "openDiff",
            "description": "Open a diff of proposed changes to a file and wait for the user to accept or reject it",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "old_file_path": {"type": "string", "description": "Path of the file before the change"},
                    "new_file_path": {"type": "string", "description": "Path of the file after the change"},
                    "new_file_contents": {"type": "string", "description": "Proposed contents of the file"},
                    "tab_name": {"type": "string", "description": "Name of the diff tab"}
                },
                "required": ["old_file_path", "new_file_path", "new_file_contents", "tab_name"],
                "additionalProperties": false,
                "$schema": schema,
            }
        },
        {
            "name": "getDiagnostics",
            "description": "Get language diagnostics from Vireo",
            "inputSchema": {
                "type": "object",
                "properties": {"uri": {"type": "string", "description": "Optional file URI to get diagnostics for. If not provided, gets diagnostics for all files."}},
                "additionalProperties": false,
                "$schema": schema,
            }
        },
        {
            "name": "close_tab",
            "inputSchema": {
                "type": "object",
                "properties": {"tab_name": {"type": "string"}},
                "required": ["tab_name"],
                "additionalProperties": false,
                "$schema": schema,
            }
        },
        no_input("closeAllDiffTabs", "Close all diff tabs in the editor"),
        {
            "name": "executeCode",
            "description": "Execute Python code in the Jupyter kernel of the active notebook or REPL. Code runs in the current kernel and its state persists across calls until the kernel restarts. Avoid changing the kernel's state unless the user asks for it.",
            "inputSchema": {
                "type": "object",
                "properties": {"code": {"type": "string", "description": "The code to be executed on the kernel."}},
                "required": ["code"],
                "additionalProperties": false,
                "$schema": schema,
            }
        },
        {
            "name": "openFile",
            "description": "Open a file in the editor and optionally select a range of text",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "filePath": {"type": "string", "description": "Path to the file to open"},
                    "preview": {"type": "boolean", "default": false},
                    "startText": {"type": "string", "description": "Text marking the start of the selection"},
                    "endText": {"type": "string", "description": "Text marking the end of the selection"},
                    "selectToEndOfLine": {"type": "boolean", "default": false},
                    "makeFrontmost": {"type": "boolean", "default": true}
                },
                "required": ["filePath"],
                "additionalProperties": false,
                "$schema": schema,
            }
        },
        no_input("getCurrentSelection", "Get the current text selection in the active editor"),
        no_input("getLatestSelection", "Get the most recent text selection, even if the editor is no longer focused"),
        no_input("getOpenEditors", "Get the editors that are open"),
        no_input("getWorkspaceFolders", "Get the folders of the workspace"),
        path_tool("checkDocumentDirty", "Check whether a document has unsaved changes"),
        path_tool("saveDocument", "Save a document that has unsaved changes"),
    ])
}

/// The running server. Dropping it stops accepting connections and closes
/// every client.
pub struct IdeServer {
    pub port: u16,
    pub auth_token: String,
    clients: Arc<Mutex<Vec<mpsc::UnboundedSender<String>>>>,
    _accept: Task<()>,
}

impl IdeServer {
    pub async fn start(
        executor: BackgroundExecutor,
        tool_calls: mpsc::UnboundedSender<ToolCallRequest>,
        server_version: String,
    ) -> Result<Self> {
        let listener = smol::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .context("failed to bind the Claude Code IDE server")?;
        let port = listener.local_addr()?.port();
        let auth_token = generate_auth_token();
        let clients = Arc::new(Mutex::new(Vec::new()));
        let accept = executor.spawn({
            let executor = executor.clone();
            let auth_token = auth_token.clone();
            let clients = clients.clone();
            async move {
                loop {
                    let stream = match listener.accept().await {
                        Ok((stream, _)) => stream,
                        Err(error) => {
                            log::warn!("Claude Code IDE server stopped accepting: {error}");
                            break;
                        }
                    };
                    executor
                        .spawn(serve_connection(
                            stream,
                            auth_token.clone(),
                            clients.clone(),
                            tool_calls.clone(),
                            server_version.clone(),
                            executor.clone(),
                        ))
                        .detach();
                }
            }
        });
        Ok(Self {
            port,
            auth_token,
            clients,
            _accept: accept,
        })
    }

    pub fn client_count(&self) -> usize {
        self.clients.lock().len()
    }

    pub fn broadcast(&self, method: &str, params: Value) {
        let message = notification(method, params).to_string();
        self.clients
            .lock()
            .retain(|client| client.unbounded_send(message.clone()).is_ok());
    }
}

async fn serve_connection(
    stream: smol::net::TcpStream,
    auth_token: String,
    clients: Arc<Mutex<Vec<mpsc::UnboundedSender<String>>>>,
    tool_calls: mpsc::UnboundedSender<ToolCallRequest>,
    server_version: String,
    executor: BackgroundExecutor,
) {
    // tungstenite's handshake callback fixes this error type.
    #[allow(clippy::result_large_err)]
    let check_auth =
        |request: &Request, mut response: Response| -> Result<Response, ErrorResponse> {
            let authorized = request
                .headers()
                .get(AUTH_HEADER)
                .and_then(|value| value.to_str().ok())
                .is_some_and(|value| value == auth_token);
            if authorized {
                // The CLI asks for the `mcp` subprotocol and drops the connection
                // unless the server selects it.
                let wants_mcp = request
                    .headers()
                    .get_all(SUBPROTOCOL_HEADER)
                    .iter()
                    .filter_map(|value| value.to_str().ok())
                    .flat_map(|value| value.split(','))
                    .any(|protocol| protocol.trim() == SUBPROTOCOL);
                if wants_mcp {
                    response.headers_mut().insert(
                        SUBPROTOCOL_HEADER,
                        async_tungstenite::tungstenite::http::HeaderValue::from_static(SUBPROTOCOL),
                    );
                }
                Ok(response)
            } else {
                let mut error = ErrorResponse::new(Some("Unauthorized".to_string()));
                *error.status_mut() = StatusCode::UNAUTHORIZED;
                Err(error)
            }
        };
    let websocket = match async_tungstenite::accept_hdr_async(stream, check_auth).await {
        Ok(websocket) => websocket,
        Err(error) => {
            log::debug!("rejected a Claude Code IDE connection: {error}");
            return;
        }
    };
    let (mut sink, mut source) = websocket.split();
    let (outgoing_tx, mut outgoing_rx) = mpsc::unbounded::<String>();
    clients.lock().push(outgoing_tx.clone());

    loop {
        futures::select_biased! {
            outgoing = outgoing_rx.next() => {
                let Some(text) = outgoing else { break };
                if sink.send(Message::text(text)).await.is_err() {
                    break;
                }
            }
            incoming = source.next().fuse() => {
                let text = match incoming {
                    Some(Ok(Message::Text(text))) => text.to_string(),
                    Some(Ok(Message::Ping(payload))) => {
                        if sink.send(Message::Pong(payload)).await.is_err() {
                            break;
                        }
                        continue;
                    }
                    Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                    Some(Ok(_)) => continue,
                };
                let Ok(message) = serde_json::from_str::<Value>(&text) else {
                    log::debug!("invalid message from claude: {text}");
                    continue;
                };
                match route_message(&message, &server_version) {
                    Route::Respond(response) => {
                        if sink.send(Message::text(response.to_string())).await.is_err() {
                            break;
                        }
                    }
                    Route::CallTool { id, name, arguments } => {
                        // Tools like openDiff wait for the user, so each call
                        // answers on its own while the connection keeps reading.
                        let (reply_tx, reply_rx) = oneshot::channel();
                        let request = ToolCallRequest { name, arguments, reply: reply_tx };
                        let outgoing_tx = outgoing_tx.clone();
                        if tool_calls.unbounded_send(request).is_err() {
                            let response = error_response(id, -32603, "Vireo is closing");
                            outgoing_tx.unbounded_send(response.to_string()).ok();
                            continue;
                        }
                        executor
                            .spawn(async move {
                                let result = reply_rx
                                    .await
                                    .unwrap_or_else(|_| ToolResult::error("The tool call was cancelled"));
                                outgoing_tx
                                    .unbounded_send(tool_response(id, &result).to_string())
                                    .ok();
                            })
                            .detach();
                    }
                    Route::Connected { pid } => {
                        log::info!("Claude Code connected to Vireo (pid {pid:?})");
                    }
                    Route::Ignore => {}
                }
            }
        }
    }
    clients
        .lock()
        .retain(|client| !client.same_receiver(&outgoing_tx));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lock_file_format_matches_the_extension() {
        let lock = LockFile {
            pid: 42,
            workspace_folders: vec!["/p".into()],
            ide_name: IDE_NAME.into(),
            transport: "ws".into(),
            running_in_windows: false,
            auth_token: "t".into(),
        };
        assert_eq!(
            serde_json::to_value(&lock).unwrap(),
            json!({"pid": 42, "workspaceFolders": ["/p"], "ideName": "Vireo", "transport": "ws", "runningInWindows": false, "authToken": "t"})
        );
        let dir = tempfile::tempdir().unwrap();
        let path = lock.write(dir.path(), 1234).unwrap();
        assert_eq!(path, dir.path().join("1234.lock"));
        let read: LockFile = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(read, lock);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }

    #[test]
    fn auth_tokens_are_random_hex() {
        let first = generate_auth_token();
        assert_eq!(first.len(), 32);
        assert!(first.chars().all(|char| char.is_ascii_hexdigit()));
        assert_ne!(first, generate_auth_token());
    }

    #[test]
    fn routes_mcp_messages() {
        let Route::Respond(initialize) = route_message(
            &json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2025-06-18", "capabilities": {}}}),
            "1.0",
        ) else {
            panic!("initialize should be answered");
        };
        assert_eq!(initialize["result"]["protocolVersion"], "2025-06-18");
        assert_eq!(
            initialize["result"]["capabilities"]["tools"]["listChanged"],
            true
        );

        let Route::Respond(list) = route_message(
            &json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}),
            "1.0",
        ) else {
            panic!("tools/list should be answered");
        };
        let names: Vec<&str> = list["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|tool| tool["name"].as_str().unwrap())
            .collect();
        for expected in [
            "openDiff",
            "getDiagnostics",
            "close_tab",
            "closeAllDiffTabs",
            "executeCode",
        ] {
            assert!(names.contains(&expected), "missing {expected}");
        }

        assert_eq!(
            route_message(
                &json!({"jsonrpc": "2.0", "id": "a", "method": "tools/call", "params": {"name": "getDiagnostics", "arguments": {"uri": "file:///a"}}}),
                "1.0",
            ),
            Route::CallTool {
                id: json!("a"),
                name: "getDiagnostics".into(),
                arguments: json!({"uri": "file:///a"}),
            }
        );
        assert_eq!(
            route_message(
                &json!({"jsonrpc": "2.0", "method": "ide_connected", "params": {"pid": 7}}),
                "1.0"
            ),
            Route::Connected { pid: Some(7) }
        );
        assert_eq!(
            route_message(
                &json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
                "1.0"
            ),
            Route::Ignore
        );
        let Route::Respond(unknown) =
            route_message(&json!({"jsonrpc": "2.0", "id": 3, "method": "nope"}), "1.0")
        else {
            panic!("unknown methods get an error");
        };
        assert_eq!(unknown["error"]["code"], -32601);
    }

    #[test]
    fn sdk_server_serves_only_its_tools() {
        let Route::Respond(list) = route_sdk_message(
            &json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"}),
            "1.0",
        ) else {
            panic!("tools/list should be answered");
        };
        let names: Vec<&str> = list["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|tool| tool["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, vec!["getDiagnostics", "executeCode"]);
        assert!(matches!(
            route_sdk_message(
                &json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {"name": "openDiff", "arguments": {}}}),
                "1.0",
            ),
            Route::Respond(_)
        ));
        assert!(matches!(
            route_sdk_message(
                &json!({"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {"name": "getDiagnostics", "arguments": {}}}),
                "1.0",
            ),
            Route::CallTool { .. }
        ));
    }

    #[test]
    fn tool_results_serialize_as_mcp_content() {
        assert_eq!(
            tool_response(
                json!(1),
                &ToolResult::texts(["FILE_SAVED".into(), "x".into()])
            ),
            json!({"jsonrpc": "2.0", "id": 1, "result": {"content": [
                {"type": "text", "text": "FILE_SAVED"},
                {"type": "text", "text": "x"}
            ]}})
        );
        assert_eq!(
            ToolResult::error("bad").to_json(),
            json!({"content": [{"type": "text", "text": "bad"}], "isError": true})
        );
    }

    #[gpui::test]
    async fn serves_authorized_websocket_clients(cx: &mut gpui::TestAppContext) {
        cx.executor().allow_parking();
        let (tool_tx, mut tool_rx) = mpsc::unbounded();
        let server = IdeServer::start(cx.executor(), tool_tx, "1.0".into())
            .await
            .unwrap();
        let url = format!("ws://127.0.0.1:{}", server.port);

        let connect = || smol::net::TcpStream::connect(("127.0.0.1", server.port));
        let unauthorized =
            async_tungstenite::client_async(url.as_str(), connect().await.unwrap()).await;
        assert!(
            unauthorized.is_err(),
            "a client without the token is refused"
        );

        let mut request =
            async_tungstenite::tungstenite::client::IntoClientRequest::into_client_request(
                url.as_str(),
            )
            .unwrap();
        request
            .headers_mut()
            .insert(AUTH_HEADER, server.auth_token.parse().unwrap());
        request
            .headers_mut()
            .insert(SUBPROTOCOL_HEADER, SUBPROTOCOL.parse().unwrap());
        let (mut client, response) =
            async_tungstenite::client_async(request, connect().await.unwrap())
                .await
                .unwrap();
        assert_eq!(
            response
                .headers()
                .get(SUBPROTOCOL_HEADER)
                .and_then(|value| value.to_str().ok()),
            Some(SUBPROTOCOL),
            "the server must select the mcp subprotocol the CLI asks for"
        );
        client
            .send(Message::text(
                json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": "close_tab", "arguments": {"tab_name": "x"}}}).to_string(),
            ))
            .await
            .unwrap();
        let call = tool_rx.next().await.unwrap();
        assert_eq!(call.name, "close_tab");
        call.reply.send(ToolResult::text("TAB_CLOSED")).ok();
        let Some(Ok(Message::Text(reply))) = client.next().await else {
            panic!("expected a reply");
        };
        let reply: Value = serde_json::from_str(&reply).unwrap();
        assert_eq!(reply["result"]["content"][0]["text"], "TAB_CLOSED");

        server.broadcast("selection_changed", json!({"text": "hi"}));
        let Some(Ok(Message::Text(notification))) = client.next().await else {
            panic!("expected a notification");
        };
        let notification: Value = serde_json::from_str(&notification).unwrap();
        assert_eq!(notification["method"], "selection_changed");
        assert_eq!(server.client_count(), 1);
    }
}
