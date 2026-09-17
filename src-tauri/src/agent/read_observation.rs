use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use serde_json::{Value, json};

use crate::protocol::ToolResult;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ReadRevisionKey {
    path: String,
    revision: String,
    byte_ranges: bool,
}

#[derive(Debug, Default)]
pub(super) struct ReadObservationTracker {
    // Half-open ranges. Bytes distinguish pages within one long source line;
    // line ranges remain a separate namespace for older persisted observations.
    coverage: HashMap<ReadRevisionKey, Vec<(u64, u64)>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ReadObservationDecision {
    NewCoverage,
    AlreadyCovered,
}

impl ReadObservationTracker {
    pub(super) fn observe(&mut self, result: &ToolResult) -> Option<ReadObservationDecision> {
        if !result.success || result.metadata["contentSuppressed"] == true {
            return None;
        }
        let metadata = &result.metadata;
        let path = metadata.get("path")?.as_str()?;
        let revision = metadata.get("fileRevision")?.as_str()?;
        if path.is_empty() || revision.is_empty() {
            return None;
        }
        let (byte_ranges, ranges) = match (metadata.get("offset"), metadata.get("bytesReturned")) {
            (Some(offset), Some(bytes)) => {
                let offset = offset.as_u64()?;
                let bytes = bytes.as_u64()?;
                let end = offset.checked_add(bytes)?;
                let ranges = if metadata["outputTruncated"] == true {
                    let retained = metadata.get("retainedOutputRanges")?.as_array()?;
                    if retained.is_empty() {
                        return None;
                    }
                    retained
                        .iter()
                        .map(|range| {
                            let pair = range.as_array()?;
                            if pair.len() != 2 {
                                return None;
                            }
                            let start = pair[0].as_u64()?;
                            let end = pair[1].as_u64()?;
                            (start <= end && end <= bytes).then(|| (offset + start, offset + end))
                        })
                        .collect::<Option<Vec<_>>>()?
                } else {
                    vec![(offset, end)]
                };
                (true, ranges)
            }
            (None, None) => {
                if metadata["outputTruncated"] == true {
                    return None;
                }
                let start = metadata.get("startLine")?.as_u64()?;
                let end = metadata.get("endLine")?.as_u64()?;
                if start == 0 || end < start {
                    return None;
                }
                (false, vec![(start, end.checked_add(1)?)])
            }
            _ => return None,
        };
        let key = ReadRevisionKey {
            path: if cfg!(windows) {
                path.to_lowercase()
            } else {
                path.to_string()
            },
            revision: revision.to_string(),
            byte_ranges,
        };
        let first_observation = !self.coverage.contains_key(&key);
        let intervals = self.coverage.entry(key).or_default();
        let previous = intervals.clone();
        // An empty file establishes its revision once. Moving an empty page
        // (including UTF-8 boundary backtracking) does not reveal new content.
        intervals.extend(ranges.into_iter().filter(|(start, end)| start < end));
        intervals.sort_unstable();
        let mut merged: Vec<(u64, u64)> = Vec::with_capacity(intervals.len());
        for &(start, end) in intervals.iter() {
            if let Some((_, previous_end)) = merged.last_mut()
                && start <= *previous_end
            {
                *previous_end = (*previous_end).max(end);
            } else {
                merged.push((start, end));
            }
        }
        let changed = first_observation || previous != merged;
        *intervals = merged;
        Some(if changed {
            ReadObservationDecision::NewCoverage
        } else {
            ReadObservationDecision::AlreadyCovered
        })
    }

    pub(super) fn reset_context(&mut self) {
        self.coverage.clear();
    }

    pub(super) fn progress_fingerprints(&self) -> impl Iterator<Item = u64> + '_ {
        self.coverage.iter().map(|(key, ranges)| {
            let mut hasher = DefaultHasher::new();
            "read_file".hash(&mut hasher);
            key.hash(&mut hasher);
            ranges.hash(&mut hasher);
            hasher.finish()
        })
    }
}

pub(super) fn read_observation_result(
    mut result: ToolResult,
    decision: ReadObservationDecision,
) -> ToolResult {
    if decision == ReadObservationDecision::AlreadyCovered {
        if !result.metadata.is_object() {
            result.metadata = json!({});
        }
        result.metadata["observationStatus"] =
            Value::String("read_observation_already_covered".into());
        result.metadata["contentSuppressed"] = Value::Bool(false);
        result.metadata["turnContinues"] = Value::Bool(true);
    }
    result
}
