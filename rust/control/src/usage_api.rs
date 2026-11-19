//! Bounded consumer of CodexMulti's existing localUsage/summary contract.
//! This module never opens, migrates, or writes the producer's accounting store.
use super::*;
use devcoordinator2_api::review::ReviewUsage;
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::Digest;
use std::collections::VecDeque;
use std::os::fd::AsRawFd;
use std::os::unix::fs::{FileTypeExt, MetadataExt};
use std::os::unix::net::UnixStream;
use std::sync::{Condvar, Mutex};
use tungstenite::{Message, WebSocket, protocol::WebSocketConfig};

#[path = "usage_api_cost.rs"]
mod cost_wire;

const MAX_RESPONSE: usize = 2 * 1024 * 1024;
const MAX_CACHED_BYTES: usize = 16 * 1024 * 1024;
const MAX_CACHED: usize = 96;
const FRESH_FOR: Duration = Duration::from_secs(5);
const RETRY_AFTER: Duration = Duration::from_secs(30);
// Normal Console retries arrive every five seconds. A cold scope that stopped
// retrying must not indefinitely prevent another scope's revalidation.
const COLD_WAITER_FOR: Duration = Duration::from_secs(15);
// The source may need more than two minutes for a real seven-day snapshot.
// Keep that one owned read alive across bounded foreground requests; closing
// its socket does not cancel the producer's scan and permits duplicate work.
const SOURCE_READ_BUDGET: Duration = Duration::from_secs(300);
const MAX_INFLIGHT_BYTES: usize = 16 * 1024 * 1024;
const MAX_PROFILE_SAMPLES: usize = 8;
const MAX_PROFILES: usize = 96;
const PROFILE_RETENTION: Duration = Duration::from_secs(24 * 60 * 60);
pub(super) const API_BUDGET: Duration = Duration::from_millis(650);

#[derive(Clone, Copy, PartialEq, Eq)]
enum QueryProjection {
    Rich,
    Headline,
}

fn projection_key(key: String, projection: QueryProjection) -> String {
    match projection {
        QueryProjection::Rich => key,
        QueryProjection::Headline => format!("{key}:token_headline"),
    }
}

pub(super) fn is_pending(reason: &str) -> bool {
    matches!(
        reason,
        "api_refresh_pending" | "api_source_busy" | "api_busy"
    )
}

// Enforce the absolute deadline on every underlying I/O, including a peer
// that slowly streams one frame. A socket timeout alone resets on each read.
struct DeadlineStream {
    socket: UnixStream,
    deadline: Instant,
}
impl std::ops::Deref for DeadlineStream {
    type Target = UnixStream;
    fn deref(&self) -> &UnixStream {
        &self.socket
    }
}
impl io::Read for DeadlineStream {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        set_deadline(&self.socket, self.deadline)
            .map_err(|_| io::Error::from(io::ErrorKind::TimedOut))?;
        self.socket.read(buffer)
    }
}
impl io::Write for DeadlineStream {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        set_deadline(&self.socket, self.deadline)
            .map_err(|_| io::Error::from(io::ErrorKind::TimedOut))?;
        io::Write::write(&mut self.socket, buffer)
    }
    fn flush(&mut self) -> io::Result<()> {
        io::Write::flush(&mut self.socket)
    }
}

#[derive(Clone, Default)]
pub(super) struct CollectorApi(Arc<(Mutex<State>, Condvar)>);
#[derive(Default)]
struct State {
    entries: HashMap<String, Entry>,
    failed_until: HashMap<String, (Instant, String)>,
    inflight: HashMap<String, Flight>,
    cold_waiters: HashMap<String, ColdWaiter>,
    profiles: HashMap<String, ProfileHistory>,
    headline_capabilities: HashMap<PathBuf, HeadlineCapability>,
}
#[derive(Clone, Copy, PartialEq, Eq)]
struct SocketIdentity {
    device: u64,
    inode: u64,
    uid: u32,
}
#[derive(Clone, Copy)]
struct HeadlineCapability {
    identity: SocketIdentity,
    supports: bool,
}
struct Flight {
    source: PathBuf,
    bytes: usize,
    started: Instant,
    profile: String,
    projection: QueryProjection,
}

struct ColdWaiter {
    source: PathBuf,
    touched: Instant,
    bytes: usize,
    first_seen: Instant,
}

#[derive(Default)]
struct ProfileHistory {
    samples: VecDeque<(Instant, u64)>,
}

impl State {
    fn pending_bytes(&self) -> usize {
        self.inflight
            .values()
            .map(|flight| flight.bytes)
            .chain(self.cold_waiters.values().map(|waiter| waiter.bytes))
            .sum()
    }

    fn remember_cold_waiter(&mut self, key: &str, source: &Path) {
        if let Some(waiter) = self.cold_waiters.get_mut(key) {
            waiter.touched = Instant::now();
            return;
        }
        let bytes = key.len().saturating_add(source.as_os_str().len());
        if self.cold_waiters.len() < MAX_CACHED
            && bytes <= MAX_INFLIGHT_BYTES.saturating_sub(self.pending_bytes())
        {
            self.cold_waiters.insert(
                key.to_owned(),
                ColdWaiter {
                    source: source.to_owned(),
                    touched: Instant::now(),
                    bytes,
                    first_seen: Instant::now(),
                },
            );
        }
    }

    fn record_duration(&mut self, profile: String, duration_ms: u64) {
        let now = Instant::now();
        self.profiles.retain(|_, history| {
            history
                .samples
                .retain(|(at, _)| now.duration_since(*at) < PROFILE_RETENTION);
            !history.samples.is_empty()
        });
        if !self.profiles.contains_key(&profile)
            && self.profiles.len() >= MAX_PROFILES
            && let Some(oldest) = self
                .profiles
                .iter()
                .min_by_key(|(_, history)| history.samples.front().map(|(at, _)| *at))
                .map(|(key, _)| key.clone())
        {
            self.profiles.remove(&oldest);
        }
        let history = self.profiles.entry(profile).or_default();
        history.samples.push_back((now, duration_ms));
        while history.samples.len() > MAX_PROFILE_SAMPLES {
            history.samples.pop_front();
        }
    }

    fn median_duration(&self, profile: &str) -> (u64, u8) {
        let Some(history) = self.profiles.get(profile) else {
            return (0, 0);
        };
        let now = Instant::now();
        let mut values = history
            .samples
            .iter()
            .filter(|(at, _)| now.duration_since(*at) < PROFILE_RETENTION)
            .map(|(_, value)| *value)
            .collect::<Vec<_>>();
        values.sort_unstable();
        let samples = u8::try_from(values.len()).unwrap_or(u8::MAX);
        (values.get(values.len() / 2).copied().unwrap_or(0), samples)
    }

    fn supports_headlines(
        &mut self,
        path: &Path,
        identity: Option<SocketIdentity>,
    ) -> Option<bool> {
        let capability = self.headline_capabilities.get(path).copied()?;
        if identity == Some(capability.identity) {
            Some(capability.supports)
        } else {
            self.headline_capabilities.remove(path);
            None
        }
    }
}

fn socket_identity(path: &Path) -> Option<SocketIdentity> {
    let metadata = path.metadata().ok()?;
    Some(SocketIdentity {
        device: metadata.dev(),
        inode: metadata.ino(),
        uid: metadata.uid(),
    })
}

struct Entry {
    report: Arc<Summary>,
    loaded: Instant,
    bytes: usize,
}

pub(super) struct SummaryRead {
    summary: Arc<Summary>,
    refresh_reason: Option<String>,
    loading: Option<UsageLoading>,
}

impl std::ops::Deref for SummaryRead {
    type Target = Summary;

    fn deref(&self) -> &Self::Target {
        &self.summary
    }
}

impl SummaryRead {
    fn cached_or_error(report: Option<Arc<Summary>>, reason: &str) -> Result<Self, String> {
        match report {
            Some(summary) => Ok(Self {
                summary,
                refresh_reason: Some(reason.into()),
                loading: None,
            }),
            None => Err(reason.into()),
        }
    }

    fn cached_with_loading(
        report: Arc<Summary>,
        reason: &str,
        loading: Option<UsageLoading>,
    ) -> Result<Self, String> {
        Ok(Self {
            summary: report,
            refresh_reason: Some(reason.into()),
            loading,
        })
    }

