//! Memory entities: scope, type, sensitivity, provenance, status, operation and page cursor.
//!
//! Every enum here is host-owned. The IPC layer accepts plain strings and this module parses them,
//! so an unknown value produces a coded domain error instead of a deserialization failure the
//! command boundary cannot describe.

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::{Deserialize, Serialize};

use crate::memory::MemoryError;
use crate::storage::memory_repository::MAX_MEMORY_SCOPE_ID_CHARS;

/// Scope kinds. `user` is global; the other three are keyed by a host-generated id.
pub const MEMORY_SCOPE_KINDS: [&str; 4] = ["user", "workspace", "project", "thread"];
pub const MEMORY_TYPES: [&str; 6] = [
    "preference",
    "fact",
    "instruction",
    "constraint",
    "work_state",
    "experience",
];
pub const MEMORY_SENSITIVITIES: [&str; 3] = ["normal", "private", "secret_candidate"];
pub const MEMORY_SOURCE_TYPES: [&str; 4] = ["user", "model", "tool", "system"];
pub const MEMORY_STATUSES: [&str; 4] = ["active", "deleted", "expired", "archived"];
pub const MEMORY_OPERATIONS: [&str; 4] = ["create", "update", "merge", "delete"];

/// Separator between the scope kind and its id in the canonical scope string.
const SCOPE_SEPARATOR: char = ':';

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryScopeKind {
    User,
    Workspace,
    Project,
    Thread,
}

impl MemoryScopeKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Workspace => "workspace",
            Self::Project => "project",
            Self::Thread => "thread",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "user" => Some(Self::User),
            "workspace" => Some(Self::Workspace),
            "project" => Some(Self::Project),
            "thread" => Some(Self::Thread),
            _ => None,
        }
    }

    /// `user` memories are global. Every other scope is meaningless without its id, because two
    /// projects must never share a memory row.
    pub fn requires_scope_id(self) -> bool {
        !matches!(self, Self::User)
    }
}

/// A validated memory scope. The canonical string form is also the clear confirmation token, so the
/// host never has to invent a second vocabulary for the same thing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryScope {
    pub kind: MemoryScopeKind,
    pub id: Option<String>,
}

impl MemoryScope {
    pub fn user() -> Self {
        Self {
            kind: MemoryScopeKind::User,
            id: None,
        }
    }

    pub fn new(kind: MemoryScopeKind, id: Option<String>) -> Self {
        Self { kind, id }
    }

    /// Parses `user`, `workspace:<id>`, `project:<id>` or `thread:<id>`.
    pub fn parse(value: &str) -> Result<Self, MemoryError> {
        let value = value.trim();
        if value.is_empty() {
            return Err(MemoryError::coded(
                "MEM_INVALID_SCOPE",
                "scope must not be empty",
            ));
        }
        let (kind, id) = match value.split_once(SCOPE_SEPARATOR) {
            Some((kind, id)) => (kind, Some(id)),
            None => (value, None),
        };
        let kind = MemoryScopeKind::parse(kind).ok_or_else(|| {
            MemoryError::coded(
                "MEM_INVALID_SCOPE",
                format!(
                    "scope kind must be one of {}",
                    MEMORY_SCOPE_KINDS.join(", ")
                ),
            )
        })?;
        let scope = match (kind.requires_scope_id(), id) {
            (true, None) => {
                return Err(MemoryError::coded(
                    "MEM_INVALID_SCOPE",
                    format!("{} scope requires a scope id", kind.as_str()),
                ));
            }
            (false, Some(_)) => {
                return Err(MemoryError::coded(
                    "MEM_INVALID_SCOPE",
                    "user scope must not carry a scope id",
                ));
            }
            (false, None) => Self::new(kind, None),
            (true, Some(id)) => Self::new(kind, Some(id.to_owned())),
        };
        scope.validate()?;
        Ok(scope)
    }

    /// Rejects empty, oversized or separator-bearing ids. Ids are host-generated UUIDs or workspace
    /// keys, so anything else is a caller mistake rather than a value to normalize.
    pub fn validate(&self) -> Result<(), MemoryError> {
        let Some(id) = self.id.as_deref() else {
            if self.kind.requires_scope_id() {
                return Err(MemoryError::coded(
                    "MEM_INVALID_SCOPE",
                    format!("{} scope requires a scope id", self.kind.as_str()),
                ));
            }
            return Ok(());
        };
        if !self.kind.requires_scope_id() {
            return Err(MemoryError::coded(
                "MEM_INVALID_SCOPE",
                "user scope must not carry a scope id",
            ));
        }
        let length = id.chars().count();
        if length == 0 || length > MAX_MEMORY_SCOPE_ID_CHARS {
            return Err(MemoryError::coded(
                "MEM_INVALID_SCOPE",
                format!("scope id must contain 1 to {MAX_MEMORY_SCOPE_ID_CHARS} characters"),
            ));
        }
        if id.contains(SCOPE_SEPARATOR)
            || id
                .chars()
                .any(|character| character.is_control() || character.is_whitespace())
        {
            return Err(MemoryError::coded(
                "MEM_INVALID_SCOPE",
                "scope id must not contain separators, whitespace or control characters",
            ));
        }
        Ok(())
    }

