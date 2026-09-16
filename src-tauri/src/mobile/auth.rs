//! 移动端身份：配对挑战、设备登记、访问令牌与撤销。
//!
//! 安全约定：
//! - 配对挑战 10 分钟有效、单次使用，失败次数有上限，一旦用掉或锁死就不再可用。
//! - 设备密钥与刷新令牌只以 SHA-256 摘要形式落盘，明文只在配对批准时下发一次。
//! - 访问令牌只存在于内存，进程重启即失效；撤销设备会立刻清掉该设备的全部访问令牌。
//! - 二维码 URI 只携带挑战 ID 与挑战密钥，不携带设备密钥、访问令牌或工作区路径。

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::RwLock;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::protocol::{MobileError, MobileErrorKind};

/// 配对挑战有效期：10 分钟。
pub const PAIRING_TTL_MS: u64 = 10 * 60 * 1000;
/// 等待桌面端确认的配对请求有效期：2 分钟。
pub const PAIRING_CONFIRM_TTL_MS: u64 = 2 * 60 * 1000;
/// 单个挑战允许的最大校验次数。
pub const PAIRING_MAX_ATTEMPTS: u32 = 5;
/// 访问令牌有效期：15 分钟。
pub const ACCESS_TOKEN_TTL_MS: u64 = 15 * 60 * 1000;

pub fn now_ms() -> u64 {
    chrono::Utc::now().timestamp_millis().max(0) as u64
}

fn hex_encode(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(DIGITS[(byte >> 4) as usize] as char);
        out.push(DIGITS[(byte & 0x0f) as usize] as char);
    }
    out
}

/// 使用 `uuid` 的 CSPRNG 源拼接出指定长度的随机字节。
///
/// 每次 `Uuid::new_v4()` 提供 122 位熵，拼接后足以用于设备密钥和令牌。
fn secure_random_bytes(length: usize) -> Vec<u8> {
    let mut buffer = Vec::with_capacity(length + 16);
    while buffer.len() < length {
        buffer.extend_from_slice(Uuid::new_v4().as_bytes());
    }
    buffer.truncate(length);
    buffer
}

pub fn random_token(byte_length: usize) -> String {
    hex_encode(&secure_random_bytes(byte_length))
}

/// 6 位人工校验码，用于确认扫码的人确实看得到电脑屏幕。
fn random_numeric_code() -> String {
    let bytes = secure_random_bytes(4);
    let value = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
    format!("{:06}", value % 1_000_000)
}

pub fn hash_secret(secret: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(secret.as_bytes());
    hex_encode(&hasher.finalize())
}

/// 定长比较，避免通过比较耗时泄露摘要前缀。
fn constant_time_eq(left: &str, right: &str) -> bool {
    let left = left.as_bytes();
    let right = right.as_bytes();
    if left.len() != right.len() {
        return false;
    }
    let mut diff = 0u8;
    for (a, b) in left.iter().zip(right.iter()) {
        diff |= a ^ b;
    }
    diff == 0
}

/// 桌面端持久化的移动网关偏好。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MobileSettings {
    /// 用户是否开启过局域网访问。默认关闭。
    pub enabled: bool,
    /// 用户显式选择的私有网卡地址。`None` 表示只监听回环地址。
    pub bind_address: Option<String>,
    pub port: u16,
}

impl Default for MobileSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            bind_address: None,
            port: 8787,
        }
    }
}

/// 已配对设备。密钥只保存摘要。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DeviceRecord {
    pub id: String,
    pub name: String,
    pub platform: Option<String>,
    pub created_at_ms: u64,
    pub last_seen_at_ms: u64,
    pub revoked: bool,
    pub secret_hash: String,
    pub refresh_hash: String,
}

impl DeviceRecord {
    pub fn is_active(&self) -> bool {
        !self.revoked
    }
}

