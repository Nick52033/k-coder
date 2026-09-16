//! Versioned IPC payloads for the memory domain (design §7.1).
//!
//! Requests stay as plain strings here and are parsed by the `memory` module, so an unknown
//! enumeration or a malformed scope surfaces as a coded domain error instead of a deserialization
//! failure the command boundary cannot describe.
//!
//! Responses are the domain types themselves, re-exported so the frontend contract has a single
//! definition.

use serde::{Deserialize, Serialize};

use crate::memory::{MemoryError, MemoryScope, MemoryType, UpsertMemoryCommand};

pub use crate::memory::{
    DreamReport, DreamStatus, MaintenanceOutcome, MaintenanceReport, MaintenanceSettings,
    MaintenanceTrigger, MemoryClearOutcome, MemoryPage, MemorySettings, MemoryUpsertOutcome,
    OfflineMaintenanceReport,
};

/// `upsert_memory` payload: `memoryId?`, `content`, `type`, `scope`, `expiresAtMs?`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpsertMemoryRequest {
    #[serde(default)]
    pub memory_id: Option<String>,
    pub content: String,
    pub memory_type: String,
    pub scope: String,
    #[serde(default)]
    pub expires_at_ms: Option<u64>,
}

impl UpsertMemoryRequest {
    pub fn into_command(self) -> Result<UpsertMemoryCommand, MemoryError> {
        Ok(UpsertMemoryCommand {
            memory_id: self.memory_id,
            content: self.content,
            memory_type: MemoryType::parse(&self.memory_type)?,
            scope: MemoryScope::parse(&self.scope)?,
            expires_at_ms: self.expires_at_ms,
        })
    }
}

/// `set_memory_settings` payload. All three fields are required so the UI cannot partially update
/// the settings row and leave it in an ambiguous state.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetMemorySettingsRequest {
    pub enabled: bool,
    pub auto_accept_high_confidence: bool,
    pub default_ttl_days: u32,
}

/// `set_memory_maintenance_settings` payload.
///
/// The acknowledgement travels with the same call that turns Dream on, so the UI cannot enable a
/// remote-disclosing feature in one request and claim consent in another. The domain service still
/// re-checks the pair, because a request payload is never an authorization source.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetMemoryMaintenanceSettingsRequest {
    pub enabled: bool,
    pub dream_enabled: bool,
    #[serde(default)]
    pub remote_disclosure_accepted: bool,
    pub token_budget: u64,
    pub idle_after_ms: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upsert_requests_parse_into_validated_domain_commands() {
        let request = UpsertMemoryRequest {
            memory_id: None,
            content: "Use pnpm".into(),
            memory_type: "preference".into(),
            scope: "project:proj-1".into(),
            expires_at_ms: Some(1_700_000_000_000),
        };
        let command = request.into_command().unwrap();
        assert_eq!(command.memory_type, MemoryType::Preference);
        assert_eq!(command.scope.canonical(), "project:proj-1");
        assert_eq!(command.expires_at_ms, Some(1_700_000_000_000));
    }

    #[test]
    fn upsert_requests_reject_unknown_types_and_malformed_scopes() {
        let base = UpsertMemoryRequest {
            memory_id: None,
            content: "Use pnpm".into(),
            memory_type: "preference".into(),
            scope: "user".into(),
            expires_at_ms: None,
        };

        let mut bad_type = base.clone();
        bad_type.memory_type = "notes".into();
        assert_eq!(
            bad_type.into_command().unwrap_err().code(),
            "MEM_INVALID_TYPE"
        );

        let mut bad_scope = base;
        bad_scope.scope = "project".into();
        assert_eq!(
            bad_scope.into_command().unwrap_err().code(),
            "MEM_INVALID_SCOPE"
        );
    }

    #[test]
    fn the_wire_shape_matches_the_documented_camel_case_contract() {
        let request = UpsertMemoryRequest {
            memory_id: Some("memory-1".into()),
            content: "Use pnpm".into(),
            memory_type: "fact".into(),
            scope: "user".into(),
            expires_at_ms: None,
        };
        let value = serde_json::to_value(&request).unwrap();
        assert_eq!(value["memoryId"], "memory-1");
        assert_eq!(value["memoryType"], "fact");
        assert_eq!(value["scope"], "user");

        // An omitted `memoryId` must deserialize rather than fail.
        let decoded: UpsertMemoryRequest = serde_json::from_value(serde_json::json!({
            "content": "Use pnpm",
            "memoryType": "fact",
            "scope": "user"
        }))
        .unwrap();
        assert_eq!(decoded.memory_id, None);
        assert_eq!(decoded.expires_at_ms, None);
    }

    #[test]
    fn maintenance_settings_requests_carry_the_disclosure_in_the_same_payload() {
        let decoded: SetMemoryMaintenanceSettingsRequest =
            serde_json::from_value(serde_json::json!({
                "enabled": true,
                "dreamEnabled": true,
                "remoteDisclosureAccepted": true,
                "tokenBudget": 20000,
                "idleAfterMs": 600000
            }))
            .unwrap();
        assert!(decoded.enabled);
        assert!(decoded.dream_enabled);
        assert!(decoded.remote_disclosure_accepted);
        assert_eq!(decoded.token_budget, 20_000);
        assert_eq!(decoded.idle_after_ms, 600_000);

        // Omitting the acknowledgement must default to "not accepted" rather than fail the call:
        // the domain layer rejects the pair, which is the error the UI needs to show.
        let without_ack: SetMemoryMaintenanceSettingsRequest =
            serde_json::from_value(serde_json::json!({
                "enabled": true,
                "dreamEnabled": true,
                "tokenBudget": 20000,
                "idleAfterMs": 600000
            }))
            .unwrap();
        assert!(!without_ack.remote_disclosure_accepted);
    }

    #[test]
    fn maintenance_reports_serialize_with_camel_case_keys() {
        let report = MaintenanceReport {
            trigger: MaintenanceTrigger::Manual,
            outcome: MaintenanceOutcome::Completed,
            offline: OfflineMaintenanceReport {
                expired_ids: vec!["memory-1".into()],
                merged_groups: Vec::new(),
            },
            dream: DreamReport {
                status: DreamStatus::Skipped,
                proposals: 0,
                accepted: 0,
                pending: 0,
                error: None,
            },
            started_at_ms: 1_700_000_000_000,
            completed_at_ms: 1_700_000_000_500,
        };
        let value = serde_json::to_value(&report).unwrap();
        assert_eq!(value["startedAtMs"], 1_700_000_000_000u64);
        assert_eq!(value["offline"]["expiredIds"][0], "memory-1");
        assert_eq!(value["dream"]["status"], "skipped");
        assert_eq!(value["outcome"], "completed");
    }
}
