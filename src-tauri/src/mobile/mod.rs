//! 手机控制网关：局域网直连（设计文档「方案 1」）。
//!
//! 结构：
//! - [`protocol`]：JSON-RPC 2.0 载荷与结构化错误；
//! - [`capability`]：移动端能力裁剪；
//! - [`auth`]：配对挑战、设备登记、访问令牌与撤销；
//! - [`events`]：按 thread 订阅、投递序号、有界队列与补发，以及下发前的载荷脱敏；
//! - [`view`]：会话历史的移动端安全投影；
//! - [`projects`]：项目清单与会话归属的服务端解析；
//! - [`rpc`]：方法分发，全部调用现有应用服务；
//! - [`server`]：HTTP/WebSocket 传输、来源校验与限流；
//! - [`tls`]：本机自签证书与指纹。
//!
//! 边界：网关不执行智能体循环，不绕过 `PolicyEngine`，不直接访问 Provider、
//! SQLite 或工作区文件；所有状态变更都复用 `commands` 里的应用服务入口。

pub mod auth;
pub mod capability;
pub mod events;
pub mod host;
pub mod projects;
pub mod protocol;
pub mod rpc;
pub mod server;
pub mod tls;
pub mod view;

use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};

use serde::Serialize;
use tauri::{AppHandle, Manager, Runtime, Wry};

use crate::protocol::AgentEventEnvelope;

pub use auth::{DeviceRecord, MobileSettings, PairingChallenge};
pub use capability::MobileCapability;
pub use protocol::{MobileError, MobileErrorKind};

use auth::{DeviceRegistry, PairingStore, TokenStore};
use capability::CapabilityPolicy;
use events::EventHub;
use host::{GatewayHost, TauriHost};
use rpc::{GatewayContext, IdempotencyCache, PendingRequestIndex};

/// 桌面端展示的已配对设备。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MobileDeviceView {
    pub id: String,
    pub name: String,
    pub platform: Option<String>,
    pub created_at_ms: u64,
    pub last_seen_at_ms: u64,
    pub revoked: bool,
}

impl From<DeviceRecord> for MobileDeviceView {
    fn from(record: DeviceRecord) -> Self {
        Self {
            id: record.id,
            name: record.name,
            platform: record.platform,
            created_at_ms: record.created_at_ms,
            last_seen_at_ms: record.last_seen_at_ms,
            revoked: record.revoked,
        }
    }
}

/// 等待桌面端确认的配对请求。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MobilePendingPairingView {
    pub id: String,
    pub device_name: String,
    pub platform: Option<String>,
    pub created_at_ms: u64,
    pub expires_at_ms: u64,
}

/// 当前配对流程的展示信息。
///
/// 只描述**仍在等待手机提交**的那个挑战。挑战一旦被提交就会被标记为已消费
/// （`PairingStore::submit`），此时 [`MobileService::pairing_view`] 返回 `None`，
/// 桌面端不再展示二维码与人工校验码。已经提交、等待桌面确认的请求不在这里，而是
/// [`MobileStatus::pending_pairings`]——它的生命周期比挑战长，且不依赖挑战是否过期。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MobilePairingView {
    pub challenge_id: String,
    /// 显示在电脑屏幕上的 6 位人工校验码。
    pub code: String,
    /// 二维码内容。只包含挑战 ID 与挑战密钥。
    pub uri: String,
    pub expires_at_ms: u64,
    pub tls: bool,
    pub fingerprint: Option<String>,
}