/// 配对批准后一次性下发给手机端的凭据。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IssuedCredentials {
    pub device_id: String,
    pub device_secret: String,
    pub refresh_token: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PersistedState {
    schema_version: u32,
    settings: MobileSettings,
    devices: Vec<DeviceRecord>,
}

const STATE_SCHEMA_VERSION: u32 = 1;

/// 设备登记表：持久化在 `<data_root>/mobile/state.json`。
#[derive(Debug)]
pub struct DeviceRegistry {
    path: PathBuf,
    state: RwLock<PersistedState>,
}

impl DeviceRegistry {
    pub fn load(data_root: &Path) -> Result<Self, MobileError> {
        let directory = data_root.join("mobile");
        fs::create_dir_all(&directory).map_err(|error| {
            MobileError::internal(format!("failed to prepare mobile data directory: {error}"))
        })?;
        let path = directory.join("state.json");
        let state = match fs::read(&path) {
            Ok(bytes) => serde_json::from_slice::<PersistedState>(&bytes).map_err(|error| {
                MobileError::internal(format!("mobile state file is invalid: {error}"))
            })?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => PersistedState {
                schema_version: STATE_SCHEMA_VERSION,
                settings: MobileSettings::default(),
                devices: Vec::new(),
            },
            Err(error) => {
                return Err(MobileError::internal(format!(
                    "failed to read mobile state file: {error}"
                )));
            }
        };
        Ok(Self {
            path,
            state: RwLock::new(state),
        })
    }

    fn persist(&self, state: &PersistedState) -> Result<(), MobileError> {
        let encoded = serde_json::to_vec_pretty(state)
            .map_err(|error| MobileError::internal(format!("failed to encode state: {error}")))?;
        let temporary = self.path.with_extension("json.tmp");
        fs::write(&temporary, &encoded).map_err(|error| {
            MobileError::internal(format!("failed to write mobile state: {error}"))
        })?;
        fs::rename(&temporary, &self.path).map_err(|error| {
            MobileError::internal(format!("failed to replace mobile state: {error}"))
        })
    }

    pub fn settings(&self) -> MobileSettings {
        self.state
            .read()
            .expect("mobile state lock poisoned")
            .settings
            .clone()
    }

    pub fn update_settings(
        &self,
        update: impl FnOnce(&mut MobileSettings),
    ) -> Result<MobileSettings, MobileError> {
        let mut state = self.state.write().expect("mobile state lock poisoned");
        update(&mut state.settings);
        let settings = state.settings.clone();
        self.persist(&state)?;
        Ok(settings)
    }

    pub fn devices(&self) -> Vec<DeviceRecord> {
        self.state
            .read()
            .expect("mobile state lock poisoned")
            .devices
            .clone()
    }

    pub fn device(&self, device_id: &str) -> Option<DeviceRecord> {
        self.state
            .read()
            .expect("mobile state lock poisoned")
            .devices
            .iter()
            .find(|device| device.id == device_id)
            .cloned()
    }

    /// 登记一台新设备并返回一次性凭据。
    pub fn register_device(
        &self,
        name: &str,
        platform: Option<String>,
    ) -> Result<(DeviceRecord, IssuedCredentials), MobileError> {
        let device_secret = random_token(32);
        let refresh_token = random_token(32);
        let timestamp = now_ms();
        let record = DeviceRecord {
            id: Uuid::new_v4().to_string(),
            name: name.to_string(),
            platform,
            created_at_ms: timestamp,
            last_seen_at_ms: timestamp,
            revoked: false,
            secret_hash: hash_secret(&device_secret),
            refresh_hash: hash_secret(&refresh_token),
        };
        let credentials = IssuedCredentials {
            device_id: record.id.clone(),
            device_secret,
            refresh_token,
        };
        let mut state = self.state.write().expect("mobile state lock poisoned");
        state.devices.push(record.clone());
        self.persist(&state)?;
        Ok((record, credentials))
    }

