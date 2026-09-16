//! 局域网直连网关的传输层：HTTP 路由、WebSocket 会话、来源校验与限流。
//!
//! 安全边界（全部是硬约束）：
//! - 只接受回环、RFC1918 私网和链路本地来源；公网路由地址一律拒绝。
//! - 拒绝 `X-Forwarded-*` / `Forwarded` 等转发头，避免代理伪造来源。
//! - `Host` 必须是允许的 IP 字面量或 `localhost`，阻断 DNS 重绑定。
//! - 握手与配对分别限流，配对挑战另有单挑战失败次数上限。
//! - 绑定到非回环地址时必须启用 TLS。

use std::collections::HashMap;
use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{ConnectInfo, State};
use axum::http::{HeaderMap, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::serve::ListenerExt;
use axum::{Json, Router};
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Semaphore;
use tokio_rustls::TlsAcceptor;
use tokio_util::sync::CancellationToken;

use super::auth::PairingPollOutcome;
use super::events::{EventPriority, QueueMessage};
use super::protocol::{MobileError, MobileErrorKind, RpcNotification, RpcRequest, RpcResponse};
use super::rpc::{self, GatewayContext, RpcSession};

/// 单条 WebSocket 文本帧上限，避免移动端上传无界载荷。
pub const MAX_FRAME_BYTES: usize = 256 * 1024;
/// 单连接同时处理的请求上限。
pub const MAX_CONCURRENT_REQUESTS: usize = 8;
/// 每 IP 每分钟的握手次数上限。
pub const HANDSHAKE_LIMIT_PER_MINUTE: u32 = 60;
/// 每 IP 每 10 分钟的配对提交次数上限。
pub const PAIRING_LIMIT_PER_WINDOW: u32 = 10;
const PAIRING_WINDOW_MS: u64 = 10 * 60 * 1000;

/// 启动网关所需的绑定信息。
#[derive(Debug, Clone)]
pub struct ServerBind {
    pub ip: IpAddr,
    pub port: u16,
}

/// 运行中的网关句柄。
#[derive(Debug)]
pub struct ServerHandle {
    pub address: SocketAddr,
    pub scheme: &'static str,
    pub fingerprint: Option<String>,
    pub cancellation: CancellationToken,
    pub join: tokio::task::JoinHandle<()>,
}

impl ServerHandle {
    pub fn shutdown(&self) {
        self.cancellation.cancel();
        self.join.abort();
    }
}

/// 按窗口限流的简易计数器。
#[derive(Debug, Default)]
pub struct RateLimiter {
    windows: Mutex<HashMap<(IpAddr, &'static str), RateWindow>>,
}

#[derive(Debug, Clone, Copy)]
struct RateWindow {
    count: u32,
    reset_at_ms: u64,
}

impl RateLimiter {
    pub fn new() -> Self {
        Self::default()
    }

    /// 消耗一次配额。超限时返回需要等待的毫秒数。
    pub fn check(
        &self,
        ip: IpAddr,
        kind: &'static str,
        limit: u32,
        window_ms: u64,
    ) -> Result<(), u64> {
        let now = super::auth::now_ms();
        let mut windows = self.windows.lock().expect("rate limiter lock poisoned");
        // 过期窗口直接回收；新窗口从 0 开始计数。
        windows.retain(|_, window| window.reset_at_ms > now);
        let window = windows.entry((ip, kind)).or_insert(RateWindow {
            count: 0,
            reset_at_ms: now + window_ms,
        });
        if window.reset_at_ms <= now {
            window.count = 0;
            window.reset_at_ms = now + window_ms;
        }
        if window.count >= limit {
            return Err(window.reset_at_ms.saturating_sub(now));
        }
        window.count += 1;
        Ok(())
    }
}

/// 判断来源地址是否属于允许的局域网范围。
pub fn is_allowed_source(ip: IpAddr) -> bool {
    match normalize_ip(ip) {
        IpAddr::V4(address) => {
            address.is_loopback() || address.is_private() || address.is_link_local()
        }
        IpAddr::V6(address) => {
            address.is_loopback() || address.is_unique_local() || address.is_unicast_link_local()
        }
    }
}

/// 把 IPv4 映射的 IPv6 地址还原成 IPv4，避免绕过私网判断。
pub fn normalize_ip(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(address) => match address.to_ipv4_mapped() {
            Some(mapped) => IpAddr::V4(mapped),
            None => IpAddr::V6(address),
        },
        other => other,
    }
}

/// 校验 `Host` 头，阻断 DNS 重绑定。
pub fn host_header_is_allowed(host: &str) -> bool {
    let hostname = host
        .rsplit_once(':')
        .map(|(name, _)| name)
        .unwrap_or(host)
        .trim_matches(|character| character == '[' || character == ']');
    if hostname.eq_ignore_ascii_case("localhost") {
        return true;
    }
    match hostname.parse::<IpAddr>() {
        Ok(address) => is_allowed_source(address),
        Err(_) => false,
    }
}

/// 拒绝任何转发头。网关不位于代理之后，出现这些头意味着来源不可信。
pub fn has_forwarding_headers(headers: &HeaderMap) -> bool {
    const FORWARDING_HEADERS: [&str; 6] = [
        "x-forwarded-for",
        "x-forwarded-host",
        "x-forwarded-proto",
        "x-real-ip",
        "forwarded",
        "via",
    ];
    FORWARDING_HEADERS
        .iter()
        .any(|name| headers.contains_key(*name))
}

/// 解析用户选择的监听地址。只允许回环和私网地址。
pub fn resolve_bind_ip(requested: Option<&str>) -> Result<IpAddr, MobileError> {
    let Some(requested) = requested.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(IpAddr::V4(Ipv4Addr::LOCALHOST));
    };
    let address: IpAddr = requested
        .parse()
        .map_err(|_| MobileError::invalid_params("bindAddress must be an IP literal"))?;
    if !is_allowed_source(address) {
        return Err(MobileError::invalid_params(
            "bindAddress must be a loopback or private network address",
        ));
    }
    Ok(address)
}

