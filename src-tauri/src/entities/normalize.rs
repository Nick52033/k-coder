//! Host-owned entity and predicate normalization.
//!
//! Identity is decided by the normalized name, never by the raw string a model proposed, so two
//! spellings of the same thing cannot become two entities. Normalization is deterministic and
//! idempotent — `normalize(normalize(x)) == normalize(x)` — which is what makes `find_entity_by_normalized_name`
//! a complete lookup rather than a heuristic.

use crate::entities::EntityError;
use crate::storage::knowledge_entity_repository::{MAX_ENTITY_NAME_CHARS, MAX_PREDICATE_CHARS};

/// The host-owned entity vocabulary.
///
/// A model never chooses one of these: a model-proposed entity is always created as
/// [`DEFAULT_ENTITY_TYPE`], and only the reviewer may set a different value. Keeping the vocabulary
/// host-owned is what stops a model from inventing a type that changes how the graph is read.
pub const ENTITY_TYPES: [&str; 8] = [
    "concept",
    "module",
    "symbol",
    "file",
    "api",
    "config",
    "service",
    "technology",
];

/// The type a model-proposed entity gets. "concept" is the honest default: the model did not say.
pub const DEFAULT_ENTITY_TYPE: &str = "concept";

/// Entity lifecycle. `candidate` is the only status a model proposal can produce, so an unreviewed
/// proposal is invisible to the relation query by construction.
pub const ENTITY_STATUSES: [&str; 3] = ["candidate", "active", "rejected"];
pub const ENTITY_STATUS_CANDIDATE: &str = "candidate";
pub const ENTITY_STATUS_ACTIVE: &str = "active";
pub const ENTITY_STATUS_REJECTED: &str = "rejected";

/// Upper bound for an entity type. Matches the storage token vocabulary.
pub const MAX_ENTITY_TYPE_CHARS: usize = 32;

/// Quotes a caller may wrap a name in. They are decoration, not part of the identity.
const NAME_DECORATION: [char; 8] = ['"', '\'', '`', '“', '”', '「', '」', '『'];

/// The identity of an entity: trimmed, decoration-stripped, whitespace-collapsed and lowercased.
///
/// Returns an empty string when nothing is left, which every caller treats as "invalid", so an
/// empty or punctuation-only name can never become an entity.
pub fn normalize_entity_name(name: &str) -> String {
    let trimmed = name
        .trim()
        .trim_matches(|character| NAME_DECORATION.contains(&character));
    let mut normalized = String::new();
    let mut pending_space = false;
    for character in trimmed.chars() {
        if character.is_whitespace() {
            pending_space = !normalized.is_empty();
            continue;
        }
        if pending_space {
            normalized.push(' ');
            pending_space = false;
        }
        // `to_lowercase` can expand one character into several, which is exactly why the fold is
        // applied per character rather than on the whole string.
        for lowered in character.to_lowercase() {
            normalized.push(lowered);
        }
    }
    normalized
}

/// The display form an entity keeps: the caller's spelling, trimmed of decoration and whitespace.
pub fn display_entity_name(name: &str) -> String {
    name.trim()
        .trim_matches(|character| NAME_DECORATION.contains(&character))
        .trim()
        .to_owned()
}

/// Normalizes a predicate into the storage token vocabulary (`[a-z0-9_]`).
///
/// Every word break — whitespace, punctuation, a path separator — becomes one underscore, so
/// `depends on` and `depends-on` are the same predicate. A predicate that cannot be expressed in
/// that vocabulary is rejected with `None` rather than silently mangled: non-ASCII letters (a
/// Chinese predicate, for instance) and anything that normalizes to nothing are refused.
pub fn normalize_predicate(value: &str) -> Option<String> {
    let mut normalized = String::new();
    let mut pending_separator = false;
    for character in value.chars() {
        if character.is_ascii_alphanumeric() {
            if pending_separator && !normalized.is_empty() {
                normalized.push('_');
            }
            pending_separator = false;
            normalized.push(character.to_ascii_lowercase());
            continue;
        }
        if character.is_alphanumeric() {
            // A letter outside ASCII cannot be stored in a token, and transliterating it would
            // invent a predicate the caller never wrote.
            return None;
        }
        pending_separator = !normalized.is_empty();
    }
    if normalized.is_empty() || normalized.len() > MAX_PREDICATE_CHARS {
        return None;
    }
    Some(normalized)
}

/// Validates a name for storage. Returns the display form to store.
pub fn validate_entity_name(name: &str) -> Result<String, EntityError> {
    let display = display_entity_name(name);
    let length = display.chars().count();
    if length == 0 || length > MAX_ENTITY_NAME_CHARS {
        return Err(EntityError::coded(
            "ENT_INVALID_RELATION",
            format!("an entity name must contain 1 to {MAX_ENTITY_NAME_CHARS} characters"),
        ));
    }
    if normalize_entity_name(&display).is_empty() {
        return Err(EntityError::coded(
            "ENT_INVALID_RELATION",
            "an entity name must contain at least one non-decoration character",
        ));
    }
    Ok(display)
}

