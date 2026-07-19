use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use futures_util::StreamExt;
use reqwest::header::{ACCEPT, CONTENT_TYPE, HeaderMap, HeaderName, HeaderValue};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::{Mutex, RwLock};
use tokio_util::sync::CancellationToken;

use crate::protocol::{ToolDefinition, ToolResult, ToolRisk};
use crate::tools::{ToolContext, ToolError, ToolHandler};

const MCP_PROTOCOL_VERSION: &str = "2025-11-25";
const MAX_MCP_MESSAGE_BYTES: usize = 1024 * 1024;
const MAX_MCP_OUTPUT_BYTES: usize = 256 * 1024;
const MAX_MCP_TOOLS: usize = 256;
const MAX_TOOL_DESCRIPTION_BYTES: usize = 4096;
const DEFAULT_TIMEOUT_MS: u64 = 30_000;
const MAX_TIMEOUT_MS: u64 = 300_000;

pub trait McpSecretStore: Send + Sync {
    fn get(&self, server: &str, name: &str) -> Result<Option<String>, McpError>;
    fn set(&self, server: &str, name: &str, value: &str) -> Result<(), McpError>;
    fn delete(&self, server: &str, name: &str) -> Result<(), McpError>;
}

#[derive(Default)]
pub struct OsMcpSecretStore {
    #[cfg(not(test))]
    access_lock: std::sync::Mutex<()>,
}

impl OsMcpSecretStore {
    pub fn new() -> Self {
        Self::default()
    }

    #[cfg(not(test))]
    fn entry(&self, server: &str, name: &str) -> Result<keyring::Entry, McpError> {
        keyring::Entry::new("com.kcoder.app.mcp", &format!("{server}:{name}"))
            .map_err(|error| McpError::Secret(error.to_string()))
    }
}

#[cfg(not(test))]
impl McpSecretStore for OsMcpSecretStore {
    fn get(&self, server: &str, name: &str) -> Result<Option<String>, McpError> {
        let _guard = self
            .access_lock
            .lock()
            .map_err(|_| McpError::Secret("credential lock was poisoned".into()))?;
        match self.entry(server, name)?.get_password() {
            Ok(value) => Ok(Some(value)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(error) => Err(McpError::Secret(error.to_string())),
        }
    }

    fn set(&self, server: &str, name: &str, value: &str) -> Result<(), McpError> {
        if value.trim().is_empty() {
            return Err(McpError::Secret("secret must not be empty".into()));
        }
        let _guard = self
            .access_lock
            .lock()
            .map_err(|_| McpError::Secret("credential lock was poisoned".into()))?;
        self.entry(server, name)?
            .set_password(value)
            .map_err(|error| McpError::Secret(error.to_string()))
    }

    fn delete(&self, server: &str, name: &str) -> Result<(), McpError> {
        let _guard = self
            .access_lock
            .lock()
            .map_err(|_| McpError::Secret("credential lock was poisoned".into()))?;
        match self.entry(server, name)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(error) => Err(McpError::Secret(error.to_string())),
        }
    }
}

#[cfg(test)]
impl McpSecretStore for OsMcpSecretStore {
    fn get(&self, _server: &str, _name: &str) -> Result<Option<String>, McpError> {
        Ok(None)
    }

    fn set(&self, _server: &str, _name: &str, _value: &str) -> Result<(), McpError> {
        Err(McpError::Secret(
            "native credential access is disabled in tests".into(),
        ))
    }

