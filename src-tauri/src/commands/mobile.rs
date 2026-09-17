//! 移动网关的 Tauri 命令边界。
//!
//! 这些命令只做参数整理和错误翻译，真正的逻辑都在 `crate::mobile::MobileService` 里。
//! 与其它命令一样，本模块不包含任何智能体循环逻辑。
//!
//! 命令泛型化到 `R: Runtime`，使 `tests` 能在 Tauri 的 mock runtime 上注册真实服务并
//! 驱动真实命令。生产路径由 `generate_handler!` 以默认的 `Wry` 实例化，行为不变。

use tauri::{AppHandle, Manager, Runtime};

use crate::mobile::{
    MobileCapability, MobileDeviceView, MobilePairingView, MobileService, MobileStatus,
};

use super::{CommandError, CommandResult};

fn service<R: Runtime>(app: &AppHandle<R>) -> CommandResult<tauri::State<'_, MobileService<R>>> {
    app.try_state::<MobileService<R>>()
        .ok_or_else(|| CommandError::internal("mobile gateway service is unavailable"))
}

#[tauri::command(rename_all = "camelCase")]
pub async fn mobile_status<R: Runtime>(app: AppHandle<R>) -> CommandResult<MobileStatus> {
    Ok(service(&app)?.status())
}

/// 启动网关。`bindAddress` 为空时只监听回环地址；绑定到局域网地址时强制使用 TLS。
#[tauri::command(rename_all = "camelCase")]
pub async fn mobile_start<R: Runtime>(
    app: AppHandle<R>,
    bind_address: Option<String>,
    port: Option<u16>,
) -> CommandResult<MobileStatus> {
    let service = service(&app)?;
    let port = port.unwrap_or_else(|| service.status().preferred_port);
    service
        .start(bind_address, port)
        .await
        .map_err(|error| CommandError::new("mobile", error))
}

#[tauri::command(rename_all = "camelCase")]
pub async fn mobile_stop<R: Runtime>(app: AppHandle<R>) -> CommandResult<MobileStatus> {
    Ok(service(&app)?.stop())
}

/// 生成一次性配对挑战（10 分钟有效、单次使用）。
#[tauri::command(rename_all = "camelCase")]
pub async fn mobile_create_pairing<R: Runtime>(
    app: AppHandle<R>,
) -> CommandResult<MobilePairingView> {
    service(&app)?
        .create_pairing()
        .map_err(|error| CommandError::new("mobile", error))
}

/// 桌面端确认配对，为设备生成凭据。
#[tauri::command(rename_all = "camelCase")]
pub async fn mobile_approve_pairing<R: Runtime>(
    app: AppHandle<R>,
    pending_id: String,
) -> CommandResult<MobileDeviceView> {
    service(&app)?
        .approve_pairing(&pending_id)
        .map_err(|error| CommandError::new("mobile", error))
}

#[tauri::command(rename_all = "camelCase")]
pub async fn mobile_deny_pairing<R: Runtime>(
    app: AppHandle<R>,
    pending_id: String,
) -> CommandResult<()> {
    service(&app)?
        .deny_pairing(&pending_id)
        .map_err(|error| CommandError::new("mobile", error))
}

/// 撤销设备。撤销立即失效该设备的访问令牌。
#[tauri::command(rename_all = "camelCase")]
pub async fn mobile_revoke_device<R: Runtime>(
    app: AppHandle<R>,
    device_id: String,
) -> CommandResult<MobileDeviceView> {
    service(&app)?
        .revoke_device(&device_id)
        .map_err(|error| CommandError::new("mobile", error))
}

/// 删除设备记录。删除活跃设备等同于「撤销 + 遗忘」，同样立即失效其访问令牌。
#[tauri::command(rename_all = "camelCase")]
pub async fn mobile_remove_device<R: Runtime>(
    app: AppHandle<R>,
    device_id: String,
) -> CommandResult<()> {
    service(&app)?
        .remove_device(&device_id)
        .map_err(|error| CommandError::new("mobile", error))
}

#[tauri::command(rename_all = "camelCase")]
pub async fn mobile_set_capabilities<R: Runtime>(
    app: AppHandle<R>,
    capabilities: Vec<MobileCapability>,
) -> CommandResult<MobileStatus> {
    let service = service(&app)?;
    service.set_capabilities(capabilities);
    Ok(service.status())
}

