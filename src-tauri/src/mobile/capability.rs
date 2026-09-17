//! 移动端能力裁剪。
//!
//! 设计约定：手机端首期只开放聊天、取消、审批和结构化用户输入；
//! 文件、Shell、插件和设置类能力必须在桌面端显式开启后才可用。
//! 客户端在 `initialize` 里声明的能力只用于界面协商，不作为授权依据。

use std::collections::HashSet;
use std::sync::RwLock;

use serde::{Deserialize, Serialize};

use super::protocol::MobileError;

/// 移动端可申请的能力集合。
///
/// 新增变体时必须同步三处：`as_str`（否则编译不过）、前端 `MobileSettingsPage.tsx`
/// 的 `Capability` 联合类型，以及 `src-tauri/tests/mobile_dto_contract.rs` 里穷尽的
/// `expected_capability_name`（否则编译不过）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MobileCapability {
    /// 查看会话、读取历史、发送消息、引导活动 Turn。
    Chat,
    /// 响应审批请求。
    Approval,
    /// 取消活动 Turn。
    Interrupt,
    /// 读取工作区文件（首期未实现，默认关闭）。
    FileRead,
    /// 执行 Shell 命令（首期未实现，默认关闭）。
    Shell,
    /// 读取或修改应用设置（首期未实现，默认关闭）。
    Settings,
    /// 启停或删除插件（首期未实现，默认关闭）。
    Plugins,
    /// 读取密钥类信息（首期未实现，默认关闭）。
    Secrets,
}

impl MobileCapability {
    /// 首期默认授予的能力：只覆盖控制面。
    pub const DEFAULT_GRANTED: [Self; 3] = [Self::Chat, Self::Approval, Self::Interrupt];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Chat => "chat",
            Self::Approval => "approval",
            Self::Interrupt => "interrupt",
            Self::FileRead => "fileRead",
            Self::Shell => "shell",
            Self::Settings => "settings",
            Self::Plugins => "plugins",
            Self::Secrets => "secrets",
        }
    }

    /// 该能力是否已经在本期实现。未实现的能力即使被显式开启也只会返回
    /// `unsupported_capability`，避免出现「界面上打开了但实际不能用」的假承诺。
    pub fn is_implemented(self) -> bool {
        matches!(self, Self::Chat | Self::Approval | Self::Interrupt)
    }
}

/// 方法到所需能力的映射。
pub fn required_capability(method: &str) -> Option<MobileCapability> {
    match method {
        "project/list" | "thread/list" | "thread/read" | "thread/subscribe"
        | "thread/unsubscribe" | "turn/start" | "turn/steer" => Some(MobileCapability::Chat),
        "turn/interrupt" => Some(MobileCapability::Interrupt),
        "approval/respond" | "tool/requestUserInput/respond" => Some(MobileCapability::Approval),
        "file/list" | "file/read" => Some(MobileCapability::FileRead),
        "shell/run" => Some(MobileCapability::Shell),
        "settings/read" | "settings/write" => Some(MobileCapability::Settings),
        "plugin/list" | "plugin/toggle" => Some(MobileCapability::Plugins),
        "secret/list" => Some(MobileCapability::Secrets),
        _ => None,
    }
}

/// 已授予能力的集合。桌面端可以在运行期调整，调整立即对所有连接生效。
#[derive(Debug)]
pub struct CapabilityPolicy {
    granted: RwLock<HashSet<MobileCapability>>,
}

impl Default for CapabilityPolicy {
    fn default() -> Self {
        Self::new()
    }
}

impl CapabilityPolicy {
    pub fn new() -> Self {
        Self {
            granted: RwLock::new(MobileCapability::DEFAULT_GRANTED.into_iter().collect()),
        }
    }

    pub fn granted(&self) -> Vec<MobileCapability> {
        let mut granted: Vec<MobileCapability> = self
            .granted
            .read()
            .expect("capability lock poisoned")
            .iter()
            .copied()
            .collect();
        granted.sort();
        granted
    }

    pub fn grants(&self, capability: MobileCapability) -> bool {
        self.granted
            .read()
            .expect("capability lock poisoned")
            .contains(&capability)
    }

    /// 替换授予集合。首期只允许在已实现能力范围内调整，未实现能力不会被授予。
    pub fn set_granted(&self, capabilities: impl IntoIterator<Item = MobileCapability>) {
        let next: HashSet<MobileCapability> = capabilities
            .into_iter()
            .filter(|capability| capability.is_implemented())
            .collect();
        *self.granted.write().expect("capability lock poisoned") = next;
    }

    /// 判断某个方法是否被当前策略允许。
    ///
    /// 返回 `None` 表示该方法不参与能力裁剪（例如 `initialize`），由会话状态机单独把关。
    pub fn authorize(&self, method: &str) -> Option<Result<(), MobileError>> {
        let capability = required_capability(method)?;
        if !capability.is_implemented() {
            return Some(Err(MobileError::unsupported_capability(
                capability.as_str(),
            )));
        }
        if self.grants(capability) {
            Some(Ok(()))
        } else {
            Some(Err(MobileError::unsupported_capability(
                capability.as_str(),
            )))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_policy_grants_only_control_plane() {
        let policy = CapabilityPolicy::new();
        assert!(policy.grants(MobileCapability::Chat));
        assert!(policy.grants(MobileCapability::Approval));
        assert!(policy.grants(MobileCapability::Interrupt));
        assert!(!policy.grants(MobileCapability::FileRead));
        assert!(!policy.grants(MobileCapability::Shell));
    }

    #[test]
    fn unimplemented_capabilities_are_never_granted() {
        let policy = CapabilityPolicy::new();
        policy.set_granted([
            MobileCapability::Chat,
            MobileCapability::FileRead,
            MobileCapability::Shell,
        ]);
        assert!(policy.grants(MobileCapability::Chat));
        assert!(!policy.grants(MobileCapability::FileRead));
        assert!(!policy.grants(MobileCapability::Shell));
    }

    #[test]
    fn unimplemented_methods_report_unsupported_capability() {
        let policy = CapabilityPolicy::new();
        let error = policy
            .authorize("file/read")
            .expect("file/read is capability gated")
            .expect_err("file/read must be denied in this phase");
        assert_eq!(error.kind(), "unsupported_capability");
        assert_eq!(
            error
                .data
                .details
                .as_ref()
                .and_then(|details| details["capability"].as_str()),
            Some("fileRead")
        );
    }

    #[test]
    fn chat_methods_are_allowed_by_default() {
        let policy = CapabilityPolicy::new();
        for method in ["thread/list", "thread/read", "turn/start", "turn/steer"] {
            assert!(
                policy.authorize(method).expect("gated").is_ok(),
                "{method} should be allowed"
            );
        }
    }

    #[test]
    fn initialize_is_not_capability_gated() {
        let policy = CapabilityPolicy::new();
        assert!(policy.authorize("initialize").is_none());
        assert!(policy.authorize("events/resume").is_none());
    }

    #[test]
    fn revoking_chat_blocks_sending() {
        let policy = CapabilityPolicy::new();
        policy.set_granted([MobileCapability::Interrupt]);
        let error = policy
            .authorize("turn/start")
            .expect("gated")
            .expect_err("chat was revoked");
        assert_eq!(error.kind(), "unsupported_capability");
    }
}