    pub(super) fn source_report(
        &self,
        repository: Option<&str>,
        buckets: usize,
        worktree_keys: Option<&[String]>,
        include_unassigned: bool,
    ) -> SourceReport {
        let mut report =
            self.summary
                .source_report(repository, buckets, worktree_keys, include_unassigned);
        if let Some(snapshot) = &mut report.snapshot {
            // Display age describes this exact summary, separately from the
            // producer publication times retained in source_snapshot.
            snapshot.updated_at_ms = Some(self.summary.generated_at);
            if let Some(reason) = &self.refresh_reason {
                snapshot.refreshing |= is_pending(reason);
                snapshot.refresh_failed |= !is_pending(reason);
                if is_pending(reason) {
                    snapshot.progress_stage = Some("source_read".into());
                }
            }
        }
        if let Some(reason) = &self.refresh_reason {
            increment(&mut report.coverage_events, "api_snapshot_stale");
            increment(&mut report.coverage_events, reason);
        }
        if let Some(loading) = &self.loading {
            report.loading = Some(loading.clone());
        }
        report
    }
}

fn loading_from_state(
    state: &State,
    key: &str,
    profile: &str,
    now: Instant,
) -> Option<UsageLoading> {
    let (elapsed, state_name) = if let Some(flight) = state.inflight.get(key) {
        (now.saturating_duration_since(flight.started), "reading")
    } else if let Some(waiter) = state.cold_waiters.get(key) {
        (
            now.saturating_duration_since(waiter.first_seen),
            "waiting_for_source",
        )
    } else {
        return None;
    };
    let elapsed_ms = u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX);
    let (median, samples) = state.median_duration(profile);
    let enough = state_name == "reading" && samples >= 3;
    let (state, remaining) = if state_name == "waiting_for_source" {
        (state_name, None)
    } else if enough && elapsed_ms >= median {
        ("estimate_exceeded", None)
    } else if enough {
        (state_name, Some(median.saturating_sub(elapsed_ms)))
    } else {
        (state_name, None)
    };
    Some(UsageLoading {
        state: state.into(),
        elapsed_ms,
        estimated_remaining_ms: remaining,
        estimate_samples: if state_name == "waiting_for_source" {
            0
        } else {
            samples
        },
        estimate_basis: (enough && state_name == "reading")
            .then(|| "recent_comparable_source_reads".into()),
    })
}

fn query_key(
    source: &CodexUsageSource,
    repository: Option<&str>,
    worktree_keys: Option<&[String]>,
    include_unassigned: bool,
    start: u64,
    end: u64,
) -> String {
    format!(
        "{}:{:?}:{}:{:?}:{include_unassigned}:{start}:{end}",
        source.uid,
        source.api_socket,
        repository.unwrap_or("all"),
        worktree_keys,
    )
}

