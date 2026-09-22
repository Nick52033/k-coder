//! 事件扇出：按 thread 订阅、连接内投递序号、有界队列与断线补发。
//!
//! 三条硬约束（来自设计文档）：
//! 1. 领域事件先由存储层落盘，再向桌面和手机分别发布同一事件。
//! 2. 正文增量和命令输出可丢弃；Turn 起止、审批、用户输入请求、工具终态和恢复游标不可丢。
//! 3. 队列满时发 `resync_required`，不无限缓存；补发游标过期时返回 `resume_required`。
//!
//! 此外，这里是「手机能看到什么」的唯一出口：所有事件都先经过 [`sanitize_event`]，
//! 剥掉工具原始参数、补丁正文、图片载荷和私有推理，只保留控制面所需的有界内容。

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use serde::Serialize;
use serde_json::{Value, json};
use tokio::sync::Notify;

use crate::protocol::{AgentEvent, AgentEventEnvelope};

/// 单连接出站队列容量。满队列按事件重要性决定丢弃还是要求重同步。
pub const OUTBOUND_QUEUE_CAPACITY: usize = 256;
/// 单连接补发缓冲区容量。超出后只能要求客户端重新读取快照。
pub const REPLAY_BUFFER_CAPACITY: usize = 512;
/// 单个工具结果在手机上保留的最大字符数。
pub const MAX_TOOL_RESULT_CHARS: usize = 1_024;

/// 事件重要性。决定队列满时是否可以静默丢弃。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventPriority {
    /// 可丢弃的展示增量：丢掉只会少一段流式文本，客户端状态仍可自洽。
    Droppable,
    /// 不可丢弃：丢失会导致客户端状态错误，必须要求重同步。
    Critical,
}

/// 事件路由信息。`None` 表示该事件不向移动端转发。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EventRoute {
    pub priority: EventPriority,
    /// 是否允许下发给手机端。私有推理只留在桌面端。
    pub forward: bool,
}

/// 决定某个领域事件是否下发、以及它的重要性。
pub fn route_event(event: &AgentEvent) -> EventRoute {
    match event {
        // 流式增量：可丢弃。
        AgentEvent::TextDelta { .. }
        | AgentEvent::ToolOutputDelta { .. }
        | AgentEvent::ActivityStatusChanged { .. }
        | AgentEvent::UsageUpdated { .. }
        | AgentEvent::ProviderRetryWaiting { .. }
        | AgentEvent::ProviderStreamRetry { .. } => EventRoute {
            priority: EventPriority::Droppable,
            forward: true,
        },
        // 私有推理：只留在桌面端，不下发到手机。
        AgentEvent::ReasoningSummaryDelta { .. } | AgentEvent::ReasoningSummaryCompleted { .. } => {
            EventRoute {
                priority: EventPriority::Droppable,
                forward: false,
            }
        }
        // 其余全部属于状态迁移，不可丢弃。
        AgentEvent::TurnStarted { .. }
        | AgentEvent::TurnSteered { .. }
        | AgentEvent::TurnRejected { .. }
        | AgentEvent::ItemStarted { .. }
        | AgentEvent::ItemCompleted { .. }
        | AgentEvent::ContextCompacted { .. }
        | AgentEvent::ToolQueued { .. }
        | AgentEvent::ToolStarted { .. }
        | AgentEvent::ToolCompleted { .. }
        | AgentEvent::ApprovalRequested { .. }
        | AgentEvent::ApprovalResolved { .. }
        | AgentEvent::ChangeApplied { .. }
        | AgentEvent::ChangeUndone { .. }
        | AgentEvent::TextReset { .. }
        | AgentEvent::TurnCompleted { .. }
        | AgentEvent::TurnFailed { .. }
        | AgentEvent::TurnCancelled { .. }
        | AgentEvent::UserInputRequested { .. }
        | AgentEvent::UserInputResolved { .. }
        | AgentEvent::TodoUpdated { .. } => EventRoute {
            priority: EventPriority::Critical,
            forward: true,
        },
    }
}