    fn delete(&self, _server: &str, _name: &str) -> Result<(), McpError> {
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "transport", rename_all = "snake_case", deny_unknown_fields)]
pub enum McpTransportConfig {
    Stdio {
        command: Vec<String>,
        #[serde(default)]
        secret_env: HashMap<String, String>,
    },
    StreamableHttp {
        url: String,
        #[serde(default)]
        secret_headers: HashMap<String, String>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct McpServerConfig {
    pub id: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_timeout")]
    pub timeout_ms: u64,
    #[serde(flatten)]
    pub transport: McpTransportConfig,
}

fn default_true() -> bool {
    true
}

fn default_timeout() -> u64 {
    DEFAULT_TIMEOUT_MS
}

impl McpServerConfig {
    pub fn validate(&self) -> Result<(), McpError> {
        if !valid_identifier(&self.id) {
            return Err(McpError::Config(format!(
                "MCP server id {} must use lowercase letters, numbers, '_' or '-'",
                self.id
            )));
        }
        if self.timeout_ms == 0 || self.timeout_ms > MAX_TIMEOUT_MS {
            return Err(McpError::Config(format!(
                "MCP server {} timeout must be between 1 and {MAX_TIMEOUT_MS} ms",
                self.id
            )));
        }
        match &self.transport {
            McpTransportConfig::Stdio {
                command,
                secret_env,
            } => {
                if command.is_empty()
                    || command.len() > 128
                    || command.iter().any(|part| part.len() > 8192)
                {
                    return Err(McpError::Config(format!(
                        "MCP server {} has an invalid structured command",
                        self.id
                    )));
                }
                for (environment, credential) in secret_env {
                    if !valid_environment_name(environment) || credential.trim().is_empty() {
                        return Err(McpError::Config(format!(
                            "MCP server {} has an invalid secret environment mapping",
                            self.id
                        )));
                    }
                }
            }
            McpTransportConfig::StreamableHttp {
                url,
                secret_headers,
            } => {
                validate_http_url(url)?;
                for (header, credential) in secret_headers {
                    HeaderName::from_bytes(header.as_bytes()).map_err(|_| {
                        McpError::Config(format!(
                            "MCP server {} has an invalid secret header name",
                            self.id
                        ))
                    })?;
                    if credential.trim().is_empty() {
                        return Err(McpError::Config(format!(
                            "MCP server {} has an empty credential name",
                            self.id
                        )));
                    }
                }
            }
        }
        Ok(())
    }

    pub fn transport_name(&self) -> &'static str {
        match self.transport {
            McpTransportConfig::Stdio { .. } => "stdio",
            McpTransportConfig::StreamableHttp { .. } => "streamable_http",
        }
    }

    pub fn credential_names(&self) -> Vec<String> {
        let mut names = match &self.transport {
            McpTransportConfig::Stdio { secret_env, .. } => {
                secret_env.values().cloned().collect::<Vec<_>>()
            }
            McpTransportConfig::StreamableHttp { secret_headers, .. } => {
                secret_headers.values().cloned().collect::<Vec<_>>()
            }
        };
        names.sort();
        names.dedup();
        names
    }
}

#[derive(Debug, thiserror::Error)]
pub enum McpError {
    #[error("MCP configuration failed: {0}")]
    Config(String),
    #[error("MCP transport failed: {0}")]
    Transport(String),
    #[error("MCP protocol failed: {0}")]
    Protocol(String),
    #[error("MCP credential store failed: {0}")]
    Secret(String),
    #[error("MCP request timed out")]
    Timeout,
    #[error("MCP request was cancelled")]
    Cancelled,
}

#[derive(Debug, Clone)]
pub struct DiscoveredMcpTool {
    pub name: String,
    pub remote_name: String,
    pub description: String,
    pub input_schema: Value,
    pub risk: ToolRisk,
    client: Arc<dyn McpClient>,
}

impl DiscoveredMcpTool {
    pub fn handler(&self) -> Arc<dyn ToolHandler> {
        Arc::new(McpToolHandler {
            definition: ToolDefinition {
                name: self.name.clone(),
                description: self.description.clone(),
                input_schema: self.input_schema.clone(),
            },
            remote_name: self.remote_name.clone(),
            client: self.client.clone(),
        })
    }
}

#[async_trait]
trait McpClient: Send + Sync + std::fmt::Debug {
    async fn request(
        &self,
        method: &str,
        params: Value,
        cancellation: CancellationToken,
    ) -> Result<Value, McpError>;
}

#[derive(Debug)]
struct McpToolHandler {
    definition: ToolDefinition,
    remote_name: String,
    client: Arc<dyn McpClient>,
}

#[async_trait]
impl ToolHandler for McpToolHandler {
    fn definition(&self) -> ToolDefinition {
        self.definition.clone()
    }