    pub fn canonical(&self) -> String {
        match self.id.as_deref() {
            Some(id) => format!("{}{SCOPE_SEPARATOR}{id}", self.kind.as_str()),
            None => self.kind.as_str().to_owned(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryType {
    Preference,
    Fact,
    Instruction,
    Constraint,
    WorkState,
    Experience,
}

impl MemoryType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Preference => "preference",
            Self::Fact => "fact",
            Self::Instruction => "instruction",
            Self::Constraint => "constraint",
            Self::WorkState => "work_state",
            Self::Experience => "experience",
        }
    }

    pub fn parse(value: &str) -> Result<Self, MemoryError> {
        match value.trim() {
            "preference" => Ok(Self::Preference),
            "fact" => Ok(Self::Fact),
            "instruction" => Ok(Self::Instruction),
            "constraint" => Ok(Self::Constraint),
            "work_state" => Ok(Self::WorkState),
            "experience" => Ok(Self::Experience),
            _ => Err(MemoryError::coded(
                "MEM_INVALID_TYPE",
                format!("type must be one of {}", MEMORY_TYPES.join(", ")),
            )),
        }
    }

    /// Design §4.3: working memory and experience carry a fixed default TTL because they describe a
    /// task that has already ended.
    pub fn design_default_ttl_days(self) -> Option<u32> {
        match self {
            Self::WorkState => Some(crate::memory::policy::DEFAULT_WORK_STATE_TTL_DAYS),
            Self::Experience => Some(crate::memory::policy::DEFAULT_EXPERIENCE_TTL_DAYS),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Sensitivity {
    Normal,
    Private,
    SecretCandidate,
}

impl Sensitivity {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Normal => "normal",
            Self::Private => "private",
            Self::SecretCandidate => "secret_candidate",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemorySourceType {
    User,
    Model,
    Tool,
    System,
}

impl MemorySourceType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Model => "model",
            Self::Tool => "tool",
            Self::System => "system",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryStatus {
    Active,
    Deleted,
    Expired,
    Archived,
}

impl MemoryStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Deleted => "deleted",
            Self::Expired => "expired",
            Self::Archived => "archived",
        }
    }

    pub fn parse(value: &str) -> Result<Self, MemoryError> {
        match value.trim() {
            "active" => Ok(Self::Active),
            "deleted" => Ok(Self::Deleted),
            "expired" => Ok(Self::Expired),
            "archived" => Ok(Self::Archived),
            _ => Err(MemoryError::coded(
                "MEM_INVALID_ARGUMENT",
                format!("status must be one of {}", MEMORY_STATUSES.join(", ")),
            )),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryOperation {
    Create,
    Update,
    Merge,
    Delete,
}

impl MemoryOperation {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Create => "create",
            Self::Update => "update",
            Self::Merge => "merge",
            Self::Delete => "delete",
        }
    }

    pub fn parse(value: &str) -> Result<Self, MemoryError> {
        match value.trim() {
            "create" => Ok(Self::Create),
            "update" => Ok(Self::Update),
            "merge" => Ok(Self::Merge),
            "delete" => Ok(Self::Delete),
            _ => Err(MemoryError::coded(
                "MEM_INVALID_OPERATION",
                format!("operation must be one of {}", MEMORY_OPERATIONS.join(", ")),
            )),
        }
    }
}

/// Keyset cursor over `(updated_at_ms DESC, id ASC)`. Opaque to the UI: the payload is base64url
/// encoded so the frontend cannot build a cursor from scratch, and a corrupted value closes the
/// request instead of silently restarting the page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryCursor {
    pub updated_at_ms: u64,
    pub id: String,
}

impl MemoryCursor {
    pub fn encode(&self) -> String {
        let payload = serde_json::to_vec(self).unwrap_or_default();
        URL_SAFE_NO_PAD.encode(payload)
    }

    pub fn decode(value: &str) -> Result<Self, MemoryError> {
        let bytes = URL_SAFE_NO_PAD.decode(value.trim()).map_err(|_| {
            MemoryError::coded("MEM_INVALID_ARGUMENT", "cursor is not a valid page token")
        })?;
        let cursor = serde_json::from_slice::<Self>(&bytes).map_err(|_| {
            MemoryError::coded("MEM_INVALID_ARGUMENT", "cursor is not a valid page token")
        })?;
        if cursor.id.trim().is_empty() {
            return Err(MemoryError::coded(
                "MEM_INVALID_ARGUMENT",
                "cursor is not a valid page token",
            ));
        }
        Ok(cursor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_scope_parses_canonical_forms() {
        let user = MemoryScope::parse("user").unwrap();
        assert_eq!(user.kind, MemoryScopeKind::User);
        assert_eq!(user.id, None);
        assert_eq!(user.canonical(), "user");

        for (input, kind, id) in [
            ("workspace:ws-1", MemoryScopeKind::Workspace, "ws-1"),
            ("project:proj-1", MemoryScopeKind::Project, "proj-1"),
            ("thread:turn-1", MemoryScopeKind::Thread, "turn-1"),
        ] {
            let scope = MemoryScope::parse(input).unwrap();
            assert_eq!(scope.kind, kind);
            assert_eq!(scope.id.as_deref(), Some(id));
            assert_eq!(scope.canonical(), input);
        }

        // Surrounding whitespace is trimmed so a UI cannot create a second vocabulary by accident.
        assert_eq!(MemoryScope::parse("  user  ").unwrap().canonical(), "user");
    }

    #[test]
    fn memory_scope_rejects_malformed_forms_without_guessing() {
        for input in [
            "",
            "   ",
            "User",
            "global",
            "project",
            "thread",
            "workspace",
            "user:abc",
            "project:",
            "project:a:b",
            "project:with space",
        ] {
            let error = MemoryScope::parse(input).unwrap_err();
            assert_eq!(error.code(), "MEM_INVALID_SCOPE", "input: {input:?}");
        }

        let long = format!("project:{}", "a".repeat(MAX_MEMORY_SCOPE_ID_CHARS + 1));
        assert_eq!(
            MemoryScope::parse(&long).unwrap_err().code(),
            "MEM_INVALID_SCOPE"
        );

        let control = format!("project:bad{}id", '\u{7}');
        assert_eq!(
            MemoryScope::parse(&control).unwrap_err().code(),
            "MEM_INVALID_SCOPE"
        );
    }

    #[test]
    fn memory_scope_validate_matches_parse_rules() {
        assert!(
            MemoryScope::new(MemoryScopeKind::User, None)
                .validate()
                .is_ok()
        );
        assert!(
            MemoryScope::new(MemoryScopeKind::User, Some("x".into()))
                .validate()
                .is_err()
        );
        assert!(
            MemoryScope::new(MemoryScopeKind::Project, None)
                .validate()
                .is_err()
        );
        assert!(
            MemoryScope::new(MemoryScopeKind::Project, Some(String::new()))
                .validate()
                .is_err()
        );
    }

    #[test]
    fn memory_type_and_status_parse_reject_unknown_values() {
        assert_eq!(
            MemoryType::parse("work_state").unwrap(),
            MemoryType::WorkState
        );
        assert_eq!(
            MemoryType::parse("experience").unwrap(),
            MemoryType::Experience
        );
        assert_eq!(
            MemoryType::parse("notes").unwrap_err().code(),
            "MEM_INVALID_TYPE"
        );
        assert_eq!(MemoryStatus::parse("active").unwrap(), MemoryStatus::Active);
        assert_eq!(
            MemoryStatus::parse("gone").unwrap_err().code(),
            "MEM_INVALID_ARGUMENT"
        );
    }

    #[test]
    fn operation_parse_round_trips_and_rejects_unknown_values() {
        for operation in [
            MemoryOperation::Create,
            MemoryOperation::Update,
            MemoryOperation::Merge,
            MemoryOperation::Delete,
        ] {
            assert_eq!(
                MemoryOperation::parse(operation.as_str()).unwrap(),
                operation
            );
        }
        assert_eq!(
            MemoryOperation::parse(" upsert ").unwrap_err().code(),
            "MEM_INVALID_OPERATION"
        );
    }

    #[test]
    fn only_work_state_and_experience_carry_a_design_default_ttl() {
        assert_eq!(MemoryType::WorkState.design_default_ttl_days(), Some(14));
        assert_eq!(MemoryType::Experience.design_default_ttl_days(), Some(180));
        for memory_type in [
            MemoryType::Preference,
            MemoryType::Fact,
            MemoryType::Instruction,
            MemoryType::Constraint,
        ] {
            assert_eq!(memory_type.design_default_ttl_days(), None);
        }
    }

    #[test]
    fn memory_cursor_round_trips_and_rejects_corruption() {
        let cursor = MemoryCursor {
            updated_at_ms: 1_700_000_000_000,
            id: "memory-1".into(),
        };
        let encoded = cursor.encode();
        assert_eq!(MemoryCursor::decode(&encoded).unwrap(), cursor);

        // The encoded token must not leak the raw ordering fields in plain text.
        assert!(!encoded.contains("memory-1"));

        for input in ["", "not-base64!", "aGVsbG8", "e30"] {
            let error = MemoryCursor::decode(input).unwrap_err();
            assert_eq!(error.code(), "MEM_INVALID_ARGUMENT", "input: {input:?}");
        }

        let blank_id = MemoryCursor {
            updated_at_ms: 1,
            id: "  ".into(),
        };
        assert!(MemoryCursor::decode(&blank_id.encode()).is_err());
    }
}