pub fn event_thread_id(event: &AgentEvent) -> &str {
    match event {
        AgentEvent::ProviderRetryWaiting { thread_id, .. }
        | AgentEvent::ProviderStreamRetry { thread_id, .. }
        | AgentEvent::TurnStarted { thread_id, .. }
        | AgentEvent::TurnSteered { thread_id, .. }
        | AgentEvent::TurnRejected { thread_id, .. }
        | AgentEvent::ItemStarted { thread_id, .. }
        | AgentEvent::ItemCompleted { thread_id, .. }
        | AgentEvent::ActivityStatusChanged { thread_id, .. }
        | AgentEvent::TextDelta { thread_id, .. }
        | AgentEvent::TextReset { thread_id, .. }
        | AgentEvent::ReasoningSummaryDelta { thread_id, .. }
        | AgentEvent::ReasoningSummaryCompleted { thread_id, .. }
        | AgentEvent::UsageUpdated { thread_id, .. }
        | AgentEvent::ContextCompacted { thread_id, .. }
        | AgentEvent::ToolQueued { thread_id, .. }
        | AgentEvent::ToolStarted { thread_id, .. }
        | AgentEvent::ToolOutputDelta { thread_id, .. }
        | AgentEvent::ToolCompleted { thread_id, .. }
        | AgentEvent::ApprovalRequested { thread_id, .. }
        | AgentEvent::ApprovalResolved { thread_id, .. }
        | AgentEvent::ChangeApplied { thread_id, .. }
        | AgentEvent::ChangeUndone { thread_id, .. }
        | AgentEvent::TurnCompleted { thread_id, .. }
        | AgentEvent::TurnFailed { thread_id, .. }
        | AgentEvent::TurnCancelled { thread_id, .. }
        | AgentEvent::UserInputRequested { thread_id, .. }
        | AgentEvent::UserInputResolved { thread_id, .. }
        | AgentEvent::TodoUpdated { thread_id, .. } => thread_id,
    }
}

/// 把领域事件投影成可以下发到手机端的载荷。
///
/// 返回 `None` 表示该事件不下发。投影规则：
/// - 剥掉工具原始参数与元数据，只保留工具名和调用 ID；
/// - 工具结果只保留有界摘要；
/// - 审批请求只保留工具名、风险、原因和工作区相对路径，剥掉补丁正文；
/// - 变更集只保留文件相对路径与操作类型，剥掉内容与 diff；
/// - 消息只保留文本块，图片与注入上下文替换为占位符。
pub fn sanitize_event(envelope: &AgentEventEnvelope) -> Option<Value> {
    let route = route_event(&envelope.event);
    if !route.forward {
        return None;
    }
    let mut value = serde_json::to_value(envelope).ok()?;
    let object = value.as_object_mut()?;

    match &envelope.event {
        AgentEvent::ToolQueued { .. } | AgentEvent::ToolStarted { .. } => {
            if let Some(call) = object.get_mut("call").and_then(Value::as_object_mut) {
                call.insert("arguments".to_string(), Value::Null);
                call.insert("metadata".to_string(), Value::Null);
            }
        }
        AgentEvent::ToolCompleted { .. } => {
            if let Some(result) = object.get_mut("result").and_then(Value::as_object_mut) {
                let truncated = result
                    .get("output")
                    .and_then(Value::as_str)
                    .map(|output| super::protocol::truncate_chars(output, MAX_TOOL_RESULT_CHARS));
                if let Some(truncated) = truncated {
                    result.insert("output".to_string(), Value::String(truncated));
                }
                result.insert("metadata".to_string(), Value::Null);
            }
        }
        AgentEvent::ApprovalRequested { .. } => {
            if let Some(request) = object.get_mut("request").and_then(Value::as_object_mut) {
                request.insert("arguments".to_string(), Value::Null);
                if let Some(preview) = request.get_mut("preview").and_then(Value::as_object_mut) {
                    preview.insert("patch".to_string(), Value::Null);
                    if let Some(files) = preview.get_mut("files").and_then(Value::as_array_mut) {
                        for file in files.iter_mut() {
                            if let Some(file) = file.as_object_mut() {
                                file.insert("beforeContent".to_string(), Value::Null);
                                file.insert("afterContent".to_string(), Value::Null);
                                file.insert("unifiedDiff".to_string(), Value::Null);
                            }
                        }
                    }
                }
            }
        }
        AgentEvent::ChangeApplied { .. } => {
            if let Some(change_set) = object.get_mut("changeSet").and_then(Value::as_object_mut) {
                if let Some(files) = change_set.get_mut("files").and_then(Value::as_array_mut) {
                    for file in files.iter_mut() {
                        if let Some(file) = file.as_object_mut() {
                            file.insert("beforeContent".to_string(), Value::Null);
                            file.insert("afterContent".to_string(), Value::Null);
                            file.insert("unifiedDiff".to_string(), Value::Null);
                        }
                    }
                }
            }
        }
        AgentEvent::TurnStarted { .. } | AgentEvent::TurnCompleted { .. } => {
            if let Some(message) = object.get_mut("message").and_then(Value::as_object_mut) {
                strip_message_blocks(message);
            }
        }
        AgentEvent::TurnSteered { .. } => {
            if let Some(message) = object.get_mut("message").and_then(Value::as_object_mut) {
                strip_message_blocks(message);
            }
        }
        _ => {}
    }

    Some(value)
}