    async fn execute(
        &self,
        _context: &ToolContext,
        arguments: Value,
        cancellation: CancellationToken,
    ) -> Result<ToolResult, ToolError> {
        let value = self
            .client
            .request(
                "tools/call",
                json!({ "name": self.remote_name, "arguments": arguments }),
                cancellation,
            )
            .await
            .map_err(|error| match error {
                McpError::Cancelled => ToolError::Cancelled,
                other => ToolError::Execution(other.to_string()),
            })?;
        tool_result(value)
    }
}

pub async fn connect(
    config: &McpServerConfig,
    secrets: Arc<dyn McpSecretStore>,
    cancellation: CancellationToken,
) -> Result<Vec<DiscoveredMcpTool>, McpError> {
    config.validate()?;
    let client: Arc<dyn McpClient> = match &config.transport {
        McpTransportConfig::Stdio {
            command,
            secret_env,
        } => Arc::new(
            StdioClient::start(&config.id, command, secret_env, secrets, config.timeout_ms).await?,
        ),
        McpTransportConfig::StreamableHttp {
            url,
            secret_headers,
        } => Arc::new(HttpClient::new(
            &config.id,
            url,
            secret_headers,
            secrets,
            config.timeout_ms,
        )?),
    };
    initialize(client.as_ref(), cancellation.clone()).await?;
    list_tools(&config.id, client, cancellation).await
}

async fn initialize(
    client: &dyn McpClient,
    cancellation: CancellationToken,
) -> Result<(), McpError> {
    let result = client
        .request(
            "initialize",
            json!({
                "protocolVersion": MCP_PROTOCOL_VERSION,
                "capabilities": {},
                "clientInfo": { "name": "k-coder", "version": env!("CARGO_PKG_VERSION") }
            }),
            cancellation.clone(),
        )
        .await?;
    let version = result
        .get("protocolVersion")
        .and_then(Value::as_str)
        .ok_or_else(|| McpError::Protocol("initialize result omitted protocolVersion".into()))?;
    if version != MCP_PROTOCOL_VERSION {
        return Err(McpError::Protocol(format!(
            "server selected unsupported protocol version {version}"
        )));
    }
    if result.pointer("/capabilities/tools").is_none() {
        return Err(McpError::Protocol(
            "server did not declare the tools capability".into(),
        ));
    }
    client
        .request("notifications/initialized", Value::Null, cancellation)
        .await?;
    Ok(())
}

async fn list_tools(
    server: &str,
    client: Arc<dyn McpClient>,
    cancellation: CancellationToken,
) -> Result<Vec<DiscoveredMcpTool>, McpError> {
    let mut cursor = None::<String>;
    let mut discovered = Vec::new();
    for _ in 0..20 {
        let params = cursor
            .as_ref()
            .map(|value| json!({ "cursor": value }))
            .unwrap_or_else(|| json!({}));
        let result = client
            .request("tools/list", params, cancellation.clone())
            .await?;
        let tools = result
            .get("tools")
            .and_then(Value::as_array)
            .ok_or_else(|| McpError::Protocol("tools/list result omitted tools".into()))?;
        for tool in tools {
            if discovered.len() >= MAX_MCP_TOOLS {
                return Err(McpError::Protocol(format!(
                    "MCP server exposed more than {MAX_MCP_TOOLS} tools"
                )));
            }
            let remote_name = tool
                .get("name")
                .and_then(Value::as_str)
                .ok_or_else(|| McpError::Protocol("MCP tool omitted name".into()))?;
            let safe_name = sanitize_tool_name(remote_name)?;
            let input_schema = tool
                .get("inputSchema")
                .cloned()
                .unwrap_or_else(|| json!({ "type": "object" }));
            jsonschema::validator_for(&input_schema).map_err(|error| {
                McpError::Protocol(format!(
                    "tool {remote_name} has invalid inputSchema: {error}"
                ))
            })?;
            discovered.push(DiscoveredMcpTool {
                name: format!("mcp__{}__{}", server.replace('-', "_"), safe_name),
                remote_name: remote_name.to_string(),
                description: bound_text(
                    tool.get("description")
                        .and_then(Value::as_str)
                        .unwrap_or("External MCP tool"),
                    MAX_TOOL_DESCRIPTION_BYTES,
                ),
                input_schema,
                risk: annotation_risk(tool.get("annotations")),
                client: client.clone(),
            });
        }
        cursor = result
            .get("nextCursor")
            .and_then(Value::as_str)
            .map(str::to_owned);
        if cursor.is_none() {
            return Ok(discovered);
        }
    }
    Err(McpError::Protocol(
        "tools/list exceeded 20 pagination requests".into(),
    ))
}

#[derive(Debug)]
struct StdioSession {
    _child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: u64,
}

#[derive(Debug)]
struct StdioClient {
    session: Mutex<StdioSession>,
    timeout: Duration,
}

impl StdioClient {
    async fn start(
        server: &str,
        command: &[String],
        secret_env: &HashMap<String, String>,
        secrets: Arc<dyn McpSecretStore>,
        timeout_ms: u64,
    ) -> Result<Self, McpError> {
        let mut environment = safe_process_environment();
        for (key, credential) in secret_env {
            let value = secrets.get(server, credential)?.ok_or_else(|| {
                McpError::Secret(format!(
                    "MCP server {server} requires credential {credential}"
                ))
            })?;
            environment.insert(key.clone(), value);
        }
        let mut process = Command::new(&command[0]);
        process
            .args(&command[1..])
            .env_clear()
            .envs(environment)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true);
        let mut child = process
            .spawn()
            .map_err(|error| McpError::Transport(format!("{server} failed to start: {error}")))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| McpError::Transport(format!("{server} stdin is unavailable")))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| McpError::Transport(format!("{server} stdout is unavailable")))?;
        Ok(Self {
            session: Mutex::new(StdioSession {
                _child: child,
                stdin,
                stdout: BufReader::new(stdout),
                next_id: 1,
            }),
            timeout: Duration::from_millis(timeout_ms),
        })
    }
}