/// 网关状态快照。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MobileStatus {
    /// 网关是否正在监听。
    pub running: bool,
    pub host: Option<String>,
    pub port: Option<u16>,
    pub scheme: Option<String>,
    pub fingerprint: Option<String>,
    /// 探测到的本机局域网地址，供用户选择。
    pub lan_addresses: Vec<String>,
    /// 用户偏好的绑定地址与端口（即使当前未运行也保留）。
    pub preferred_bind_address: Option<String>,
    pub preferred_port: u16,
    pub connections: usize,
    pub capabilities: Vec<MobileCapability>,
    pub pairing: Option<MobilePairingView>,
    /// 已提交、等待桌面端确认的配对请求。
    ///
    /// 刻意放在状态顶层而不是 [`MobilePairingView`] 里：手机提交后挑战立即变成已消费，
    /// `pairing` 随之变回 `None`，如果待确认请求挂在它下面就会在提交成功的那一刻消失，
    /// 桌面端再也看不到「等待确认的设备」——那正是这个字段此前放错位置导致的缺陷。
    pub pending_pairings: Vec<MobilePendingPairingView>,
    pub devices: Vec<MobileDeviceView>,
}

/// 网关服务。由 Tauri 管理，桌面命令和事件扇出都通过它。
///
/// 泛型化到 `R: Runtime` 是为了让测试能在 Tauri 的 mock runtime 下注册本服务并驱动
/// 真实命令；生产路径始终实例化为默认的 `Wry`，因此现有调用点无需显式标注。
pub struct MobileService<R: Runtime = Wry> {
    /// 宿主应用句柄。只用于恢复监听和让发布器找回本服务。
    app: AppHandle<R>,
    ctx: GatewayContext,
    data_root: PathBuf,
    runtime: Mutex<Option<server::ServerHandle>>,
}

impl MobileService<Wry> {
    /// 生产路径：宿主是真实 Tauri 应用。
    pub fn new(app: AppHandle<Wry>, data_root: PathBuf) -> Result<Self, MobileError> {
        let host: Arc<dyn GatewayHost> = Arc::new(TauriHost::new(app.clone()));
        Self::with_host(app, data_root, host)
    }
}

impl<R: Runtime> MobileService<R> {
    /// 允许注入宿主。
    ///
    /// 网关内部本就把宿主当作 `dyn GatewayHost` 使用，因此测试可以在 Tauri 的 mock
    /// runtime 上注册本服务并驱动真实命令，而不必把 `commands` 里的 Turn 执行链整体
    /// 泛型化（`TauriHost::start_turn` 会级联到 `drain_thread_mailbox` 等一长串函数）。
    pub fn with_host(
        app: AppHandle<R>,
        data_root: PathBuf,
        host: Arc<dyn GatewayHost>,
    ) -> Result<Self, MobileError> {
        let registry = Arc::new(DeviceRegistry::load(&data_root)?);
        let ctx = GatewayContext {
            host,
            registry,
            tokens: Arc::new(TokenStore::new()),
            pairing: Arc::new(PairingStore::new()),
            policy: Arc::new(CapabilityPolicy::new()),
            hub: Arc::new(EventHub::new()),
            pending: Arc::new(PendingRequestIndex::new()),
            idempotency: Arc::new(IdempotencyCache::new()),
            fingerprint: Arc::new(RwLock::new(None)),
        };
        Ok(Self {
            app,
            ctx,
            data_root,
            runtime: Mutex::new(None),
        })
    }

    pub fn context(&self) -> &GatewayContext {
        &self.ctx
    }

    /// 把领域事件扇出到已订阅的移动端连接。
    ///
    /// 由 `TauriEventPublisher` 在每次发布时调用，因此桌面和手机看到的是同一份事实。
    pub fn publish_event(&self, envelope: &AgentEventEnvelope) {
        self.ctx.pending.observe(envelope);
        self.ctx.hub.publish(envelope);
    }