/// 消息只保留文本块，图片与注入上下文替换为占位符。
fn strip_message_blocks(message: &mut serde_json::Map<String, Value>) {
    let Some(blocks) = message.get_mut("content").and_then(Value::as_array_mut) else {
        return;
    };
    let mut sanitized = Vec::with_capacity(blocks.len());
    for block in blocks.iter() {
        let Some(block) = block.as_object() else {
            continue;
        };
        match block.get("type").and_then(Value::as_str) {
            Some("text") => sanitized.push(json!({
                "type": "text",
                "text": block.get("text").cloned().unwrap_or(Value::String(String::new())),
            })),
            Some("image") => sanitized.push(json!({
                "type": "text",
                "text": format!(
                    "[图片: {}]",
                    block.get("name").and_then(Value::as_str).unwrap_or("image")
                ),
            })),
            _ => {}
        }
    }
    message.insert("content".to_string(), Value::Array(sanitized));
}

/// 出站消息：要么是载荷，要么是要求客户端重新同步的信号。
#[derive(Debug, Clone)]
pub enum QueueMessage {
    Payload(Value),
    Resync,
}

#[derive(Debug, Default)]
struct QueueState {
    items: VecDeque<Value>,
    /// 需要向客户端发 `resync_required`。置位期间不再入队新事件。
    resync: bool,
    closed: bool,
    dropped: u64,
}

/// 单连接的有界出站队列。
///
/// 用 `Mutex<VecDeque>` + `Notify` 而不是 mpsc，是为了在队列满时能按事件重要性
/// 分别处理（丢弃增量 / 要求重同步），而不是无界缓存或整条连接断开。
#[derive(Debug, Default)]
pub struct OutboundQueue {
    state: Mutex<QueueState>,
    notify: Notify,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PushOutcome {
    Queued,
    Dropped,
    Resync,
    Closed,
}

impl OutboundQueue {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&self, message: Value, priority: EventPriority) -> PushOutcome {
        let mut state = self.state.lock().expect("outbound queue lock poisoned");
        if state.closed {
            return PushOutcome::Closed;
        }
        if state.resync {
            // 客户端即将重新读取快照，期间的事件不再入队。
            return PushOutcome::Resync;
        }
        if state.items.len() < OUTBOUND_QUEUE_CAPACITY {
            state.items.push_back(message);
            drop(state);
            self.notify.notify_one();
            return PushOutcome::Queued;
        }
        match priority {
            EventPriority::Droppable => {
                state.dropped += 1;
                PushOutcome::Dropped
            }
            EventPriority::Critical => {
                state.resync = true;
                drop(state);
                self.notify.notify_one();
                PushOutcome::Resync
            }
        }
    }

    pub fn dropped_count(&self) -> u64 {
        self.state
            .lock()
            .expect("outbound queue lock poisoned")
            .dropped
    }