#[async_trait]
impl McpClient for StdioClient {
    async fn request(
        &self,
        method: &str,
        params: Value,
        cancellation: CancellationToken,
    ) -> Result<Value, McpError> {
        let mut session = self.session.lock().await;
        let is_notification = method.starts_with("notifications/");
        let id = (!is_notification).then(|| {
            let id = session.next_id;
            session.next_id = session.next_id.saturating_add(1);
            id
        });
        let mut message = json!({ "jsonrpc": "2.0", "method": method });
        if !params.is_null() {
            message["params"] = params;
        }
        if let Some(id) = id {
            message["id"] = json!(id);
        }
        let mut bytes =
            serde_json::to_vec(&message).map_err(|error| McpError::Protocol(error.to_string()))?;
        bytes.push(b'\n');
        session
            .stdin
            .write_all(&bytes)
            .await
            .map_err(|error| McpError::Transport(error.to_string()))?;
        session
            .stdin
            .flush()
            .await
            .map_err(|error| McpError::Transport(error.to_string()))?;
        if id.is_none() {
            return Ok(json!({}));
        }
        let future = read_stdio_response(&mut session, id.unwrap());
        tokio::select! {
            _ = cancellation.cancelled() => Err(McpError::Cancelled),
            result = tokio::time::timeout(self.timeout, future) => {
                result.map_err(|_| McpError::Timeout)?
            }
        }
    }
}