#[expect(
    clippy::too_many_arguments,
    reason = "The timing profile must match the complete source query scope"
)]
fn profile_key(
    source: &CodexUsageSource,
    repository: Option<&str>,
    worktree_keys: Option<&[String]>,
    include_unassigned: bool,
    workstream: Option<&str>,
    bindings: &[crate::usage_context::Binding],
    start: u64,
    end: u64,
) -> Result<String, String> {
    let span = end.saturating_sub(start);
    let span_class = if start == 0 {
        "lifetime"
    } else if span <= 86_400_000 {
        "24h"
    } else if span <= 7 * 86_400_000 {
        "7d"
    } else if span <= 30 * 86_400_000 {
        "30d"
    } else {
        "long"
    };
    let bytes = serde_json::to_vec(&(
        source.uid,
        source
            .api_socket
            .as_ref()
            .map(|path| path.to_string_lossy()),
        repository,
        worktree_keys,
        include_unassigned,
        workstream,
        bindings,
        span_class,
        "summary",
    ))
    .map_err(|_| "work_context_unavailable")?;
    Ok(sha2::Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

#[expect(
    clippy::too_many_arguments,
    reason = "The cache key must retain the complete source query scope"
)]
fn full_query_key(
    source: &CodexUsageSource,
    repository: Option<&str>,
    worktree_keys: Option<&[String]>,
    include_unassigned: bool,
    workstream: Option<&str>,
    bindings: &[crate::usage_context::Binding],
    start: u64,
    end: u64,
) -> Result<String, String> {
    let revision = serde_json::to_vec(bindings).map_err(|_| "work_context_unavailable")?;
    let revision = sha2::Sha256::digest(revision)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    Ok(format!(
        "{}:{}:{}",
        query_key(
            source,
            repository,
            worktree_keys,
            include_unassigned,
            start,
            end
        ),
        workstream.unwrap_or("all"),
        revision
    ))
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Summary {
    /// Negotiated version of the authenticated usage-v2 contract.
    #[serde(default)]
    pub contract_version: Option<u32>,
    pub report: Report,
    pub generated_at: u64,
    #[serde(default)]
    pub snapshot: Option<Snapshot>,
    /// Fresh usage-v2 identity and the exact pinned frontiers used to create
    /// this response. These values are checked together before the report is
    /// admitted into a cache or review.
    #[serde(default)]
    pub source: Option<Value>,
    /// Optional producer-side review projection. Missing bindings remain
    /// unattributed; the consumer never derives them from repository totals.
    #[serde(default)]
    pub review: Option<ReviewUsage>,
}

// A headline has a separate wire parser: rich summaries must continue to
// reject nullable counts and activity vectors rather than accepting them as
// an incomplete review. Only a negotiated headline can create Counts::Deferred.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct HeadlineSummary {
    contract_version: Option<u32>,
    source: Option<Value>,
    generated_at: u64,
    snapshot: Option<Snapshot>,
    report: HeadlineReport,
    summary: HeadlineNativeSummary,
    review: Value,
    work_contexts: Vec<Value>,
    operation_bindings: Vec<Value>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct HeadlineNativeSummary {
    projection: String,
    counts: Value,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct HeadlineReport {
    schema_version: u32,
    kind: String,
    projection: String,
    #[serde(default)]
    projection_schema_version: Option<u32>,
    database_schema_version: u64,
    taxonomy_version: i64,
    scope: Scope,
    time_range: Option<TimeRange>,
    coverage: HeadlineCoverage,
    counts: Value,
    provider_tokens: Vec<Token>,
    provider_tokens_by_activity: Value,
    measured_execution_interval_sum_ms: Value,
    cost: Value,
    timing: Value,
    lifecycle: Value,
    context_size: Value,
    work_bindings: Value,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct HeadlineCoverage {
    #[serde(flatten)]
    tokens: Coverage,
    counts: String,
    cost: String,
    timing: String,
    lifecycle: String,
    context_size: String,
    work_bindings: String,
    activity: String,
}

impl HeadlineSummary {
    fn into_summary(self) -> Result<Summary, String> {
        let r = self.report;
        if r.projection != "token_headline"
            || r.projection_schema_version
                .is_some_and(|version| version != 1)
            || self.summary.projection != "token_headline"
            || !self.summary.counts.is_null()
            || !self.review.is_null()
            || !self.work_contexts.is_empty()
            || !self.operation_bindings.is_empty()
            || [
                r.counts,
                r.provider_tokens_by_activity,
                r.measured_execution_interval_sum_ms,
                r.cost,
                r.timing,
                r.lifecycle,
                r.context_size,
                r.work_bindings,
            ]
            .iter()
            .any(|value| !value.is_null())
            || [
                &r.coverage.counts,
                &r.coverage.cost,
                &r.coverage.timing,
                &r.coverage.lifecycle,
                &r.coverage.context_size,
                &r.coverage.work_bindings,
                &r.coverage.activity,
            ]
            .iter()
            .any(|value| value.as_str() != "deferred")
        {
            return Err("api_contract_unsupported".into());
        }
        let tokens = r.provider_tokens;
        Ok(Summary {
            contract_version: self.contract_version,
            source: self.source,
            generated_at: self.generated_at,
            snapshot: self.snapshot,
            report: Report {
                projection: Some(r.projection),
                schema_version: r.schema_version,
                kind: r.kind,
                database_schema_version: r.database_schema_version,
                taxonomy_version: r.taxonomy_version,
                scope: r.scope,
                time_range: r.time_range,
                coverage: r.coverage.tokens,
                counts: Counts::Deferred,
                provider_tokens: tokens,
                provider_tokens_by_activity: Vec::new(),
                measured_execution_interval_sum_ms: None,
                cost: None,
                worktrees: Vec::new(),
                worktree_coverage: None,
            },
            review: None,
        })
    }
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Snapshot {
    #[serde(alias = "sourceWatermark")]
    pub source_watermark: Option<String>,
    #[serde(alias = "generatedAt")]
    pub generated_at: u64,
    pub freshness: String,
    #[serde(alias = "refreshId")]
    pub refresh_id: Option<String>,
    #[serde(default)]
    pub generation: Option<u64>,
    #[serde(default)]
    pub watermark: Option<u64>,
    #[serde(default)]
    pub journal_watermark: Option<u64>,
    #[serde(default)]
    pub watermark_at_ms: Option<i64>,
    #[serde(default)]
    pub updated_at_ms: Option<u64>,
    #[serde(default)]
    pub pending_entries: Option<u64>,
    #[serde(default)]
    pub pending_corrections: Option<u64>,
}
#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Report {
    #[serde(default)]
    pub projection: Option<String>,
    pub schema_version: u32,
    pub kind: String,
    pub database_schema_version: u64,
    pub taxonomy_version: i64,
    pub scope: Scope,
    pub time_range: Option<TimeRange>,
    pub coverage: Coverage,
    pub counts: Counts,
    pub provider_tokens: Vec<Token>,
    pub provider_tokens_by_activity: Vec<Activity>,
    #[serde(default)]
    pub measured_execution_interval_sum_ms: Option<u64>,
    #[serde(default)]
    pub cost: Option<SourceCost>,
    #[serde(default)]
    #[allow(dead_code)]
    pub worktrees: Vec<WorktreeSummary>,
    #[serde(default)]
    #[allow(dead_code)]
    pub worktree_coverage: Option<WorktreeCoverage>,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct WorktreeSummary {
    #[allow(dead_code)]
    pub worktree_key: String,
    #[allow(dead_code)]
    pub repository_key: String,
    #[allow(dead_code)]
    pub label: String,
    #[allow(dead_code)]
    pub operation_count: u64,
    #[allow(dead_code)]
    pub model_request_count: u64,
    #[allow(dead_code)]
    pub tool_count: u64,
    #[allow(dead_code)]
    pub total_tokens: Option<u64>,
    #[allow(dead_code)]
    pub unknown_token_observations: u64,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct WorktreeCoverage {
    #[allow(dead_code)]
    pub assigned_operation_count: u64,
    #[allow(dead_code)]
    pub unassigned_operation_count: u64,
    #[allow(dead_code)]
    pub assigned_token_observation_count: u64,
    #[allow(dead_code)]
    pub unassigned_token_observation_count: u64,
}

#[derive(Clone)]
pub(super) struct SourceCost {
    pub basis: String,
    pub currency: String,
    pub status: String,
    pub processing_tier: String,
    pub estimated_usd_micros: Option<u64>,
    pub input_usd_micros: Option<u64>,
    pub cached_input_usd_micros: Option<u64>,
    pub cache_write_usd_micros: Option<u64>,
    pub output_usd_micros: Option<u64>,
    pub input_tokens: i64,
    pub uncached_input_tokens: i64,
    pub cached_input_tokens: i64,
    pub cache_write_tokens: u64,
    pub output_tokens: i64,
    pub reasoning_tokens: i64,
    pub provider_total_tokens: u64,
    pub priced_observations: u64,
    pub unknown_observations: u64,
    pub rate_card_refs: Vec<String>,
    pub unavailable_reasons: BTreeMap<String, u64>,
}

impl SourceCost {
    fn usage_cost(&self) -> UsageCost {
        UsageCost {
            status: self.status.clone(),
            basis: self.basis.clone(),
            currency: self.currency.clone(),
            processing_tier: self.processing_tier.clone(),
            estimated_usd: self
                .estimated_usd_micros
                .map(|v| format!("{}.{:06}", v / 1_000_000, v % 1_000_000)),
            estimated_usd_micros: self.estimated_usd_micros,
            input_usd_micros: self.input_usd_micros,
            cached_input_usd_micros: self.cached_input_usd_micros,
            cache_write_usd_micros: self.cache_write_usd_micros,
            output_usd_micros: self.output_usd_micros,
            input_tokens: Some(self.input_tokens),
            cached_input_tokens: Some(self.cached_input_tokens),
            cache_write_tokens: Some(self.cache_write_tokens),
            uncached_input_tokens: Some(self.uncached_input_tokens),
            output_tokens: Some(self.output_tokens),
            reasoning_tokens: Some(self.reasoning_tokens),
            unknown_requests: None,
            unknown_tokens: 0,
            unknown_observations: Some(self.unknown_observations),
            rate_card_ref: self.rate_card_refs.first().cloned(),
            rate_card_refs: self.rate_card_refs.clone(),
            model_requests: None,
            priced_requests: None,
            priced_observations: Some(self.priced_observations),
            unavailable_reasons: self.unavailable_reasons.clone(),
            matched_rate_cards: Vec::new(),
        }
    }
}
#[derive(Clone, Deserialize)]
pub(super) struct Scope {
    #[serde(rename = "type")]
    pub kind: String,
    pub id: Option<String>,
}
#[derive(Clone, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(super) struct TimeRange {
    pub start_ms: u64,
    pub end_ms: u64,
}
#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Coverage {
    pub state: String,
    pub has_gaps: bool,
}
#[derive(Clone)]
pub(super) enum Counts {
    Measured(MeasuredCounts),
    Deferred,
}

impl<'de> Deserialize<'de> for Counts {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        MeasuredCounts::deserialize(deserializer).map(Self::Measured)
    }
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct MeasuredCounts {
    pub operations: u64,
    pub model_requests: u64,
    pub tools: u64,
}
#[derive(Clone, Deserialize)]
#[serde(try_from = "WireToken")]
pub(super) struct Token {
    pub category: String,
    pub repository_bucket: String,
    pub measurement_provenance: String,
    pub measured_tokens: i128,
    pub exact_tokens: Option<i128>,
    pub unknown_observations: u64,
    pub observation_count: u64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireToken {
    category: String,
    repository_bucket: String,
    measurement_provenance: String,
    measured_tokens: i128,
    exact_tokens: Option<i128>,
    unknown_observations: u64,
    observation_count: u64,
}

pub(super) fn signed_component(category: &str) -> bool {
    matches!(
        category,
        "input_tokens"
            | "input_tokens_details.cached_tokens"
            | "output_tokens"
            | "output_tokens_details.reasoning_tokens"
    )
}

impl TryFrom<WireToken> for Token {
    type Error = &'static str;
    fn try_from(value: WireToken) -> Result<Self, Self::Error> {
        let permitted = |quantity| {
            if signed_component(&value.category) {
                i64::try_from(quantity).is_ok()
            } else {
                u64::try_from(quantity).is_ok()
            }
        };
        if !permitted(value.measured_tokens)
            || value
                .exact_tokens
                .is_some_and(|quantity| !permitted(quantity))
        {
            return Err("token_quantity_invalid");
        }
        let category = if value.category == "cache_write_input_tokens" {
            "input_tokens_details.cache_write_tokens".into()
        } else {
            value.category
        };
        Ok(Self {
            category,
            repository_bucket: value.repository_bucket,
            measurement_provenance: value.measurement_provenance,
            measured_tokens: value.measured_tokens,
            exact_tokens: value.exact_tokens,
            unknown_observations: value.unknown_observations,
            observation_count: value.observation_count,
        })
    }
}
#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Activity {
    pub phase: String,
    pub activity: String,
    pub attribution_provenance: String,
    pub measured_tokens: u64,
    pub exact_tokens: Option<u64>,
    pub unknown_observations: u64,
}

impl CollectorApi {
    #[expect(
        clippy::too_many_arguments,
        reason = "Headline requests retain explicit exact repository scope"
    )]
    pub(super) fn summary_headline(
        &self,
        source: &CodexUsageSource,
        repository: Option<&str>,
        worktree_keys: Option<&[String]>,
        include_unassigned: bool,
        start: u64,
        end: u64,
        deadline: Instant,
    ) -> Result<SummaryRead, String> {
        self.summary_with_projection(
            source,
            repository,
            worktree_keys,
            include_unassigned,
            None,
            &[],
            start,
            end,
            deadline,
            QueryProjection::Headline,
        )
    }
    #[expect(
        clippy::too_many_arguments,
        reason = "Compatibility wrapper preserves the existing collector call shape"
    )]
    pub(super) fn summary(
        &self,
        source: &CodexUsageSource,
        repository: Option<&str>,
        worktree_keys: Option<&[String]>,
        include_unassigned: bool,
        start: u64,
        end: u64,
        deadline: Instant,
    ) -> Result<SummaryRead, String> {
        self.summary_with_workstream(
            source,
            repository,
            worktree_keys,
            include_unassigned,
            None,
            &[],
            start,
            end,
            deadline,
        )
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "This external collector API keeps its explicit scope, workstream, and time bounds"
    )]
    pub(super) fn summary_with_workstream(
        &self,
        source: &CodexUsageSource,
        repository: Option<&str>,
        worktree_keys: Option<&[String]>,
        include_unassigned: bool,
        workstream: Option<&str>,
        bindings: &[crate::usage_context::Binding],
        start: u64,
        end: u64,
        deadline: Instant,
    ) -> Result<SummaryRead, String> {
        self.summary_with_projection(
            source,
            repository,
            worktree_keys,
            include_unassigned,
            workstream,
            bindings,
            start,
            end,
            deadline,
            QueryProjection::Rich,
        )
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "Projection selection adds no ambiguity to exact source scope"
    )]
    fn summary_with_projection(
        &self,
        source: &CodexUsageSource,
        repository: Option<&str>,
        worktree_keys: Option<&[String]>,
        include_unassigned: bool,
        workstream: Option<&str>,
        bindings: &[crate::usage_context::Binding],
        start: u64,
        end: u64,
        deadline: Instant,
        projection: QueryProjection,
    ) -> Result<SummaryRead, String> {
        let deadline = deadline.min(Instant::now() + API_BUDGET);
        let path = source.api_socket.as_ref().ok_or("api_not_configured")?;
        let key = projection_key(
            full_query_key(
                source,
                repository,
                worktree_keys,
                include_unassigned,
                workstream,
                bindings,
                start,
                end,
            )?,
            projection,
        );
        let profile = projection_key(
            profile_key(
                source,
                repository,
                worktree_keys,
                include_unassigned,
                workstream,
                bindings,
                start,
                end,
            )?,
            projection,
        );
        if bindings.len() > crate::usage_context::MAX_BINDINGS {
            return Err("api_contract_unsupported".into());
        }
        let request_bytes = serde_json::to_vec(bindings)
            .map_err(|_| "work_context_unavailable")?
            .len()
            .saturating_add(key.len())
            .saturating_add(source.codex_home.as_os_str().len());
        let source_identity = socket_identity(path);
        let (guard, changed) = &*self.0;
        let mut state = guard.lock().map_err(|_| "api_busy")?;
        let mut waited_for_source = false;
        loop {
            state
                .failed_until
                .retain(|_, (until, _)| *until > Instant::now());
            state
                .cold_waiters
                .retain(|_, waiter| waiter.touched.elapsed() < COLD_WAITER_FOR);
            if let Some(entry) = state
                .entries
                .get(&key)
                .filter(|entry| entry.loaded.elapsed() < FRESH_FOR)
            {
                return Ok(SummaryRead {
                    summary: entry.report.clone(),
                    refresh_reason: None,
                    loading: None,
                });
            }
            let saved = state.entries.get(&key).map(|entry| entry.report.clone());
            if let Some((_, reason)) = state.failed_until.get(&key) {
                return SummaryRead::cached_or_error(saved, reason);
            }
            if !state.inflight.contains_key(&key) {
                let known_headlines = state.supports_headlines(path, source_identity);
                let same_source = state
                    .inflight
                    .values()
                    .filter(|flight| &flight.source == path)
                    .collect::<Vec<_>>();
                let can_share_compact = known_headlines == Some(true)
                    && same_source.len() == 1
                    && ((projection == QueryProjection::Headline
                        && same_source[0].projection == QueryProjection::Rich)
                        || (projection == QueryProjection::Rich
                            && same_source[0].projection == QueryProjection::Headline));
                if !same_source.is_empty() && !can_share_compact {
                    if let Some(saved) = saved {
                        return SummaryRead::cached_or_error(Some(saved), "api_source_busy");
                    }
                    // Remember only bounded admission metadata, not another
                    // worker or query payload. A cold foreground continuation
                    // may claim the existing slot after this read completes.
                    #[cfg(test)]
                    let observer_notification = !state.cold_waiters.contains_key(&key);
                    state.remember_cold_waiter(&key, path);
                    #[cfg(test)]
                    if observer_notification {
                        changed.notify_all();
                    }
                    let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
                        return Err("api_source_busy".into());
                    };
                    waited_for_source = true;
                    let (updated, _) = changed
                        .wait_timeout(state, remaining)
                        .map_err(|_| "api_busy")?;
                    state = updated;
                    // Completion or another waiter may have changed this exact
                    // key. Recheck cache, failures and ownership under the lock.
                    continue;
                }
                if waited_for_source && Instant::now() >= deadline {
                    return SummaryRead::cached_or_error(saved, "api_source_busy");
                }
                if saved.is_some()
                    && state
                        .cold_waiters
                        .iter()
                        .any(|(waiting_key, waiter)| waiting_key != &key && &waiter.source == path)
                {
                    // A repeated stale read already has useful measurements.
                    // Give a still-polling cold query a turn before revalidating
                    // those measurements again, without borrowing its scope.
                    return SummaryRead::cached_or_error(saved, "api_source_busy");
                }
                state.cold_waiters.remove(&key);
                let inflight_bytes = state.pending_bytes();
                if state.inflight.len() >= MAX_CACHED
                    || request_bytes > MAX_INFLIGHT_BYTES.saturating_sub(inflight_bytes)
                {
                    return SummaryRead::cached_or_error(saved, "api_busy");
                }
                state.inflight.insert(
                    key.clone(),
                    Flight {
                        source: path.clone(),
                        bytes: request_bytes,
                        started: Instant::now(),
                        profile: profile.clone(),
                        projection,
                    },
                );
                let cache = self.clone();
                let request_key = key.clone();
                let source = source.clone();
                let capability_source = source.api_socket.clone();
                let repository = repository.map(str::to_owned);
                let worktree_keys = worktree_keys.map(<[String]>::to_vec);
                let workstream = workstream.map(str::to_owned);
                let bindings = bindings.to_vec();
                let capability_cache = cache.clone();
                let capability_key = request_key.clone();
                drop(state);
                // A bounded source read may outlive this foreground request. It
                // remains owned by this exact-query cache until completion; another
                // request for the same source cannot launch a competing cold read.
                let spawned = std::thread::Builder::new()
                    .name("usage-api-refresh".into())
                    .spawn(move || {
                        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                            fetch(
                                &source,
                                SummaryQuery {
                                    repository: repository.as_deref(),
                                    worktree_keys: worktree_keys.as_deref(),
                                    include_unassigned,
                                    workstream: workstream.as_deref(),
                                    bindings: &bindings,
                                    start,
                                    end,
                                    projection,
                                },
                                move |supports| {
                                    capability_cache.note_headline_capability(
                                        &capability_key,
                                        capability_source.as_deref(),
                                        supports,
                                    )
                                },
                                Instant::now() + SOURCE_READ_BUDGET,
                            )
                        }))
                        .unwrap_or_else(|_| Err("api_refresh_failed".into()));
                        cache.finish(request_key, result);
                    });
                if spawned.is_err() {
                    self.finish(key.clone(), Err("api_refresh_failed".into()));
                }
                state = guard.lock().map_err(|_| "api_busy")?;
            }
            if let Some(entry) = state
                .entries
                .get(&key)
                .filter(|entry| entry.loaded.elapsed() < FRESH_FOR)
            {
                return Ok(SummaryRead {
                    summary: entry.report.clone(),
                    refresh_reason: None,
                    loading: None,
                });
            }
            if let Some((_, reason)) = state.failed_until.get(&key) {
                return SummaryRead::cached_or_error(saved, reason);
            }
            // Freshness triggers one revalidation; it does not erase a
            // completed exact-query observation before its caller can read it.
            if let Some(saved) = saved {
                return SummaryRead::cached_with_loading(
                    saved,
                    "api_refresh_pending",
                    loading_from_state(&state, &key, &profile, Instant::now()),
                );
            }
            let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
                return Err("api_refresh_pending".into());
            };
            if remaining.is_zero() {
                return Err("api_refresh_pending".into());
            }
            let (updated, _) = changed
                .wait_timeout(state, remaining)
                .map_err(|_| "api_busy")?;
            state = updated;
        }
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "Loading metadata uses the same exact scope as the source request"
    )]
    pub(super) fn loading_for(
        &self,
        source: &CodexUsageSource,
        repository: Option<&str>,
        worktree_keys: Option<&[String]>,
        include_unassigned: bool,
        workstream: Option<&str>,
        bindings: &[crate::usage_context::Binding],
        start: u64,
        end: u64,
    ) -> Option<UsageLoading> {
        self.loading_for_projection(
            source,
            repository,
            worktree_keys,
            include_unassigned,
            workstream,
            bindings,
            start,
            end,
            QueryProjection::Rich,
        )
    }

    pub(super) fn headline_loading_for(
        &self,
        source: &CodexUsageSource,
        repository: Option<&str>,
        worktree_keys: Option<&[String]>,
        include_unassigned: bool,
        start: u64,
        end: u64,
    ) -> Option<UsageLoading> {
        self.loading_for_projection(
            source,
            repository,
            worktree_keys,
            include_unassigned,
            None,
            &[],
            start,
            end,
            QueryProjection::Headline,
        )
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "Loading scope mirrors the selected cached projection"
    )]
    fn loading_for_projection(
        &self,
        source: &CodexUsageSource,
        repository: Option<&str>,
        worktree_keys: Option<&[String]>,
        include_unassigned: bool,
        workstream: Option<&str>,
        bindings: &[crate::usage_context::Binding],
        start: u64,
        end: u64,
        projection: QueryProjection,
    ) -> Option<UsageLoading> {
        let key = projection_key(
            full_query_key(
                source,
                repository,
                worktree_keys,
                include_unassigned,
                workstream,
                bindings,
                start,
                end,
            )
            .ok()?,
            projection,
        );
        let profile = projection_key(
            profile_key(
                source,
                repository,
                worktree_keys,
                include_unassigned,
                workstream,
                bindings,
                start,
                end,
            )
            .ok()?,
            projection,
        );
        let (state, _) = &*self.0;
        let state = state.lock().ok()?;
        loading_from_state(&state, &key, &profile, Instant::now())
    }

    /// Publish the negotiated capability as soon as the handshake completes.
    /// This lets one compact read share the source with one already-running
    /// rich read; a false or changed capability stays on the ordinary lane.
    fn note_headline_capability(&self, key: &str, path: Option<&Path>, supports: bool) -> bool {
        let (guard, changed) = &*self.0;
        let mut state = guard
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let shared_rich = path.is_some_and(|path| {
            state.inflight.iter().any(|(other_key, flight)| {
                other_key != key
                    && flight.source == path
                    && flight.projection == QueryProjection::Rich
            })
        });
        if let Some(path) = path
            && let Some(identity) = socket_identity(path)
        {
            if !state.headline_capabilities.contains_key(path)
                && state.headline_capabilities.len() >= MAX_CACHED
                && let Some(evicted) = state.headline_capabilities.keys().next().cloned()
            {
                state.headline_capabilities.remove(&evicted);
            }
            state
                .headline_capabilities
                .insert(path.to_owned(), HeadlineCapability { identity, supports });
        }
        changed.notify_all();
        shared_rich
    }

    fn finish(&self, key: String, result: Result<(Summary, usize), String>) {
        let (guard, changed) = &*self.0;
        let mut state = guard
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let flight = state.inflight.remove(&key);
        if let (Some(flight), Ok((summary, _))) = (&flight, &result)
            && (flight.projection == QueryProjection::Rich
                || summary.report.projection.as_deref() == Some("token_headline"))
        {
            let duration_ms =
                u64::try_from(flight.started.elapsed().as_millis()).unwrap_or(u64::MAX);
            state.record_duration(flight.profile.clone(), duration_ms);
        }
        match result {
            Ok((report, bytes)) if bytes <= MAX_CACHED_BYTES => {
                state.failed_until.remove(&key);
                while state.entries.len() >= MAX_CACHED
                    || state
                        .entries
                        .values()
                        .map(|entry| entry.bytes)
                        .sum::<usize>()
                        .saturating_add(bytes)
                        > MAX_CACHED_BYTES
                {
                    let oldest = state
                        .entries
                        .iter()
                        .min_by_key(|(_, entry)| entry.loaded)
                        .map(|(key, _)| key.clone());
                    if let Some(oldest) = oldest {
                        state.entries.remove(&oldest);
                    } else {
                        break;
                    }
                }
                state.entries.insert(
                    key,
                    Entry {
                        report: Arc::new(report),
                        loaded: Instant::now(),
                        bytes,
                    },
                );
            }
            result => {
                let reason = result
                    .err()
                    .unwrap_or_else(|| "api_response_too_large".into());
                let retry = if reason.ends_with("_timeout") || reason == "query_budget_exhausted" {
                    Duration::from_secs(1)
                } else {
                    RETRY_AFTER
                };
                if state.failed_until.len() >= MAX_CACHED
                    && let Some(oldest) = state
                        .failed_until
                        .iter()
                        .min_by_key(|(_, (until, _))| until)
                        .map(|(key, _)| key.clone())
                {
                    state.failed_until.remove(&oldest);
                }
                state
                    .failed_until
                    .insert(key, (Instant::now() + retry, reason));
            }
        }
        changed.notify_all();
    }

    #[cfg(test)]
    pub(super) fn expire(&self) {
        let mut state = self.0.0.lock().unwrap();
        for entry in state.entries.values_mut() {
            entry.loaded = Instant::now() - FRESH_FOR;
        }
        for (until, _) in state.failed_until.values_mut() {
            *until = Instant::now();
        }
        for waiter in state.cold_waiters.values_mut() {
            waiter.touched = Instant::now() - COLD_WAITER_FOR;
        }
    }

    #[cfg(test)]
    pub(super) fn wait_until_idle(&self, deadline: Instant) -> bool {
        let (guard, changed) = &*self.0;
        let mut state = guard.lock().unwrap();
        while !state.inflight.is_empty() {
            let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
                return false;
            };
            let (updated, waited) = changed.wait_timeout(state, remaining).unwrap();
            state = updated;
            if waited.timed_out() && !state.inflight.is_empty() {
                return false;
            }
        }
        true
    }
}