    /// 启动时按用户此前的选择恢复监听。
    pub fn restore_on_startup(&self) -> Result<(), MobileError> {
        let settings = self.ctx.registry.settings();
        if !settings.enabled {
            return Ok(());
        }
        // 启动阶段不能阻塞事件循环，因此把真正的绑定放到异步任务里。
        let app = self.app.clone();
        let bind_address = settings.bind_address.clone();
        let port = settings.port;
        tauri::async_runtime::spawn(async move {
            let Some(state) = app.try_state::<MobileService<R>>() else {
                return;
            };
            if let Err(error) = state.start(bind_address, port).await {
                // 恢复失败不能影响桌面启动，但必须留痕。
                if let Some(app_state) = app.try_state::<crate::app_state::AppState>() {
                    let _ = app_state.logger().log(
                        "error",
                        "mobile.gateway.restore_failed",
                        serde_json::json!({ "reason": error.message }),
                    );
                }
            }
        });
        Ok(())
    }

    /// 启动网关。`bind_address` 为空表示只监听回环地址。
    pub async fn start(
        &self,
        bind_address: Option<String>,
        port: u16,
    ) -> Result<MobileStatus, MobileError> {
        if self.is_running() {
            return Err(MobileError::new(
                MobileErrorKind::InvalidRequest,
                "mobile gateway is already running",
            ));
        }
        if port == 0 {
            return Err(MobileError::invalid_params("port must be 1..=65535"));
        }
        let ip = server::resolve_bind_ip(bind_address.as_deref())?;
        let tls = if ip.is_loopback() {
            None
        } else {
            Some(tls::load_or_create_identity(
                &self.data_root,
                &ip.to_string(),
            )?)
        };

        let handle = server::start(self.ctx.clone(), server::ServerBind { ip, port }, tls).await?;

        self.ctx.registry.update_settings(|settings| {
            settings.enabled = true;
            settings.bind_address = if ip.is_loopback() {
                None
            } else {
                Some(ip.to_string())
            };
            settings.port = handle.address.port();
        })?;
        *self.runtime.lock().expect("runtime lock poisoned") = Some(handle);
        Ok(self.status())
    }

    pub fn stop(&self) -> MobileStatus {
        let handle = self.runtime.lock().expect("runtime lock poisoned").take();
        if let Some(handle) = handle {
            handle.shutdown();
        }
        self.ctx.pairing.clear();
        {
            let mut fingerprint = self
                .ctx
                .fingerprint
                .write()
                .expect("fingerprint lock poisoned");
            *fingerprint = None;
        }
        let _ = self
            .ctx
            .registry
            .update_settings(|settings| settings.enabled = false);
        self.status()
    }

    pub fn is_running(&self) -> bool {
        self.runtime
            .lock()
            .expect("runtime lock poisoned")
            .is_some()
    }

    pub fn status(&self) -> MobileStatus {
        let settings = self.ctx.registry.settings();
        let runtime = self.runtime.lock().expect("runtime lock poisoned");
        let (host, port, scheme, connections) = match runtime.as_ref() {
            Some(handle) => (
                Some(handle.address.ip().to_string()),
                Some(handle.address.port()),
                Some(handle.scheme.to_string()),
                self.ctx.hub.connection_count(),
            ),
            None => (None, None, None, 0),
        };
        drop(runtime);

        MobileStatus {
            running: self.is_running(),
            host,
            port,
            scheme,
            fingerprint: self
                .ctx
                .fingerprint
                .read()
                .expect("fingerprint lock poisoned")
                .clone(),
            lan_addresses: server::detect_lan_addresses(),
            preferred_bind_address: settings.bind_address,
            preferred_port: settings.port,
            connections,
            capabilities: self.ctx.policy.granted(),
            pairing: self.pairing_view(),
            pending_pairings: self.pending_pairing_views(),
            devices: self
                .ctx
                .registry
                .devices()
                .into_iter()
                .map(MobileDeviceView::from)
                .collect(),
        }
    }