/// 驱动真实命令函数、经 mock runtime 走「设置页 → 网关」这条命令通路的测试。
///
/// **为什么默认不编译**：`tauri::test` 的 mock runtime 会把窗口层链进测试二进制，从而导入
/// `comctl32.dll!TaskDialogIndirect`（Common Controls v6 专有导出）。Windows 上缺少 v6 清单
/// 声明时，加载器解析到 System32 的 v5.82 并报 `STATUS_ENTRYPOINT_NOT_FOUND`（`0xc0000139`），
/// 整个 lib 测试二进制在跑任何用例之前就死掉。补清单的唯一链接参数作用域
/// （`cargo:rustc-link-arg`）同时作用于 bin，会和 `tauri_build` 注入的应用清单叠加，
/// 所以整体挂在 `mock-runtime-tests` feature 后面，默认构建完全不受影响。
///
/// 跑法：`cargo test --lib --features mock-runtime-tests`
#[cfg(all(test, feature = "mock-runtime-tests"))]
mod tests {
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::Arc;

    use serde_json::Value;
    use tauri::Manager;
    use tauri::test::{MockRuntime, mock_builder, mock_context, noop_assets};
    use tempfile::TempDir;

    use super::*;
    use crate::agent::RunTurnRequest;
    use crate::app_state::AppState;
    use crate::mobile::MobileError;
    use crate::mobile::host::GatewayHost;
    use crate::protocol::{ImageAttachment, TurnHandle};

    /// 只满足 `MobileService` 的构造需求。
    ///
    /// 本模块的八个命令没有一个会真正发起 Turn，所以宿主不会被调用；这里显式失败而不是
    /// 静默返回成功，避免将来有人在命令里接了 Turn 之后测试假装通过。
    struct StubHost;

    impl GatewayHost for StubHost {
        fn app_state(&self) -> Option<&AppState> {
            None
        }

        fn log(&self, _level: &str, _event: &str, _fields: Value) {}