struct SummaryQuery<'a> {
    repository: Option<&'a str>,
    worktree_keys: Option<&'a [String]>,
    include_unassigned: bool,
    workstream: Option<&'a str>,
    bindings: &'a [crate::usage_context::Binding],
    start: u64,
    end: u64,
    projection: QueryProjection,
}

fn fetch(
    source: &CodexUsageSource,
    query: SummaryQuery<'_>,
    note_capability: impl FnOnce(bool) -> bool,
    deadline: Instant,
) -> Result<(Summary, usize), String> {
    let SummaryQuery {
        repository,
        worktree_keys,
        include_unassigned,
        workstream,
        bindings,
        start,
        end,
        projection,
    } = query;
    let (mut socket, supports_headlines) = connect(source, deadline)?;
    let rich_read_is_occupied = note_capability(supports_headlines);
    if projection == QueryProjection::Headline && !supports_headlines && rich_read_is_occupied {
        // A changed or false capability cannot silently turn the compact lane
        // into a second rich query. The next ordinary retry will use the
        // single heavy lane after the existing read completes.
        return Err("api_contract_unsupported".into());
    }
    let actual_projection = if projection == QueryProjection::Headline && supports_headlines {
        QueryProjection::Headline
    } else {
        QueryProjection::Rich
    };
    let value = exchange(
        &mut socket,
        2,
        "localUsage/summary",
        json!({"contractVersion":1,"repositoryKey":repository,"worktreeKeys":worktree_keys,"includeUnassigned":include_unassigned,"workstreamId":workstream,"fromAt":start,"toAt":end,"includeReview":actual_projection == QueryProjection::Rich,"workContextBindings":bindings}),
        deadline,
    )?;
    let bytes = serde_json::to_vec(&value)
        .map_err(|_| "api_contract_unsupported")?
        .len();
    let summary: Summary = match actual_projection {
        QueryProjection::Rich => {
            serde_json::from_value(value).map_err(|_| "api_contract_unsupported")?
        }
        QueryProjection::Headline => serde_json::from_value::<HeadlineSummary>(value)
            .map_err(|_| "api_contract_unsupported")?
            .into_summary()?,
    };
    match actual_projection {
        QueryProjection::Rich => validate(&summary, repository, start, end)?,
        QueryProjection::Headline => {
            validate_projection(&summary, repository, start, end, actual_projection)?
        }
    }
    // Dropping the connection stops response delivery. The producer keeps its
    // bounded read isolated in its local worker; this consumer does not claim
    // that transport closure cancels that worker synchronously.
    Ok((summary, bytes.saturating_mul(4)))
}

