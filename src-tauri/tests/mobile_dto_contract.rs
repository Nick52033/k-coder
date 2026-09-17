//! 手机网关 DTO 的线上契约。
//!
//! `e2e/fixtures/mobile-dto.json` 是「Rust 序列化出来的 JSON」与「前端读到的字段」
//! 之间唯一的数据源：前端 `e2e/mobile-settings.spec.ts` 用它当桩数据，本测试用它
//! 构造 DTO 再序列化回来做全等断言。
//!
//! 这样一来字段改名、改大小写、改可空性、或者多出/少掉一个字段，都会先在 Rust 侧
//! 失败，而不是等到界面上某个值静默变成 `undefined`。这两个文件必须一起改。

use std::fs;
use std::path::PathBuf;

use k_coder_lib::mobile::view::{MobileProject, MobileThreadSummary};
use k_coder_lib::mobile::{
    MobileCapability, MobileDeviceView, MobilePairingView, MobilePendingPairingView, MobileStatus,
};
use k_coder_lib::protocol::PROTOCOL_VERSION;
use k_coder_lib::storage::ThreadSummary;
use serde_json::{Value, json};

/// 与前端 Playwright 桩数据共用的唯一数据源。
fn fixture() -> Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("e2e")
        .join("fixtures")
        .join("mobile-dto.json");
    let raw = fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("无法读取 {}：{error}", path.display()));
    serde_json::from_str(&raw).expect("mobile-dto.json 必须是合法 JSON")
}

fn text(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("{key} 必须是字符串"))
        .to_string()
}

fn number(value: &Value, key: &str) -> u64 {
    value
        .get(key)
        .and_then(Value::as_u64)
        .unwrap_or_else(|| panic!("{key} 必须是数字"))
}

fn flag(value: &Value, key: &str) -> bool {
    value
        .get(key)
        .and_then(Value::as_bool)
        .unwrap_or_else(|| panic!("{key} 必须是布尔值"))
}

fn serialized(value: &impl serde::Serialize) -> Value {
    serde_json::to_value(value).expect("DTO 必须可序列化")
}

#[test]
fn device_view_matches_the_frontend_fixture() {
    let fixture = fixture();
    let device = &fixture["device"];
    let view = MobileDeviceView {
        id: text(device, "id"),
        name: text(device, "name"),
        platform: device["platform"].as_str().map(str::to_string),
        created_at_ms: number(device, "createdAtMs"),
        last_seen_at_ms: number(device, "lastSeenAtMs"),
        revoked: flag(device, "revoked"),
    };
    assert_eq!(serialized(&view), *device);
}

#[test]
fn pending_pairing_view_matches_the_frontend_fixture() {
    let fixture = fixture();
    let pending = &fixture["pending"];
    let view = MobilePendingPairingView {
        id: text(pending, "id"),
        device_name: text(pending, "deviceName"),
        platform: pending["platform"].as_str().map(str::to_string),
        created_at_ms: number(pending, "createdAtMs"),
        expires_at_ms: number(pending, "expiresAtMs"),
    };
    assert_eq!(serialized(&view), *pending);
}

#[test]
fn pairing_view_matches_the_frontend_fixture() {
    let fixture = fixture();
    let pairing = &fixture["pairing"];
    let view = MobilePairingView {
        challenge_id: text(pairing, "challengeId"),
        code: text(pairing, "code"),
        uri: text(pairing, "uri"),
        expires_at_ms: number(pairing, "expiresAtMs"),
        tls: flag(pairing, "tls"),
        fingerprint: pairing["fingerprint"].as_str().map(str::to_string),
    };
    assert_eq!(serialized(&view), *pairing);
}

/// 手机端唯一能看到的项目结构。它必须**不**带路径——项目归属键是路径的折叠形式，
/// 而绝对路径本身属于不该跨越设备边界的信息。
#[test]
fn project_matches_the_frontend_fixture() {
    let fixture = fixture();
    let project = &fixture["project"];
    let view = MobileProject {
        id: text(project, "id"),
        name: text(project, "name"),
        key: text(project, "key"),
        last_opened_at_ms: number(project, "lastOpenedAtMs"),
    };
    assert_eq!(serialized(&view), *project);

    let mut keys: Vec<&str> = project
        .as_object()
        .expect("project 必须是对象")
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    let mut expected = vec!["id", "key", "lastOpenedAtMs", "name"];
    expected.sort_unstable();
    assert_eq!(keys, expected, "项目视图不得新增字段，尤其不得暴露 path");
}