/// 探测本机在局域网中的地址，供桌面端预填。
///
/// 通过 UDP `connect` 让内核选出出口网卡；不会真的发送数据包。
/// 探测失败时返回空列表，由用户手工填写。
pub fn detect_lan_addresses() -> Vec<String> {
    let mut addresses = Vec::new();
    for probe in ["1.1.1.1:53", "192.168.1.1:53", "10.0.0.1:53"] {
        let Ok(socket) = std::net::UdpSocket::bind("0.0.0.0:0") else {
            continue;
        };
        if socket.connect(probe).is_err() {
            continue;
        }
        let Ok(local) = socket.local_addr() else {
            continue;
        };
        let ip = normalize_ip(local.ip());
        if ip.is_loopback() {
            continue;
        }
        let text = ip.to_string();
        if !addresses.contains(&text) {
            addresses.push(text);
        }
    }
    addresses
}

#[derive(Clone)]
struct ServerState {
    ctx: GatewayContext,
    limiter: Arc<RateLimiter>,
    connections: Arc<AtomicUsize>,
    tls: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PairSubmitRequest {
    challenge_id: String,
    challenge_secret: String,
    code: String,
    device_name: String,
    platform: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PairStatusRequest {
    pending_id: String,
    challenge_secret: String,
}

/// 启动网关并返回句柄。`tls` 为 `None` 时只能绑定回环地址。
pub async fn start(
    ctx: GatewayContext,
    bind: ServerBind,
    tls: Option<super::tls::TlsIdentity>,
) -> Result<ServerHandle, MobileError> {
    if !is_allowed_source(bind.ip) {
        return Err(MobileError::invalid_params(
            "listen address must be loopback or private",
        ));
    }
    if tls.is_none() && !bind.ip.is_loopback() {
        return Err(MobileError::new(
            MobileErrorKind::InvalidRequest,
            "TLS is required when listening beyond loopback",
        ));
    }

    let listener = TcpListener::bind(SocketAddr::new(bind.ip, bind.port))
        .await
        .map_err(|error| MobileError::internal(format!("failed to bind {}: {error}", bind.ip)))?;
    let address = listener
        .local_addr()
        .map_err(|error| MobileError::internal(format!("failed to read local address: {error}")))?;

    let state = ServerState {
        ctx: ctx.clone(),
        limiter: Arc::new(RateLimiter::new()),
        connections: Arc::new(AtomicUsize::new(0)),
        tls: tls.is_some(),
    };
    let router = router(state);
    let cancellation = CancellationToken::new();

    let (scheme, fingerprint, join) = match tls {
        Some(identity) => {
            let fingerprint = identity.fingerprint.clone();
            {
                let mut slot = ctx.fingerprint.write().expect("fingerprint lock poisoned");
                *slot = Some(fingerprint.clone());
            }
            let tls_listener = TlsListener {
                listener,
                acceptor: identity.acceptor,
            }
            // `tap_io` 是 axum 唯一为自定义监听器提供 `Connected` 的入口。
            .tap_io(|_stream| {});
            let shutdown = cancellation.clone();
            let join = tokio::spawn(async move {
                let _ = axum::serve(
                    tls_listener,
                    router.into_make_service_with_connect_info::<SocketAddr>(),
                )
                .with_graceful_shutdown(async move { shutdown.cancelled().await })
                .await;
            });
            ("https", Some(fingerprint), join)
        }
        None => {
            let shutdown = cancellation.clone();
            let join = tokio::spawn(async move {
                let _ = axum::serve(
                    listener,
                    router.into_make_service_with_connect_info::<SocketAddr>(),
                )
                .with_graceful_shutdown(async move { shutdown.cancelled().await })
                .await;
            });
            ("http", None, join)
        }
    };

    Ok(ServerHandle {
        address,
        scheme,
        fingerprint,
        cancellation,
        join,
    })
}

fn router(state: ServerState) -> Router {
    Router::new()
        .route("/", get(|| async { Redirect::permanent("/m") }))
        .route("/health", get(health))
        .route("/m", get(mobile_page))
        .route("/m/", get(mobile_page))
        .route("/pair", post(pair_submit))
        .route("/pair/status", post(pair_status))
        .route("/ws", get(ws_upgrade))
        .layer(middleware::from_fn_with_state(state.clone(), guard))
        .with_state(state)
}

async fn guard(
    State(state): State<ServerState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    request: axum::extract::Request,
    next: Next,
) -> Response {
    if has_forwarding_headers(request.headers()) {
        return reject(
            StatusCode::BAD_REQUEST,
            "forwarding headers are not accepted",
        );
    }
    if !is_allowed_source(peer.ip()) {
        return reject(
            StatusCode::FORBIDDEN,
            "only loopback and private network clients are accepted",
        );
    }
    let host_allowed = request
        .headers()
        .get(axum::http::header::HOST)
        .and_then(|value| value.to_str().ok())
        .map(host_header_is_allowed)
        .unwrap_or(false);
    if !host_allowed {
        return reject(StatusCode::FORBIDDEN, "host header is not allowed");
    }
    if let Err(retry_after_ms) = state.limiter.check(
        normalize_ip(peer.ip()),
        "handshake",
        HANDSHAKE_LIMIT_PER_MINUTE,
        60_000,
    ) {
        return rate_limited(retry_after_ms);
    }
    next.run(request).await
}

fn reject(status: StatusCode, message: &str) -> Response {
    (status, Json(json!({ "error": message }))).into_response()
}

fn rate_limited(retry_after_ms: u64) -> Response {
    (
        StatusCode::TOO_MANY_REQUESTS,
        Json(json!({
            "error": "rate limited",
            "retryAfterMs": retry_after_ms,
        })),
    )
        .into_response()
}

fn mobile_error_response(error: MobileError) -> Response {
    let status = match error.kind() {
        "unauthorized" => StatusCode::UNAUTHORIZED,
        "forbidden" => StatusCode::FORBIDDEN,
        "not_found" => StatusCode::NOT_FOUND,
        "stale_request" | "invalid_params" | "invalid_request" => StatusCode::BAD_REQUEST,
        "rate_limited" => StatusCode::TOO_MANY_REQUESTS,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    };
    (
        status,
        Json(serde_json::to_value(&error).unwrap_or(Value::Null)),
    )
        .into_response()
}

async fn health(State(state): State<ServerState>) -> Response {
    Json(json!({
        "status": "ok",
        "protocolVersion": super::protocol::MOBILE_PROTOCOL_VERSION,
        "tls": state.tls,
    }))
    .into_response()
}

async fn mobile_page() -> Response {
    (
        [
            (
                axum::http::header::CONTENT_TYPE,
                "text/html; charset=utf-8",
            ),
            (axum::http::header::CACHE_CONTROL, "no-store"),
            (
                axum::http::header::CONTENT_SECURITY_POLICY,
                "default-src 'none'; style-src 'unsafe-inline'; script-src 'unsafe-inline'; connect-src 'self'",
            ),
        ],
        MOBILE_PAGE,
    )
        .into_response()
}

/// 手机端静态页面。作为单文件内联在二进制里，手机不需要任何构建产物。
const MOBILE_PAGE: &str = include_str!("assets/mobile.html");

async fn pair_submit(
    State(state): State<ServerState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Json(request): Json<PairSubmitRequest>,
) -> Response {
    if let Err(retry_after_ms) = state.limiter.check(
        normalize_ip(peer.ip()),
        "pairing",
        PAIRING_LIMIT_PER_WINDOW,
        PAIRING_WINDOW_MS,
    ) {
        return rate_limited(retry_after_ms);
    }
    match state.ctx.pairing.submit(
        &request.challenge_id,
        &request.challenge_secret,
        &request.code,
        &request.device_name,
        request.platform,
    ) {
        Ok(pending) => Json(json!({
            "pendingId": pending.id,
            "status": "awaiting_confirmation",
            "expiresAtMs": pending.expires_at_ms,
        }))
        .into_response(),
        Err(error) => mobile_error_response(error),
    }
}

async fn pair_status(
    State(state): State<ServerState>,
    Json(request): Json<PairStatusRequest>,
) -> Response {
    match state
        .ctx
        .pairing
        .poll(&request.pending_id, &request.challenge_secret)
    {
        Ok(PairingPollOutcome::Awaiting) => {
            Json(json!({ "status": "awaiting_confirmation" })).into_response()
        }
        Ok(PairingPollOutcome::Denied) => Json(json!({ "status": "denied" })).into_response(),
        Ok(PairingPollOutcome::Approved(credentials)) => Json(json!({
            "status": "approved",
            "deviceId": credentials.device_id,
            "deviceSecret": credentials.device_secret,
            "refreshToken": credentials.refresh_token,
        }))
        .into_response(),
        Err(error) => mobile_error_response(error),
    }
}

async fn ws_upgrade(
    State(state): State<ServerState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    upgrade: WebSocketUpgrade,
) -> Response {
    upgrade
        .max_message_size(MAX_FRAME_BYTES)
        .max_frame_size(MAX_FRAME_BYTES)
        .on_upgrade(move |socket| run_connection(state, normalize_ip(peer.ip()), socket))
}

async fn run_connection(state: ServerState, peer_ip: IpAddr, socket: WebSocket) {
    let (mut sink, mut stream) = socket.split();
    let (connection_id, outbound) = state.ctx.hub.register();
    state.connections.fetch_add(1, Ordering::SeqCst);
    let session = Arc::new(Mutex::new(RpcSession {
        connection_id,
        peer: peer_ip.to_string(),
        ..Default::default()
    }));

    let writer_outbound = outbound.clone();
    let writer = tokio::spawn(async move {
        while let Some(message) = writer_outbound.next().await {
            let text = match message {
                QueueMessage::Payload(value) => value.to_string(),
                QueueMessage::Resync => serde_json::to_string(&RpcNotification::new(
                    "resync_required",
                    json!({ "reason": "backpressure" }),
                ))
                .unwrap_or_else(|_| "{}".to_string()),
            };
            if sink.send(Message::Text(text.into())).await.is_err() {
                break;
            }
        }
        let _ = sink.close().await;
    });

    let permits = Arc::new(Semaphore::new(MAX_CONCURRENT_REQUESTS));
    while let Some(frame) = stream.next().await {
        let Ok(frame) = frame else {
            break;
        };
        match frame {
            Message::Text(text) => {
                let ctx = state.ctx.clone();
                let session = session.clone();
                let outbound = outbound.clone();
                let permits = permits.clone();
                tokio::spawn(async move {
                    let Ok(_permit) = permits.acquire_owned().await else {
                        return;
                    };
                    let response = match serde_json::from_str::<RpcRequest>(&text) {
                        Ok(request) => rpc::dispatch(&ctx, &session, request).await,
                        Err(error) => Some(RpcResponse::failure(
                            Value::Null,
                            MobileError::new(MobileErrorKind::ParseError, error.to_string()),
                        )),
                    };
                    if let Some(response) = response {
                        if let Ok(value) = serde_json::to_value(&response) {
                            outbound.push(value, EventPriority::Critical);
                        }
                    }
                });
            }
            Message::Binary(_) => {
                // 首期只接受文本帧；二进制帧直接忽略。
            }
            Message::Close(_) => break,
            Message::Ping(_) | Message::Pong(_) => {}
        }
    }

    state.ctx.hub.unregister(connection_id);
    state.connections.fetch_sub(1, Ordering::SeqCst);
    writer.abort();
}

/// 把 TLS 握手接进 axum 的 `Listener`，避免引入额外的 HTTP 服务栈。
///
/// `Connected<IncomingStream<'_, _>>` 只能由 axum 自己实现（孤儿规则），
/// 因此调用方必须再套一层 `ListenerExt::tap_io`，才能继续使用 `ConnectInfo`。
struct TlsListener {
    listener: TcpListener,
    acceptor: TlsAcceptor,
}

impl axum::serve::Listener for TlsListener {
    type Io = tokio_rustls::server::TlsStream<TcpStream>;
    type Addr = SocketAddr;

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        loop {
            match self.listener.accept().await {
                Ok((stream, address)) => match self.acceptor.accept(stream).await {
                    Ok(tls) => return (tls, address),
                    // 握手失败（例如客户端拒绝自签证书）：丢弃这一条，继续接受。
                    Err(_) => continue,
                },
                // 单次 accept 失败不应终止整个监听循环。
                Err(_) => tokio::time::sleep(Duration::from_millis(50)).await,
            }
        }
    }

    fn local_addr(&self) -> io::Result<SocketAddr> {
        self.listener.local_addr()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loopback_and_private_sources_are_allowed() {
        assert!(is_allowed_source("127.0.0.1".parse().unwrap()));
        assert!(is_allowed_source("192.168.1.20".parse().unwrap()));
        assert!(is_allowed_source("10.0.0.5".parse().unwrap()));
        assert!(is_allowed_source("172.16.4.1".parse().unwrap()));
        assert!(is_allowed_source("169.254.1.1".parse().unwrap()));
        assert!(is_allowed_source("::1".parse().unwrap()));
        assert!(is_allowed_source("fd00::1".parse().unwrap()));
    }

    #[test]
    fn public_sources_are_rejected() {
        assert!(!is_allowed_source("8.8.8.8".parse().unwrap()));
        assert!(!is_allowed_source("172.32.0.1".parse().unwrap()));
        assert!(!is_allowed_source("2001:4860:4860::8888".parse().unwrap()));
    }

    #[test]
    fn ipv4_mapped_addresses_are_normalized() {
        let mapped: IpAddr = "::ffff:192.168.1.20".parse().unwrap();
        assert!(is_allowed_source(mapped));
        assert!(normalize_ip(mapped).is_ipv4());

        let public: IpAddr = "::ffff:8.8.8.8".parse().unwrap();
        assert!(!is_allowed_source(public));
    }

    #[test]
    fn host_header_must_be_local() {
        assert!(host_header_is_allowed("192.168.1.10:8787"));
        assert!(host_header_is_allowed("localhost:8787"));
        assert!(host_header_is_allowed("127.0.0.1"));
        assert!(host_header_is_allowed("[::1]:8787"));
        assert!(!host_header_is_allowed("evil.example.com"));
        assert!(!host_header_is_allowed("8.8.8.8:8787"));
        assert!(!host_header_is_allowed("attacker.local"));
    }

    #[test]
    fn forwarding_headers_are_detected() {
        let mut headers = HeaderMap::new();
        assert!(!has_forwarding_headers(&headers));
        headers.insert("x-forwarded-for", "1.2.3.4".parse().unwrap());
        assert!(has_forwarding_headers(&headers));

        let mut headers = HeaderMap::new();
        headers.insert("forwarded", "for=1.2.3.4".parse().unwrap());
        assert!(has_forwarding_headers(&headers));
    }

    #[test]
    fn bind_ip_defaults_to_loopback_and_rejects_public() {
        assert_eq!(
            resolve_bind_ip(None).unwrap(),
            IpAddr::V4(Ipv4Addr::LOCALHOST)
        );
        assert_eq!(
            resolve_bind_ip(Some("  ")).unwrap(),
            IpAddr::V4(Ipv4Addr::LOCALHOST)
        );
        assert_eq!(
            resolve_bind_ip(Some("192.168.1.10")).unwrap().to_string(),
            "192.168.1.10"
        );
        let error = resolve_bind_ip(Some("8.8.8.8")).unwrap_err();
        assert_eq!(error.kind(), "invalid_params");
        let error = resolve_bind_ip(Some("0.0.0.0")).unwrap_err();
        assert_eq!(error.kind(), "invalid_params");
        let error = resolve_bind_ip(Some("example.com")).unwrap_err();
        assert_eq!(error.kind(), "invalid_params");
    }

    /// 自建 WireGuard 中继（隧道网段 `10.8.0.0/24`）走的是「不做任何代码改动、
    /// 直接复用局域网网关」这条路，前提是 `10.0.0.0/8` 被判为私网。
    /// 这条测试把该前提钉住：若将来收窄 `is_allowed_source`，会在这里失败而不是
    /// 等到真机上连不上才发现。
    #[test]
    fn wireguard_tunnel_address_passes_every_gate() {
        let tunnel: IpAddr = "10.8.0.2".parse().unwrap();
        assert!(is_allowed_source(tunnel));
        assert!(host_header_is_allowed("10.8.0.2:8787"));
        assert_eq!(
            resolve_bind_ip(Some("10.8.0.2")).unwrap().to_string(),
            "10.8.0.2"
        );
    }

    /// CGNAT 段 `100.64.0.0/10`（Tailscale 与 ZeroTier 使用的网段）不在
    /// `Ipv4Addr::is_private()` 的范围内，因此当前会被拒绝。这是有意保留的现状
    /// 而非缺陷：改用 Tailscale 必须先在 `is_allowed_source` 里显式放行该网段，
    /// 并同步评估 `host_header_is_allowed` 与 `resolve_bind_ip` 的连带语义。
    #[test]
    fn cgnat_sources_are_rejected_today() {
        assert!(!is_allowed_source("100.64.0.1".parse().unwrap()));
        assert!(!is_allowed_source("100.101.102.103".parse().unwrap()));
        assert!(!host_header_is_allowed("100.64.0.1:8787"));
    }

    #[test]
    fn rate_limiter_enforces_window() {
        let limiter = RateLimiter::new();
        let ip: IpAddr = "192.168.1.5".parse().unwrap();
        for _ in 0..3 {
            limiter.check(ip, "pairing", 3, PAIRING_WINDOW_MS).unwrap();
        }
        let retry_after = limiter
            .check(ip, "pairing", 3, PAIRING_WINDOW_MS)
            .unwrap_err();
        assert!(retry_after > 0);
        // 其他 IP 不受影响。
        limiter
            .check(
                "192.168.1.6".parse().unwrap(),
                "pairing",
                3,
                PAIRING_WINDOW_MS,
            )
            .unwrap();
        // 不同类别独立计数。
        limiter
            .check(ip, "handshake", 3, PAIRING_WINDOW_MS)
            .unwrap();
    }

    #[test]
    fn expired_rate_window_resets() {
        let limiter = RateLimiter::new();
        let ip: IpAddr = "192.168.1.7".parse().unwrap();
        limiter.check(ip, "pairing", 1, PAIRING_WINDOW_MS).unwrap();
        assert!(limiter.check(ip, "pairing", 1, PAIRING_WINDOW_MS).is_err());
        {
            let mut windows = limiter.windows.lock().unwrap();
            for window in windows.values_mut() {
                window.reset_at_ms = 0;
            }
        }
        limiter.check(ip, "pairing", 1, PAIRING_WINDOW_MS).unwrap();
    }
}