    /// 用设备密钥换取身份。已撤销设备一律拒绝。
    pub fn authenticate(
        &self,
        device_id: &str,
        device_secret: &str,
    ) -> Result<DeviceRecord, MobileError> {
        let device = self
            .device(device_id)
            .ok_or_else(|| MobileError::unauthorized("unknown device"))?;
        if !device.is_active() {
            return Err(MobileError::unauthorized("device was revoked"));
        }
        if !constant_time_eq(&device.secret_hash, &hash_secret(device_secret)) {
            return Err(MobileError::unauthorized("device credentials are invalid"));
        }
        Ok(device)
    }

    /// 用刷新令牌换新凭据，并轮换设备密钥与刷新令牌。
    pub fn refresh(
        &self,
        device_id: &str,
        refresh_token: &str,
    ) -> Result<(DeviceRecord, IssuedCredentials), MobileError> {
        let device = self
            .device(device_id)
            .ok_or_else(|| MobileError::unauthorized("unknown device"))?;
        if !device.is_active() {
            return Err(MobileError::unauthorized("device was revoked"));
        }
        if !constant_time_eq(&device.refresh_hash, &hash_secret(refresh_token)) {
            return Err(MobileError::unauthorized("refresh token is invalid"));
        }

        let device_secret = random_token(32);
        let next_refresh = random_token(32);
        let mut state = self.state.write().expect("mobile state lock poisoned");
        let record = state
            .devices
            .iter_mut()
            .find(|candidate| candidate.id == device_id)
            .ok_or_else(|| MobileError::unauthorized("unknown device"))?;
        record.secret_hash = hash_secret(&device_secret);
        record.refresh_hash = hash_secret(&next_refresh);
        record.last_seen_at_ms = now_ms();
        let record = record.clone();
        self.persist(&state)?;
        Ok((
            record,
            IssuedCredentials {
                device_id: device_id.to_string(),
                device_secret,
                refresh_token: next_refresh,
            },
        ))
    }

    pub fn touch(&self, device_id: &str) {
        let mut state = self.state.write().expect("mobile state lock poisoned");
        let Some(record) = state
            .devices
            .iter_mut()
            .find(|candidate| candidate.id == device_id)
        else {
            return;
        };
        record.last_seen_at_ms = now_ms();
        let _ = self.persist(&state);
    }

    /// 撤销设备。返回是否确实发生了状态变化。
    pub fn revoke(&self, device_id: &str) -> Result<bool, MobileError> {
        let mut state = self.state.write().expect("mobile state lock poisoned");
        let Some(record) = state
            .devices
            .iter_mut()
            .find(|candidate| candidate.id == device_id)
        else {
            return Err(MobileError::not_found("unknown device"));
        };
        if record.revoked {
            return Ok(false);
        }
        record.revoked = true;
        self.persist(&state)?;
        Ok(true)
    }
}

/// 内存中的短期访问令牌。进程重启即全部失效。
#[derive(Debug, Default)]
pub struct TokenStore {
    grants: RwLock<HashMap<String, AccessGrant>>,
}

#[derive(Debug, Clone)]
struct AccessGrant {
    device_id: String,
    expires_at_ms: u64,
}