    pub fn close(&self) {
        let mut state = self.state.lock().expect("outbound queue lock poisoned");
        state.closed = true;
        drop(state);
        self.notify.notify_waiters();
    }

    /// 取出下一条待发送内容。返回 `None` 表示队列已关闭且已排空。
    pub async fn next(&self) -> Option<QueueMessage> {
        loop {
            // 先注册通知再检查状态，避免丢唤醒。
            let notified = self.notify.notified();
            {
                let mut state = self.state.lock().expect("outbound queue lock poisoned");
                if state.resync {
                    state.resync = false;
                    state.items.clear();
                    return Some(QueueMessage::Resync);
                }
                if let Some(item) = state.items.pop_front() {
                    return Some(QueueMessage::Payload(item));
                }
                if state.closed {
                    return None;
                }
            }
            notified.await;
        }
    }
}

/// 补发缓冲中的一条已投递事件。
#[derive(Debug, Clone)]
struct DeliveredEvent {
    delivery_seq: u64,
    thread_id: String,
    payload: Value,
}

#[derive(Debug)]
struct ConnectionSubscriptions {
    outbound: Arc<OutboundQueue>,
    threads: Vec<String>,
    next_delivery_seq: u64,
    replay: VecDeque<DeliveredEvent>,
}

/// 补发结果。
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ResumeOutcome {
    pub replayed: usize,
    pub delivery_seq: u64,
}

/// 事件扇出中心。
#[derive(Debug, Default)]
pub struct EventHub {
    connections: Mutex<HashMap<u64, ConnectionSubscriptions>>,
    next_connection_id: AtomicU64,
}

impl EventHub {
    pub fn new() -> Self {
        Self::default()
    }

    /// 注册一条连接，返回连接 ID 和它独占的出站队列。
    pub fn register(&self) -> (u64, Arc<OutboundQueue>) {
        let connection_id = self.next_connection_id.fetch_add(1, Ordering::SeqCst) + 1;
        let outbound = Arc::new(OutboundQueue::new());
        self.connections
            .lock()
            .expect("event hub lock poisoned")
            .insert(
                connection_id,
                ConnectionSubscriptions {
                    outbound: outbound.clone(),
                    threads: Vec::new(),
                    next_delivery_seq: 1,
                    replay: VecDeque::new(),
                },
            );
        (connection_id, outbound)
    }

    /// 注销连接并关闭它的出站队列。
    pub fn unregister(&self, connection_id: u64) {
        let removed = self
            .connections
            .lock()
            .expect("event hub lock poisoned")
            .remove(&connection_id);
        if let Some(removed) = removed {
            removed.outbound.close();
        }
    }

    pub fn connection_count(&self) -> usize {
        self.connections
            .lock()
            .expect("event hub lock poisoned")
            .len()
    }

    /// 订阅 thread 并返回当前补发游标。
    ///
    /// 游标语义必须与 [`EventHub::resume`] 的 `after_delivery_seq` 一致，也就是
    /// 「本连接最后已分配的 `deliverySeq`」而不是「下一个待分配序号」。两者差一
    /// 会让手机端在重连时拿到一个超过 `latest` 的游标，从而被误判成游标过期、
    /// 每次重连都退化成整段重读。
    pub fn subscribe(
        &self,
        connection_id: u64,
        thread_id: &str,
    ) -> Result<u64, super::protocol::MobileError> {
        let mut connections = self.connections.lock().expect("event hub lock poisoned");
        let connection = connections.get_mut(&connection_id).ok_or_else(|| {
            super::protocol::MobileError::internal("connection is no longer registered")
        })?;
        if !connection.threads.iter().any(|thread| thread == thread_id) {
            connection.threads.push(thread_id.to_string());
        }
        Ok(connection.next_delivery_seq - 1)
    }

    pub fn unsubscribe(&self, connection_id: u64, thread_id: &str) {
        let mut connections = self.connections.lock().expect("event hub lock poisoned");
        if let Some(connection) = connections.get_mut(&connection_id) {
            connection.threads.retain(|thread| thread != thread_id);
        }
    }

