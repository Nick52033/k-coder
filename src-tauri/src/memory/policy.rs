//! Deterministic host-side memory policy: TTL, sensitivity grading and confirmation tokens.
//!
//! Every rule here is host-owned and reproducible. Nothing in this module reads model output as an
//! authority: the model cannot shorten a TTL, lower a sensitivity level or satisfy a confirmation
//! check, because it never supplies those fields in the first place.

use crate::memory::{MemoryError, MemoryScope, MemoryType, Sensitivity};
use crate::storage::memory_repository::MemoryRecord;

/// Design §4.3: working memory describes a task that has ended, so it expires quickly.
pub const DEFAULT_WORK_STATE_TTL_DAYS: u32 = 14;
/// Design §4.3: experience is useful for longer, but still bounded.
pub const DEFAULT_EXPERIENCE_TTL_DAYS: u32 = 180;
/// Upper bound for any explicit or configured TTL, so a caller cannot pin a memory forever by
/// passing a huge `expiresAtMs`.
pub const MAX_MEMORY_TTL_DAYS: u32 = 3_650;
/// Design §4.2: only high-confidence, non-sensitive creates and updates may be auto-accepted.
pub const AUTO_ACCEPT_CONFIDENCE: f64 = 0.8;

const MS_PER_DAY: u64 = 24 * 60 * 60 * 1_000;
/// A value shorter than this is treated as prose rather than a credential.
const MIN_SECRET_VALUE_CHARS: usize = 8;

pub fn days_to_ms(days: u32) -> u64 {
    u64::from(days).saturating_mul(MS_PER_DAY)
}

/// Resolves the effective expiry for a write.
///
/// An explicit `expiresAtMs` always wins, but must be in the future and within
/// [`MAX_MEMORY_TTL_DAYS`]. Otherwise the type-specific design TTL applies first, and only then the
/// user-configured `defaultTtlDays` (where `0` means "no expiry").
pub fn effective_expiry(
    memory_type: MemoryType,
    requested_expires_at_ms: Option<u64>,
    now_ms: u64,
    default_ttl_days: u32,
) -> Result<Option<u64>, MemoryError> {
    if let Some(requested) = requested_expires_at_ms {
        if requested <= now_ms {
            return Err(MemoryError::coded(
                "MEM_INVALID_ARGUMENT",
                "expiresAtMs must be in the future",
            ));
        }
        let ceiling = now_ms.saturating_add(days_to_ms(MAX_MEMORY_TTL_DAYS));
        if requested > ceiling {
            return Err(MemoryError::coded(
                "MEM_INVALID_ARGUMENT",
                format!("expiresAtMs must be within {MAX_MEMORY_TTL_DAYS} days"),
            ));
        }
        return Ok(Some(requested));
    }
    let days = memory_type
        .design_default_ttl_days()
        .or_else(|| (default_ttl_days > 0).then_some(default_ttl_days));
    Ok(days.map(|days| now_ms.saturating_add(days_to_ms(days))))
}

/// Host-side sensitivity grading. The level can only be raised: no caller may pass `normal` to
/// downgrade content the host already considers private or secret-bearing.
pub fn detect_sensitivity(content: &str) -> Sensitivity {
    if contains_credential(content) {
        Sensitivity::SecretCandidate
    } else if contains_private_marker(content) {
        Sensitivity::Private
    } else {
        Sensitivity::Normal
    }
}

/// Raises `declared` to at least the host-detected level. Used where a caller legitimately supplies
/// a level (a user confirming "this is private"), never where the model does.
pub fn raise_sensitivity(declared: Sensitivity, detected: Sensitivity) -> Sensitivity {
    declared.max(detected)
}

/// Credentials are detected by reusing the runtime redactor, so memory and logs agree on what a
/// secret looks like, plus a narrow `name: value` scan for the shapes the redactor does not cover.
fn contains_credential(content: &str) -> bool {
    if crate::execution::redact(content) != content {
        return true;
    }
    let tokens = content.split_whitespace().collect::<Vec<_>>();
    for (index, token) in tokens.iter().enumerate() {
        let Some((name, inline_value)) = split_name_and_value(token) else {
            continue;
        };
        if !is_sensitive_name(name) {
            continue;
        }
        if !inline_value.is_empty() {
            return true;
        }
        if let Some(next) = tokens.get(index + 1)
            && looks_like_secret(next)
        {
            return true;
        }
    }
    false
}