async fn read_stdio_response(
    session: &mut StdioSession,
    expected_id: u64,
) -> Result<Value, McpError> {
    loop {
        let line = read_json_line(&mut session.stdout).await?;
        let value: Value = serde_json::from_slice(&line).map_err(|error| {
            McpError::Protocol(format!("server wrote invalid JSON-RPC to stdout: {error}"))
        })?;
        if value.get("id").and_then(Value::as_u64) == Some(expected_id) {
            return response_result(value);
        }
        if value.get("id").is_some() && value.get("method").is_some() {
            let response = json!({
                "jsonrpc": "2.0",
                "id": value["id"],
                "error": { "code": -32601, "message": "client method is not supported" }
            });
            let mut bytes = serde_json::to_vec(&response)
                .map_err(|error| McpError::Protocol(error.to_string()))?;
            bytes.push(b'\n');
            session
                .stdin
                .write_all(&bytes)
                .await
                .map_err(|error| McpError::Transport(error.to_string()))?;
            session
                .stdin
                .flush()
                .await
                .map_err(|error| McpError::Transport(error.to_string()))?;
        }
    }
}

#[derive(Debug)]
struct HttpClient {
    client: reqwest::Client,
    url: String,
    headers: HeaderMap,
    session_id: RwLock<Option<String>>,
    next_id: Mutex<u64>,
    timeout: Duration,
}

impl HttpClient {
    fn new(
        server: &str,
        url: &str,
        secret_headers: &HashMap<String, String>,
        secrets: Arc<dyn McpSecretStore>,
        timeout_ms: u64,
    ) -> Result<Self, McpError> {
        validate_http_url(url)?;
        let mut headers = HeaderMap::new();
        for (name, credential) in secret_headers {
            let value = secrets.get(server, credential)?.ok_or_else(|| {
                McpError::Secret(format!(
                    "MCP server {server} requires credential {credential}"
                ))
            })?;
            headers.insert(
                HeaderName::from_bytes(name.as_bytes())
                    .map_err(|error| McpError::Config(error.to_string()))?,
                HeaderValue::from_str(&value).map_err(|_| {
                    McpError::Secret("secret is not a valid HTTP header value".into())
                })?,
            );
        }
        Ok(Self {
            client: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .map_err(|error| McpError::Transport(error.to_string()))?,
            url: url.into(),
            headers,
            session_id: RwLock::new(None),
            next_id: Mutex::new(1),
            timeout: Duration::from_millis(timeout_ms),
        })
    }
}

#[async_trait]
impl McpClient for HttpClient {
    async fn request(
        &self,
        method: &str,
        params: Value,
        cancellation: CancellationToken,
    ) -> Result<Value, McpError> {
        let is_notification = method.starts_with("notifications/");
        let id = if is_notification {
            None
        } else {
            let mut next = self.next_id.lock().await;
            let value = *next;
            *next = next.saturating_add(1);
            Some(value)
        };
        let mut message = json!({ "jsonrpc": "2.0", "method": method });
        if !params.is_null() {
            message["params"] = params;
        }
        if let Some(id) = id {
            message["id"] = json!(id);
        }
        let mut request = self
            .client
            .post(&self.url)
            .headers(self.headers.clone())
            .header(ACCEPT, "application/json, text/event-stream")
            .header(CONTENT_TYPE, "application/json")
            .header("MCP-Protocol-Version", MCP_PROTOCOL_VERSION)
            .json(&message);
        if let Some(session) = self.session_id.read().await.as_ref() {
            request = request.header("MCP-Session-Id", session);
        }
        let response = tokio::select! {
            _ = cancellation.cancelled() => return Err(McpError::Cancelled),
            response = tokio::time::timeout(self.timeout, request.send()) => {
                response.map_err(|_| McpError::Timeout)?
                    .map_err(|error| McpError::Transport(error.to_string()))?
            }
        };
        if let Some(session) = response
            .headers()
            .get("MCP-Session-Id")
            .and_then(|value| value.to_str().ok())
        {
            *self.session_id.write().await = Some(session.to_string());
        }
        if !response.status().is_success() {
            return Err(McpError::Transport(format!(
                "HTTP MCP returned {}",
                response.status()
            )));
        }
        if is_notification {
            return Ok(json!({}));
        }
        let content_type = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or("")
            .to_string();
        let bytes = tokio::select! {
            _ = cancellation.cancelled() => return Err(McpError::Cancelled),
            bytes = tokio::time::timeout(self.timeout, read_http_body(response)) => {
                bytes.map_err(|_| McpError::Timeout)?
                    ?
            }
        };
        let value = if content_type.starts_with("text/event-stream") {
            parse_sse_response(&bytes, id.unwrap())?
        } else {
            serde_json::from_slice(&bytes).map_err(|error| {
                McpError::Protocol(format!("invalid JSON-RPC response: {error}"))
            })?
        };
        response_result(value)
    }
}