fn connect(
    source: &CodexUsageSource,
    deadline: Instant,
) -> Result<(WebSocket<DeadlineStream>, bool), String> {
    let path = source.api_socket.as_ref().ok_or("api_not_configured")?;
    let metadata = path.metadata().map_err(|_| "api_socket_unavailable")?;
    if !metadata.file_type().is_socket() || metadata.uid() != source.uid {
        return Err("api_socket_identity_mismatch".into());
    }
    let budget = deadline
        .checked_duration_since(Instant::now())
        .ok_or("query_budget_exhausted")?;
    // Connect has a deadline even when the server's accept queue is full.
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_io()
        .enable_time()
        .build()
        .map_err(|_| "api_unavailable")?;
    let stream = runtime.block_on(async {
        tokio::time::timeout(budget, tokio::net::UnixStream::connect(path))
            .await
            .map_err(|_| "query_budget_exhausted")?
            .map_err(|_| "api_unavailable")?
            .into_std()
            .map_err(|_| "api_unavailable")
    })?;
    stream
        .set_nonblocking(false)
        .map_err(|_| "api_unavailable")?;
    // Bind the configured same-owner source to the actual peer, including a replaced socket.
    if peer_uid(&stream)? != source.uid {
        return Err("api_peer_identity_mismatch".into());
    }
    set_deadline(&stream, deadline)?;
    let config = WebSocketConfig::default()
        .max_message_size(Some(MAX_RESPONSE))
        .max_frame_size(Some(MAX_RESPONSE));
    let (mut socket, _) = tungstenite::client::client_with_config(
        "ws://localhost/rpc",
        DeadlineStream {
            socket: stream,
            deadline,
        },
        Some(config),
    )
    .map_err(|error| match error {
        tungstenite::HandshakeError::Failure(error) => transport_error(&error, "handshake"),
        tungstenite::HandshakeError::Interrupted(_) => "api_handshake_incomplete".into(),
    })?;
    let initialize = exchange(
        &mut socket,
        1,
        "initialize",
        json!({"clientInfo":{"name":"devcoordinator2-usage","version":env!("CARGO_PKG_VERSION")},"capabilities":{"experimentalApi":true}}),
        deadline,
    )?;
    // Do not route a configured source to another account's daemon.
    if initialize.get("codexHome").and_then(Value::as_str) != source.codex_home.to_str() {
        return Err("api_source_identity_mismatch".into());
    }
    validate_capability(&initialize)?;
    let supports_headlines = initialize
        .get("localUsageAccounting")
        .and_then(|capability| capability.get("supportsTokenHeadlines"))
        .and_then(Value::as_bool)
        == Some(true);
    socket
        .send(Message::Text(
            json!({"method":"initialized"}).to_string().into(),
        ))
        .map_err(|_| "api_unavailable")?;
    Ok((socket, supports_headlines))
}