impl TokenStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn issue(&self, device_id: &str) -> (String, u64) {
        let token = random_token(32);
        let expires_at_ms = now_ms() + ACCESS_TOKEN_TTL_MS;
        let mut grants = self.grants.write().expect("token lock poisoned");
        grants.retain(|_, grant| grant.expires_at_ms > now_ms());
        grants.insert(
            token.clone(),
            AccessGrant {
                device_id: device_id.to_string(),
                expires_at_ms,
            },
        );
        (token, expires_at_ms)
    }

    /// 解析访问令牌，同时校验设备是否仍被授权。
    pub fn resolve(&self, token: &str, registry: &DeviceRegistry) -> Result<String, MobileError> {
        let grant = {
            let mut grants = self.grants.write().expect("token lock poisoned");
            let now = now_ms();
            grants.retain(|_, grant| grant.expires_at_ms > now);
            grants.get(token).cloned()
        };
        let grant =
            grant.ok_or_else(|| MobileError::unauthorized("access token is invalid or expired"))?;
        let device = registry
            .device(&grant.device_id)
            .ok_or_else(|| MobileError::unauthorized("device is no longer registered"))?;
        if !device.is_active() {
            return Err(MobileError::unauthorized("device was revoked"));
        }
        Ok(grant.device_id)
    }

    /// 撤销设备时立即失效其全部访问令牌。
    pub fn revoke_device(&self, device_id: &str) {
        self.grants
            .write()
            .expect("token lock poisoned")
            .retain(|_, grant| grant.device_id != device_id);
    }

    pub fn active_count(&self) -> usize {
        self.grants
            .read()
            .expect("token lock poisoned")
            .values()
            .filter(|grant| grant.expires_at_ms > now_ms())
            .count()
    }
}

/// 桌面端屏幕上展示的一次性配对挑战。
#[derive(Debug, Clone)]
pub struct PairingChallenge {
    pub id: String,
    pub secret: String,
    pub code: String,
    pub created_at_ms: u64,
    pub expires_at_ms: u64,
    pub consumed: bool,
    pub failed_attempts: u32,
    pub host: String,
    pub port: u16,
    pub tls: bool,
    pub fingerprint: Option<String>,
}

impl PairingChallenge {
    pub fn is_expired(&self, now: u64) -> bool {
        now >= self.expires_at_ms
    }

    /// 手机端可扫描的配对 URI。
    ///
    /// 挑战密钥放在 fragment 中，浏览器不会把它发给服务端，也不会进入服务端访问日志。
    pub fn pairing_uri(&self) -> String {
        let scheme = if self.tls { "https" } else { "http" };
        format!(
            "{scheme}://{host}:{port}/m/#c={id}.{secret}",
            host = self.host,
            port = self.port,
            id = self.id,
            secret = self.secret
        )
    }
}

/// 已通过校验、等待桌面端确认的配对请求。
#[derive(Debug, Clone)]
pub enum PendingPairingState {
    Awaiting,
    Approved(IssuedCredentials),
    Denied,
}

#[derive(Debug, Clone)]
pub struct PendingPairing {
    pub id: String,
    pub challenge_id: String,
    pub device_name: String,
    pub platform: Option<String>,
    pub created_at_ms: u64,
    pub expires_at_ms: u64,
    pub state: PendingPairingState,
}

/// 配对挑战与待确认请求的内存登记表。
#[derive(Debug, Default)]
pub struct PairingStore {
    challenges: RwLock<Vec<PairingChallenge>>,
    pending: RwLock<Vec<PendingPairing>>,
}