/// Validates a host-supplied entity type against the host-owned vocabulary.
pub fn validate_entity_type(value: &str) -> Result<String, EntityError> {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed.chars().count() > MAX_ENTITY_TYPE_CHARS {
        return Err(EntityError::coded(
            "ENT_INVALID_ARGUMENT",
            format!("entityType must contain 1 to {MAX_ENTITY_TYPE_CHARS} characters"),
        ));
    }
    if !ENTITY_TYPES.contains(&trimmed) {
        return Err(EntityError::coded(
            "ENT_INVALID_ARGUMENT",
            format!(
                "entityType must be one of {}, got {trimmed}",
                ENTITY_TYPES.join(", ")
            ),
        ));
    }
    Ok(trimmed.to_owned())
}

/// Normalizes a predicate or rejects it.
pub fn validate_predicate(value: &str) -> Result<String, EntityError> {
    normalize_predicate(value).ok_or_else(|| {
        EntityError::coded(
            "ENT_INVALID_RELATION",
            format!(
                "a predicate must be an ascii word of 1 to {MAX_PREDICATE_CHARS} characters \
                 (letters, digits and underscores)"
            ),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalization_is_deterministic_and_idempotent() {
        for raw in [
            "  MemoryService  ",
            "k-coder",
            "\"知识库\"",
            "`bootstrap.ps1`",
            "多个   空格",
            "Already_Lower",
        ] {
            let once = normalize_entity_name(raw);
            assert_eq!(
                once,
                normalize_entity_name(&once),
                "normalization must be idempotent for {raw:?}"
            );
        }
    }

    #[test]
    fn decoration_and_case_collapse_to_one_identity() {
        let expected = normalize_entity_name("MemoryService");
        for variant in [
            " memoryservice ",
            "\"MemoryService\"",
            "`MemoryService`",
            "「MemoryService」",
        ] {
            assert_eq!(
                normalize_entity_name(variant),
                expected,
                "{variant:?} must resolve to the same entity"
            );
        }
        assert_eq!(expected, "memoryservice");
        // A word break is part of the identity, so it collapses to exactly one space and stays
        // distinct from the joined spelling.
        assert_eq!(normalize_entity_name("Memory   Service"), "memory service");
        assert_ne!(expected, normalize_entity_name("Memory Service"));
        // Distinct things stay distinct: the separator is part of the identity.
        assert_ne!(
            normalize_entity_name("k-coder"),
            normalize_entity_name("kcoder")
        );
    }

    #[test]
    fn an_empty_or_decoration_only_name_has_no_identity() {
        assert_eq!(normalize_entity_name("   "), "");
        assert_eq!(normalize_entity_name("\"\"``"), "");
        assert_eq!(
            validate_entity_name("  ").unwrap_err().code(),
            "ENT_INVALID_RELATION"
        );
        assert_eq!(validate_entity_name("\"知识库\"").unwrap(), "知识库");
    }

    #[test]
    fn predicates_collapse_word_breaks_and_reject_non_ascii() {
        assert_eq!(normalize_predicate("depends on").unwrap(), "depends_on");
        assert_eq!(normalize_predicate("depends-on").unwrap(), "depends_on");
        assert_eq!(
            normalize_predicate("  Depends   On  ").unwrap(),
            "depends_on"
        );
        assert_eq!(
            normalize_predicate("supports_http2").unwrap(),
            "supports_http2"
        );
        assert_eq!(normalize_predicate("uses::v3").unwrap(), "uses_v3");
        assert_eq!(normalize_predicate("_leads_").unwrap(), "leads");
        assert_eq!(normalize_predicate("___"), None);
        assert_eq!(normalize_predicate("依赖"), None);
        assert_eq!(normalize_predicate(""), None);
        assert_eq!(
            normalize_predicate(&"a".repeat(MAX_PREDICATE_CHARS + 1)),
            None
        );
        assert_eq!(
            validate_predicate("依赖").unwrap_err().code(),
            "ENT_INVALID_RELATION"
        );
    }

    #[test]
    fn entity_types_are_limited_to_the_host_vocabulary() {
        assert_eq!(validate_entity_type(" module ").unwrap(), "module");
        assert_eq!(
            validate_entity_type("whatever").unwrap_err().code(),
            "ENT_INVALID_ARGUMENT"
        );
        assert_eq!(
            validate_entity_type("").unwrap_err().code(),
            "ENT_INVALID_ARGUMENT"
        );
        assert!(ENTITY_TYPES.contains(&DEFAULT_ENTITY_TYPE));
    }
}
