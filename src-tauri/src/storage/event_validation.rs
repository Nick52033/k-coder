//! Shared payload validation for the versioned knowledge and memory fact logs.
//!
//! The design requires every unknown schema version, malformed value or over-limit payload to close
//! the operation instead of being coerced, so both repositories validate a complete event before
//! any projection row is touched.

use crate::persistence::ProjectionError;

pub const MAX_ID_CHARS: usize = 128;
pub const MAX_TOKEN_CHARS: usize = 32;
pub const MAX_HASH_CHARS: usize = 128;

pub fn validate_confidence(value: f64) -> Result<(), ProjectionError> {
    if !value.is_finite() || !(0.0..=1.0).contains(&value) {
        return Err(ProjectionError::InvalidData(
            "confidence must be a finite number between 0 and 1".into(),
        ));
    }
    Ok(())
}

pub fn validate_id(value: &str, field: &str) -> Result<(), ProjectionError> {
    validate_bounded(value, field, 1, MAX_ID_CHARS)
}

pub fn validate_optional_id(value: Option<&str>, field: &str) -> Result<(), ProjectionError> {
    if let Some(value) = value {
        validate_id(value, field)?;
    }
    Ok(())
}

/// Enumerations stay host-owned, so this only rejects empty, oversized or malformed values instead
/// of freezing a vocabulary that later tasks still have to settle.
///
/// Digits are part of the vocabulary because host-owned tokens include versioned names
/// (`supports_http2`, `v3`). What stays rejected is anything that could smuggle structure through
/// the value: uppercase, whitespace, punctuation and path separators.
pub fn validate_token(value: &str, field: &str) -> Result<(), ProjectionError> {
    if value.is_empty() || value.len() > MAX_TOKEN_CHARS {
        return Err(ProjectionError::InvalidData(format!(
            "{field} must contain 1 to {MAX_TOKEN_CHARS} characters"
        )));
    }
    if !value.chars().all(|character| {
        character.is_ascii_lowercase() || character.is_ascii_digit() || character == '_'
    }) {
        return Err(ProjectionError::InvalidData(format!(
            "{field} must use lowercase ascii letters, digits or underscores"
        )));
    }
    Ok(())
}

/// Validates a value against a documented enumeration.
pub fn validate_enum(value: &str, field: &str, allowed: &[&str]) -> Result<(), ProjectionError> {
    if !allowed.contains(&value) {
        return Err(ProjectionError::InvalidData(format!(
            "{field} must be one of {}",
            allowed.join(", ")
        )));
    }
    Ok(())
}

/// Hashes and opaque identifiers are never raw text, so only lowercase hexadecimal is accepted.
pub fn validate_hash(value: &str, field: &str) -> Result<(), ProjectionError> {
    validate_bounded(value, field, 1, MAX_HASH_CHARS)?;
    if !value
        .chars()
        .all(|character| character.is_ascii_hexdigit() && !character.is_ascii_uppercase())
    {
        return Err(ProjectionError::InvalidData(format!(
            "{field} must be a lowercase hexadecimal digest"
        )));
    }
    Ok(())
}

pub fn validate_bounded(
    value: &str,
    field: &str,
    minimum: usize,
    maximum: usize,
) -> Result<(), ProjectionError> {
    let length = value.chars().count();
    if length < minimum || length > maximum {
        return Err(ProjectionError::InvalidData(format!(
            "{field} must contain {minimum} to {maximum} characters"
        )));
    }
    Ok(())
}

pub fn validate_optional_bounded(
    value: Option<&str>,
    field: &str,
    maximum: usize,
) -> Result<(), ProjectionError> {
    if let Some(value) = value {
        validate_bounded(value, field, 1, maximum)?;
    }
    Ok(())
}

pub fn validate_non_negative(value: i64, field: &str) -> Result<(), ProjectionError> {
    if value < 0 {
        return Err(ProjectionError::InvalidData(format!(
            "{field} must not be negative"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_token_accepts_digits_but_still_rejects_structure() {
        // Versioned host-owned tokens are legitimate vocabulary.
        assert!(validate_token("supports_http2", "predicate").is_ok());
        assert!(validate_token("v3", "entityType").is_ok());
        assert!(validate_token("a1_b2", "predicate").is_ok());
        assert!(validate_token("_", "predicate").is_ok());

        for rejected in [
            "",
            "UPPER",
            "has space",
            "has-dash",
            "path/like",
            "dot.ted",
            "中文",
        ] {
            assert!(
                validate_token(rejected, "predicate").is_err(),
                "{rejected:?} must not be accepted as a token"
            );
        }
        assert!(validate_token(&"a".repeat(MAX_TOKEN_CHARS + 1), "predicate").is_err());
    }

    #[test]
    fn bounded_optional_and_confidence_validation_reject_bad_values() {
        assert!(validate_confidence(f64::NAN).is_err());
        assert!(validate_confidence(1.5).is_err());
        assert!(validate_confidence(0.5).is_ok());
        assert!(validate_bounded("", "name", 1, 10).is_err());
        assert!(validate_optional_bounded(Some(""), "description", 10).is_err());
        assert!(validate_optional_bounded(None, "description", 10).is_ok());
        assert!(validate_non_negative(-1, "count").is_err());
        // Only lowercase hexadecimal digests are opaque enough to stand in for a raw value.
        assert!(validate_hash("abc123", "queryHash").is_ok());
        assert!(validate_hash("ABC123", "queryHash").is_err());
        assert!(validate_hash("xyz", "queryHash").is_err());
    }
}
