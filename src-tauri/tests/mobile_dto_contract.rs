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

use k_coder_lib::mobile::{
    MobileCapability, MobileDeviceView, MobilePairingView, MobilePendingPairingView, MobileStatus,
};
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
