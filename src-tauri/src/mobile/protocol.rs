//! 移动网关的 JSON-RPC 2.0 载荷与结构化错误。
//!
//! 网关只做协议、身份和连接管理；这里的类型是跨边界传输的版本化载荷，
//! 与 `crate::protocol` 的领域事件载荷保持独立，避免移动端协议演进污染桌面协议。

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// 移动网关的 JSON-RPC 协议版本。客户端在 `initialize` 中声明，服务端据此协商。
pub const MOBILE_PROTOCOL_VERSION: u32 = 1;
/// 事件协议版本。与领域事件 `schema_version` 分开，便于单独演进投递语义。
pub const EVENT_PROTOCOL_VERSION: u32 = 1;

/// 客户端发来的 JSON-RPC 请求或通知。`id` 缺失表示通知。
#[derive(Debug, Clone, Deserialize)]
pub struct RpcRequest {
    pub jsonrpc: Option<String>,
    #[serde(default)]
    pub id: Option<Value>,
    pub method: String,
    #[serde(default)]
    pub params: Option<Value>,
}

impl RpcRequest {
    /// 通知没有 `id`，服务端不得回包。
    pub fn is_notification(&self) -> bool {
        self.id.is_none()
    }

    /// 参数必须是对象；缺省时视为空对象。
    pub fn params_object(&self) -> Result<serde_json::Map<String, Value>, MobileError> {
        match self.params.as_ref() {
            None | Some(Value::Null) => Ok(serde_json::Map::new()),
            Some(Value::Object(map)) => Ok(map.clone()),
            Some(_) => Err(MobileError::new(
                MobileErrorKind::InvalidParams,
                "params must be a JSON object",
            )),
        }
    }
}

/// 服务端回包。成功时 `result` 必须出现（允许为 `null`），失败时只出现 `error`。
#[derive(Debug, Clone, Serialize)]
pub struct RpcResponse {
    pub jsonrpc: &'static str,
    pub id: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<MobileError>,
}

impl RpcResponse {
    pub fn success(id: Value, result: Value) -> Self {
        Self {
            jsonrpc: "2.0",
            id,
            result: Some(result),
            error: None,
        }
    }

    pub fn failure(id: Value, error: MobileError) -> Self {
        Self {
            jsonrpc: "2.0",
            id,
            result: None,
            error: Some(error),
        }
    }
}

/// 服务端主动下发的通知（无 `id`）。
#[derive(Debug, Clone, Serialize)]
pub struct RpcNotification {
    pub jsonrpc: &'static str,
    pub method: &'static str,
    pub params: Value,
}

impl RpcNotification {
    pub fn new(method: &'static str, params: Value) -> Self {
        Self {
            jsonrpc: "2.0",
            method,
            params,
        }
    }
}

/// 结构化错误种类。客户端按 `kind` 决策，不解析 `message` 文本。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MobileErrorKind {
    ParseError,
    InvalidRequest,
    MethodNotFound,
    InvalidParams,
    Unauthorized,
    Forbidden,
    NotFound,
    StaleRequest,
    RateLimited,
    ServerOverloaded,
    ResumeRequired,
    UnsupportedCapability,
    Internal,
}

impl MobileErrorKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ParseError => "parse_error",
            Self::InvalidRequest => "invalid_request",
            Self::MethodNotFound => "method_not_found",
            Self::InvalidParams => "invalid_params",
            Self::Unauthorized => "unauthorized",
            Self::Forbidden => "forbidden",
            Self::NotFound => "not_found",
            Self::StaleRequest => "stale_request",
            Self::RateLimited => "rate_limited",
            Self::ServerOverloaded => "server_overloaded",
            Self::ResumeRequired => "resume_required",
            Self::UnsupportedCapability => "unsupported_capability",
            Self::Internal => "internal_error",
        }
    }

    /// JSON-RPC 标准错误码；服务端错误落在 -32000..=-32099 区间。
    pub fn rpc_code(self) -> i32 {
        match self {
            Self::ParseError => -32700,
            Self::InvalidRequest => -32600,
            Self::MethodNotFound => -32601,
            Self::InvalidParams => -32602,
            Self::Internal => -32603,
            Self::Unauthorized => -32001,
            Self::Forbidden => -32002,
            Self::NotFound => -32004,
            Self::StaleRequest => -32009,
            Self::RateLimited => -32010,
            Self::ServerOverloaded => -32011,
            Self::ResumeRequired => -32012,
            Self::UnsupportedCapability => -32013,
        }
    }

    /// 客户端是否可以原样重试。只有瞬态失败才可重试。
    pub fn retryable(self) -> bool {
        matches!(
            self,
            Self::RateLimited | Self::ServerOverloaded | Self::Internal
        )
    }

    /// 是否属于「请求已失效，需要重新读取状态」的一类。
    pub fn requires_resync(self) -> bool {
        matches!(self, Self::StaleRequest | Self::ResumeRequired)
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MobileErrorData {
    pub kind: &'static str,
    pub retryable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry_after_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
}

#[derive(Debug, Clone, Serialize)]
pub struct MobileError {
    pub code: i32,
    pub message: String,
    pub data: MobileErrorData,
}

/// 回给客户端或写入日志的错误消息上限，避免把整段工具输出或路径带出去。
pub const MAX_ERROR_MESSAGE_CHARS: usize = 400;

impl MobileError {
    pub fn new(kind: MobileErrorKind, message: impl Into<String>) -> Self {
        Self {
            code: kind.rpc_code(),
            message: truncate_chars(&message.into(), MAX_ERROR_MESSAGE_CHARS),
            data: MobileErrorData {
                kind: kind.as_str(),
                retryable: kind.retryable(),
                retry_after_ms: None,
                details: None,
            },
        }
    }

    pub fn kind(&self) -> &'static str {
        self.data.kind
    }

    pub fn with_retry_after_ms(mut self, retry_after_ms: u64) -> Self {
        self.data.retry_after_ms = Some(retry_after_ms);
        self.data.retryable = true;
        self
    }

    pub fn with_details(mut self, details: Value) -> Self {
        self.data.details = Some(details);
        self
    }

    pub fn unauthorized(message: impl Into<String>) -> Self {
        Self::new(MobileErrorKind::Unauthorized, message)
    }

    pub fn forbidden(message: impl Into<String>) -> Self {
        Self::new(MobileErrorKind::Forbidden, message)
    }

    pub fn invalid_params(message: impl Into<String>) -> Self {
        Self::new(MobileErrorKind::InvalidParams, message)
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(MobileErrorKind::NotFound, message)
    }

    pub fn stale_request(message: impl Into<String>) -> Self {
        Self::new(MobileErrorKind::StaleRequest, message)
    }

    pub fn unsupported_capability(capability: &str) -> Self {
        Self::new(
            MobileErrorKind::UnsupportedCapability,
            format!("mobile capability is not granted: {capability}"),
        )
        .with_details(serde_json::json!({ "capability": capability }))
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(MobileErrorKind::Internal, message)
    }
}

