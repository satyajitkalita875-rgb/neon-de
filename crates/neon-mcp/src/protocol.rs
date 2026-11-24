//! Spec-compliant Model Context Protocol (MCP) JSON-RPC 2.0 stdio transport & dispatcher.
//!
//! Handles newline-delimited JSON-RPC 2.0 requests on `stdin`, writes responses exclusively to
//! `stdout`, and keeps all diagnostics on `stderr`.

use std::io::{BufRead, Write};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tracing::{debug, warn};

use crate::{ipc::NeonBridge, tools};

pub const SERVER_NAME: &str = "neon-mcp";
pub const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");
pub const DEFAULT_PROTOCOL_VERSION: &str = "2025-03-26";
pub const SUPPORTED_PROTOCOL_VERSIONS: &[&str] = &["2025-03-26", "2024-11-05"];

/// Incoming JSON-RPC 2.0 message envelope.
#[derive(Debug, Deserialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: Option<String>,
    pub id: Option<Value>,
    pub method: String,
    #[serde(default)]
    pub params: Option<Value>,
}

/// Outgoing JSON-RPC 2.0 response envelope.
#[derive(Debug, Serialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: &'static str,
    pub id: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

#[derive(Debug, Serialize)]
pub struct JsonRpcError {
    pub code: i32,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl JsonRpcResponse {
    pub fn ok(id: Value, result: Value) -> Self {
        Self {
            jsonrpc: "2.0",
            id,
            result: Some(result),
            error: None,
        }
    }

    pub fn err(id: Value, code: i32, message: impl Into<String>) -> Self {
        Self {
            jsonrpc: "2.0",
            id,
            result: None,
            error: Some(JsonRpcError {
                code,
                message: message.into(),
                data: None,
            }),
        }
    }
}

/// Runs the blocking MCP stdio JSON-RPC 2.0 loop until EOF.
pub fn run_stdio_loop<R: BufRead, W: Write>(
    mut reader: R,
    mut writer: W,
    bridge: &NeonBridge,
) -> std::io::Result<()> {
    let mut line = String::new();
    loop {
        line.clear();
        let bytes = reader.read_line(&mut line)?;
        if bytes == 0 {
            break;
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let response = match serde_json::from_str::<JsonRpcRequest>(trimmed) {
            Ok(req) => handle_request(req, bridge),
            Err(err) => {
                warn!(%err, "Invalid JSON-RPC frame received on stdin");
                Some(JsonRpcResponse::err(
                    Value::Null,
                    -32700,
                    format!("Parse error: {err}"),
                ))
            }
        };

        if let Some(resp) = response {
            let encoded = serde_json::to_string(&resp)
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
            writeln!(writer, "{encoded}")?;
            writer.flush()?;
        }
    }
    Ok(())
}

/// Dispatches a single parsed MCP JSON-RPC request (returning `None` for notifications).
pub fn handle_request(req: JsonRpcRequest, bridge: &NeonBridge) -> Option<JsonRpcResponse> {
    if let Some(ref ver) = req.jsonrpc
        && ver != "2.0"
    {
        return req.id.map(|id| {
            JsonRpcResponse::err(id, -32600, format!("Unsupported jsonrpc version: {ver}"))
        });
    }

    // Notifications carry no `id` and must never receive a JSON-RPC response.
    let id = match req.id {
        Some(id) => id,
        None => {
            debug!(method = %req.method, "Received MCP notification");
            return None;
        }
    };

    let params = req.params.unwrap_or_else(|| json!({}));

    let resp = match req.method.as_str() {
        "initialize" => handle_initialize(id, &params),
        "ping" => JsonRpcResponse::ok(id, json!({})),
        "tools/list" => JsonRpcResponse::ok(id, json!({ "tools": tools::list_tool_definitions() })),
        "tools/call" => handle_tool_call(id, &params, bridge),
        "resources/list" => {
            JsonRpcResponse::ok(id, json!({ "resources": tools::list_resource_definitions() }))
        }
        "resources/read" => handle_resource_read(id, &params, bridge),
        "prompts/list" => JsonRpcResponse::ok(id, json!({ "prompts": [] })),
        other => JsonRpcResponse::err(id, -32601, format!("Method not found: {other}")),
    };

    Some(resp)
}

fn handle_initialize(id: Value, params: &Value) -> JsonRpcResponse {
    let requested = params
        .get("protocolVersion")
        .and_then(Value::as_str)
        .unwrap_or(DEFAULT_PROTOCOL_VERSION);
    let negotiated = if SUPPORTED_PROTOCOL_VERSIONS.contains(&requested) {
        requested
    } else {
        DEFAULT_PROTOCOL_VERSION
    };

    JsonRpcResponse::ok(
        id,
        json!({
            "protocolVersion": negotiated,
            "capabilities": {
                "tools": { "listChanged": false },
                "resources": { "subscribe": false, "listChanged": false }
            },
            "serverInfo": {
                "name": SERVER_NAME,
                "version": SERVER_VERSION
            },
            "instructions": "NEON DE desktop control server. Inspect and manage windows, up to 10 workspaces, screenshots, theme/blur/effects, monitors, brightness, audio, Wi-Fi, Bluetooth, media playback, notifications, and installed XDG applications safely via authenticated NEON DE IPC and native Linux D-Bus/PipeWire APIs."
        }),
    )
}

fn handle_tool_call(id: Value, params: &Value, bridge: &NeonBridge) -> JsonRpcResponse {
    let Some(name) = params.get("name").and_then(Value::as_str) else {
        return JsonRpcResponse::err(id, -32602, "Missing required parameter: name");
    };
    let args = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));

    match tools::dispatch_tool(name, &args, bridge) {
        Ok(value) => {
            let pretty = serde_json::to_string_pretty(&value).unwrap_or_else(|_| value.to_string());
            JsonRpcResponse::ok(
                id,
                json!({
                    "content": [
                        {
                            "type": "text",
                            "text": pretty
                        }
                    ],
                    "structuredContent": value,
                    "isError": false
                }),
            )
        }
        Err(err) => JsonRpcResponse::ok(
            id,
            json!({
                "content": [
                    {
                        "type": "text",
                        "text": err
                    }
                ],
                "isError": true
            }),
        ),
    }
}

fn handle_resource_read(id: Value, params: &Value, bridge: &NeonBridge) -> JsonRpcResponse {
    let Some(uri) = params.get("uri").and_then(Value::as_str) else {
        return JsonRpcResponse::err(id, -32602, "Missing required parameter: uri");
    };

    match tools::read_resource(uri, bridge) {
        Ok(value) => {
            let text = serde_json::to_string_pretty(&value).unwrap_or_else(|_| value.to_string());
            JsonRpcResponse::ok(
                id,
                json!({
                    "contents": [
                        {
                            "uri": uri,
                            "mimeType": "application/json",
                            "text": text
                        }
                    ]
                }),
            )
        }
        Err(err) => JsonRpcResponse::err(id, -32602, err),
    }
}
