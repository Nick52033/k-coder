//! 网关与宿主应用的边界。
//!
//! 网关需要宿主提供三件事：应用状态、结构化日志、以及「把一条用户消息放进
//! Thread mailbox」的入口。把它抽成 trait 有两个好处：
//! 1. 网关不依赖具体的 Tauri 运行时，传输层可以在测试里用桩宿主驱动；
//! 2. 网关无法绕过应用服务去碰 Provider、SQLite 或工作区文件。

use std::future::Future;
use std::pin::Pin;

use serde_json::Value;
use tauri::{AppHandle, Manager};

use crate::agent::RunTurnRequest;
use crate::app_state::AppState;
use crate::protocol::{ImageAttachment, TurnHandle};

use super::protocol::MobileError;

/// 宿主应用暴露给网关的最小能力集合。
pub trait GatewayHost: Send + Sync {
    /// 当前应用状态。未注册时返回 `None`，网关据此返回结构化内部错误。
    fn app_state(&self) -> Option<&AppState>;

    /// 写入结构化运行日志。网关只记录设备 ID、方法、结果和时间。
    fn log(&self, level: &str, event: &str, fields: Value);

    /// 把用户消息放进 Thread mailbox。返回的 future 借用应用状态。
    fn start_turn<'a>(
        &'a self,
        state: &'a AppState,
        request: RunTurnRequest,
        attachments: Vec<ImageAttachment>,
        workflow_id: Option<String>,
    ) -> Pin<Box<dyn Future<Output = Result<TurnHandle, MobileError>> + Send + 'a>>;
}

/// 真实 Tauri 应用上的宿主实现。
///
/// 保持具体运行时：`start_turn` 会级联到 `commands` 里的 Turn 执行链，把那条链整体
/// 泛型化不划算。测试改用 [`MobileService::with_host`](super::MobileService::with_host)
/// 注入桩宿主，网关内部本就把宿主当 `dyn GatewayHost` 使用。
pub struct TauriHost {
    app: AppHandle,
}

impl TauriHost {
    pub fn new(app: AppHandle) -> Self {
        Self { app }
    }
}

impl GatewayHost for TauriHost {
    fn app_state(&self) -> Option<&AppState> {
        self.app.try_state::<AppState>().map(|state| state.inner())
    }

    fn log(&self, level: &str, event: &str, fields: Value) {
        if let Some(state) = self.app.try_state::<AppState>() {
            let _ = state.logger().log(level, event, fields);
        }
    }

    fn start_turn<'a>(
        &'a self,
        state: &'a AppState,
        request: RunTurnRequest,
        attachments: Vec<ImageAttachment>,
        workflow_id: Option<String>,
    ) -> Pin<Box<dyn Future<Output = Result<TurnHandle, MobileError>> + Send + 'a>> {
        let app = self.app.clone();
        Box::pin(async move {
            crate::commands::enqueue_message_turn(app, state, request, attachments, workflow_id)
                .await
                .map_err(MobileError::from)
        })
    }
}
