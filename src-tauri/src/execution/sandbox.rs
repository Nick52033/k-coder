//! ADR 0056：操作系统级命令沙箱的抽象、能力协商与审计。
//!
//! 本阶段只落地契约、能力协商、fail-closed 和审计事件，不引入任何平台机制。
//! 唯一内置后端 `NoSandboxBackend` 报告 `Unsupported`，因此：
//!
//! - 不需要隔离的命令（profile 全开）照常执行；
//! - 需要隔离但后端无法兑现时，默认降级执行并写入审计记录。当前没有平台后端，
//!   直接拒绝等于禁用命令能力；`P10-002b` 引入真实后端之后应把
//!   `SandboxGate::allow_degraded` 的默认值改为 `false`，让能力不足真正关闭失败。
//!
//! 平台后端只应在 `spawn()` 之前通过 `SandboxBackend::apply` 修改命令，
//! 不得改变命令参数本身，也不得向模型暴露原始系统错误。

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::process::Command;

use super::CommandRisk;

pub const SANDBOX_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileSystemPolicy {
    /// 只允许写入工作区（以及平台必需的临时目录）。
    WorkspaceOnly,
    /// 允许工作区和临时目录，其他位置只读。
    WorkspacePlusTemp,
    /// 不做文件系统限制。
    Full,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NetworkPolicy {
    /// 阻断全部网络访问。
    Deny,
    /// 只允许访问给定主机。
    AllowHosts(Vec<String>),
    /// 不做网络限制。
    Allow,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ResourceLimits {
    pub max_processes: Option<u32>,
    pub max_memory_bytes: Option<u64>,
    pub cpu_rate_percent: Option<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UiPolicy {
    /// 禁止弹窗、剪贴板、桌面切换和退出 Windows 等 UI 影响。
    Deny,
    Allow,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SandboxProfile {
    pub filesystem: FileSystemPolicy,
    pub network: NetworkPolicy,
    pub resources: ResourceLimits,
    pub ui: UiPolicy,
}

impl SandboxProfile {
    /// 不做任何限制，仅用于显式声明"这条命令不需要隔离"。
    pub fn unrestricted() -> Self {
        Self {
            filesystem: FileSystemPolicy::Full,
            network: NetworkPolicy::Allow,
            resources: ResourceLimits::default(),
            ui: UiPolicy::Allow,
        }
    }

    /// 由命令风险派生期望的隔离强度。
    ///
    /// `Network` 风险的命令本身需要联网，而当前平台后端还不能真正阻断网络，
    /// 因此这里不谎称 `Deny`，避免用无法兑现的策略把命令全部挡在门外。
    pub fn for_risk(risk: &CommandRisk) -> Self {
        let network = match risk {
            CommandRisk::Network => NetworkPolicy::Allow,
            _ => NetworkPolicy::Deny,
        };
        Self {
            filesystem: FileSystemPolicy::WorkspaceOnly,
            network,
            resources: ResourceLimits::default(),
            ui: UiPolicy::Deny,
        }
    }

    pub fn requires_isolation(&self) -> bool {
        self.filesystem != FileSystemPolicy::Full
            || !matches!(self.network, NetworkPolicy::Allow)
            || self.resources != ResourceLimits::default()
            || self.ui != UiPolicy::Allow
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SandboxCapability {
    Full,
    Partial {
        filesystem: bool,
        network: bool,
        resources: bool,
    },
    Unsupported,
}

impl SandboxCapability {
    /// 平台后端能否兑现这个 profile。能力不足必须由调用方决定拒绝还是降级。
    pub fn supports(&self, profile: &SandboxProfile) -> bool {
        if !profile.requires_isolation() {
            return true;
        }
        let (filesystem, network, resources) = match self {
            Self::Full => return true,
            Self::Partial {
                filesystem,
                network,
                resources,
            } => (*filesystem, *network, *resources),
            Self::Unsupported => return false,
        };
        let needs_filesystem = profile.filesystem != FileSystemPolicy::Full;
        let needs_network = !matches!(profile.network, NetworkPolicy::Allow);
        let needs_resources = profile.resources != ResourceLimits::default();
        (!needs_filesystem || filesystem)
            && (!needs_network || network)
            && (!needs_resources || resources)
    }
}

impl std::fmt::Display for SandboxCapability {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Full => formatter.write_str("full"),
            Self::Partial {
                filesystem,
                network,
                resources,
            } => write!(
                formatter,
                "partial(filesystem={filesystem},network={network},resources={resources})"
            ),
            Self::Unsupported => formatter.write_str("unsupported"),
        }
    }
}

/// 后端内部的失败详情。只进入日志和命令事实，不回传给模型。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxApplyError(pub String);

/// 能力不足导致的关闭失败。调用方必须把它转成固定的中文文案。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxUnavailable {
    pub backend: String,
    pub capability: SandboxCapability,
}

impl std::fmt::Display for SandboxUnavailable {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "当前平台无法提供命令隔离（后端 {}，能力 {}），已按设置阻止执行",
            self.backend, self.capability
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SandboxError {
    Unavailable(SandboxUnavailable),
    ApplyFailed(String),
}

impl std::fmt::Display for SandboxError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unavailable(error) => error.fmt(formatter),
            // 后端细节不回传给调用方，避免把系统错误泄漏给模型。
            Self::ApplyFailed(_) => formatter.write_str("命令隔离初始化失败，已阻止执行"),
        }
    }
}

impl std::error::Error for SandboxError {}

pub trait SandboxBackend: Send + Sync {
    fn name(&self) -> &'static str;
    fn capability(&self) -> SandboxCapability;
    /// 在 `spawn()` 之前应用平台机制。不得修改命令参数本身。
    fn apply(
        &self,
        command: &mut Command,
        profile: &SandboxProfile,
    ) -> Result<(), SandboxApplyError>;
}

/// 本阶段的占位后端：明确声明自己不提供任何隔离。
pub struct NoSandboxBackend;

impl SandboxBackend for NoSandboxBackend {
    fn name(&self) -> &'static str {
        "none"
    }

    fn capability(&self) -> SandboxCapability {
        SandboxCapability::Unsupported
    }

    fn apply(
        &self,
        _command: &mut Command,
        _profile: &SandboxProfile,
    ) -> Result<(), SandboxApplyError> {
        Err(SandboxApplyError(
            "no sandbox backend is registered".to_string(),
        ))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SandboxOutcome {
    /// 命令不需要隔离。
    Skipped,
    /// 已按 profile 应用隔离。
    Applied,
    /// 需要隔离但平台无法兑现，按设置继续（必须可审计）。
    Degraded,
}

/// 每次命令执行的隔离事实，进入命令会话视图和审计。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SandboxAudit {
    pub schema_version: u32,
    pub backend: String,
    pub capability: SandboxCapability,
    pub profile: SandboxProfile,
    pub outcome: SandboxOutcome,
    pub reason: Option<String>,
}

impl SandboxAudit {
    fn new(
        backend: &str,
        capability: SandboxCapability,
        profile: &SandboxProfile,
        outcome: SandboxOutcome,
        reason: Option<String>,
    ) -> Self {
        Self {
            schema_version: SANDBOX_SCHEMA_VERSION,
            backend: backend.to_string(),
            capability,
            profile: profile.clone(),
            outcome,
            reason,
        }
    }

    /// 命令会话创建时占位：真正的结论在 `spawn()` 之前写入。
    pub fn pending(backend: &str) -> Self {
        Self::new(
            backend,
            SandboxCapability::Unsupported,
            &SandboxProfile::unrestricted(),
            SandboxOutcome::Skipped,
            None,
        )
    }
}

impl Default for SandboxAudit {
    fn default() -> Self {
        Self::pending("none")
    }
}

/// 命令执行前的唯一沙箱入口。策略引擎产出 profile，这里决定放行、隔离、降级还是拒绝。
pub struct SandboxGate {
    backend: Arc<dyn SandboxBackend>,
    allow_degraded: bool,
}

impl SandboxGate {
    pub fn new(backend: Arc<dyn SandboxBackend>) -> Self {
        Self {
            backend,
            allow_degraded: true,
        }
    }

    /// 能力不足时是否继续。真实后端上线前保持 `true`，之后应默认 `false`。
    pub fn with_degraded_execution(mut self, allow: bool) -> Self {
        self.allow_degraded = allow;
        self
    }

    pub fn backend_name(&self) -> &'static str {
        self.backend.name()
    }

    pub fn capability(&self) -> SandboxCapability {
        self.backend.capability()
    }

    pub fn prepare(
        &self,
        command: &mut Command,
        profile: &SandboxProfile,
    ) -> Result<SandboxAudit, SandboxError> {
        let backend = self.backend.name();
        let capability = self.backend.capability();
        if !profile.requires_isolation() {
            return Ok(SandboxAudit::new(
                backend,
                capability,
                profile,
                SandboxOutcome::Skipped,
                None,
            ));
        }
        if capability.supports(profile) {
            self.backend
                .apply(command, profile)
                .map_err(|error| SandboxError::ApplyFailed(error.0))?;
            return Ok(SandboxAudit::new(
                backend,
                capability,
                profile,
                SandboxOutcome::Applied,
                None,
            ));
        }
        if !self.allow_degraded {
            return Err(SandboxError::Unavailable(SandboxUnavailable {
                backend: backend.to_string(),
                capability,
            }));
        }
        Ok(SandboxAudit::new(
            backend,
            capability,
            profile,
            SandboxOutcome::Degraded,
            Some("当前平台无法兑现所需的命令隔离，已按设置继续并记录本次降级".to_string()),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct StubBackend {
        name: &'static str,
        capability: SandboxCapability,
        applied: StdMutex<usize>,
    }

    impl SandboxBackend for StubBackend {
        fn name(&self) -> &'static str {
            self.name
        }

        fn capability(&self) -> SandboxCapability {
            self.capability
        }

        fn apply(
            &self,
            _command: &mut Command,
            _profile: &SandboxProfile,
        ) -> Result<(), SandboxApplyError> {
            *self.applied.lock().unwrap() += 1;
            Ok(())
        }
    }

    use std::sync::Mutex as StdMutex;

    fn command() -> Command {
        Command::new(if cfg!(windows) { "cmd" } else { "true" })
    }

    fn isolated_profile() -> SandboxProfile {
        SandboxProfile {
            filesystem: FileSystemPolicy::WorkspaceOnly,
            network: NetworkPolicy::Deny,
            resources: ResourceLimits::default(),
            ui: UiPolicy::Deny,
        }
    }

    #[test]
    fn unrestricted_profiles_are_skipped_without_a_backend() {
        let gate = SandboxGate::new(Arc::new(NoSandboxBackend));
        let audit = gate
            .prepare(&mut command(), &SandboxProfile::unrestricted())
            .unwrap();
        assert_eq!(audit.outcome, SandboxOutcome::Skipped);
        assert_eq!(audit.backend, "none");
    }

    #[test]
    fn unsupported_backend_fails_closed_when_degraded_execution_is_disabled() {
        let gate = SandboxGate::new(Arc::new(NoSandboxBackend)).with_degraded_execution(false);
        let error = gate
            .prepare(&mut command(), &isolated_profile())
            .unwrap_err();
        let message = error.to_string();
        match error {
            SandboxError::Unavailable(unavailable) => {
                assert_eq!(unavailable.backend, "none");
                assert_eq!(unavailable.capability, SandboxCapability::Unsupported);
            }
            other => panic!("unexpected error: {other}"),
        }
        assert!(message.contains("已按设置阻止执行"));
    }

    #[test]
    fn unsupported_backend_records_an_audited_degraded_execution() {
        let gate = SandboxGate::new(Arc::new(NoSandboxBackend));
        let audit = gate.prepare(&mut command(), &isolated_profile()).unwrap();
        assert_eq!(audit.outcome, SandboxOutcome::Degraded);
        assert!(audit.reason.is_some());
    }

    #[test]
    fn full_backend_applies_the_profile() {
        let backend = Arc::new(StubBackend {
            name: "stub",
            capability: SandboxCapability::Full,
            applied: StdMutex::new(0),
        });
        let gate = SandboxGate::new(backend.clone()).with_degraded_execution(false);
        let audit = gate.prepare(&mut command(), &isolated_profile()).unwrap();
        assert_eq!(audit.outcome, SandboxOutcome::Applied);
        assert_eq!(*backend.applied.lock().unwrap(), 1);
    }

    #[test]
    fn partial_backend_rejects_profiles_it_cannot_enforce() {
        let backend = StubBackend {
            name: "partial",
            capability: SandboxCapability::Partial {
                filesystem: true,
                network: false,
                resources: false,
            },
            applied: StdMutex::new(0),
        };
        let gate = SandboxGate::new(Arc::new(backend)).with_degraded_execution(false);
        // 只要求文件系统隔离时可以应用。
        let filesystem_only = SandboxProfile {
            filesystem: FileSystemPolicy::WorkspaceOnly,
            network: NetworkPolicy::Allow,
            resources: ResourceLimits::default(),
            ui: UiPolicy::Allow,
        };
        assert_eq!(
            gate.prepare(&mut command(), &filesystem_only)
                .unwrap()
                .outcome,
            SandboxOutcome::Applied
        );
        // 要求网络隔离时能力不足，必须关闭失败。
        assert!(matches!(
            gate.prepare(&mut command(), &isolated_profile())
                .unwrap_err(),
            SandboxError::Unavailable(_)
        ));
    }

    #[test]
    fn network_risk_commands_do_not_claim_network_isolation() {
        let profile = SandboxProfile::for_risk(&CommandRisk::Network);
        assert_eq!(profile.network, NetworkPolicy::Allow);
        let readonly = SandboxProfile::for_risk(&CommandRisk::ReadOnly);
        assert_eq!(readonly.network, NetworkPolicy::Deny);
        assert_eq!(readonly.filesystem, FileSystemPolicy::WorkspaceOnly);
        assert!(readonly.requires_isolation());
    }
}