fn split_name_and_value(token: &str) -> Option<(&str, &str)> {
    if let Some((name, value)) = token.split_once('=') {
        return Some((name.trim(), value.trim()));
    }
    if let Some(name) = token.strip_suffix(':') {
        return Some((name.trim(), ""));
    }
    token
        .split_once(':')
        .map(|(name, value)| (name.trim(), value.trim()))
}

/// Mirrors `execution::is_sensitive_key`, which is private to that module.
fn is_sensitive_name(name: &str) -> bool {
    let name = name.to_ascii_uppercase();
    [
        "KEY",
        "TOKEN",
        "SECRET",
        "PASSWORD",
        "PASSWD",
        "CREDENTIAL",
        "AUTH",
    ]
    .iter()
    .any(|part| name.contains(part))
}

fn looks_like_secret(token: &str) -> bool {
    let token = token.trim_matches(|character: char| {
        !character.is_alphanumeric() && character != '-' && character != '_'
    });
    if token.chars().count() < MIN_SECRET_VALUE_CHARS {
        return false;
    }
    let has_digit = token.chars().any(|character| character.is_ascii_digit());
    let has_alpha = token
        .chars()
        .any(|character| character.is_ascii_alphabetic());
    let mixed_case = token
        .chars()
        .any(|character| character.is_ascii_uppercase())
        && token
            .chars()
            .any(|character| character.is_ascii_lowercase());
    (has_digit && has_alpha) || mixed_case
}

/// Absolute local paths, e-mail addresses and phone numbers are graded private: design §8 forbids
/// sending them to an external embedding or completion provider.
fn contains_private_marker(content: &str) -> bool {
    contains_absolute_path(content) || contains_email(content) || contains_phone(content)
}

fn contains_absolute_path(content: &str) -> bool {
    if content.contains("/home/") || content.contains("/Users/") || content.contains(r"\\?\") {
        return true;
    }
    let characters = content.chars().collect::<Vec<_>>();
    characters.windows(3).any(|window| {
        window[0].is_ascii_alphabetic() && window[1] == ':' && matches!(window[2], '\\' | '/')
    })
}

fn contains_email(content: &str) -> bool {
    content.split_whitespace().any(|token| {
        let token = token.trim_matches(|character: char| {
            !character.is_alphanumeric() && !matches!(character, '@' | '.' | '_' | '-' | '+')
        });
        let Some((local, domain)) = token.split_once('@') else {
            return false;
        };
        if local.is_empty() {
            return false;
        }
        let Some((host, suffix)) = domain.rsplit_once('.') else {
            return false;
        };
        !host.is_empty()
            && suffix.len() >= 2
            && suffix
                .chars()
                .all(|character| character.is_ascii_alphabetic())
    })
}

/// Mainland China mobile numbers: eleven digits beginning with `1`.
fn contains_phone(content: &str) -> bool {
    content
        .split(|character: char| !character.is_ascii_digit())
        .any(|run| run.len() == 11 && run.starts_with('1'))
}

/// The single liveness rule for a stored memory.
///
/// Design §4.3 + §5.2: a memory is past its lifetime when an explicit `expires_at_ms` has passed, or
/// when a type with a design-mandated TTL is older than that TTL even though no expiry was stored.
/// The deadline itself already counts as expired.
///
/// Both the context assembler (injection) and offline maintenance (the expiry sweep) call this, so a
/// row cannot be injectable but un-expirable, or vice versa.
pub fn memory_is_expired(record: &MemoryRecord, now_ms: u64) -> bool {
    if record
        .expires_at_ms
        .is_some_and(|expires| expires <= now_ms)
    {
        return true;
    }
    let Ok(memory_type) = MemoryType::parse(&record.memory_type) else {
        // An unparseable type cannot be dated by the design clock; the projection refuses to store
        // one, so treating it as live keeps the sweep from inventing a deadline.
        return false;
    };
    let Some(ttl_days) = memory_type.design_default_ttl_days() else {
        return false;
    };
    record.created_at_ms.saturating_add(days_to_ms(ttl_days)) <= now_ms
}