    pub fn subscribed_threads(&self, connection_id: u64) -> Vec<String> {
        self.connections
            .lock()
            .expect("event hub lock poisoned")
            .get(&connection_id)
            .map(|connection| connection.threads.clone())
            .unwrap_or_default()
    }

    pub fn subscriber_count(&self, thread_id: &str) -> usize {
        self.connections
            .lock()
            .expect("event hub lock poisoned")
            .values()
            .filter(|connection| connection.threads.iter().any(|thread| thread == thread_id))
            .count()
    }

    /// 向所有订阅了该 thread 的连接发布领域事件。
    ///
    /// 返回实际入队的连接数，便于测试与诊断。
    pub fn publish(&self, envelope: &AgentEventEnvelope) -> usize {
        let Some(payload) = sanitize_event(envelope) else {
            return 0;
        };
        let thread_id = event_thread_id(&envelope.event).to_string();
        let priority = route_event(&envelope.event).priority;

        let mut connections = self.connections.lock().expect("event hub lock poisoned");
        let mut delivered = 0;
        for connection in connections.values_mut() {
            if !connection.threads.iter().any(|thread| thread == &thread_id) {
                continue;
            }
            let delivery_seq = connection.next_delivery_seq;
            connection.next_delivery_seq += 1;

            let mut params = payload.clone();
            if let Some(object) = params.as_object_mut() {
                object.insert("deliverySeq".to_string(), json!(delivery_seq));
            }
            let message = json!({
                "jsonrpc": "2.0",
                "method": "event",
                "params": params,
            });

            connection.replay.push_back(DeliveredEvent {
                delivery_seq,
                thread_id: thread_id.clone(),
                payload: params,
            });
            while connection.replay.len() > REPLAY_BUFFER_CAPACITY {
                connection.replay.pop_front();
            }

            if connection.outbound.push(message, priority) != PushOutcome::Dropped {
                delivered += 1;
            }
        }
        delivered
    }