async fn read_json_line(reader: &mut BufReader<ChildStdout>) -> Result<Vec<u8>, McpError> {
    let mut result = Vec::new();
    loop {
        let buffer = reader
            .fill_buf()
            .await
            .map_err(|error| McpError::Transport(error.to_string()))?;
        if buffer.is_empty() {
            return Err(McpError::Transport("MCP server closed stdout".into()));
        }
        let take = buffer
            .iter()
            .position(|byte| *byte == b'\n')
            .map(|index| index + 1)
            .unwrap_or(buffer.len());
        if result.len().saturating_add(take) > MAX_MCP_MESSAGE_BYTES {
            return Err(McpError::Protocol("MCP message exceeded 1 MiB".into()));
        }
        result.extend_from_slice(&buffer[..take]);
        reader.consume(take);
        if result.last() == Some(&b'\n') {
            while matches!(result.last(), Some(b'\n' | b'\r')) {
                result.pop();
            }
            return Ok(result);
        }
    }
}

async fn read_http_body(response: reqwest::Response) -> Result<Vec<u8>, McpError> {
    let mut stream = response.bytes_stream();
    let mut result = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|error| McpError::Transport(error.to_string()))?;
        if result.len().saturating_add(chunk.len()) > MAX_MCP_MESSAGE_BYTES {
            return Err(McpError::Protocol(
                "HTTP MCP response exceeded 1 MiB".into(),
            ));
        }
        result.extend_from_slice(&chunk);
    }
    Ok(result)
}

fn response_result(value: Value) -> Result<Value, McpError> {
    if let Some(error) = value.get("error") {
        let code = error.get("code").and_then(Value::as_i64).unwrap_or(-32000);
        return Err(McpError::Protocol(format!(
            "server returned JSON-RPC error code {code}"
        )));
    }
    value
        .get("result")
        .cloned()
        .ok_or_else(|| McpError::Protocol("JSON-RPC response omitted result".into()))
}

fn parse_sse_response(bytes: &[u8], expected_id: u64) -> Result<Value, McpError> {
    let text = std::str::from_utf8(bytes)
        .map_err(|error| McpError::Protocol(format!("SSE response is not UTF-8: {error}")))?;
    for line in text.lines() {
        let Some(data) = line.strip_prefix("data:") else {
            continue;
        };
        let value: Value = serde_json::from_str(data.trim())
            .map_err(|error| McpError::Protocol(format!("invalid SSE JSON-RPC: {error}")))?;
        if value.get("id").and_then(Value::as_u64) == Some(expected_id) {
            return Ok(value);
        }
    }
    Err(McpError::Protocol(
        "SSE response did not contain the matching JSON-RPC response".into(),
    ))
}

fn tool_result(value: Value) -> Result<ToolResult, ToolError> {
    let is_error = value
        .get("isError")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let mut parts = Vec::new();
    if let Some(content) = value.get("content").and_then(Value::as_array) {
        for item in content {
            if item.get("type").and_then(Value::as_str) == Some("text") {
                if let Some(text) = item.get("text").and_then(Value::as_str) {
                    parts.push(text.to_string());
                }
            }
        }
    }
    if let Some(structured) = value.get("structuredContent") {
        parts.push(
            serde_json::to_string_pretty(structured)
                .map_err(|error| ToolError::Execution(error.to_string()))?,
        );
    }
    let output = parts.join("\n");
    if output.len() > MAX_MCP_OUTPUT_BYTES {
        return Err(ToolError::Execution(format!(
            "MCP tool output exceeded {MAX_MCP_OUTPUT_BYTES} bytes"
        )));
    }
    Ok(ToolResult {
        success: !is_error,
        output,
        metadata: json!({ "mcp": true }),
    })
}