    /// 创建新的配对挑战。旧的挑战和待确认请求都会失效。
    pub fn create_pairing(&self) -> Result<MobilePairingView, MobileError> {
        let runtime = self.runtime.lock().expect("runtime lock poisoned");
        let Some(handle) = runtime.as_ref() else {
            return Err(MobileError::new(
                MobileErrorKind::InvalidRequest,
                "start the mobile gateway before creating a pairing challenge",
            ));
        };
        let challenge = self.ctx.pairing.create_challenge(
            &handle.address.ip().to_string(),
            handle.address.port(),
            handle.scheme == "https",
            handle.fingerprint.clone(),
        );
        Ok(MobilePairingView {
            challenge_id: challenge.id.clone(),
            code: challenge.code.clone(),
            uri: challenge.pairing_uri(),
            expires_at_ms: challenge.expires_at_ms,
            tls: challenge.tls,
            fingerprint: challenge.fingerprint.clone(),
        })
    }

    fn pairing_view(&self) -> Option<MobilePairingView> {
        let challenge = self.ctx.pairing.current_challenge()?;
        Some(MobilePairingView {
            challenge_id: challenge.id.clone(),
            code: challenge.code.clone(),
            uri: challenge.pairing_uri(),
            expires_at_ms: challenge.expires_at_ms,
            tls: challenge.tls,
            fingerprint: challenge.fingerprint.clone(),
        })
    }

    /// 已提交、等待桌面端确认的配对请求。
    ///
    /// 与 `pairing_view` 解耦：挑战被消费后 `pairing_view` 会变成 `None`，但这里的请求
    /// 在 `PAIRING_CONFIRM_TTL_MS` 内仍然有效，必须继续对桌面端可见。
    fn pending_pairing_views(&self) -> Vec<MobilePendingPairingView> {
        self.ctx
            .pairing
            .pending()
            .into_iter()
            .map(|pending| MobilePendingPairingView {
                id: pending.id,
                device_name: pending.device_name,
                platform: pending.platform,
                created_at_ms: pending.created_at_ms,
                expires_at_ms: pending.expires_at_ms,
            })
            .collect()
    }

    /// 桌面端确认配对，生成设备凭据。
    pub fn approve_pairing(&self, pending_id: &str) -> Result<MobileDeviceView, MobileError> {
        let record = self.ctx.pairing.approve(pending_id, &self.ctx.registry)?;
        Ok(MobileDeviceView::from(record))
    }

    pub fn deny_pairing(&self, pending_id: &str) -> Result<(), MobileError> {
        self.ctx.pairing.deny(pending_id)
    }

    /// 撤销设备：登记表标记为已撤销，并立即清掉它的全部访问令牌。
    pub fn revoke_device(&self, device_id: &str) -> Result<MobileDeviceView, MobileError> {
        self.ctx.registry.revoke(device_id)?;
        self.ctx.tokens.revoke_device(device_id);
        let record = self
            .ctx
            .registry
            .device(device_id)
            .ok_or_else(|| MobileError::not_found("unknown device"))?;
        Ok(MobileDeviceView::from(record))
    }

    /// 调整授予移动端的能力。首期只允许在已实现能力范围内变更。
    pub fn set_capabilities(&self, capabilities: Vec<MobileCapability>) {
        self.ctx.policy.set_granted(capabilities);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_view_never_carries_secrets() {
        let record = DeviceRecord {
            id: "device-1".to_string(),
            name: "Pixel".to_string(),
            platform: Some("android".to_string()),
            created_at_ms: 1,
            last_seen_at_ms: 2,
            revoked: false,
            secret_hash: "hash".to_string(),
            refresh_hash: "hash".to_string(),
        };
        let encoded = serde_json::to_string(&MobileDeviceView::from(record)).unwrap();
        assert!(!encoded.contains("secret"));
        assert!(!encoded.contains("refresh"));
        assert!(!encoded.contains("hash"));
    }

    #[test]
    fn lan_detection_never_returns_loopback() {
        for address in server::detect_lan_addresses() {
            let parsed: std::net::IpAddr = address.parse().unwrap();
            assert!(!parsed.is_loopback());
            assert!(server::is_allowed_source(parsed));
        }
    }
}