    /// 断线重连后的补发。游标过期时返回 `resume_required`。
    pub fn resume(
        &self,
        connection_id: u64,
        thread_id: &str,
        after_delivery_seq: u64,
    ) -> Result<ResumeOutcome, super::protocol::MobileError> {
        let mut connections = self.connections.lock().expect("event hub lock poisoned");
        let connection = connections.get_mut(&connection_id).ok_or_else(|| {
            super::protocol::MobileError::internal("connection is no longer registered")
        })?;
        if !connection.threads.iter().any(|thread| thread == thread_id) {
            return Err(super::protocol::MobileError::forbidden(
                "connection is not subscribed to this thread",
            ));
        }

        let latest = connection.next_delivery_seq - 1;
        if after_delivery_seq > latest {
            // 游标来自上一条连接（或服务端重启过）：本连接无法补齐，客户端应改用
            // `thread/read` 取快照，而不是继续等待增量。
            return Err(super::protocol::MobileError::new(
                super::protocol::MobileErrorKind::ResumeRequired,
                "resume cursor belongs to an earlier connection",
            )
            .with_details(json!({
                "reason": "cursor_not_buffered",
                "latestSeq": latest,
            })));
        }
        if after_delivery_seq == latest {
            return Ok(ResumeOutcome {
                replayed: 0,
                delivery_seq: latest,
            });
        }

        let oldest = connection
            .replay
            .front()
            .map(|event| event.delivery_seq)
            .unwrap_or(latest + 1);
        if oldest > after_delivery_seq + 1 {
            return Err(super::protocol::MobileError::new(
                super::protocol::MobileErrorKind::ResumeRequired,
                "resume cursor is no longer buffered",
            )
            .with_details(json!({
                "reason": "cursor_expired",
                "oldestBufferedSeq": oldest,
                "latestSeq": latest,
            })));
        }

        let pending: Vec<DeliveredEvent> = connection
            .replay
            .iter()
            .filter(|event| event.thread_id == thread_id && event.delivery_seq > after_delivery_seq)
            .cloned()
            .collect();
        let replayed = pending.len();
        for event in pending {
            let message = json!({
                "jsonrpc": "2.0",
                "method": "event",
                "params": event.payload,
            });
            connection.outbound.push(message, EventPriority::Critical);
        }
        Ok(ResumeOutcome {
            replayed,
            delivery_seq: latest,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{
        AgentEvent, AgentEventEnvelope, ApprovalRequest, ChatMessage, ContentBlock, MessageRole,
        ToolCall, ToolResult, TurnPhase,
    };
    use std::time::Duration;

    fn envelope(event: AgentEvent) -> AgentEventEnvelope {
        AgentEventEnvelope::with_phase(event, TurnPhase::Executing)
    }

    fn text_delta(thread_id: &str, delta: &str) -> AgentEventEnvelope {
        envelope(AgentEvent::TextDelta {
            thread_id: thread_id.to_string(),
            turn_id: "turn-1".to_string(),
            item_id: "item-1".to_string(),
            delta: delta.to_string(),
        })
    }

    fn approval_requested(thread_id: &str) -> AgentEventEnvelope {
        envelope(AgentEvent::ApprovalRequested {
            thread_id: thread_id.to_string(),
            turn_id: "turn-1".to_string(),
            request: ApprovalRequest {
                id: "approval-1".to_string(),
                thread_id: thread_id.to_string(),
                turn_id: "turn-1".to_string(),
                tool_call_id: "call-1".to_string(),
                tool_name: "apply_patch".to_string(),
                reason: "write access".to_string(),
                auto_approved: false,
                risk: crate::protocol::ToolRisk::Write,
                arguments: json!({ "token": "super-secret" }),
                preview: None,
                created_at_ms: 1,
                expires_at_ms: 2,
            },
        })
    }

    fn user_message(text: &str) -> ChatMessage {
        ChatMessage {
            schema_version: 1,
            id: "message-1".to_string(),
            role: MessageRole::Assistant,
            content: vec![
                ContentBlock::Text {
                    text: text.to_string(),
                },
                ContentBlock::Image {
                    name: "shot.png".to_string(),
                    data_url: "data:image/png;base64,AAAA".to_string(),
                },
            ],
            created_at_ms: 0,
        }
    }

    #[test]
    fn tool_arguments_are_never_forwarded() {
        let envelope = envelope(AgentEvent::ToolStarted {
            thread_id: "t1".to_string(),
            turn_id: "turn-1".to_string(),
            call: ToolCall {
                id: "call-1".to_string(),
                name: "run_command".to_string(),
                arguments: json!({ "command": "curl -H 'Authorization: Bearer abc'" }),
                metadata: json!({ "cwd": "D:\\secret" }),
            },
        });
        let payload = sanitize_event(&envelope).expect("tool events are forwarded");
        assert_eq!(payload["call"]["name"], json!("run_command"));
        assert_eq!(payload["call"]["arguments"], Value::Null);
        assert_eq!(payload["call"]["metadata"], Value::Null);
        assert!(!payload.to_string().contains("Authorization"));
    }

    #[test]
    fn tool_results_are_bounded() {
        let envelope = envelope(AgentEvent::ToolCompleted {
            thread_id: "t1".to_string(),
            turn_id: "turn-1".to_string(),
            call_id: "call-1".to_string(),
            name: "read_file".to_string(),
            result: ToolResult {
                success: true,
                output: "x".repeat(MAX_TOOL_RESULT_CHARS * 3),
                metadata: json!({ "path": "D:\\secret" }),
            },
        });
        let payload = sanitize_event(&envelope).unwrap();
        let output = payload["result"]["output"].as_str().unwrap();
        assert_eq!(output.chars().count(), MAX_TOOL_RESULT_CHARS + 1);
        assert_eq!(payload["result"]["metadata"], Value::Null);
    }

    #[test]
    fn approval_keeps_tool_identity_but_drops_arguments() {
        let payload = sanitize_event(&approval_requested("t1")).unwrap();
        assert_eq!(payload["request"]["toolName"], json!("apply_patch"));
        assert_eq!(payload["request"]["risk"], json!("write"));
        assert_eq!(payload["request"]["arguments"], Value::Null);
        assert!(!payload.to_string().contains("super-secret"));
    }

    #[test]
    fn reasoning_is_not_forwarded() {
        let envelope = envelope(AgentEvent::ReasoningSummaryCompleted {
            thread_id: "t1".to_string(),
            turn_id: "turn-1".to_string(),
            item_id: "item-1".to_string(),
            summary: "private chain of thought".to_string(),
        });
        assert!(sanitize_event(&envelope).is_none());
    }

    #[test]
    fn message_images_are_replaced_by_placeholder() {
        let envelope = envelope(AgentEvent::TurnCompleted {
            thread_id: "t1".to_string(),
            turn_id: "turn-1".to_string(),
            message: user_message("done"),
            usage: None,
            started_at_ms: 0,
            completed_at_ms: 1,
            duration_ms: 1,
        });
        let payload = sanitize_event(&envelope).unwrap();
        let blocks = payload["message"]["content"].as_array().unwrap();
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0]["text"], json!("done"));
        assert_eq!(blocks[1]["text"], json!("[图片: shot.png]"));
        assert!(!payload.to_string().contains("data:image"));
    }

    #[test]
    fn events_only_reach_subscribers() {
        let hub = EventHub::new();
        let (first, _) = hub.register();
        let (second, _) = hub.register();
        hub.subscribe(first, "t1").unwrap();
        hub.subscribe(second, "t2").unwrap();

        assert_eq!(hub.publish(&text_delta("t1", "hello")), 1);
        assert_eq!(hub.publish(&text_delta("t3", "nobody")), 0);
        assert_eq!(hub.subscriber_count("t1"), 1);
    }

    #[test]
    fn delivery_sequence_is_monotonic_per_connection() {
        let hub = EventHub::new();
        let (connection, outbound) = hub.register();
        hub.subscribe(connection, "t1").unwrap();
        hub.publish(&text_delta("t1", "a"));
        hub.publish(&text_delta("t1", "b"));

        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            let first = outbound.next().await.unwrap();
            let second = outbound.next().await.unwrap();
            let QueueMessage::Payload(first) = first else {
                panic!("expected payload");
            };
            let QueueMessage::Payload(second) = second else {
                panic!("expected payload");
            };
            assert_eq!(first["params"]["deliverySeq"], json!(1));
            assert_eq!(second["params"]["deliverySeq"], json!(2));
        });
    }

    #[test]
    fn critical_events_request_resync_when_queue_is_full() {
        let outbound = OutboundQueue::new();
        for index in 0..OUTBOUND_QUEUE_CAPACITY {
            assert_eq!(
                outbound.push(json!(index), EventPriority::Droppable),
                PushOutcome::Queued
            );
        }
        assert_eq!(
            outbound.push(json!("dropped"), EventPriority::Droppable),
            PushOutcome::Dropped
        );
        assert_eq!(outbound.dropped_count(), 1);
        assert_eq!(
            outbound.push(json!("important"), EventPriority::Critical),
            PushOutcome::Resync
        );

        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            match outbound.next().await.unwrap() {
                QueueMessage::Resync => {}
                QueueMessage::Payload(_) => panic!("resync must be delivered first"),
            }
        });
    }

    #[test]
    fn resume_replays_buffered_events() {
        let hub = EventHub::new();
        let (connection, outbound) = hub.register();
        hub.subscribe(connection, "t1").unwrap();
        hub.publish(&text_delta("t1", "a"));
        hub.publish(&text_delta("t1", "b"));
        hub.publish(&text_delta("t1", "c"));

        let outcome = hub.resume(connection, "t1", 1).unwrap();
        assert_eq!(outcome.replayed, 2);
        assert_eq!(outcome.delivery_seq, 3);

        // 补发的事件与原始投递一起排队；客户端按 deliverySeq 去重。
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            let mut sequences = Vec::new();
            for _ in 0..5 {
                let Ok(message) =
                    tokio::time::timeout(Duration::from_millis(100), outbound.next()).await
                else {
                    break;
                };
                let Some(QueueMessage::Payload(payload)) = message else {
                    break;
                };
                sequences.push(payload["params"]["deliverySeq"].as_u64().unwrap());
            }
            assert!(
                sequences.contains(&2),
                "seq 2 must be replayed: {sequences:?}"
            );
            assert!(
                sequences.contains(&3),
                "seq 3 must be replayed: {sequences:?}"
            );
        });
    }

    #[test]
    fn resume_with_current_cursor_replays_nothing() {
        let hub = EventHub::new();
        let (connection, _) = hub.register();
        hub.subscribe(connection, "t1").unwrap();
        hub.publish(&text_delta("t1", "a"));
        let outcome = hub.resume(connection, "t1", 1).unwrap();
        assert_eq!(outcome.replayed, 0);
    }

    #[test]
    fn expired_cursor_requires_full_resync() {
        let hub = EventHub::new();
        let (connection, _) = hub.register();
        hub.subscribe(connection, "t1").unwrap();
        for index in 0..(REPLAY_BUFFER_CAPACITY + 10) {
            hub.publish(&text_delta("t1", &index.to_string()));
        }
        let error = hub.resume(connection, "t1", 0).unwrap_err();
        assert_eq!(error.kind(), "resume_required");
        assert_eq!(
            error.data.details.as_ref().unwrap()["reason"],
            json!("cursor_expired")
        );
    }

    #[test]
    fn resume_requires_subscription() {
        let hub = EventHub::new();
        let (connection, _) = hub.register();
        let error = hub.resume(connection, "t1", 0).unwrap_err();
        assert_eq!(error.kind(), "forbidden");
    }

    #[test]
    fn cursor_from_earlier_connection_requires_resync() {
        let hub = EventHub::new();
        let (connection, _) = hub.register();
        hub.subscribe(connection, "t1").unwrap();
        let error = hub.resume(connection, "t1", 5).unwrap_err();
        assert_eq!(error.kind(), "resume_required");
        assert_eq!(
            error.data.details.as_ref().unwrap()["reason"],
            json!("cursor_not_buffered")
        );
    }

    #[test]
    fn resume_from_zero_on_fresh_connection_is_a_noop() {
        let hub = EventHub::new();
        let (connection, _) = hub.register();
        hub.subscribe(connection, "t1").unwrap();
        let outcome = hub.resume(connection, "t1", 0).unwrap();
        assert_eq!(outcome.replayed, 0);
        assert_eq!(outcome.delivery_seq, 0);
    }

    /// 回归：`thread/subscribe` 返回的游标必须能直接喂给 `events/resume`。
    ///
    /// 曾经的实现返回「下一个待分配序号」，比 `resume` 期望的「最后已分配序号」
    /// 大 1，导致手机端重连时必然收到 `resume_required`，增量补发形同虚设。
    #[test]
    fn subscribe_cursor_is_directly_resumable() {
        let hub = EventHub::new();
        let (connection, _) = hub.register();

        // 尚未投递任何事件时，订阅返回的游标必须能原样用于补发。
        let fresh = hub.subscribe(connection, "t1").unwrap();
        assert_eq!(fresh, 0);
        let outcome = hub.resume(connection, "t1", fresh).unwrap();
        assert_eq!(outcome.replayed, 0);
        assert_eq!(outcome.delivery_seq, fresh);

        // 投递若干事件后，订阅返回的游标应等于最后一条事件的序号。
        hub.publish(&text_delta("t1", "a"));
        hub.publish(&text_delta("t1", "b"));
        let after_two = hub.subscribe(connection, "t1").unwrap();
        assert_eq!(after_two, 2);
        let outcome = hub.resume(connection, "t1", after_two).unwrap();
        assert_eq!(outcome.replayed, 0);
        assert_eq!(outcome.delivery_seq, after_two);

        // 订阅游标之后又投递的事件必须被补齐。
        hub.publish(&text_delta("t1", "c"));
        let outcome = hub.resume(connection, "t1", after_two).unwrap();
        assert_eq!(outcome.replayed, 1);
        assert_eq!(outcome.delivery_seq, 3);
    }

    #[test]
    fn unregister_closes_the_queue() {
        let hub = EventHub::new();
        let (connection, outbound) = hub.register();
        assert_eq!(hub.connection_count(), 1);
        hub.unregister(connection);
        assert_eq!(hub.connection_count(), 0);

        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            assert!(outbound.next().await.is_none());
        });
    }
}