fn annotation_risk(annotations: Option<&Value>) -> ToolRisk {
    let destructive = annotations
        .and_then(|value| value.get("destructiveHint"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let read_only = annotations
        .and_then(|value| value.get("readOnlyHint"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let open_world = annotations
        .and_then(|value| value.get("openWorldHint"))
        .and_then(Value::as_bool)
        .unwrap_or(true);
    if destructive {
        ToolRisk::Delete
    } else if read_only && !open_world {
        ToolRisk::Read
    } else if read_only {
        ToolRisk::External
    } else {
        ToolRisk::Write
    }
}

fn sanitize_tool_name(name: &str) -> Result<String, McpError> {
    if name.is_empty() || name.len() > 128 {
        return Err(McpError::Protocol("MCP tool name is invalid".into()));
    }
    let value = name
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '_' {
                character.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect::<String>();
    if value.trim_matches('_').is_empty() {
        return Err(McpError::Protocol(format!(
            "MCP tool name {name} cannot be namespaced safely"
        )));
    }
    Ok(value)
}

fn valid_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value.chars().all(|character| {
            character.is_ascii_lowercase()
                || character.is_ascii_digit()
                || matches!(character, '_' | '-')
        })
}

fn valid_environment_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value.chars().all(|character| {
            character.is_ascii_uppercase() || character.is_ascii_digit() || character == '_'
        })
}

fn validate_http_url(value: &str) -> Result<(), McpError> {
    let url = url::Url::parse(value).map_err(|error| McpError::Config(error.to_string()))?;
    let secure = url.scheme() == "https";
    let loopback = url.scheme() == "http"
        && url
            .host_str()
            .is_some_and(|host| matches!(host, "localhost" | "127.0.0.1" | "::1"));
    if !secure && !loopback {
        return Err(McpError::Config(
            "Streamable HTTP MCP must use HTTPS or a loopback HTTP URL".into(),
        ));
    }
    if !url.username().is_empty() || url.password().is_some() || url.fragment().is_some() {
        return Err(McpError::Config(
            "MCP URL must not contain credentials or a fragment".into(),
        ));
    }
    Ok(())
}

fn safe_process_environment() -> HashMap<String, String> {
    [
        "PATH",
        "PATHEXT",
        "SYSTEMROOT",
        "WINDIR",
        "HOME",
        "TMP",
        "TEMP",
    ]
    .into_iter()
    .filter_map(|key| std::env::var(key).ok().map(|value| (key.into(), value)))
    .collect()
}

fn bound_text(value: &str, limit: usize) -> String {
    if value.len() <= limit {
        return value.to_string();
    }
    let mut end = limit;
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}...", &value[..end])
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    struct FakeSecrets(HashMap<(String, String), String>);

    impl McpSecretStore for FakeSecrets {
        fn get(&self, server: &str, name: &str) -> Result<Option<String>, McpError> {
            Ok(self.0.get(&(server.into(), name.into())).cloned())
        }

        fn set(&self, _server: &str, _name: &str, _value: &str) -> Result<(), McpError> {
            Ok(())
        }

        fn delete(&self, _server: &str, _name: &str) -> Result<(), McpError> {
            Ok(())
        }
    }

    #[test]
    fn namespaces_tools_and_derives_conservative_risk() {
        assert_eq!(sanitize_tool_name("Get-Issue").unwrap(), "get_issue");
        assert_eq!(
            annotation_risk(Some(
                &json!({ "readOnlyHint": true, "openWorldHint": false })
            )),
            ToolRisk::Read
        );
        assert_eq!(
            annotation_risk(Some(&json!({ "destructiveHint": true }))),
            ToolRisk::Delete
        );
    }

    #[test]
    fn validates_http_and_stdio_configuration() {
        assert!(validate_http_url("https://example.com/mcp").is_ok());
        assert!(validate_http_url("http://127.0.0.1:3000/mcp").is_ok());
        assert!(validate_http_url("http://example.com/mcp").is_err());
        let config = McpServerConfig {
            id: "local".into(),
            enabled: true,
            timeout_ms: 1000,
            transport: McpTransportConfig::Stdio {
                command: vec!["server".into()],
                secret_env: HashMap::new(),
            },
        };
        assert!(config.validate().is_ok());
    }

    #[test]
    fn parses_bounded_tool_results() {
        let result = tool_result(json!({
            "content": [{ "type": "text", "text": "done" }],
            "isError": false
        }))
        .unwrap();
        assert!(result.success);
        assert_eq!(result.output, "done");
    }

    #[tokio::test]
    async fn stdio_initializes_lists_and_calls_namespaced_tools_with_injected_secrets() {
        let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("test-fixtures")
            .join("mcp-server.mjs");
        let config = McpServerConfig {
            id: "fixture".into(),
            enabled: true,
            timeout_ms: 10_000,
            transport: McpTransportConfig::Stdio {
                command: vec!["node".into(), fixture.to_string_lossy().to_string()],
                secret_env: HashMap::from([("TEST_SECRET".into(), "token".into())]),
            },
        };
        let secrets = Arc::new(FakeSecrets(HashMap::from([(
            ("fixture".into(), "token".into()),
            "hidden-value".into(),
        )])));
        let tools = connect(&config, secrets, CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "mcp__fixture__echo_text");
        assert_eq!(tools[0].risk, ToolRisk::Read);
        let workspace = tempfile::tempdir().unwrap();
        let result = tools[0]
            .handler()
            .execute(
                &ToolContext {
                    thread_id: "thread".into(),
                    turn_id: "turn".into(),
                    call_id: "call".into(),
                    workspace_root: workspace.path().into(),
                    approval: None,
                },
                json!({ "text": "hello" }),
                CancellationToken::new(),
            )
            .await
            .unwrap();
        assert_eq!(result.output, "echo:hello");
        assert!(!format!("{result:?}").contains("hidden-value"));
    }

    #[tokio::test]
    async fn streamable_http_initializes_and_calls_with_secret_headers() {
        let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("test-fixtures")
            .join("mcp-http-server.mjs");
        let mut child = Command::new("node")
            .arg(fixture)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let stdout = child.stdout.take().unwrap();
        let mut reader = BufReader::new(stdout);
        let mut port = String::new();
        reader.read_line(&mut port).await.unwrap();
        let config = McpServerConfig {
            id: "remote".into(),
            enabled: true,
            timeout_ms: 10_000,
            transport: McpTransportConfig::StreamableHttp {
                url: format!("http://127.0.0.1:{}/mcp", port.trim()),
                secret_headers: HashMap::from([("Authorization".into(), "token".into())]),
            },
        };
        let secrets = Arc::new(FakeSecrets(HashMap::from([(
            ("remote".into(), "token".into()),
            "Bearer hidden-value".into(),
        )])));
        let tools = connect(&config, secrets, CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "mcp__remote__remote_read");
        assert_eq!(tools[0].risk, ToolRisk::External);
        let workspace = tempfile::tempdir().unwrap();
        let result = tools[0]
            .handler()
            .execute(
                &ToolContext {
                    thread_id: "thread".into(),
                    turn_id: "turn".into(),
                    call_id: "call".into(),
                    workspace_root: workspace.path().into(),
                    approval: None,
                },
                json!({}),
                CancellationToken::new(),
            )
            .await
            .unwrap();
        assert_eq!(result.output, "remote result");
        let _ = child.kill().await;
    }
}