/// The clear confirmation token is the canonical scope string, so the host never has to invent a
/// second vocabulary and the user-facing dialog can show exactly what will be cleared.
pub fn scope_confirmation_token(scope: &MemoryScope) -> String {
    scope.canonical()
}

/// Deletion always requires an explicit confirmation that matches the host-computed value. A model
/// cannot satisfy this: it never sees the token and cannot pass one through a candidate.
pub fn require_confirmation(expected: &str, provided: &str) -> Result<(), MemoryError> {
    if provided.trim() != expected {
        return Err(MemoryError::coded(
            "MEM_CONFIRMATION_REQUIRED",
            "confirmationToken must match the host-issued value",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::{MemoryScopeKind, MemoryStatus};

    #[test]
    fn default_ttl_follows_memory_type_then_settings() {
        let now = 1_000_000_000_000;

        // Explicit expiry wins.
        let explicit =
            effective_expiry(MemoryType::Preference, Some(now + 60_000), now, 0).unwrap();
        assert_eq!(explicit, Some(now + 60_000));

        // Design-mandated TTLs are independent of the user setting.
        assert_eq!(
            effective_expiry(MemoryType::WorkState, None, now, 0).unwrap(),
            Some(now + days_to_ms(DEFAULT_WORK_STATE_TTL_DAYS))
        );
        assert_eq!(
            effective_expiry(MemoryType::Experience, None, now, 999).unwrap(),
            Some(now + days_to_ms(DEFAULT_EXPERIENCE_TTL_DAYS))
        );

        // Types without a design TTL fall back to the setting, and `0` means "never expires".
        assert_eq!(
            effective_expiry(MemoryType::Preference, None, now, 0).unwrap(),
            None
        );
        assert_eq!(
            effective_expiry(MemoryType::Preference, None, now, 30).unwrap(),
            Some(now + days_to_ms(30))
        );
        assert_eq!(
            effective_expiry(MemoryType::Fact, None, now, 30).unwrap(),
            Some(now + days_to_ms(30))
        );
    }

    #[test]
    fn explicit_expiry_must_be_in_the_future_and_bounded() {
        let now = 1_000_000_000_000;
        assert_eq!(
            effective_expiry(MemoryType::Fact, Some(now), now, 0)
                .unwrap_err()
                .code(),
            "MEM_INVALID_ARGUMENT"
        );
        assert_eq!(
            effective_expiry(MemoryType::Fact, Some(now - 1), now, 0)
                .unwrap_err()
                .code(),
            "MEM_INVALID_ARGUMENT"
        );
        let too_far = now + days_to_ms(MAX_MEMORY_TTL_DAYS) + 1;
        assert_eq!(
            effective_expiry(MemoryType::Fact, Some(too_far), now, 0)
                .unwrap_err()
                .code(),
            "MEM_INVALID_ARGUMENT"
        );
        // The exact ceiling is accepted.
        assert!(
            effective_expiry(
                MemoryType::Fact,
                Some(now + days_to_ms(MAX_MEMORY_TTL_DAYS)),
                now,
                0
            )
            .is_ok()
        );
    }

    #[test]
    fn sensitivity_detection_flags_credentials_and_private_markers() {
        for secret in [
            "API_KEY=sk-live-abcdefghijklmnop",
            "the api key: sk-proj-abcdefghijklmnop",
            "password: hunter2sword",
            "token: ghp_abcdefghijklmnopqrst",
            "credential=AKIAIOSFODNN7EXAMPLE",
        ] {
            assert_eq!(
                detect_sensitivity(secret),
                Sensitivity::SecretCandidate,
                "input: {secret}"
            );
        }

        for private in [
            r"the repository lives at D:\code\k-coder",
            "the build runs in /home/ci/workspace",
            "reach me at user@example.com",
            "call 13800138000 before deploying",
            r"unc path \\?\C:\very\long",
        ] {
            assert_eq!(
                detect_sensitivity(private),
                Sensitivity::Private,
                "input: {private}"
            );
        }

        for normal in [
            "Use pnpm instead of npm for this repository",
            "Prefer tabs over spaces in Rust files",
            "the token is stored in the operating system keychain",
            "run the tests before pushing",
        ] {
            assert_eq!(
                detect_sensitivity(normal),
                Sensitivity::Normal,
                "input: {normal}"
            );
        }
    }

    #[test]
    fn sensitivity_can_only_be_raised() {
        assert_eq!(
            raise_sensitivity(Sensitivity::Normal, Sensitivity::Private),
            Sensitivity::Private
        );
        assert_eq!(
            raise_sensitivity(Sensitivity::SecretCandidate, Sensitivity::Normal),
            Sensitivity::SecretCandidate
        );
        assert_eq!(
            raise_sensitivity(Sensitivity::Private, Sensitivity::Private),
            Sensitivity::Private
        );
    }

    #[test]
    fn scope_confirmation_token_is_the_canonical_scope() {
        assert_eq!(scope_confirmation_token(&MemoryScope::user()), "user");
        assert_eq!(
            scope_confirmation_token(&MemoryScope::new(
                MemoryScopeKind::Project,
                Some("proj-1".into())
            )),
            "project:proj-1"
        );
        assert!(require_confirmation("user", "user").is_ok());
        assert!(require_confirmation("user", " user ").is_ok());
        assert_eq!(
            require_confirmation("user", "project:proj-1")
                .unwrap_err()
                .code(),
            "MEM_CONFIRMATION_REQUIRED"
        );
        assert_eq!(
            require_confirmation("project:proj-1", "")
                .unwrap_err()
                .code(),
            "MEM_CONFIRMATION_REQUIRED"
        );
    }

    fn record(
        memory_type: MemoryType,
        created_at_ms: u64,
        expires_at_ms: Option<u64>,
    ) -> MemoryRecord {
        MemoryRecord {
            id: "mem-1".into(),
            scope_type: "user".into(),
            scope_id: None,
            memory_type: memory_type.as_str().to_owned(),
            normalized_key: "key".into(),
            content: "content".into(),
            source_type: "user".into(),
            source_ref: None,
            confidence: 1.0,
            sensitivity: Sensitivity::Normal.as_str().to_owned(),
            status: MemoryStatus::Active.as_str().to_owned(),
            revision: 1,
            expires_at_ms,
            created_at_ms,
            updated_at_ms: created_at_ms,
        }
    }

    #[test]
    fn liveness_uses_the_explicit_expiry_and_the_design_ttl() {
        let now = 1_000_000_000_000u64;

        // An explicit expiry already in the past is expired; the deadline itself counts.
        assert!(memory_is_expired(
            &record(MemoryType::Fact, now, Some(now - 1)),
            now
        ));
        assert!(memory_is_expired(
            &record(MemoryType::Fact, now, Some(now)),
            now
        ));
        assert!(!memory_is_expired(
            &record(MemoryType::Fact, now, Some(now + 1)),
            now
        ));

        // Types without a design TTL and without an expiry never expire on the design clock.
        assert!(!memory_is_expired(
            &record(MemoryType::Preference, 0, None),
            now
        ));

        // work_state and experience expire on their own clock even with no stored expiry.
        assert!(memory_is_expired(
            &record(
                MemoryType::WorkState,
                now - days_to_ms(DEFAULT_WORK_STATE_TTL_DAYS),
                None
            ),
            now
        ));
        assert!(!memory_is_expired(
            &record(
                MemoryType::WorkState,
                now - days_to_ms(DEFAULT_WORK_STATE_TTL_DAYS) + 1,
                None
            ),
            now
        ));
        assert!(memory_is_expired(
            &record(
                MemoryType::Experience,
                now - days_to_ms(DEFAULT_EXPERIENCE_TTL_DAYS),
                None
            ),
            now
        ));

        // An unparseable type cannot be dated, so the sweep must not invent a deadline for it.
        let mut unknown = record(MemoryType::Fact, 0, None);
        unknown.memory_type = "notes".into();
        assert!(!memory_is_expired(&unknown, now));
    }
}
