use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex as StdMutex};

use tokio::sync::{Mutex, oneshot};

use super::RunTurnRequest;
use crate::protocol::{
    ChatMessage, ImageAttachment, PROTOCOL_VERSION, QueuedTurn, QueuedTurnKind,
    ThreadMailboxSnapshot, TurnHandle,
};

#[derive(Debug)]
pub enum MailboxTurnKind {
    Message {
        request: RunTurnRequest,
        attachments: Vec<ImageAttachment>,
    },
    Retry,
}

#[derive(Debug)]
pub struct MailboxTurn {
    pub handle: TurnHandle,
    pub kind: MailboxTurnKind,
    pub started: Option<oneshot::Sender<Result<(), String>>>,
}

#[derive(Debug, Clone)]
pub struct PendingMailboxMessage {
    pub request: RunTurnRequest,
    pub attachments: Vec<ImageAttachment>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueuedTurnSteerError {
    NotFound,
    NotMessage,
    TurnClosed,
}

#[derive(Debug, Default)]
struct ThreadQueue {
    worker_running: bool,
    pending: VecDeque<MailboxTurn>,
}

#[derive(Debug, Default)]
pub struct ThreadMailbox {
    state: Mutex<ThreadMailboxState>,
}

#[derive(Debug, Default)]
struct ThreadMailboxState {
    queues: HashMap<String, ThreadQueue>,
    revisions: HashMap<String, u64>,
}

impl ThreadMailbox {
    pub async fn enqueue(&self, item: MailboxTurn) -> bool {
        let mut state = self.state.lock().await;
        let thread_id = item.handle.thread_id.clone();
        let queue = state.queues.entry(thread_id.clone()).or_default();
        queue.pending.push_back(item);
        let should_start = if queue.worker_running {
            false
        } else {
            queue.worker_running = true;
            true
        };
        bump_revision(&mut state, &thread_id);
        should_start
    }

    pub async fn next(&self, thread_id: &str) -> Option<MailboxTurn> {
        let mut state = self.state.lock().await;
        let queue = state.queues.get_mut(thread_id)?;
        if let Some(item) = queue.pending.pop_front() {
            bump_revision(&mut state, thread_id);
            return Some(item);
        }
        state.queues.remove(thread_id);
        bump_revision(&mut state, thread_id);
        None
    }