fn validate_capability(initialize: &Value) -> Result<(), String> {
    let cap = initialize
        .get("localUsageAccounting")
        .ok_or("api_contract_unsupported")?;
    let methods = cap
        .get("methods")
        .and_then(Value::as_array)
        .ok_or("api_contract_unsupported")?;
    if cap.get("version").and_then(Value::as_u64) != Some(1)
        || !cap
            .get("supportedVersions")
            .and_then(Value::as_array)
            .is_some_and(|v| v.iter().any(|v| v.as_u64() == Some(1)))
        || cap.get("storage").and_then(Value::as_str) != Some("usage-v2")
        || cap
            .get("supportsWorkContextBindings")
            .and_then(Value::as_bool)
            != Some(true)
        || cap
            .get("supportsWorkstreamOutcomes")
            .and_then(Value::as_bool)
            != Some(true)
        || ["localUsage/summary", "localUsage/repositoryIdentity"]
            .iter()
            .any(|name| !methods.iter().any(|m| m.as_str() == Some(name)))
    {
        return Err("api_contract_unsupported".into());
    }
    Ok(())
}

pub(super) fn repository_identity(
    source: &CodexUsageSource,
    path: &Path,
    deadline: Instant,
) -> Result<String, String> {
    let (mut socket, _) = connect(source, deadline)?;
    let value = exchange(
        &mut socket,
        2,
        "localUsage/repositoryIdentity",
        json!({"contractVersion":1,"path":path}),
        deadline,
    )?;
    if value.get("contractVersion").and_then(Value::as_u64) != Some(1)
        || value.get("source").and_then(Value::as_str) != Some("usage-v2")
        || value.get("formatVersion").and_then(Value::as_u64) != Some(3)
    {
        return Err("api_contract_unsupported".into());
    }
    value
        .get("repositoryId")
        .and_then(Value::as_str)
        .filter(|key| super::valid_repository_key(key))
        .map(str::to_owned)
        .ok_or_else(|| "mapping_unavailable".into())
}

#[cfg(target_os = "linux")]
fn peer_uid(stream: &UnixStream) -> Result<u32, String> {
    let mut credentials: libc::ucred = unsafe { std::mem::zeroed() };
    let mut size = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    let result = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            (&mut credentials as *mut libc::ucred).cast(),
            &mut size,
        )
    };
    if result != 0 {
        return Err("api_unavailable".into());
    }
    Ok(credentials.uid)
}

#[cfg(target_vendor = "apple")]
fn peer_uid(stream: &UnixStream) -> Result<u32, String> {
    let mut uid = 0;
    let mut gid = 0;
    let result = unsafe { libc::getpeereid(stream.as_raw_fd(), &mut uid, &mut gid) };
    if result != 0 {
        return Err("api_unavailable".into());
    }
    u32::try_from(uid).map_err(|_| "api_unavailable".to_owned())
}

#[cfg(all(unix, not(any(target_os = "linux", target_vendor = "apple"))))]
fn peer_uid(_stream: &UnixStream) -> Result<u32, String> {
    Err("api_unavailable".into())
}

fn set_deadline(stream: &UnixStream, deadline: Instant) -> Result<(), String> {
    let remaining = deadline
        .checked_duration_since(Instant::now())
        .filter(|d| !d.is_zero())
        .ok_or("query_budget_exhausted")?;
    stream
        .set_read_timeout(Some(remaining))
        .and_then(|_| stream.set_write_timeout(Some(remaining)))
        .map_err(|_| "api_unavailable".into())
}
fn transport_error(error: &tungstenite::Error, stage: &str) -> String {
    if matches!(error, tungstenite::Error::Io(io) if matches!(io.kind(),io::ErrorKind::TimedOut|io::ErrorKind::WouldBlock))
    {
        format!("api_{stage}_timeout")
    } else {
        format!("api_{stage}_failed")
    }
}

fn exchange(
    socket: &mut WebSocket<DeadlineStream>,
    id: u64,
    method: &str,
    params: Value,
    deadline: Instant,
) -> Result<Value, String> {
    let stage = match method {
        "initialize" => "initialize",
        "localUsage/repositoryIdentity" => "identity",
        _ => "summary",
    };
    set_deadline(socket.get_ref(), deadline)?;
    socket
        .send(Message::Text(
            json!({"id":id,"method":method,"params":params})
                .to_string()
                .into(),
        ))
        .map_err(|error| transport_error(&error, stage))?;
    for _ in 0..64 {
        set_deadline(socket.get_ref(), deadline)?;
        match socket
            .read()
            .map_err(|error| transport_error(&error, stage))?
        {
            Message::Text(text) => {
                let response: Value =
                    serde_json::from_str(&text).map_err(|_| "api_contract_unsupported")?;
                if response.get("id").and_then(Value::as_u64) == Some(id) {
                    return match (response.get("result"), response.get("error")) {
                        (Some(result), None) => Ok(result.clone()),
                        (None, Some(error)) => {
                            // Only fixed numeric categories cross this boundary.
                            // Upstream message/data may contain private source
                            // details and must never enter reports or logs.
                            let reason = match error.get("code").and_then(Value::as_i64) {
                                Some(-32603) => "api_source_unavailable",
                                Some(-32601) => "api_contract_unsupported",
                                Some(-32602) => "api_params_invalid",
                                Some(-32600 | -32700) => "api_request_invalid",
                                Some(_) => "api_source_error",
                                None => "api_contract_unsupported",
                            };
                            Err(reason.into())
                        }
                        _ => Err("api_contract_unsupported".into()),
                    };
                }
                // Ignore unrelated notifications; never copy raw content or errors into results.
            }
            Message::Ping(_) | Message::Pong(_) => {}
            _ => return Err("api_contract_unsupported".into()),
        }
    }
    Err("api_contract_unsupported".into())
}
fn validate(
    summary: &Summary,
    repository: Option<&str>,
    start: u64,
    end: u64,
) -> Result<(), String> {
    validate_projection(summary, repository, start, end, QueryProjection::Rich)
}