        fn start_turn<'a>(
            &'a self,
            _state: &'a AppState,
            _request: RunTurnRequest,
            _attachments: Vec<ImageAttachment>,
            _workflow_id: Option<String>,
        ) -> Pin<Box<dyn Future<Output = Result<TurnHandle, MobileError>> + Send + 'a>> {
            Box::pin(async { Err(MobileError::internal("stub host cannot start turns")) })
        }
    }

    /// 建一个注册了真实 `MobileService` 的 mock 应用。
    ///
    /// 刻意不创建 mock 窗口：`WebviewWindowBuilder` 会把窗口层链进测试二进制，而本项目
    /// 依赖 `tauri-plugin-dialog`（→ `rfd`），窗口层一旦链入就会带上 `comctl32.dll` 的
    /// `TaskDialogIndirect`。那是 Common Controls v6 专有导出，测试二进制没有声明 v6
    /// 清单，加载器会解析到 System32 下的 v5 并报 `STATUS_ENTRYPOINT_NOT_FOUND`
    /// （`0xc0000139`），整个测试进程起不来。命令函数本身不需要窗口，因此这里只取
    /// `AppHandle` 直接调用。
    fn app_with_service() -> (tauri::App<MockRuntime>, TempDir) {
        let app = mock_builder()
            .invoke_handler(tauri::generate_handler![
                mobile_status,
                mobile_start,
                mobile_stop,
                mobile_create_pairing,
                mobile_approve_pairing,
                mobile_deny_pairing,
                mobile_revoke_device,
                mobile_remove_device,
                mobile_set_capabilities
            ])
            .build(mock_context(noop_assets()))
            .expect("mock app must build");
        let directory = TempDir::new().expect("temp dir");
        let service = MobileService::with_host(
            app.handle().clone(),
            directory.path().to_path_buf(),
            Arc::new(StubHost),
        )
        .expect("mobile service must build");
        app.manage(service);
        (app, directory)
    }

    fn app_without_service() -> tauri::App<MockRuntime> {
        mock_builder()
            .invoke_handler(tauri::generate_handler![mobile_status])
            .build(mock_context(noop_assets()))
            .expect("mock app must build")
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn status_reports_a_stopped_gateway() {
        let (app, _directory) = app_with_service();
        let status = mobile_status(app.handle().clone())
            .await
            .expect("status must succeed");

        assert!(!status.running);
        assert!(status.host.is_none());
        assert!(status.scheme.is_none());
        assert!(status.fingerprint.is_none());
        assert!(status.devices.is_empty());
        // `capabilities` 是 `CapabilityPolicy` 里**已配置**的授权集，与网关是否在跑无关；
        // 新建服务时就是 `DEFAULT_GRANTED`。这条断言把「停止态仍能看到已授权能力」这个语义
        // 钉住，免得将来有人把它误改成运行态字段——那样设置页在未启动时就无法回显开关状态。
        assert_eq!(
            status.capabilities,
            MobileCapability::DEFAULT_GRANTED.to_vec()
        );
        assert!(status.pairing.is_none());
    }

    /// 服务未注册时必须走 `CommandError::internal`，而不是 panic 或返回默认值。
    #[tokio::test(flavor = "multi_thread")]
    async fn missing_service_reports_an_internal_error() {
        let app = app_without_service();

        let error = mobile_status(app.handle().clone())
            .await
            .expect_err("status must fail without a registered service");

        assert_eq!(error.code, "internal_error");
        assert!(
            error
                .message
                .contains("mobile gateway service is unavailable")
        );
    }

    /// `MobileError` 经 `CommandError::new("mobile", …)` 翻译后必须保留种类前缀，
    /// 否则设置页只能显示一句无从判断的错误文本。
    #[tokio::test(flavor = "multi_thread")]
    async fn start_translates_a_rejected_bind_address() {
        let (app, _directory) = app_with_service();

        let error = mobile_start(
            app.handle().clone(),
            Some("8.8.8.8".to_string()),
            Some(18790),
        )
        .await
        .expect_err("a public bind address must be rejected");

        assert_eq!(error.code, "mobile");
        assert!(
            error.message.starts_with("invalid_params: "),
            "unexpected message: {}",
            error.message
        );
        assert!(
            error
                .message
                .contains("loopback or private network address")
        );
    }

    /// 端口越界在触碰网络之前就被拒绝，因此这条不会占用真实端口。
    #[tokio::test(flavor = "multi_thread")]
    async fn start_rejects_port_zero_without_touching_the_network() {
        let (app, _directory) = app_with_service();
        let handle = app.handle().clone();

        let error = mobile_start(handle.clone(), Some("127.0.0.1".to_string()), Some(0))
            .await
            .expect_err("port 0 must be rejected");

        assert_eq!(error.code, "mobile");
        assert!(
            error
                .message
                .contains("invalid_params: port must be 1..=65535")
        );

        // 失败的启动不得让服务进入「正在运行」。
        let status = mobile_status(handle).await.expect("status must succeed");
        assert!(!status.running);
    }

    /// 设置页「开启局域网访问」按钮的完整往返：真实命令 → 真实绑定 → 真实停止。
    #[tokio::test(flavor = "multi_thread")]
    async fn start_and_stop_round_trip_binds_a_real_socket() {
        let (app, _directory) = app_with_service();
        let handle = app.handle().clone();

        // 回环绑定不启用 TLS。端口被占用时换下一个，避免把环境问题误判成回归。
        let mut started = None;
        let mut last_error = String::new();
        for port in [18871u16, 18872, 18873] {
            match mobile_start(handle.clone(), Some("127.0.0.1".to_string()), Some(port)).await {
                Ok(status) => {
                    started = Some(status);
                    break;
                }
                Err(error) => {
                    last_error = error.message.clone();
                    assert!(
                        last_error.contains("failed to bind"),
                        "unexpected start failure: {last_error}"
                    );
                }
            }
        }
        let status = started
            .unwrap_or_else(|| panic!("no loopback port was free; last error: {last_error}"));

        assert!(status.running);
        assert_eq!(status.host.as_deref(), Some("127.0.0.1"));
        assert_eq!(status.scheme.as_deref(), Some("http"));

        let stopped = mobile_stop(handle).await.expect("stop must succeed");
        assert!(!stopped.running);
        assert!(stopped.scheme.is_none());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn stop_is_idempotent_when_the_gateway_is_not_running() {
        let (app, _directory) = app_with_service();
        let handle = app.handle().clone();

        let first = mobile_stop(handle.clone())
            .await
            .expect("stop must succeed");
        let second = mobile_stop(handle).await.expect("stop must succeed");

        assert!(!first.running);
        assert!(!second.running);
    }

    /// 未启动网关时不能签发配对挑战，否则二维码会指向一个不存在的监听地址。
    #[tokio::test(flavor = "multi_thread")]
    async fn pairing_cannot_be_created_before_the_gateway_starts() {
        let (app, _directory) = app_with_service();

        let error = mobile_create_pairing(app.handle().clone())
            .await
            .expect_err("pairing must fail while the gateway is stopped");

        assert_eq!(error.code, "mobile");
        assert!(error.message.contains("invalid_request: "));
        assert!(error.message.contains("start the mobile gateway"));
    }

    /// 手机提交配对后挑战立即变成已消费，桌面端必须**仍然**能看到这条待确认请求。
    ///
    /// 回归背景：待确认请求曾经挂在 `MobilePairingView.pending` 下面，而 `pairing` 在挑战被
    /// 消费的那一刻就返回 `None`——于是手机提交成功的同时请求从设置页消失，「允许」按钮
    /// 永远没有机会出现，配对流程在最后一步静默断掉。此前的测试全都绕过了这条接缝：
    /// `mobile_gateway.rs` 直接调 `PairingStore::approve`，`verify-mobile-native.mjs` 用的是
    /// 伪造挑战，`e2e/mobile-settings.spec.ts` 打桩的状态里 `pairing` 根本不会被消费。
    #[tokio::test(flavor = "multi_thread")]
    async fn submitted_pairing_stays_visible_after_the_challenge_is_consumed() {
        let (app, _directory) = app_with_service();
        let handle = app.handle().clone();

        let mut started = false;
        for port in [18874u16, 18875, 18876] {
            match mobile_start(handle.clone(), Some("127.0.0.1".to_string()), Some(port)).await {
                Ok(status) => {
                    assert!(status.running);
                    started = true;
                    break;
                }
                Err(error) => assert!(
                    error.message.contains("failed to bind"),
                    "unexpected start failure: {}",
                    error.message
                ),
            }
        }
        assert!(started, "no loopback port was free");

        let created = mobile_create_pairing(handle.clone())
            .await
            .expect("pairing challenge must be created");

        // 手机侧拿到的只有二维码 fragment 里的挑战密钥，测试直接从登记表取同一份材料。
        let service = app.state::<MobileService<MockRuntime>>();
        let challenge = service
            .context()
            .pairing
            .current_challenge()
            .expect("a fresh challenge must be current");
        assert_eq!(challenge.id, created.challenge_id);

        let submitted = service
            .context()
            .pairing
            .submit(
                &challenge.id,
                &challenge.secret,
                &challenge.code,
                "iPhone 16",
                Some("iOS".to_string()),
            )
            .expect("submit must succeed");

        let status = mobile_status(handle.clone())
            .await
            .expect("status must succeed");
        assert!(
            status.pairing.is_none(),
            "a consumed challenge must leave the screen"
        );
        assert_eq!(
            status.pending_pairings.len(),
            1,
            "the submitted request must stay visible to the desktop"
        );
        assert_eq!(status.pending_pairings[0].id, submitted.id);
        assert_eq!(status.pending_pairings[0].device_name, "iPhone 16");
        assert_eq!(status.pending_pairings[0].platform.as_deref(), Some("iOS"));

        let device = mobile_approve_pairing(handle.clone(), submitted.id.clone())
            .await
            .expect("the visible request must be approvable");
        assert_eq!(device.name, "iPhone 16");
        assert!(!device.revoked);

        let after = mobile_status(handle.clone())
            .await
            .expect("status must succeed");
        assert!(after.pending_pairings.is_empty());
        assert!(after.devices.iter().any(|entry| entry.id == device.id));
    }

    /// 能力开关是设置页唯一的写路径。
    #[tokio::test(flavor = "multi_thread")]
    async fn set_capabilities_updates_the_granted_set() {
        let (app, _directory) = app_with_service();
        let handle = app.handle().clone();

        let status = mobile_set_capabilities(
            handle.clone(),
            vec![MobileCapability::Chat, MobileCapability::Approval],
        )
        .await
        .expect("setting capabilities must succeed");

        assert_eq!(
            status.capabilities,
            vec![MobileCapability::Chat, MobileCapability::Approval]
        );

        let read_back = mobile_status(handle).await.expect("status must succeed");
        assert_eq!(
            read_back.capabilities,
            vec![MobileCapability::Chat, MobileCapability::Approval]
        );
    }

    /// 配对与撤销的失败路径必须走 `not_found`，且不泄露任何设备凭据。
    #[tokio::test(flavor = "multi_thread")]
    async fn pairing_and_device_decisions_on_unknown_ids_fail_closed() {
        let (app, _directory) = app_with_service();
        let handle = app.handle().clone();

        let approve = mobile_approve_pairing(handle.clone(), "pending-does-not-exist".to_string())
            .await
            .expect_err("approving an unknown pending pairing must fail");
        assert_eq!(approve.code, "mobile");
        assert!(approve.message.starts_with("not_found: "));

        let deny = mobile_deny_pairing(handle.clone(), "pending-does-not-exist".to_string())
            .await
            .expect_err("denying an unknown pending pairing must fail");
        assert_eq!(deny.code, "mobile");
        assert!(deny.message.starts_with("not_found: "));

        let revoke = mobile_revoke_device(handle.clone(), "device-does-not-exist".to_string())
            .await
            .expect_err("revoking an unknown device must fail");
        assert_eq!(revoke.code, "mobile");
        assert!(revoke.message.starts_with("not_found: "));

        let remove = mobile_remove_device(handle, "device-does-not-exist".to_string())
            .await
            .expect_err("removing an unknown device must fail");
        assert_eq!(remove.code, "mobile");
        assert!(remove.message.starts_with("not_found: "));

        let encoded = format!("{} {}", revoke.code, revoke.message);
        assert!(!encoded.contains("secret"));
        assert!(!encoded.contains("refresh"));
    }

    /// 「删除」必须同时做到三件事：从状态快照里消失、访问令牌立即失效、重开登记表不复活。
    ///
    /// 只做其中一件都会留下可被利用的缺口：只改内存则重启后设备复活；只删记录不清令牌
    /// 则旧令牌在下一次 `resolve` 之前仍然指向一台「曾经合法」的设备。
    #[tokio::test(flavor = "multi_thread")]
    async fn removing_a_device_drops_it_from_status_and_invalidates_its_tokens() {
        let (app, directory) = app_with_service();
        let handle = app.handle().clone();
        let service = app.state::<MobileService<MockRuntime>>();
        let (record, _credentials) = service
            .context()
            .registry
            .register_device("Pixel", Some("android".to_string()))
            .expect("device registration must succeed");
        let (token, _) = service.context().tokens.issue(&record.id);

        let before = mobile_status(handle.clone())
            .await
            .expect("status must succeed");
        assert_eq!(before.devices.len(), 1);
        assert_eq!(before.devices[0].id, record.id);

        mobile_remove_device(handle.clone(), record.id.clone())
            .await
            .expect("removing a known device must succeed");

        let after = mobile_status(handle.clone())
            .await
            .expect("status must succeed");
        assert!(
            after.devices.is_empty(),
            "the removed device must leave the settings list"
        );

        assert!(
            service
                .context()
                .tokens
                .resolve(&token, &service.context().registry)
                .is_err(),
            "the access token must stop resolving once the device is gone"
        );

        // 落盘状态同样不能把它带回来。
        let reloaded =
            crate::mobile::auth::DeviceRegistry::load(directory.path()).expect("reload must work");
        assert!(reloaded.devices().is_empty());

        // 重复删除走 not_found，不静默成功。
        let error = mobile_remove_device(handle, record.id)
            .await
            .expect_err("removing an unknown device must fail");
        assert_eq!(error.code, "mobile");
        assert!(error.message.starts_with("not_found: "));
    }
}