impl PairingStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// 创建新挑战，并使此前的挑战全部失效（同时只允许一个配对流程）。
    pub fn create_challenge(
        &self,
        host: &str,
        port: u16,
        tls: bool,
        fingerprint: Option<String>,
    ) -> PairingChallenge {
        let now = now_ms();
        let challenge = PairingChallenge {
            id: Uuid::new_v4().to_string(),
            secret: random_token(16),
            code: random_numeric_code(),
            created_at_ms: now,
            expires_at_ms: now + PAIRING_TTL_MS,
            consumed: false,
            failed_attempts: 0,
            host: host.to_string(),
            port,
            tls,
            fingerprint,
        };
        let mut challenges = self.challenges.write().expect("pairing lock poisoned");
        challenges.clear();
        challenges.push(challenge.clone());
        drop(challenges);
        self.pending.write().expect("pairing lock poisoned").clear();
        challenge
    }

    pub fn current_challenge(&self) -> Option<PairingChallenge> {
        let now = now_ms();
        self.challenges
            .read()
            .expect("pairing lock poisoned")
            .iter()
            .find(|challenge| !challenge.consumed && !challenge.is_expired(now))
            .cloned()
    }

    pub fn clear(&self) {
        self.challenges
            .write()
            .expect("pairing lock poisoned")
            .clear();
        self.pending.write().expect("pairing lock poisoned").clear();
    }

    /// 校验挑战密钥与人工校验码，成功后登记一个待确认的配对请求。
    ///
    /// 挑战是单次使用的：一旦通过校验就立即标记为已消费，重复提交返回 `stale_request`。
    pub fn submit(
        &self,
        challenge_id: &str,
        challenge_secret: &str,
        code: &str,
        device_name: &str,
        platform: Option<String>,
    ) -> Result<PendingPairing, MobileError> {
        let now = now_ms();
        let mut challenges = self.challenges.write().expect("pairing lock poisoned");
        let Some(challenge) = challenges
            .iter_mut()
            .find(|candidate| candidate.id == challenge_id)
        else {
            return Err(MobileError::stale_request(
                "pairing challenge is unknown or already replaced",
            ));
        };
        if challenge.consumed {
            return Err(MobileError::stale_request(
                "pairing challenge was already used",
            ));
        }
        if challenge.is_expired(now) {
            challenge.consumed = true;
            return Err(MobileError::stale_request("pairing challenge expired"));
        }
        if challenge.failed_attempts >= PAIRING_MAX_ATTEMPTS {
            challenge.consumed = true;
            return Err(MobileError::new(
                MobileErrorKind::RateLimited,
                "too many pairing attempts",
            ));
        }
        if !constant_time_eq(&challenge.secret, challenge_secret) {
            challenge.failed_attempts += 1;
            return Err(MobileError::unauthorized("pairing challenge is invalid"));
        }
        if !constant_time_eq(&challenge.code, code.trim()) {
            challenge.failed_attempts += 1;
            return Err(MobileError::unauthorized("pairing code does not match"));
        }

        let name = device_name.trim();
        if name.is_empty() || name.chars().count() > 64 {
            return Err(MobileError::invalid_params(
                "deviceName must be 1..=64 characters",
            ));
        }

        challenge.consumed = true;
        let pending = PendingPairing {
            id: Uuid::new_v4().to_string(),
            challenge_id: challenge.id.clone(),
            device_name: name.to_string(),
            platform,
            created_at_ms: now,
            expires_at_ms: now + PAIRING_CONFIRM_TTL_MS,
            state: PendingPairingState::Awaiting,
        };
        self.pending
            .write()
            .expect("pairing lock poisoned")
            .push(pending.clone());
        Ok(pending)
    }

    pub fn pending(&self) -> Vec<PendingPairing> {
        let now = now_ms();
        self.pending
            .read()
            .expect("pairing lock poisoned")
            .iter()
            .filter(|pending| {
                pending.expires_at_ms > now
                    && matches!(pending.state, PendingPairingState::Awaiting)
            })
            .cloned()
            .collect()
    }

    /// 桌面端批准配对。
    pub fn approve(
        &self,
        pending_id: &str,
        registry: &DeviceRegistry,
    ) -> Result<DeviceRecord, MobileError> {
        let now = now_ms();
        let mut pending_list = self.pending.write().expect("pairing lock poisoned");
        let Some(pending) = pending_list
            .iter_mut()
            .find(|candidate| candidate.id == pending_id)
        else {
            return Err(MobileError::not_found("pairing request is unknown"));
        };
        if !matches!(pending.state, PendingPairingState::Awaiting) {
            return Err(MobileError::stale_request(
                "pairing request was already resolved",
            ));
        }
        if pending.expires_at_ms <= now {
            pending.state = PendingPairingState::Denied;
            return Err(MobileError::stale_request("pairing request expired"));
        }
        let (record, credentials) =
            registry.register_device(&pending.device_name, pending.platform.clone())?;
        pending.state = PendingPairingState::Approved(credentials);
        Ok(record)
    }

    pub fn deny(&self, pending_id: &str) -> Result<(), MobileError> {
        let mut pending_list = self.pending.write().expect("pairing lock poisoned");
        let Some(pending) = pending_list
            .iter_mut()
            .find(|candidate| candidate.id == pending_id)
        else {
            return Err(MobileError::not_found("pairing request is unknown"));
        };
        pending.state = PendingPairingState::Denied;
        Ok(())
    }

    /// 手机端轮询配对结果。凭据只下发一次。
    pub fn poll(
        &self,
        pending_id: &str,
        challenge_secret: &str,
    ) -> Result<PairingPollOutcome, MobileError> {
        let now = now_ms();
        let mut pending_list = self.pending.write().expect("pairing lock poisoned");
        let Some(pending) = pending_list
            .iter_mut()
            .find(|candidate| candidate.id == pending_id)
        else {
            return Err(MobileError::not_found("pairing request is unknown"));
        };

        let challenge_secret_valid = {
            let challenges = self.challenges.read().expect("pairing lock poisoned");
            challenges
                .iter()
                .find(|challenge| challenge.id == pending.challenge_id)
                .map(|challenge| constant_time_eq(&challenge.secret, challenge_secret))
                .unwrap_or(false)
        };
        if !challenge_secret_valid {
            return Err(MobileError::unauthorized("pairing challenge is invalid"));
        }

        match pending.state.clone() {
            PendingPairingState::Awaiting if pending.expires_at_ms <= now => {
                pending.state = PendingPairingState::Denied;
                Ok(PairingPollOutcome::Denied)
            }
            PendingPairingState::Awaiting => Ok(PairingPollOutcome::Awaiting),
            PendingPairingState::Denied => Ok(PairingPollOutcome::Denied),
            PendingPairingState::Approved(credentials) => {
                pending.state = PendingPairingState::Denied;
                Ok(PairingPollOutcome::Approved(credentials))
            }
        }
    }
}