impl std::fmt::Display for MobileError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}: {}", self.data.kind, self.message)
    }
}

impl std::error::Error for MobileError {}

/// 现有 Tauri 边界错误的迁移：保留可识别错误码，其余收敛为内部错误。
///
/// 不直接透传未知错误的原始文本，避免把工作区绝对路径等细节带到手机上。
impl From<crate::commands::CommandError> for MobileError {
    fn from(error: crate::commands::CommandError) -> Self {
        let kind = match error.code {
            "unauthorized" => MobileErrorKind::Unauthorized,
            "forbidden" => MobileErrorKind::Forbidden,
            "not_found" => MobileErrorKind::NotFound,
            "invalid_request" => MobileErrorKind::InvalidParams,
            "no_active_turn" | "turn_mismatch" | "stale_request" => MobileErrorKind::StaleRequest,
            "rate_limited" => MobileErrorKind::RateLimited,
            _ => MobileErrorKind::Internal,
        };
        Self::new(kind, error.message)
    }
}

/// 按字符（而非字节）截断，保证不会截断 UTF-8 序列。
pub fn truncate_chars(value: &str, limit: usize) -> String {
    if value.chars().count() <= limit {
        return value.to_string();
    }
    let mut truncated: String = value.chars().take(limit).collect();
    truncated.push('…');
    truncated
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notification_has_no_id() {
        let request: RpcRequest =
            serde_json::from_str(r#"{"jsonrpc":"2.0","method":"initialized"}"#).unwrap();
        assert!(request.is_notification());
        assert!(request.params_object().unwrap().is_empty());
    }

    #[test]
    fn non_object_params_are_rejected() {
        let request: RpcRequest =
            serde_json::from_str(r#"{"jsonrpc":"2.0","id":1,"method":"ping","params":[1,2]}"#)
                .unwrap();
        let error = request.params_object().unwrap_err();
        assert_eq!(error.kind(), "invalid_params");
    }

    #[test]
    fn success_response_keeps_null_result() {
        let response = RpcResponse::success(Value::from(7), Value::Null);
        let encoded = serde_json::to_value(&response).unwrap();
        assert_eq!(encoded["id"], serde_json::json!(7));
        assert!(encoded.get("result").is_some());
        assert!(encoded.get("error").is_none());
    }

    #[test]
    fn structured_error_exposes_kind_and_retryability() {
        let error = MobileError::new(MobileErrorKind::RateLimited, "slow down");
        let encoded = serde_json::to_value(&error).unwrap();
        assert_eq!(encoded["code"], serde_json::json!(-32010));
        assert_eq!(encoded["data"]["kind"], serde_json::json!("rate_limited"));
        assert_eq!(encoded["data"]["retryable"], serde_json::json!(true));

        let stale = MobileError::stale_request("expired");
        assert_eq!(stale.data.retryable, false);
        assert!(MobileErrorKind::StaleRequest.requires_resync());
    }

    #[test]
    fn retry_after_is_attached_to_details() {
        let error =
            MobileError::new(MobileErrorKind::RateLimited, "slow down").with_retry_after_ms(1_500);
        assert_eq!(error.data.retry_after_ms, Some(1_500));
        assert_eq!(error.data.retryable, true);
    }

    #[test]
    fn long_messages_are_truncated_by_chars() {
        let message = "绝".repeat(MAX_ERROR_MESSAGE_CHARS + 50);
        let error = MobileError::new(MobileErrorKind::Internal, message);
        assert_eq!(error.message.chars().count(), MAX_ERROR_MESSAGE_CHARS + 1);
    }

    #[test]
    fn command_error_maps_to_structured_kind() {
        let mapped: MobileError =
            crate::commands::CommandError::new("turn_mismatch", "stale").into();
        assert_eq!(mapped.kind(), "stale_request");
        assert!(MobileErrorKind::StaleRequest.requires_resync());
    }
}
