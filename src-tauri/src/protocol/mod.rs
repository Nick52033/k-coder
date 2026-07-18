use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeStatus {
    pub ready: bool,
    pub phase: String,
    pub version: String,
    pub uptime_seconds: u64,
    pub capabilities: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentEvent {
    TurnStarted {
        thread_id: String,
        turn_id: String,
    },
    TextDelta {
        thread_id: String,
        delta: String,
    },
    ToolStarted {
        thread_id: String,
        call_id: String,
        tool: String,
    },
    ToolCompleted {
        thread_id: String,
        call_id: String,
        success: bool,
    },
    TurnCompleted {
        thread_id: String,
        turn_id: String,
    },
    TurnFailed {
        thread_id: String,
        turn_id: String,
        message: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_status_uses_the_frontend_protocol_shape() {
        let status = RuntimeStatus {
            ready: true,
            phase: "foundation".to_string(),
            version: "0.1.0".to_string(),
            uptime_seconds: 3,
            capabilities: vec!["typed-ipc".to_string()],
        };

        let value = serde_json::to_value(status).expect("runtime status should serialize");

        assert_eq!(value["ready"], true);
        assert_eq!(value["uptimeSeconds"], 3);
        assert_eq!(value["capabilities"][0], "typed-ipc");
        assert!(value.get("uptime_seconds").is_none());
    }
}