/// 手机端轮询配对状态的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PairingPollOutcome {
    Awaiting,
    Approved(IssuedCredentials),
    Denied,
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn registry() -> (TempDir, DeviceRegistry) {
        let directory = TempDir::new().unwrap();
        let registry = DeviceRegistry::load(directory.path()).unwrap();
        (directory, registry)
    }

    fn challenge(store: &PairingStore) -> PairingChallenge {
        store.create_challenge("192.168.1.10", 8787, true, Some("AB:CD".to_string()))
    }

    #[test]
    fn pairing_uri_keeps_secret_in_fragment() {
        let store = PairingStore::new();
        let challenge = challenge(&store);
        let uri = challenge.pairing_uri();
        assert!(uri.starts_with("https://192.168.1.10:8787/m/#c="));
        assert!(uri.contains(&challenge.secret));
        assert!(
            !uri.contains("192.168.1.10:8787/m/?"),
            "secret must not be a query parameter"
        );
    }

    #[test]
    fn challenge_is_single_use() {
        let store = PairingStore::new();
        let challenge = challenge(&store);
        let pending = store
            .submit(
                &challenge.id,
                &challenge.secret,
                &challenge.code,
                "Pixel",
                None,
            )
            .expect("first submit succeeds");
        let replay = store
            .submit(
                &challenge.id,
                &challenge.secret,
                &challenge.code,
                "Pixel",
                None,
            )
            .expect_err("second submit must fail");
        assert_eq!(replay.kind(), "stale_request");
        assert_eq!(store.pending().len(), 1);
        assert_eq!(store.pending()[0].id, pending.id);
    }

    #[test]
    fn wrong_code_counts_attempts_and_locks_out() {
        let store = PairingStore::new();
        let challenge = challenge(&store);
        for _ in 0..PAIRING_MAX_ATTEMPTS {
            let error = store
                .submit(&challenge.id, &challenge.secret, "000000", "Pixel", None)
                .expect_err("wrong code must fail");
            assert_eq!(error.kind(), "unauthorized");
        }
        let locked = store
            .submit(
                &challenge.id,
                &challenge.secret,
                &challenge.code,
                "Pixel",
                None,
            )
            .expect_err("challenge must be locked");
        assert_eq!(locked.kind(), "rate_limited");
    }

    #[test]
    fn wrong_secret_does_not_leak_challenge() {
        let store = PairingStore::new();
        let challenge = challenge(&store);
        let error = store
            .submit(&challenge.id, "deadbeef", &challenge.code, "Pixel", None)
            .expect_err("wrong secret must fail");
        assert_eq!(error.kind(), "unauthorized");
    }

    #[test]
    fn expired_challenge_is_rejected() {
        let store = PairingStore::new();
        let challenge = challenge(&store);
        {
            let mut challenges = store.challenges.write().unwrap();
            challenges[0].expires_at_ms = now_ms() - 1;
        }
        let error = store
            .submit(
                &challenge.id,
                &challenge.secret,
                &challenge.code,
                "Pixel",
                None,
            )
            .expect_err("expired challenge must fail");
        assert_eq!(error.kind(), "stale_request");
    }

    #[test]
    fn new_challenge_invalidates_previous_one() {
        let store = PairingStore::new();
        let first = challenge(&store);
        let second = challenge(&store);
        let error = store
            .submit(&first.id, &first.secret, &first.code, "Pixel", None)
            .expect_err("previous challenge must be replaced");
        assert_eq!(error.kind(), "stale_request");
        assert!(
            store
                .submit(&second.id, &second.secret, &second.code, "Pixel", None)
                .is_ok()
        );
    }

    #[test]
    fn approved_pairing_delivers_credentials_once() {
        let (_directory, registry) = registry();
        let store = PairingStore::new();
        let challenge = challenge(&store);
        let pending = store
            .submit(
                &challenge.id,
                &challenge.secret,
                &challenge.code,
                "Pixel 8",
                Some("android".to_string()),
            )
            .unwrap();
        assert_eq!(
            store.poll(&pending.id, &challenge.secret).unwrap(),
            PairingPollOutcome::Awaiting
        );
        let device = store.approve(&pending.id, &registry).unwrap();
        assert_eq!(device.name, "Pixel 8");

        let outcome = store.poll(&pending.id, &challenge.secret).unwrap();
        let PairingPollOutcome::Approved(credentials) = outcome else {
            panic!("expected credentials");
        };
        assert_eq!(credentials.device_id, device.id);
        assert_eq!(
            store.poll(&pending.id, &challenge.secret).unwrap(),
            PairingPollOutcome::Denied,
            "credentials must not be delivered twice"
        );
    }

    #[test]
    fn poll_requires_matching_challenge_secret() {
        let store = PairingStore::new();
        let challenge = challenge(&store);
        let pending = store
            .submit(
                &challenge.id,
                &challenge.secret,
                &challenge.code,
                "Pixel",
                None,
            )
            .unwrap();
        let error = store.poll(&pending.id, "not-the-secret").unwrap_err();
        assert_eq!(error.kind(), "unauthorized");
    }

    #[test]
    fn denied_pairing_never_registers_device() {
        let (_directory, registry) = registry();
        let store = PairingStore::new();
        let challenge = challenge(&store);
        let pending = store
            .submit(
                &challenge.id,
                &challenge.secret,
                &challenge.code,
                "Pixel",
                None,
            )
            .unwrap();
        store.deny(&pending.id).unwrap();
        assert!(registry.devices().is_empty());
        assert_eq!(
            store.poll(&pending.id, &challenge.secret).unwrap(),
            PairingPollOutcome::Denied
        );
    }

    #[test]
    fn device_authentication_rejects_wrong_secret() {
        let (_directory, registry) = registry();
        let (record, credentials) = registry.register_device("Pixel", None).unwrap();
        assert!(
            registry
                .authenticate(&record.id, &credentials.device_secret)
                .is_ok()
        );
        let error = registry
            .authenticate(&record.id, "nope")
            .expect_err("wrong secret must fail");
        assert_eq!(error.kind(), "unauthorized");
    }

    #[test]
    fn revoked_device_cannot_authenticate_and_tokens_are_dropped() {
        let (_directory, registry) = registry();
        let (record, credentials) = registry.register_device("Pixel", None).unwrap();
        let tokens = TokenStore::new();
        let (token, _) = tokens.issue(&record.id);
        assert_eq!(tokens.resolve(&token, &registry).unwrap(), record.id);

        assert!(registry.revoke(&record.id).unwrap());
        assert!(
            !registry.revoke(&record.id).unwrap(),
            "revoke is idempotent"
        );
        tokens.revoke_device(&record.id);

        let error = registry
            .authenticate(&record.id, &credentials.device_secret)
            .expect_err("revoked device must fail");
        assert_eq!(error.kind(), "unauthorized");
        let error = tokens
            .resolve(&token, &registry)
            .expect_err("revoked token must fail");
        assert_eq!(error.kind(), "unauthorized");
    }

    #[test]
    fn refresh_rotates_secret_and_refresh_token() {
        let (_directory, registry) = registry();
        let (record, credentials) = registry.register_device("Pixel", None).unwrap();
        let (_, rotated) = registry
            .refresh(&record.id, &credentials.refresh_token)
            .unwrap();
        assert_ne!(rotated.device_secret, credentials.device_secret);
        assert_ne!(rotated.refresh_token, credentials.refresh_token);
        assert!(
            registry
                .authenticate(&record.id, &rotated.device_secret)
                .is_ok()
        );
        assert!(
            registry
                .authenticate(&record.id, &credentials.device_secret)
                .is_err()
        );
        let error = registry
            .refresh(&record.id, &credentials.refresh_token)
            .expect_err("old refresh token must stop working");
        assert_eq!(error.kind(), "unauthorized");
    }

    #[test]
    fn access_tokens_expire() {
        let (_directory, registry) = registry();
        let (record, _) = registry.register_device("Pixel", None).unwrap();
        let tokens = TokenStore::new();
        let (token, _) = tokens.issue(&record.id);
        {
            let mut grants = tokens.grants.write().unwrap();
            grants.get_mut(&token).unwrap().expires_at_ms = now_ms() - 1;
        }
        let error = tokens.resolve(&token, &registry).unwrap_err();
        assert_eq!(error.kind(), "unauthorized");
    }

    #[test]
    fn secrets_are_persisted_as_digests_only() {
        let directory = TempDir::new().unwrap();
        let registry = DeviceRegistry::load(directory.path()).unwrap();
        let (_, credentials) = registry.register_device("Pixel", None).unwrap();
        let raw = fs::read_to_string(directory.path().join("mobile/state.json")).unwrap();
        assert!(!raw.contains(&credentials.device_secret));
        assert!(!raw.contains(&credentials.refresh_token));
        assert!(raw.contains(&hash_secret(&credentials.device_secret)));
    }

    #[test]
    fn settings_round_trip_through_disk() {
        let directory = TempDir::new().unwrap();
        let registry = DeviceRegistry::load(directory.path()).unwrap();
        assert!(!registry.settings().enabled);
        registry
            .update_settings(|settings| {
                settings.enabled = true;
                settings.bind_address = Some("192.168.1.10".to_string());
                settings.port = 9000;
            })
            .unwrap();
        let reloaded = DeviceRegistry::load(directory.path()).unwrap();
        let settings = reloaded.settings();
        assert!(settings.enabled);
        assert_eq!(settings.bind_address.as_deref(), Some("192.168.1.10"));
        assert_eq!(settings.port, 9000);
    }
}