    pub async fn snapshot(
        &self,
        thread_id: &str,
        active_turn_id: Option<String>,
    ) -> ThreadMailboxSnapshot {
        let state = self.state.lock().await;
        let pending = state
            .queues
            .get(thread_id)
            .map(|queue| {
                queue
                    .pending
                    .iter()
                    .map(|item| {
                        let (kind, input, agent_mode, attachments) = match &item.kind {
                            MailboxTurnKind::Message {
                                request,
                                attachments,
                            } => (
                                QueuedTurnKind::Message,
                                request.input.clone(),
                                request.agent_mode.clone(),
                                attachments.clone(),
                            ),
                            MailboxTurnKind::Retry => {
                                (QueuedTurnKind::Retry, String::new(), None, Vec::new())
                            }
                        };
                        QueuedTurn {
                            schema_version: PROTOCOL_VERSION,
                            turn_id: item.handle.turn_id.clone(),
                            thread_id: item.handle.thread_id.clone(),
                            kind,
                            input,
                            agent_mode,
                            attachments,
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();
        ThreadMailboxSnapshot {
            schema_version: PROTOCOL_VERSION,
            thread_id: thread_id.to_string(),
            revision: state.revisions.get(thread_id).copied().unwrap_or_default(),
            active_turn_id,
            pending,
        }
    }

    pub async fn revision(&self, thread_id: &str) -> u64 {
        self.state
            .lock()
            .await
            .revisions
            .get(thread_id)
            .copied()
            .unwrap_or_default()
    }

    pub async fn remove(&self, thread_id: &str, turn_id: &str) -> bool {
        let mut state = self.state.lock().await;
        let Some(queue) = state.queues.get_mut(thread_id) else {
            return false;
        };
        let Some(index) = queue
            .pending
            .iter()
            .position(|item| item.handle.turn_id == turn_id)
        else {
            return false;
        };
        queue.pending.remove(index);
        bump_revision(&mut state, thread_id);
        true
    }

    pub async fn pending_message(
        &self,
        thread_id: &str,
        turn_id: &str,
    ) -> Result<PendingMailboxMessage, QueuedTurnSteerError> {
        let state = self.state.lock().await;
        let item = state
            .queues
            .get(thread_id)
            .and_then(|queue| {
                queue
                    .pending
                    .iter()
                    .find(|item| item.handle.turn_id == turn_id)
            })
            .ok_or(QueuedTurnSteerError::NotFound)?;
        match &item.kind {
            MailboxTurnKind::Message {
                request,
                attachments,
            } => Ok(PendingMailboxMessage {
                request: request.clone(),
                attachments: attachments.clone(),
            }),
            MailboxTurnKind::Retry => Err(QueuedTurnSteerError::NotMessage),
        }
    }

    pub async fn steer_message(
        &self,
        thread_id: &str,
        queued_turn_id: &str,
        control: &TurnControl,
        message: ChatMessage,
    ) -> Result<(), QueuedTurnSteerError> {
        let mut state = self.state.lock().await;
        let queue = state
            .queues
            .get_mut(thread_id)
            .ok_or(QueuedTurnSteerError::NotFound)?;
        let index = queue
            .pending
            .iter()
            .position(|item| item.handle.turn_id == queued_turn_id)
            .ok_or(QueuedTurnSteerError::NotFound)?;
        if !matches!(queue.pending[index].kind, MailboxTurnKind::Message { .. }) {
            return Err(QueuedTurnSteerError::NotMessage);
        }
        control
            .steer(message)
            .map_err(|_| QueuedTurnSteerError::TurnClosed)?;
        queue.pending.remove(index);
        bump_revision(&mut state, thread_id);
        Ok(())
    }

    pub async fn clear(&self, thread_id: &str) -> usize {
        let mut state = self.state.lock().await;
        let Some(queue) = state.queues.get_mut(thread_id) else {
            return 0;
        };
        let removed = queue.pending.len();
        queue.pending.clear();
        if removed > 0 {
            bump_revision(&mut state, thread_id);
        }
        removed
    }

    pub async fn is_idle(&self, thread_id: &str) -> bool {
        self.state
            .lock()
            .await
            .queues
            .get(thread_id)
            .is_none_or(|queue| !queue.worker_running && queue.pending.is_empty())
    }
}

fn bump_revision(state: &mut ThreadMailboxState, thread_id: &str) -> u64 {
    let revision = state.revisions.entry(thread_id.to_string()).or_default();
    *revision = revision.saturating_add(1);
    *revision
}

#[derive(Debug, Default)]
struct TurnControlState {
    accepting: bool,
    messages: VecDeque<ChatMessage>,
}

#[derive(Debug, Default)]
pub struct TurnControl {
    state: StdMutex<TurnControlState>,
}

impl TurnControl {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            state: StdMutex::new(TurnControlState {
                accepting: true,
                messages: VecDeque::new(),
            }),
        })
    }

    pub fn steer(&self, message: ChatMessage) -> Result<(), ()> {
        let mut state = self.state.lock().unwrap();
        if !state.accepting {
            return Err(());
        }
        state.messages.push_back(message);
        Ok(())
    }

    pub fn take_pending(&self) -> Vec<ChatMessage> {
        self.state.lock().unwrap().messages.drain(..).collect()
    }

    pub fn close_if_idle(&self) -> Option<Vec<ChatMessage>> {
        let mut state = self.state.lock().unwrap();
        if state.messages.is_empty() {
            state.accepting = false;
            None
        } else {
            Some(state.messages.drain(..).collect())
        }
    }

    pub fn close(&self) {
        self.state.lock().unwrap().accepting = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{MessageRole, PROTOCOL_VERSION, TurnState};

    fn mailbox_turn(thread_id: &str, turn_id: &str, input: &str) -> MailboxTurn {
        MailboxTurn {
            handle: TurnHandle {
                schema_version: PROTOCOL_VERSION,
                thread_id: thread_id.into(),
                turn_id: turn_id.into(),
                state: TurnState::Queued,
            },
            kind: MailboxTurnKind::Message {
                request: RunTurnRequest {
                    thread_id: thread_id.into(),
                    input: input.into(),
                    agent_mode: None,
                },
                attachments: Vec::new(),
            },
            started: None,
        }
    }

    #[tokio::test]
    async fn mailbox_owns_fifo_order_per_thread() {
        let mailbox = ThreadMailbox::default();
        assert!(mailbox.enqueue(mailbox_turn("a", "1", "first")).await);
        assert!(!mailbox.enqueue(mailbox_turn("a", "2", "second")).await);
        assert!(mailbox.enqueue(mailbox_turn("b", "3", "parallel")).await);

        assert_eq!(mailbox.next("a").await.unwrap().handle.turn_id, "1");
        assert_eq!(mailbox.next("a").await.unwrap().handle.turn_id, "2");
        assert!(mailbox.next("a").await.is_none());
        assert_eq!(mailbox.next("b").await.unwrap().handle.turn_id, "3");
    }

    #[tokio::test]
    async fn mailbox_snapshot_distinguishes_retry_work_from_messages() {
        let mailbox = ThreadMailbox::default();
        assert!(
            mailbox
                .enqueue(MailboxTurn {
                    handle: TurnHandle {
                        schema_version: PROTOCOL_VERSION,
                        thread_id: "thread".into(),
                        turn_id: "retry-turn".into(),
                        state: TurnState::Queued,
                    },
                    kind: MailboxTurnKind::Retry,
                    started: None,
                })
                .await
        );

        let snapshot = mailbox.snapshot("thread", None).await;
        assert_eq!(snapshot.pending.len(), 1);
        assert_eq!(snapshot.pending[0].kind, QueuedTurnKind::Retry);
        assert!(snapshot.pending[0].input.is_empty());
        assert!(snapshot.pending[0].attachments.is_empty());
    }

    #[tokio::test]
    async fn mailbox_revision_advances_only_for_successful_mutations() {
        let mailbox = ThreadMailbox::default();
        assert_eq!(mailbox.revision("thread").await, 0);
        mailbox
            .enqueue(mailbox_turn("thread", "message-turn", "queued"))
            .await;
        assert_eq!(mailbox.snapshot("thread", None).await.revision, 1);
        assert!(!mailbox.remove("thread", "missing").await);
        assert_eq!(mailbox.revision("thread").await, 1);
        assert!(mailbox.remove("thread", "message-turn").await);
        assert_eq!(mailbox.revision("thread").await, 2);
        assert_eq!(mailbox.clear("thread").await, 0);
        assert_eq!(mailbox.revision("thread").await, 2);
        assert!(mailbox.next("thread").await.is_none());
        assert_eq!(mailbox.revision("thread").await, 3);
    }

    #[tokio::test]
    async fn queued_steer_removes_the_message_only_after_control_accepts_it() {
        let mailbox = ThreadMailbox::default();
        mailbox
            .enqueue(mailbox_turn("thread", "message-turn", "queued"))
            .await;
        mailbox
            .enqueue(MailboxTurn {
                handle: TurnHandle {
                    schema_version: PROTOCOL_VERSION,
                    thread_id: "thread".into(),
                    turn_id: "retry-turn".into(),
                    state: TurnState::Queued,
                },
                kind: MailboxTurnKind::Retry,
                started: None,
            })
            .await;
        assert_eq!(
            mailbox
                .pending_message("thread", "message-turn")
                .await
                .unwrap()
                .request
                .input,
            "queued"
        );

        let control = TurnControl::new();
        let message = ChatMessage {
            schema_version: PROTOCOL_VERSION,
            id: "steered-message".into(),
            role: MessageRole::User,
            content: Vec::new(),
            created_at_ms: 1,
        };
        mailbox
            .steer_message("thread", "message-turn", control.as_ref(), message.clone())
            .await
            .unwrap();
        assert_eq!(control.take_pending(), vec![message]);
        assert_eq!(
            mailbox.snapshot("thread", None).await.pending[0].turn_id,
            "retry-turn"
        );
        assert_eq!(
            mailbox
                .steer_message(
                    "thread",
                    "message-turn",
                    control.as_ref(),
                    ChatMessage {
                        schema_version: PROTOCOL_VERSION,
                        id: "duplicate".into(),
                        role: MessageRole::User,
                        content: Vec::new(),
                        created_at_ms: 2,
                    },
                )
                .await,
            Err(QueuedTurnSteerError::NotFound)
        );
        assert_eq!(
            mailbox
                .pending_message("thread", "retry-turn")
                .await
                .unwrap_err(),
            QueuedTurnSteerError::NotMessage
        );
    }

    #[tokio::test]
    async fn queued_steer_keeps_the_message_when_the_turn_is_closed() {
        let mailbox = ThreadMailbox::default();
        mailbox
            .enqueue(mailbox_turn("thread", "queued-turn", "queued"))
            .await;
        let control = TurnControl::new();
        control.close();

        assert_eq!(
            mailbox
                .steer_message(
                    "thread",
                    "queued-turn",
                    control.as_ref(),
                    ChatMessage {
                        schema_version: PROTOCOL_VERSION,
                        id: "late".into(),
                        role: MessageRole::User,
                        content: Vec::new(),
                        created_at_ms: 1,
                    },
                )
                .await,
            Err(QueuedTurnSteerError::TurnClosed)
        );
        assert_eq!(mailbox.snapshot("thread", None).await.pending.len(), 1);
    }

    #[test]
    fn closing_control_rejects_late_steering_without_losing_accepted_input() {
        let control = TurnControl::new();
        let message = ChatMessage {
            schema_version: PROTOCOL_VERSION,
            id: "steer-1".into(),
            role: MessageRole::User,
            content: Vec::new(),
            created_at_ms: 1,
        };
        control.steer(message.clone()).unwrap();
        assert_eq!(control.close_if_idle(), Some(vec![message]));
        assert!(control.close_if_idle().is_none());
        assert!(
            control
                .steer(ChatMessage {
                    schema_version: PROTOCOL_VERSION,
                    id: "late".into(),
                    role: MessageRole::User,
                    content: Vec::new(),
                    created_at_ms: 2,
                })
                .is_err()
        );
    }
}