fn validate_projection(
    summary: &Summary,
    repository: Option<&str>,
    start: u64,
    end: u64,
    projection: QueryProjection,
) -> Result<(), String> {
    if summary.contract_version != Some(1)
        || summary.source.as_ref().and_then(Value::as_str) != Some("usage-v2")
        || summary.snapshot.is_none()
        || match projection {
            QueryProjection::Rich => {
                summary.review.is_none()
                    || summary.report.projection.is_some()
                    || matches!(summary.report.counts, Counts::Deferred)
            }
            QueryProjection::Headline => {
                summary.review.is_some()
                    || summary.report.projection.as_deref() != Some("token_headline")
                    || !matches!(summary.report.counts, Counts::Deferred)
            }
        }
    {
        return Err("api_contract_unsupported".into());
    }
    let r = &summary.report;
    if summary.source.is_some() && r.database_schema_version != 3 {
        return Err("api_contract_unsupported".into());
    }
    if projection == QueryProjection::Headline
        && r.provider_tokens.iter().any(|token| {
            token.repository_bucket != repository.unwrap_or("all")
                || token.measurement_provenance != "provider_reported"
                || token.exact_tokens.is_some_and(|value| {
                    value != token.measured_tokens || token.unknown_observations != 0
                })
                || token.unknown_observations > token.observation_count
        })
    {
        return Err("api_contract_unsupported".into());
    }
    if r.schema_version != 1
        || r.taxonomy_version != i64::from(SUPPORTED_TAXONOMY)
        || r.kind != "usageSummary"
        || r.time_range.as_ref()
            != Some(&TimeRange {
                start_ms: start,
                end_ms: end,
            })
        || r.scope.kind
            != if repository.is_some() {
                "repository"
            } else {
                "all"
            }
        || r.scope.id.as_deref() != repository
        || !matches!(
            r.coverage.state.as_str(),
            "complete" | "partial" | "unknown" | "unobserved" | "unavailable"
        )
    {
        return Err("api_contract_unsupported".into());
    }
    let snapshot = summary
        .snapshot
        .as_ref()
        .ok_or("api_contract_unsupported")?;
    if !matches!(
        snapshot.freshness.as_str(),
        "unknown" | "fresh" | "stale" | "refreshing" | "failed"
    ) || snapshot.source_watermark.as_ref().is_none_or(|s| {
        s.len() > 256 || !s.starts_with("usage-v2:") || s.chars().any(char::is_control)
    }) || snapshot.generation.is_none()
        || snapshot.watermark.is_none()
        || snapshot.journal_watermark.is_none()
        || snapshot.watermark_at_ms.is_none()
        || snapshot.updated_at_ms.is_none()
        || snapshot.pending_entries.is_none()
        || snapshot.pending_corrections.is_none()
        || snapshot
            .refresh_id
            .as_ref()
            .is_some_and(|s| s.len() > 256 || s.chars().any(char::is_control))
    {
        return Err("api_contract_unsupported".into());
    }
    if snapshot.journal_watermark < snapshot.watermark
        || (snapshot.generation == Some(0) && snapshot.watermark != Some(0))
        || snapshot
            .watermark_at_ms
            .is_some_and(|at| at < 0 || at as u64 > snapshot.generated_at)
        || snapshot
            .updated_at_ms
            .is_some_and(|at| at > snapshot.generated_at)
        || snapshot.generated_at > summary.generated_at
    {
        return Err("api_snapshot_inconsistent".into());
    }
    if let Some(cost) = &summary.report.cost {
        let provider_total = summary
            .report
            .provider_tokens
            .iter()
            .filter(|token| token.category == "total_tokens")
            .try_fold(0u64, |total, token| {
                total.checked_add(u64::try_from(token.measured_tokens).ok()?)
            });
        let components = [
            cost.input_usd_micros,
            cost.cached_input_usd_micros,
            cost.cache_write_usd_micros,
            cost.output_usd_micros,
        ];
        let total = components
            .iter()
            .flatten()
            .try_fold(0u64, |sum, v| sum.checked_add(*v));
        if cost.basis != "api_equivalent"
            || cost.currency != "USD"
            || !matches!(cost.processing_tier.as_str(), "standard" | "mixed")
            || !matches!(cost.status.as_str(), "complete" | "partial" | "unavailable")
            || (cost.status == "complete"
                && (components.iter().any(Option::is_none)
                    || total
                        .zip(cost.estimated_usd_micros)
                        .is_none_or(|(parts, total)| total < parts || total - parts > 3)
                    || Some(cost.provider_total_tokens) != provider_total
                    || cost.rate_card_refs.is_empty()))
            || cost
                .rate_card_refs
                .iter()
                .any(|r| r.len() > 128 || r.chars().any(char::is_control))
            || cost.unavailable_reasons.len() > 64
            || cost.unavailable_reasons.keys().any(|reason| {
                reason.is_empty()
                    || reason.len() > 64
                    || !reason
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
            })
        {
            return Err("api_contract_unsupported".into());
        }
    }
    Ok(())
}

impl Summary {
    pub(super) fn source_report(
        &self,
        repository: Option<&str>,
        buckets: usize,
        worktree_keys: Option<&[String]>,
        include_unassigned: bool,
    ) -> SourceReport {
        let r = &self.report;
        let headline = matches!(r.counts, Counts::Deferred);
        let source_partial = self.snapshot.as_ref().is_some_and(|snapshot| {
            snapshot.freshness != "fresh"
                || snapshot.pending_entries.unwrap_or_default() != 0
                || snapshot.pending_corrections.unwrap_or_default() != 0
        });
        let partial =
            r.coverage.has_gaps || r.coverage.state != "complete" || (!headline && source_partial);
        let mut result = SourceReport {
            supplied_execution_wall: self
                .review
                .as_ref()
                .map(|review| review.time.execution_wall.clone()),
            supplied_agent_active: self
                .review
                .as_ref()
                .map(|review| review.time.summed_agent_active.clone()),
            source_snapshot: self.snapshot.as_ref().map(|s| {
                devcoordinator2_api::results::UsageSourceSnapshot {
                    source: "usage-v2".into(),
                    contract_version: 1,
                    format_version: 3,
                    source_watermark: s.source_watermark.clone().unwrap_or_default(),
                    generated_at_ms: s.generated_at,
                    generation: s.generation.unwrap_or_default(),
                    watermark: s.watermark.unwrap_or_default(),
                    journal_watermark: s.journal_watermark.unwrap_or_default(),
                    watermark_at_ms: s
                        .watermark_at_ms
                        .and_then(|v| u64::try_from(v).ok())
                        .unwrap_or_default(),
                    updated_at_ms: s.updated_at_ms.unwrap_or_default(),
                    freshness: s.freshness.clone(),
                    pending_entries: s.pending_entries.unwrap_or_default(),
                    pending_corrections: s.pending_corrections.unwrap_or_default(),
                    refresh_id: s.refresh_id.clone(),
                    measured_execution_interval_sum_ms: r.measured_execution_interval_sum_ms,
                }
            }),
            supplied_cost: r.cost.as_ref().map(SourceCost::usage_cost),
            snapshot: Some(devcoordinator2_api::results::UsageSnapshot {
                updated_at_ms: self
                    .snapshot
                    .as_ref()
                    .and_then(|s| {
                        s.updated_at_ms
                            .or_else(|| s.watermark_at_ms.and_then(|at| u64::try_from(at).ok()))
                            .or(Some(s.generated_at))
                    })
                    .or(Some(self.generated_at)),
                refreshing: self
                    .snapshot
                    .as_ref()
                    .is_some_and(|s| s.freshness == "refreshing")
                    || self.snapshot.as_ref().is_some_and(|s| {
                        s.pending_entries.unwrap_or_default() > 0
                            || s.pending_corrections.unwrap_or_default() > 0
                    }),
                refresh_failed: self
                    .snapshot
                    .as_ref()
                    .is_some_and(|s| s.freshness == "failed"),
                progress_completed: None,
                progress_total: None,
                progress_stage: None,
            }),
            database_schema: u32::try_from(r.database_schema_version).unwrap_or(u32::MAX),
            taxonomy_version: u32::try_from(r.taxonomy_version).unwrap_or(u32::MAX),
            freshest_at_ms: self
                .snapshot
                .as_ref()
                .and_then(|s| {
                    s.watermark_at_ms
                        .and_then(|at| u64::try_from(at).ok())
                        .or(Some(s.generated_at))
                })
                .or(Some(self.generated_at)),
            phase_series: vec![BTreeMap::new(); buckets],
            token_buckets_observed: vec![false; buckets],
            bucket_coverage: vec![CoverageState::Unobserved; buckets],
            // Aggregated durations cannot be unioned across collectors. Do not invent intervals.
            request_unknown: 1,
            execution_unknown: u64::from(self.review.is_none()),
            agent_unknown: u64::from(self.review.is_none()),
            ..Default::default()
        };
        if source_partial && !headline {
            increment(&mut result.coverage_events, "partial");
        }
        for t in &r.provider_tokens {
            if t.measurement_provenance != "provider_reported"
                || repository.is_some_and(|key| t.repository_bucket != key)
            {
                continue;
            }
            if !TOKEN_CATEGORIES
                .iter()
                .any(|(category, _)| *category == t.category)
            {
                continue;
            }
            result.evidence |= t.observation_count > 0;
            if t.observation_count > 0 && (t.exact_tokens.is_some() || t.measured_tokens != 0) {
                add_token_quantity(
                    &mut result.tokens,
                    &mut result.unavailable_tokens,
                    &t.category,
                    t.measured_tokens,
                );
            }
            let state = if partial || t.unknown_observations > 0 || t.exact_tokens.is_none() {
                "partial"
            } else {
                "complete"
            };
            if !headline || t.category == "total_tokens" {
                *result.token_observations.entry(state.into()).or_default() += t.observation_count;
            }
        }
        {
            match &r.counts {
                Counts::Measured(counts) => {
                    result.operation_count = counts.operations;
                    result.model_request_count = counts.model_requests;
                    result.tool_count = counts.tools;
                }
                Counts::Deferred => result.counts_deferred = true,
            }
            for a in &r.provider_tokens_by_activity {
                let key = (safe_phase(&a.phase), safe_label(&a.activity));
                *result.activities.entry(key.clone()).or_default() += a.measured_tokens;
                *result
                    .activity_provenance
                    .entry((key.0, key.1, safe_label(&a.attribution_provenance)))
                    .or_default() += a.measured_tokens;
                if a.exact_tokens.is_none() || a.unknown_observations > 0 {
                    increment(&mut result.coverage_events, "partial");
                }
            }
        }
        // The producer has already applied the worktree selection to every
        // vector in this summary. Keep the extra arguments in the consumer
        // contract so cache keys cannot accidentally mix scopes.
        let _ = (worktree_keys, include_unassigned);
        if buckets == 1 {
            for ((phase, _), value) in &result.activities {
                *result.phase_series[0].entry(phase.clone()).or_default() += value;
            }
            if let Some(total) = result
                .tokens
                .get("total_tokens")
                .and_then(|v| u64::try_from(*v).ok())
            {
                let classified = result.phase_series[0]
                    .values()
                    .copied()
                    .try_fold(0u64, u64::checked_add);
                match classified {
                    Some(classified) if classified <= total => {
                        *result.phase_series[0]
                            .entry("unattributed".into())
                            .or_default() += total - classified;
                    }
                    _ => {
                        result.phase_series[0] = BTreeMap::from([("unattributed".into(), total)]);
                        increment(&mut result.coverage_events, "partial");
                    }
                }
            }
            result.token_buckets_observed[0] = result.tokens.contains_key("total_tokens");
            result.bucket_coverage[0] = if partial {
                CoverageState::Partial
            } else {
                CoverageState::Complete
            };
        }
        result
    }
}