/// 归属键是「路径折叠」而不是路径本身：大小写与分隔符都已归一，
/// 因此手机端拿到的 `key` 无法反推出真实绝对路径。
#[test]
fn project_key_is_not_a_path() {
    let fixture = fixture();
    let key = text(&fixture["project"], "key");
    assert!(!key.contains('\\'), "归属键不得保留反斜杠：{key}");
    assert!(
        !key.contains(r"\\?\"),
        "归属键不得保留 Windows 扩展前缀：{key}"
    );
    assert_eq!(key, key.to_lowercase(), "归属键必须已折叠大小写：{key}");
    assert!(!key.ends_with('/'), "归属键不得以分隔符结尾：{key}");
}

/// 会话摘要携带归属键，`None` 表示独立会话。这里钉住可空性，
/// 避免某天有人把它改成必填后手机端把所有会话误归到同一个项目。
#[test]
fn thread_summary_carries_an_optional_project_key() {
    let fixture = fixture();
    let project_key = text(&fixture["project"], "key");
    let summary = ThreadSummary {
        schema_version: PROTOCOL_VERSION,
        id: "thread-1".to_string(),
        title: "会话".to_string(),
        created_at_ms: 1760000700000,
        updated_at_ms: 1760000800000,
        archived: false,
        in_project: true,
        workspace_path: None,
    };

    let attached =
        MobileThreadSummary::from_summary(&summary, Some(project_key.clone()), None, 0, 0);
    let value = serialized(&attached);
    assert_eq!(value["projectKey"], json!(project_key));
    assert!(value["projectKey"].is_string());

    let standalone = MobileThreadSummary::from_summary(&summary, None, None, 0, 0);
    let value = serialized(&standalone);
    assert_eq!(value["projectKey"], Value::Null);
    assert!(
        value.get("projectKey").is_some(),
        "字段必须始终存在，只是可为 null"
    );
}

#[test]
fn status_matches_the_frontend_fixture() {
    let fixture = fixture();
    let status = &fixture["status"];
    let capabilities: Vec<MobileCapability> = status["capabilities"]
        .as_array()
        .expect("capabilities 必须是数组")
        .iter()
        .map(|entry| serde_json::from_value(entry.clone()).expect("能力名必须能反序列化回枚举"))
        .collect();
    let view = MobileStatus {
        running: flag(status, "running"),
        host: status["host"].as_str().map(str::to_string),
        port: status["port"].as_u64().map(|value| value as u16),
        scheme: status["scheme"].as_str().map(str::to_string),
        fingerprint: status["fingerprint"].as_str().map(str::to_string),
        lan_addresses: status["lanAddresses"]
            .as_array()
            .expect("lanAddresses 必须是数组")
            .iter()
            .map(|entry| entry.as_str().expect("地址必须是字符串").to_string())
            .collect(),
        preferred_bind_address: status["preferredBindAddress"].as_str().map(str::to_string),
        preferred_port: status["preferredPort"].as_u64().expect("preferredPort") as u16,
        connections: status["connections"].as_u64().expect("connections") as usize,
        capabilities,
        pairing: None,
        pending_pairings: Vec::new(),
        devices: Vec::new(),
    };
    assert_eq!(serialized(&view), *status);
}

/// 每个能力变体都必须在这里出现一次。
///
/// 这个 `match` 是穷尽的，所以给 `MobileCapability` 新增变体时它会直接编译失败，
/// 逼着人回来同时更新前端 `Capability` 联合类型，而不是让新能力静默漂移。
fn expected_capability_name(capability: MobileCapability) -> &'static str {
    match capability {
        MobileCapability::Chat => "chat",
        MobileCapability::Approval => "approval",
        MobileCapability::Interrupt => "interrupt",
        MobileCapability::FileRead => "fileRead",
        MobileCapability::Shell => "shell",
        MobileCapability::Settings => "settings",
        MobileCapability::Plugins => "plugins",
        MobileCapability::Secrets => "secrets",
    }
}

/// 前端 `Capability` 联合类型逐个列出了这 8 个名字。改名必须两边一起改。
#[test]
fn capability_names_are_pinned_to_the_frontend_union() {
    let expected = [
        MobileCapability::Chat,
        MobileCapability::Approval,
        MobileCapability::Interrupt,
        MobileCapability::FileRead,
        MobileCapability::Shell,
        MobileCapability::Settings,
        MobileCapability::Plugins,
        MobileCapability::Secrets,
    ];
    for capability in expected {
        let name = expected_capability_name(capability);
        assert_eq!(serialized(&capability), json!(name));
        assert_eq!(capability.as_str(), name);
    }

    let fixture = fixture();
    for entry in fixture["status"]["capabilities"]
        .as_array()
        .expect("capabilities 必须是数组")
    {
        let name = entry.as_str().expect("能力名必须是字符串");
        assert!(
            expected
                .iter()
                .any(|capability| expected_capability_name(*capability) == name),
            "fixture 里的能力 {name} 不在前端联合类型中"
        );
    }
}

/// 设备视图是唯一直接展示给桌面的设备结构，绝不能带上任何凭据摘要。
#[test]
fn device_view_never_carries_credential_material() {
    let fixture = fixture();
    let device = &fixture["device"];
    for forbidden in ["secretHash", "refreshHash", "deviceSecret", "refreshToken"] {
        assert!(
            device.get(forbidden).is_none(),
            "设备视图不得暴露 {forbidden}"
        );
    }
    let mut keys: Vec<&str> = device
        .as_object()
        .expect("device 必须是对象")
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    let mut expected = vec![
        "createdAtMs",
        "id",
        "lastSeenAtMs",
        "name",
        "platform",
        "revoked",
    ];
    expected.sort_unstable();
    assert_eq!(keys, expected);
}
