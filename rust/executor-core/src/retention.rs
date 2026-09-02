//! Deterministic age-or-depth selection for completed governed-test logs.

use std::collections::BTreeMap;
use std::path::PathBuf;

use devcoordinator2_executor_protocol::LogPhase;

use crate::ExecutorError;

pub const DEFAULT_MAX_AGE_SECONDS: u64 = 86_400;
pub const DEFAULT_HISTORY_DEPTH: usize = 3;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RetentionPolicy {
    pub max_age_seconds: u64,
    pub history_depth: usize,
}

impl Default for RetentionPolicy {
    fn default() -> Self {
        Self {
            max_age_seconds: DEFAULT_MAX_AGE_SECONDS,
            history_depth: DEFAULT_HISTORY_DEPTH,
        }
    }
}

impl RetentionPolicy {
    pub fn validate(self) -> Result<Self, ExecutorError> {
        if self.max_age_seconds == 0 {
            return Err(ExecutorError::new(
                "log retention max_age_seconds must be positive",
            ));
        }
        if self.history_depth == 0 {
            return Err(ExecutorError::new(
                "log retention history_depth must be positive",
            ));
        }
        Ok(self)
    }

    fn max_age_ms(self) -> u64 {
        self.max_age_seconds.saturating_mul(1_000)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RetentionEntry {
    pub run_id: String,
    pub test: String,
    pub check: Option<String>,
    pub phase: LogPhase,
    pub case: Option<String>,
    pub directory: PathBuf,
    pub finished_at_ms: u64,
    pub active: bool,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct HistoryKey {
    test: String,
    check: Option<String>,
    phase: LogPhase,
    case: Option<String>,
}

impl From<&RetentionEntry> for HistoryKey {
    fn from(entry: &RetentionEntry) -> Self {
        Self {
            test: entry.test.clone(),
            check: entry.check.clone(),
            phase: entry.phase,
            case: entry.case.clone(),
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RetentionDecision {
    pub victims: Vec<PathBuf>,
    pub retained_active: usize,
    pub next_age_expiry_ms: Option<u64>,
}

/// Select exact leaf directories to remove.
///
/// A completed entry expires when it is old enough *or* falls beyond the
/// configured newest-first depth for its logical test/check/phase/case key.
/// Active entries never expire, even if another process presents stale
/// metadata for them. The caller performs no-follow rename-to-garbage cleanup.
pub fn select_expired(
    entries: &[RetentionEntry],
    now_ms: u64,
    policy: RetentionPolicy,
) -> Result<RetentionDecision, ExecutorError> {
    let policy = policy.validate()?;
    let mut grouped: BTreeMap<HistoryKey, Vec<&RetentionEntry>> = BTreeMap::new();
    let mut decision = RetentionDecision::default();
    for entry in entries {
        if entry.active {
            decision.retained_active = decision.retained_active.saturating_add(1);
            continue;
        }
        grouped.entry(entry.into()).or_default().push(entry);
    }

    for rows in grouped.values_mut() {
        rows.sort_by(|left, right| {
            right
                .finished_at_ms
                .cmp(&left.finished_at_ms)
                .then_with(|| right.run_id.cmp(&left.run_id))
                .then_with(|| right.directory.cmp(&left.directory))
        });
        for (index, entry) in rows.iter().enumerate() {
            let age_expiry = entry.finished_at_ms.saturating_add(policy.max_age_ms());
            let age_expired = now_ms >= age_expiry;
            let depth_expired = index >= policy.history_depth;
            if age_expired || depth_expired {
                decision.victims.push(entry.directory.clone());
            } else if age_expiry > now_ms {
                decision.next_age_expiry_ms = Some(
                    decision
                        .next_age_expiry_ms
                        .map_or(age_expiry, |current| current.min(age_expiry)),
                );
            }
        }
    }
    decision.victims.sort();
    decision.victims.dedup();
    Ok(decision)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(run: &str, key: &str, finished_at_ms: u64) -> RetentionEntry {
        RetentionEntry {
            run_id: run.into(),
            test: "complete".into(),
            check: Some("unit".into()),
            phase: LogPhase::Case,
            case: Some(key.into()),
            directory: PathBuf::from(format!("runs/{run}/{key}")),
            finished_at_ms,
            active: false,
        }
    }

    #[test]
    fn default_policy_is_one_day_and_three_histories() {
        assert_eq!(RetentionPolicy::default().max_age_seconds, 86_400);
        assert_eq!(RetentionPolicy::default().history_depth, 3);
    }

    #[test]
    fn depth_removes_only_older_entries_for_the_same_identity() {
        let entries = vec![
            entry("r1", "same", 1_000),
            entry("r2", "same", 2_000),
            entry("r3", "same", 3_000),
            entry("r4", "same", 4_000),
            entry("r5", "other", 500),
        ];
        let decision = select_expired(
            &entries,
            5_000,
            RetentionPolicy {
                max_age_seconds: 100,
                history_depth: 3,
            },
        )
        .expect("selection");
        assert_eq!(decision.victims, vec![PathBuf::from("runs/r1/same")]);
    }

    #[test]
    fn age_and_depth_are_independent_or_boundaries() {
        let entries = vec![entry("new", "same", 99_500), entry("old", "same", 1_000)];
        let decision = select_expired(
            &entries,
            100_000,
            RetentionPolicy {
                max_age_seconds: 2,
                history_depth: 10,
            },
        )
        .expect("selection");
        assert_eq!(decision.victims, vec![PathBuf::from("runs/old/same")]);
        assert_eq!(decision.next_age_expiry_ms, Some(101_500));
    }

    #[test]
    fn active_entries_are_never_victims() {
        let mut protected = entry("active", "same", 1);
        protected.active = true;
        let entries = vec![protected, entry("new", "same", 20_000)];
        let decision = select_expired(
            &entries,
            30_000,
            RetentionPolicy {
                max_age_seconds: 1,
                history_depth: 1,
            },
        )
        .expect("selection");
        assert_eq!(decision.retained_active, 1);
        assert_eq!(decision.victims, vec![PathBuf::from("runs/new/same")]);
    }

    #[test]
    fn invalid_zero_boundaries_are_rejected() {
        for policy in [
            RetentionPolicy {
                max_age_seconds: 0,
                history_depth: 1,
            },
            RetentionPolicy {
                max_age_seconds: 1,
                history_depth: 0,
            },
        ] {
            assert!(select_expired(&[], 0, policy).is_err());
        }
    }
}