impl CodexUsage {
    /// Source-wide repository totals: one exact-window summary per configured
    /// source, independent of the number of registered repositories.
    pub(super) fn api_repositories(
        &self,
        repositories: &[RepositoryRecord],
        range: UsageRange,
    ) -> Result<Option<UsageRepositories>, ProtocolError> {
        if self.config.codex_usage_sources.is_empty()
            || self
                .config
                .codex_usage_sources
                .iter()
                .any(|s| s.api_socket.is_none())
        {
            return Ok(None);
        }
        let now = self.now_ms()?;
        // Display snapshots share one five-second window. Exact review reads
        // use their original boundaries and never enter this display cache.
        let observed = now / 5_000 * 5_000;
        let (start, end, bucket, count) = usage_window(&range, observed);
        let deadline = Instant::now() + API_BUDGET;
        let mut rows = Vec::with_capacity(repositories.len());
        for repository in repositories {
            let mut reports = Vec::new();
            let mut failures = BTreeMap::new();
            for source in &self.config.codex_usage_sources {
                match self.repository_key(source, repository, now, false) {
                    Ok(key) => {
                        match self
                            .api
                            .summary(source, Some(&key), None, true, start, end, deadline)
                        {
                            Ok(summary) => reports.push((
                                source.uid,
                                summary.source_report(Some(&key), count, None, true),
                            )),
                            Err(_) => return Ok(None),
                        }
                    }
                    // The collection must start its own existing background
                    // mapper instead of waiting for a detail/lifetime request.
                    Err(error) if error.message == "mapping_pending" => return Ok(None),
                    Err(error) => increment(&mut failures, &error.message),
                }
            }
            let report = combine(
                repository,
                range.clone(),
                observed,
                start,
                bucket,
                count,
                &reports,
                failures,
                None,
                self.config.codex_usage_sources.len(),
            );
            rows.push(UsageRepositoryRow {
                repository_id: report.repository_id,
                display_name: report.display_name,
                range: report.range,
                coverage: report.coverage,
                total_tokens: report.totals.total_tokens,
                model_requests: report.totals.model_requests,
                tool_calls: report.totals.tool_calls,
                execution_wall_ms: Some(report.time.execution_wall.measured_ms),
                cost: report.totals.cost,
            });
        }
        Ok(Some(UsageRepositories {
            range,
            generated_at_ms: observed,
            repositories: rows,
        }))
    }
}

/// Never add a producer estimate and a local estimate of the same source.
/// Missing components remain unknown and make the combined valuation partial.
pub(super) fn merge_costs(costs: &[UsageCost], source_gaps: bool) -> UsageCost {
    let mut result = UsageCost {
        basis: "api_equivalent".into(),
        currency: "USD".into(),
        processing_tier: "standard".into(),
        ..Default::default()
    };
    let mut partial = source_gaps;
    let mut refs = BTreeSet::new();
    let counter = |select: fn(&UsageCost) -> Option<u64>| {
        if costs.is_empty() {
            return None;
        }
        costs
            .iter()
            .try_fold(0_u64, |total, cost| total.checked_add(select(cost)?))
    };
    result.model_requests = counter(|cost| cost.model_requests);
    result.priced_requests = counter(|cost| cost.priced_requests);
    result.unknown_requests = counter(|cost| cost.unknown_requests);
    result.priced_observations = counter(|cost| cost.priced_observations);
    result.unknown_observations = counter(|cost| cost.unknown_observations);
    let mut quantity_overflow = false;
    macro_rules! sum {
        ($field:ident) => {{
            let mut amount: Option<i128> = None;
            let mut overflow = false;
            for cost in costs {
                match cost.$field {
                    Some(v) => {
                        if !overflow {
                            amount = amount.unwrap_or_default().checked_add(i128::from(v));
                            overflow = amount.is_none();
                            partial |= overflow;
                        }
                    }
                    None => partial = true,
                }
            }
            result.$field = amount.and_then(|value| value.try_into().ok());
            quantity_overflow |= overflow || (amount.is_some() && result.$field.is_none());
        }};
    }
    sum!(estimated_usd_micros);
    sum!(input_usd_micros);
    sum!(cached_input_usd_micros);
    sum!(cache_write_usd_micros);
    sum!(output_usd_micros);
    sum!(input_tokens);
    sum!(cached_input_tokens);
    sum!(cache_write_tokens);
    sum!(uncached_input_tokens);
    sum!(output_tokens);
    // Reasoning is an optional output subset, not a separate billed component.
    let reasoning = costs
        .iter()
        .filter_map(|c| c.reasoning_tokens)
        .collect::<Vec<_>>();
    result.reasoning_tokens = (!reasoning.is_empty())
        .then(|| {
            reasoning
                .iter()
                .try_fold(0i128, |total, value| total.checked_add(i128::from(*value)))
                .and_then(|value| i64::try_from(value).ok())
        })
        .flatten();
    quantity_overflow |= !reasoning.is_empty() && result.reasoning_tokens.is_none();
    for cost in costs {
        partial |= cost.status != "complete";
        result.unknown_tokens = result.unknown_tokens.saturating_add(cost.unknown_tokens);
        refs.extend(cost.rate_card_refs.clone());
        for card in &cost.matched_rate_cards {
            if !result.matched_rate_cards.contains(card) {
                result.matched_rate_cards.push(card.clone());
            }
        }
        merge_counts(&mut result.unavailable_reasons, &cost.unavailable_reasons);
    }
    if source_gaps {
        increment(&mut result.unavailable_reasons, "unavailable_collectors");
    }
    if quantity_overflow {
        partial = true;
        increment(&mut result.unavailable_reasons, "quantity_overflow");
    }
    result.rate_card_refs = refs.into_iter().collect();
    result.rate_card_ref = result.rate_card_refs.first().cloned();
    result.status = if result.estimated_usd_micros.is_none() {
        "unavailable"
    } else if partial {
        "partial"
    } else {
        "complete"
    }
    .into();
    result.estimated_usd = result
        .estimated_usd_micros
        .map(|v| format!("{}.{:06}", v / 1_000_000, v % 1_000_000));
    result
}

#[cfg(test)]
#[path = "usage_api_tests.rs"]
mod tests;
