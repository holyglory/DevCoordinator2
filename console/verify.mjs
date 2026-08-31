// Browser verification of the Console: every primary view in every required
// state at a wide and a narrow viewport, with software-detectable checks
// (no document overflow, no clipped tiles, controls visible and enabled)
// and click-through proofs that controls call the real API and re-render.
//
// Usage:  CONSOLE_VERIFY_PLAYWRIGHT=<dir containing node_modules/playwright> \
//         CONSOLE_VERIFY_OUT=<dir> node console/verify.mjs
// Fixture data is used only here, never in production views.

import crypto from 'node:crypto';
import fs from 'node:fs/promises';
import http from 'node:http';
import net from 'node:net';
import os from 'node:os';
import path from 'node:path';
import { createRequire } from 'node:module';

import { createEdge } from '../edge/devcoordinator2-edge.mjs';
import { createSessionManager } from '../edge/lib/session.mjs';
import { canonicalJson } from '../edge/lib/routes-store.mjs';

const BASE = 'example.test';
const HOST = process.env.CONSOLE_VERIFY_HOST || `console.${BASE}`;
const OUT = process.env.CONSOLE_VERIFY_OUT || path.join(os.tmpdir(), 'dc2-console-verify');
const pw = createRequire(path.join(process.env.CONSOLE_VERIFY_PLAYWRIGHT || process.cwd(), 'package.json'))('playwright');

const DEP = 'd0123456789abcdef';
const OBS = 'd3333333333333333';
const REPO = 'r0123456789abcdef';
const LONG = 'a-very-long-deployment-name-that-keeps-going-and-going-for-quite-a-while';
const V_EARLY = 'v0000000000000001'; const V_DONE = 'v0000000000000002';
const V_R1 = 'v0000000000000003'; const V_R2 = 'v0000000000000004';
const P_PAR = 'p1111111111111101'; const P_C1 = 'p1111111111111102';
const P_C2 = 'p1111111111111103'; const P_G1 = 'p1111111111111104';
const P_D1 = 'p1111111111111105'; const P_FB = 'p1111111111111106';
const P_LT = 'p1111111111111107'; const P_UNSIZED = 'p1111111111111108';
const P_UNSIZED_PARENT = 'p1111111111111109';

const progressFixture = (scenario, period = 'day') => {
  const spec = { hour: [3600000, 24], day: [86400000, 7], week: [604800000, 8] }[period];
  const [bucketMs, count] = spec;
  const alignedEnd = period === 'week' ? Date.UTC(2026, 7, 31) : Date.UTC(2026, 7, 31);
  const series = Array.from({ length: count }, (_, index) => ({
    bucket_start_ms: alignedEnd - (count - index) * bucketMs,
    bucket_end_ms: alignedEnd - (count - index - 1) * bucketMs,
    tasks_completed: scenario.empty ? 0 : [1, 0, 2, 1, 0, 2, 1, 1][index % 8],
    tasks_created: scenario.empty ? 0 : [0, 1, 0, 0, 2, 0, 0, 1][index % 8],
    tasks_reopened: scenario.empty ? 0 : (index === count - 2 ? 1 : 0),
    planned_lines_completed: scenario.empty ? 0 : [80, 0, 140, 95, 0, 220, 110, 75][index % 8],
    scope_lines_changed: scenario.empty ? 0 : [0, 40, 0, -20, 120, 0, 0, 30][index % 8],
    test_runs: scenario.empty ? 0 : [3, 2, 4, 3, 5, 2, 4, 3][index % 8],
    tests_passed: scenario.empty ? 0 : [3, 2, 3, 3, 4, 2, 4, 2][index % 8],
    test_pass_rate: scenario.empty ? null : [1, 1, .75, 1, .8, 1, 1, .667][index % 8],
    total_tokens: scenario.empty ? null : [120000, 90000, 180000, 150000, 210000, 110000, 170000, 130000][index % 8],
    token_coverage: scenario.empty ? 'unobserved' : (scenario.partial && index === 2 ? 'partial' : 'complete'),
  }));
  const currentTotals = {
    tasks_completed: series.reduce((sum, point) => sum + point.tasks_completed, 0),
    tasks_created: series.reduce((sum, point) => sum + point.tasks_created, 0),
    tasks_reopened: series.reduce((sum, point) => sum + point.tasks_reopened, 0),
    planned_lines_completed: series.reduce((sum, point) => sum + point.planned_lines_completed, 0),
    scope_lines_changed: series.reduce((sum, point) => sum + point.scope_lines_changed, 0),
    test_runs: series.reduce((sum, point) => sum + point.test_runs, 0),
    tests_passed: series.reduce((sum, point) => sum + point.tests_passed, 0),
    test_pass_rate: scenario.empty ? null : .86,
    total_tokens: scenario.empty ? null : series.reduce((sum, point) => sum + (point.total_tokens || 0), 0),
    tokens_per_completed_task: scenario.empty ? null : 142500,
    tokens_per_planned_line: scenario.empty ? null : 1220,
    tasks_completed_per_day: scenario.empty ? 0 : 1.1,
    tasks_created_per_day: scenario.empty ? 0 : .6,
    tests_per_completed_task: scenario.empty ? null : 3.25,
  };
  const priorities = scenario.empty ? [] : [
    { task_id: P_C2, rank: 1, title: 'Wrong password message', outcome: 'People understand why sign-in failed and how to try again.', status: 'in_progress', kind: 'goal', estimated_loc: 275, impact_days: 1.2, group: null, dependency: null, elaboration_needed: false, reason: 'Finishing work already underway reduces handoff and delay risk.', forecast_if_deferred: { earliest_at_ms: Date.UTC(2026, 8, 2), likely_at_ms: Date.UTC(2026, 8, 3), latest_at_ms: Date.UTC(2026, 8, 5), confidence_percent: 60, explanation: 'This assumes the selected task moves out of this release; nothing is changed automatically.' } },
    { task_id: P_UNSIZED, rank: 2, title: 'Check release', outcome: 'The complete release works in a real browser before people use it.', status: 'planned', kind: 'improvement', estimated_loc: null, impact_days: 1.0, group: null, dependency: null, elaboration_needed: false, reason: 'A missing estimate makes the release range less certain.', forecast_if_deferred: null },
    { task_id: P_G1, rank: 3, title: 'Check e-mail spelling', outcome: 'People are told when an e-mail address is written incorrectly.', status: 'planned', kind: 'stub', estimated_loc: 100, impact_days: .4, group: null, dependency: null, elaboration_needed: false, reason: 'This is about 12% of the remaining measured scope.', forecast_if_deferred: { earliest_at_ms: Date.UTC(2026, 8, 3), likely_at_ms: Date.UTC(2026, 8, 4), latest_at_ms: Date.UTC(2026, 8, 6), confidence_percent: 64, explanation: 'This assumes the selected task moves out of this release; nothing is changed automatically.' } },
  ];
  return {
    repository_id: REPO, display_name: 'repo-one', period,
    generated_at_ms: Date.UTC(2026, 7, 30, 23, 59),
    window: { bucket_ms: bucketMs, start_ms: alignedEnd - count * bucketMs,
      end_ms: Date.UTC(2026, 7, 30, 23, 59),
      comparison_start_ms: alignedEnd - count * 2 * bucketMs, timezone: 'UTC' },
    scope: { tasks_total: scenario.empty ? 0 : 12, tasks_done: scenario.empty ? 0 : 7,
      planned_lines_total: scenario.empty ? 0 : 2500,
      planned_lines_done: scenario.empty ? 0 : 1558,
      unestimated_open_tasks: scenario.empty ? 0 : 1 },
    series,
    comparison: { current: currentTotals, previous: scenario.empty ? { ...currentTotals } : {
      ...currentTotals, tasks_completed: 6, tasks_created: 7, tasks_reopened: 0,
      planned_lines_completed: 920, scope_lines_changed: 80, test_runs: 20,
      tests_passed: 17, test_pass_rate: .82, total_tokens: 1300000,
      tokens_per_completed_task: 216667, tokens_per_planned_line: 1413,
      tasks_completed_per_day: .86, tasks_created_per_day: 1,
      tests_per_completed_task: 3.33 } },
    forecast: scenario.empty ? {
      state: 'unavailable', reason: 'no_planned_release', release: null,
      remaining_tasks: 0, remaining_planned_lines: 0, unestimated_tasks: 0,
      velocity: { tasks_per_day: 0, planned_lines_per_day: 0, lookback_days: 28 },
      target_date_recorded: false,
      explanation: 'Plan a release before estimating its delivery range.',
    } : {
      state: 'available', release: { release_id: V_R1, name: 'Release 1', status: 'planned' },
      remaining_tasks: 5, remaining_planned_lines: 942, unestimated_tasks: 1,
      velocity: { tasks: 8, planned_lines: 1180, lookback_days: 14,
        tasks_per_day: .57, planned_lines_per_day: 84.3 },
      target_date_recorded: false, as_of_ms: Date.UTC(2026, 7, 30, 23, 59),
      likely_at_ms: Date.UTC(2026, 8, 5), earliest_at_ms: Date.UTC(2026, 8, 3),
      latest_at_ms: Date.UTC(2026, 8, 8), confidence_percent: 64,
      confidence: 'medium', drivers: ['1 remaining task is not estimated'],
      explanation: 'One remaining task is not estimated. No target date is recorded.',
      assumptions: ['Current task estimates are used as planned size.'],
    },
    priorities,
    coverage: { state: scenario.empty ? 'unavailable' : scenario.partial ? 'partial' : 'complete',
      plan: { state: 'complete', completed_with_estimate: 7, completed_total: 7 },
      tests: { state: scenario.empty ? 'unobserved' : scenario.partial ? 'partial' : 'complete', recorded_runs: scenario.empty ? 0 : 26, history_sources: scenario.empty ? 0 : 1, unavailable_sources: 0, earliest_at: scenario.empty ? null : '2026-08-01T00:00:00Z' },
      tokens: { state: scenario.empty ? 'unobserved' : scenario.partial ? 'partial' : 'complete', has_gaps: !!scenario.partial, configured_collectors: 2, available_collectors: 2, contributing_collectors: scenario.empty ? 0 : 2, freshest_at_ms: scenario.empty ? null : Date.UTC(2026, 7, 30, 23, 58), unavailable_reasons: {} } },
    semantics: { tasks: 'terminal task status events in the permanent plan ledger', lines: 'current planned task estimates completed; not measured Git changes', tests: 'bounded repository-local terminal test summaries', tokens: 'provider total_tokens; missing collector coverage stays missing', forecast: 'deterministic range from recent pace, scope, estimates, and test stability' },
  };
};

const fixtures = (scenario) => {
  const runningState = scenario.applying ? 'applying' : (scenario.stopped ? 'stopped' : (scenario.serviceStopped ? 'degraded' : 'running'));
  const running = { deployment_id: DEP, repository_id: 'r0123456789abcdef', repository_name: 'repo-one', name: 'web', source: 'worktree', state: runningState, domain: `app-dev.${BASE}`, public: false, current_generation: 17, updated_at: new Date(Date.now() - 90000).toISOString(), ttl_expires_at: null };
  const degraded = { deployment_id: 'd1111111111111111', repository_id: 'r0123456789abcdef', repository_name: 'repo-one', name: LONG, source: 'checkout', state: 'degraded', domain: `${LONG}.${BASE}`, public: false, current_generation: 2147483647, updated_at: new Date().toISOString(), ttl_expires_at: '2026-12-31T00:00:00Z' };
  const observed = { deployment_id: OBS, repository_id: 'r9999999999999999', repository_name: 'legacy-repo', name: 'existing-compose-stack', source: 'observed', state: 'running', health: 'healthy', domain: `observed.${BASE}`, public: true, route_port: 5001, current_generation: null, updated_at: new Date().toISOString(), ttl_expires_at: null, observed_only: true };
  const observedComponents = [{ name: 'app', display_name: 'existing-compose-stack-app-1', type: 'container', state: 'running', health: 'healthy', generation: null, binding: { kind: 'observed-container', identity: 'd'.repeat(64) }, port: 5001, restarts: null, owned: false, independent_control: false, last_error: null }];
  const components = [
    { name: 'db', type: 'postgres', state: 'running', health: 'healthy', generation: 0, binding: { kind: 'container', identity: 'c'.repeat(64) }, port: 20001, restarts: 0, owned: true, independent_control: true, last_error: null },
    { name: 'api', type: 'process', state: scenario.stopped ? 'stopped' : 'running', health: scenario.stopped ? 'none' : 'healthy', generation: 17, binding: { kind: 'unit', identity: `devcoordinator2-deploy-${DEP}-api-g17.service` }, port: 20002, restarts: 3, owned: true, independent_control: true, last_error: null },
    { name: 'worker', type: 'process', state: 'failed', health: 'unhealthy', generation: 17, binding: { kind: 'unit', identity: `devcoordinator2-deploy-${DEP}-worker-g17.service` }, port: null, restarts: 9999999, owned: true, independent_control: false, last_error: 'exited 1: ' + 'x'.repeat(120) },
    { name: 'stack', type: 'compose', state: scenario.serviceStopped ? 'failed' : 'running', health: scenario.serviceStopped ? 'unhealthy' : 'healthy', generation: 17, binding: { kind: 'compose', identity: `dc2-${DEP}-stack` }, port: null, restarts: 0, owned: true, independent_control: true, last_error: null, services: [
      { name: 'bootstrap', role: 'finite', state: 'completed', containers: 1, independent: false },
      { name: 'stream-capture', role: 'running', state: 'running', containers: 1, independent: false },
      { name: 'projection-worker', role: 'running', state: scenario.serviceStopped ? 'stopped' : 'running', containers: 1, independent: true },
    ], completed_services: [{ service: 'bootstrap', generation: 17, exit_code: 0, recorded_at: '2026-08-28T00:00:00Z' }] },
    { name: 'smtp', type: 'external', state: 'running', health: 'healthy', generation: null, binding: { kind: null, identity: null }, port: null, restarts: 0, owned: false, independent_control: true, last_error: null },
  ];
  const points = Array.from({ length: 30 }, (_, i) => ({ minute: `2026-01-01T00:${String(i).padStart(2, '0')}Z`, min: i, avg: i * 1.5, max: i * 2, samples: 4 }));
  const denseUnsizedTasks = scenario.densePlan ? Array.from({ length: 110 }, (_, index) => ({
    task_id: `pdense${String(index).padStart(11, '0')}`,
    parent_task_id: null,
    release_id: V_R2,
    seq: 1000 + index,
    position: 100 + index,
    title: `Check customer journey ${index + 1}`,
    impact: 'This visible journey must work before the release is ready.',
    status: 'planned',
    kind: 'improvement',
    estimated_loc: null,
  })) : [];
  const usageHasNoMeasurements = scenario.empty || scenario.usageUnavailable;
  const usageCoverage = scenario.usageUnavailable
    ? { state: 'unavailable', has_gaps: true, configured_collectors: 4, available_collectors: 0, contributing_collectors: 0, freshest_at_ms: null, events: {}, token_observations: {}, unavailable_reasons: { source_unavailable: 4 }, database_schemas: [], taxonomy_versions: [] }
    : scenario.usageComplete
      ? { state: 'complete', has_gaps: false, configured_collectors: 4, available_collectors: 4, contributing_collectors: 4, freshest_at_ms: Date.now() - 120000, events: { complete: 82 }, token_observations: { complete: 147 }, unavailable_reasons: {}, database_schemas: [4], taxonomy_versions: [1] }
      : scenario.empty
        ? { state: 'unobserved', has_gaps: true, configured_collectors: 4, available_collectors: 4, contributing_collectors: 0, freshest_at_ms: null, events: {}, token_observations: {}, unavailable_reasons: {}, database_schemas: [4], taxonomy_versions: [1] }
        : { state: 'partial', has_gaps: true, configured_collectors: 4, available_collectors: 3, contributing_collectors: 3, freshest_at_ms: Date.now() - 120000, events: { complete: 80, partial: 2 }, token_observations: { complete: 144, partial: 3 }, unavailable_reasons: { source_unavailable: 1 }, database_schemas: [4], taxonomy_versions: [1] };
  const usageSeries = usageHasNoMeasurements ? [] : Array.from({ length: 24 }, (_, index) => {
    const start = Date.UTC(2026, 7, 28, 19 + index);
    const phases = {
      planning: 45000 + index * 1200,
      implementation: 70000 + (index % 7) * 15000,
      testing: 30000 + (index % 5) * 9000,
      deployment: index % 6 === 0 ? 18000 : 7000,
      reporting: 5000 + (index % 4) * 2000,
      unattributed: 4000,
    };
    return { bucket_start_ms: start, bucket_end_ms: start + 3600000,
      coverage: scenario.usageComplete || index !== 17 ? 'complete' : 'partial', phases,
      total_tokens: Object.values(phases).reduce((sum, value) => sum + value, 0) };
  });
  const usageDetail = {
    repository_id: REPO, display_name: 'repo-one', range: '24h', generated_at_ms: Date.now(),
    coverage: usageCoverage,
    totals: usageHasNoMeasurements
      ? { total_tokens: null, input_tokens: null, cached_input_tokens: null, output_tokens: null, reasoning_tokens: null, model_requests: 0, tool_calls: 0, operations: 0 }
      : { total_tokens: 6405721, input_tokens: 6391772, cached_input_tokens: 6212608, output_tokens: 13949, reasoning_tokens: 7821, model_requests: 104, tool_calls: 236, operations: 340 },
    series: usageSeries,
    activities: usageHasNoMeasurements ? [] : [
      { phase: 'implementation', activity: 'coding', total_tokens: 2660000, share: .416, operations: 42, provenance: { agent_declared: 42 } },
      { phase: 'planning', activity: 'repository_analysis', total_tokens: 1740000, share: .272, operations: 28, provenance: { agent_declared: 28 } },
      { phase: 'testing', activity: 'integration_testing', total_tokens: 1240000, share: .194, operations: 35, provenance: { deterministic_classification: 35 } },
      { phase: 'deployment', activity: 'deployment', total_tokens: 540000, share: .084, operations: 9, provenance: { agent_declared: 9 } },
      { phase: 'reporting', activity: 'completion_handoff', total_tokens: 120000, share: .019, operations: 5, provenance: { agent_declared: 5 } },
      { phase: 'unattributed', activity: 'unknown', total_tokens: 70000, share: .011, operations: 8, provenance: { unknown: 8 } },
      { phase: 'unattributed', activity: 'mixed', total_tokens: 22000, share: .003, operations: 3, provenance: { unknown: 3 } },
      { phase: 'unattributed', activity: 'accounting_overhead', total_tokens: 13721, share: .002, operations: 12, provenance: { deterministic_classification: 12 } },
    ],
    time: {
      request_to_delivery: { measured_ms: usageHasNoMeasurements ? 0 : 472000, unknown_intervals: usageHasNoMeasurements ? 0 : 1 },
      execution_wall: { measured_ms: usageHasNoMeasurements ? 0 : 147000, unknown_intervals: usageHasNoMeasurements ? 0 : 2 },
      summed_agent_active: { measured_ms: usageHasNoMeasurements ? 0 : 349000, unknown_intervals: usageHasNoMeasurements ? 0 : 2 },
      phases: [],
    },
    tools: { outcomes: usageHasNoMeasurements ? [] : [
      { outcome: 'completed', count: 176 }, { outcome: 'failed', count: 19 },
      { outcome: 'interrupted', count: 21 }, { outcome: 'rejected', count: 12 },
      { outcome: 'unknown', count: 8 }], families: usageHasNoMeasurements ? [] : [
      { family: 'execution', count: 150 }, { family: 'coding', count: 64 }] },
    semantics: { tokens: 'provider total_tokens only; cached input and reasoning are subsets', time: 'wall, execution, phase, agent, and tool durations are separate', coverage: 'partial and unavailable collectors never contribute synthetic zeroes' },
  };
  return {
    'user.whoami': { local: false, identity: scenario.identity, user_id: 'u1', administrator: scenario.admin, grants: scenario.admin ? {} : { [DEP]: scenario.denied ? 'viewer' : 'operator' } },
    'deployment.list': { deployments: scenario.empty ? [] : [running, degraded, observed], declared: scenario.empty ? [] : [{ name: 'tool', source: 'worktree', deployment_id: 'd2222222222222222' }] },
    'deployment.status': { ...running, previous_generation: 16, route_port: 20002, route_component: 'api', components, log_dir: '/state/logs' },
    'deployment.observed-status': { ...observed, previous_generation: null, components: observedComponents, log_dir: null, native_project: 'existing-compose-stack', observation_source: 'legacy-current-import' },
    'deployment.logs': { component: 'api', tail: 'line 1\nline 2 ' + 'long '.repeat(60) + '\nline 3', truncated_before_tail: true, log_path: '/state/logs/api.log' },
    'health.history': { subject_kind: 'component', subject_id: `${DEP}/api`, metric: 'cpu_percent', minutes: 60, points: scenario.empty ? [] : points, truncated: false },
    'test.list': { runs: scenario.empty ? [] : [
      { run_id: 't20260101T000000Z-abc123', test: 'unit', status: 'running', started_at: new Date().toISOString(), finished_at: null, duration_seconds: null, exit_code: null, stdout_bytes_observed: 123456789, stderr_bytes_observed: 0, stdout_truncated: true, stderr_truncated: false, display_name: 'repo-one', worktree_path: '/srv/repos/repo-one', repository_id: 'r1', worktree_id: 'w1', summary_path: '/srv/repos/repo-one/.devcoordinator/test/current/summary.json' },
      { run_id: 't20260101T000100Z-def456', test: 'integration-with-a-long-name', status: 'failed', started_at: new Date(Date.now() - 3600000).toISOString(), finished_at: new Date().toISOString(), duration_seconds: 3599.123, exit_code: 1, stdout_bytes_observed: 10, stderr_bytes_observed: 4194304, stdout_truncated: false, stderr_truncated: true, display_name: LONG, worktree_path: `/srv/repos/${LONG}`, repository_id: 'r2', worktree_id: 'w2', summary_path: '/x' }] },
    'test.output': { run_id: 't1', stream: 'stdout', tail: 'ok\n'.repeat(5), tail_bytes: 15, truncated_before_tail: true, log_path: '/srv/repos/repo-one/.devcoordinator/test/current/stdout.log' },
    'health.summary': { host: { cpu_percent: 93.4, memory_total: 264122252 * 1024, memory_used: 108579328 * 1024, memory_available: 155542924 * 1024, swap_total: 0, swap_used: 0, load_1: 8.32, load_5: 8.39, load_15: 7.69, fs_size: 2113513742336, fs_free: 148698841088, fs_used: 1964814901248, ncpu: 32, reconciliation: { managed_cpu_percent: 40.1, daemon_cpu_percent: 0.3, other_cpu_percent: 53.0, managed_memory: 50e9, daemon_memory: 120e6, other_memory: 60e9 } }, storage: { fs_used: 1964814901248, managed_repositories: 4e11, devcoordinator_state: 5e7, docker_shared: 3e10, docker_images: 2.7e10, docker_build_cache: 2.8e9, docker_shared_volumes: 1e8, other: 1.5e12 }, unhealthy_deployments: scenario.empty ? [] : [{ ...degraded, reasons: [{ component: 'worker', state: 'failed', detail: 'exited 1: boom' }, { component: 'api', state: 'stopped', detail: null }] }, { deployment_id: OBS, name: 'existing-compose-stack', source: 'observed', state: 'running', health: 'unhealthy', repository_name: 'legacy-repo', observed_only: true, reasons: [{ component: 'app', state: 'running', detail: 'container healthcheck failing (Up 3 days (unhealthy))' }] }], active_tests: scenario.empty ? [] : ['unit'], container_counts: { 'managed-test': 1, 'managed-preview': 0, 'managed-permanent': 3, 'orphaned-managed': 1, unmanaged: 43 }, alerts: scenario.empty ? [] : [{ alert_key: 'host/cpu', kind: 'host_cpu', severity: 'warning', message: 'host CPU 93% sustained', opened_at: new Date().toISOString() }, { alert_key: `component/${DEP}/worker/unhealthy`, kind: 'component_unhealthy', severity: 'critical', message: `component ${DEP}/worker is failed`, opened_at: new Date().toISOString() }], sampling: { retention_days: 30 } },
    'health.repositories': { repositories: scenario.empty ? [] : [{ repository_id: 'r0123456789abcdef', display_name: 'repo-one', root_path: '/srv/repos/repo-one', cpu_percent: 40.1, memory_bytes: 5e10, storage_bytes: 4e11, storage: {}, health: 'unhealthy', deployments: [running, degraded, observed], trend_cpu: [1, 5, 3, 8, 2, 9, 4, 7, 3, 6, 2, 5], trend_memory: [1, 1, 2, 2, 3, 3, 3, 4, 4, 4, 5, 5] }, { repository_id: 'r2', display_name: LONG, root_path: `/srv/repos/${LONG}`, cpu_percent: 0, memory_bytes: 0, storage_bytes: 1234567890123, storage: {}, health: 'none', deployments: [], trend_cpu: [], trend_memory: [] }], devcoordinator: { cpu_percent: 0.3, memory_bytes: 120e6, storage_bytes: 5e7 }, shared_unattributed: { cpu_percent: 53, memory_bytes: 60e9, storage: { docker_shared: 3e10, docker_images: 2.7e10, docker_build_cache: 2.8e9, docker_shared_volumes: 1e8, other: 1.5e12 } }, host: {} },
    'health.containers': { containers: scenario.empty ? [] : [
      { id: 'a'.repeat(64), name: 'devcoordinator2-deploy-x-db', image: 'postgres:16-alpine', state: 'running', status: 'Up 3 days', created: '2026-08-20 10:00:00 +0000 UTC', repository_id: 'r0123456789abcdef', deployment_id: DEP, component: 'db', run_id: null, caller_uid: 1000, client: 'claude', ttl_seconds: null, data: 'persistent', classification: 'managed-permanent', cpu_percent: 1.2, memory_bytes: 2677821440, pids: 7, container_layer_bytes: 0 },
      { id: 'b'.repeat(64), name: 'legacy-thing-1', image: 'some/image:latest', state: 'running', status: 'Up 6 weeks', created: '2026-07-01', repository_id: null, deployment_id: null, component: null, run_id: null, caller_uid: null, client: null, ttl_seconds: null, data: null, classification: 'unmanaged', cpu_percent: 12.5, memory_bytes: 9e9, pids: 100, container_layer_bytes: null },
      { id: 'c'.repeat(64), name: 'devcoordinator2-test-old-postgres', image: 'postgres:16-alpine', state: 'exited', status: 'Exited (0)', created: '2026-08-22', repository_id: 'r1', deployment_id: null, component: null, run_id: 't-old', caller_uid: 1001, client: 'codex', ttl_seconds: 3600, data: 'disposable', classification: 'orphaned-managed', cpu_percent: null, memory_bytes: null, pids: null, container_layer_bytes: 12345 },
      { id: 'd'.repeat(64), name: 'existing-compose-stack-app-1', image: 'app:1', state: 'running', status: 'Up 3 days (healthy)', created: '2026-08-20', repository_id: 'r0123456789abcdef', deployment_id: OBS, component: 'app', run_id: null, caller_uid: null, client: 'legacy-current-import', ttl_seconds: null, data: 'observed-only', classification: 'observed-current', cpu_percent: 2.5, memory_bytes: 123456789, pids: 4, container_layer_bytes: 45678 }], counts: { 'managed-test': 0, 'managed-preview': 0, 'managed-permanent': 1, 'observed-current': 1, 'orphaned-managed': 1, unmanaged: 1 } },
    'usage.repositories': { range: '24h', generated_at_ms: Date.now(), repositories: scenario.empty ? [] : [
      { repository_id: REPO, display_name: 'repo-one', range: '24h', coverage: usageCoverage, total_tokens: 6405721, model_requests: 104, tool_calls: 236, execution_wall_ms: 147000 },
      { repository_id: 'r2', display_name: LONG, range: '24h', coverage: { ...usageCoverage, state: 'complete', has_gaps: false, configured_collectors: 4, available_collectors: 4, contributing_collectors: 4 }, total_tokens: 2100000, model_requests: 38, tool_calls: 74, execution_wall_ms: 72000 }] },
    'usage.repository': usageDetail,
    'progress.repositories': { repositories: scenario.empty ? [] : [
      { repository_id: REPO, display_name: 'repo-one', open_tasks: 5,
        tasks_done: 7, planned_lines_done: 1558, planned_lines_total: 2500,
        next_release: { release_id: V_R1, name: 'Release 1', status: 'planned' } },
      { repository_id: 'r2', display_name: LONG, open_tasks: 0,
        tasks_done: 0, planned_lines_done: 0, planned_lines_total: 0,
        next_release: null }] },
    'progress.repository': progressFixture(scenario),
    'bug.list': { bugs: scenario.empty ? [] : [{ bug_id: 'b0123456789ab', component: 'api', summary: 'Returns 500 on /export when the report is large', expected: '200 with CSV', actual: '500', steps: '1. open /export 2. choose all-time 3. submit', opened_at: '2026-08-20T10:00:00Z', last_seen_at: new Date().toISOString(), occurrences: 42, reporter: 'dev@example.test', correlations: { deployment_id: DEP } }], store: '/bugs' },
    'user.list': { users: [{ user_id: 'u1', email: 'owner@example.test', administrator: true, grants: [], last_seen_at: new Date().toISOString() }, { user_id: 'u2', email: `${'verylongmailboxname'.repeat(3)}@example.test`, administrator: false, grants: [{ deployment_id: DEP, role: 'operator', granted_at: 't' }], last_seen_at: null }], invitations: [{ invitation_id: 'i1', email: 'new@example.test', administrator: false, grants: [{ deployment_id: DEP, role: 'viewer' }], created_at: 't', created_by: 'owner', expires_at: '2026-09-06T00:00:00Z' }], roles: ['access', 'viewer', 'operator', 'administrator'], owners: ['owner@example.test'] },
    'telegram.list': { configured: true, chats: [{ chat_id: 4242, email: 'owner@example.test', label: 'Owner', linked_at: 't', subscriptions: ['server', `deployment:${DEP}`] }], outbox_pending: 0, last_poll_at: new Date().toISOString(), last_error: null },
    ping: { daemon_version: '0.1.0', schema_version: 11, socket: '/run/x.sock' },
    'plan.overview': {
      repository_id: REPO, display_name: 'repo-one',
      releases: scenario.empty ? [] : [
        { release_id: V_EARLY, name: 'Early look', kind: 'preview', status: 'delivered', seq: 1, note: null, requested_at: '2026-08-20T10:00:00Z', delivered_at: '2026-08-20T12:00:00Z', url: null, port: 20005, tasks_total: 1, tasks_done: 1, loc_total: 150, loc_done: 150 },
        { release_id: V_DONE, name: 'First look', kind: 'preview', status: 'delivered', seq: 2, note: null, requested_at: '2026-08-21T10:00:00Z', delivered_at: '2026-08-21T12:00:00Z', url: `https://app-dev.${BASE}`, port: 20002, tasks_total: 1, tasks_done: 1, loc_total: 400, loc_done: 400 },
        { release_id: V_R1, name: 'Release 1', kind: 'release', status: 'planned', seq: 3, note: null, requested_at: null, delivered_at: null, url: null, port: null, tasks_total: 2, tasks_done: 0, loc_total: 300, loc_done: 0 },
        { release_id: V_R2, name: 'Release 2', kind: 'release', status: 'planned', seq: 4, note: null, requested_at: null, delivered_at: null, url: null, port: null, tasks_total: 1, tasks_done: 0, loc_total: 800, loc_done: 0 }],
      tasks: scenario.empty ? [] : [
        { task_id: 'p1111111111111100', parent_task_id: null, release_id: V_EARLY, seq: 0, position: 1, title: 'First greeting text', impact: null, status: 'done', kind: 'goal', estimated_loc: 150 },
        { task_id: P_D1, parent_task_id: null, release_id: V_DONE, seq: 1, position: 1, title: 'Show the welcome page', impact: null, status: 'done', kind: 'goal', estimated_loc: 400, elaboration_needed: true },
        { task_id: P_PAR, parent_task_id: null, release_id: V_R1, seq: 2, position: 1, title: 'Sign-in works', impact: 'Nobody can sign in yet.', status: 'planned', kind: 'goal', estimated_loc: null },
        { task_id: P_C1, parent_task_id: P_PAR, release_id: V_R1, seq: 3, position: 1, title: 'Sign-in form', impact: null, status: 'in_progress', kind: 'goal', estimated_loc: 300 },
        { task_id: P_G1, parent_task_id: P_C1, release_id: V_R1, seq: 4, position: 1, title: 'E-mail field checks its spelling', impact: null, status: 'planned', kind: 'stub', estimated_loc: 100 },
        { task_id: P_C2, parent_task_id: P_PAR, release_id: V_R1, seq: 5, position: 2, title: 'Wrong password message', impact: null, status: 'planned', kind: 'stub', estimated_loc: 200, technical_note: 'tech-note-marker-must-not-render' },
        { task_id: P_LT, parent_task_id: null, release_id: V_R2, seq: 6, position: 1, title: 'A very long task title that keeps going and going for quite a while so wrapping is exercised', impact: null, status: 'planned', kind: 'improvement', estimated_loc: 800 },
        { task_id: P_UNSIZED_PARENT, parent_task_id: null, release_id: V_R1, seq: 7, position: 0, title: 'Check the release before people use it', impact: 'The final checks must be easy to inspect as one group.', status: 'planned', kind: 'goal', estimated_loc: null },
        { task_id: P_UNSIZED, parent_task_id: P_UNSIZED_PARENT, release_id: V_R1, seq: 8, position: 1, title: 'Check the complete release in a real browser', impact: 'A release can look finished in code while visible journeys still fail.', status: 'planned', kind: 'improvement', estimated_loc: null },
        { task_id: P_FB, parent_task_id: null, release_id: null, seq: 9, position: 1, title: 'Make the export faster', impact: 'Big exports take minutes.', status: 'planned', kind: 'user_feedback', estimated_loc: 150 },
        ...denseUnsizedTasks],
      tasks_truncated: false, preview_requested: [],
      elaboration_requests: scenario.empty ? [] : [{ task_id: P_D1, title: 'Show the welcome page', outcome: 'Show the welcome page', status: 'done', kind: 'goal', requested_at: '2026-08-30T12:00:00Z' }],
      decisions: { unsummarized_count: scenario.empty ? 0 : 4, summary_due: false },
    },
    'plan.overview-list': { repositories: scenario.empty ? [] : [
      { repository_id: REPO, display_name: 'repo-one', open_tasks: 4, loc_done: 550, loc_total: 1800, current_release: { name: 'Release 1', kind: 'release', status: 'planned' }, preview_requested: false, elaboration_request_count: 1 },
      { repository_id: 'r2', display_name: LONG, open_tasks: 0, loc_done: 0, loc_total: 0, current_release: null, preview_requested: true, elaboration_request_count: 0 }] },
    'decision.tail': {
      repository_id: REPO, display_name: 'repo-one',
      summary: scenario.empty ? null : { body: 'The story so far: the app greets people plainly and exports are files.', covers_through_seq: 40, created_at: '2026-08-20T10:00:00Z' },
      decisions: scenario.empty ? [] : [
        { decision_id: 'n0000000000000041', seq: 41, ref: null, aspect: 'ui', title: 'Buttons were blue', body: 'Primary buttons used the blue accent.', technical_note: null, superseded_by: 'n0000000000000044', created_at: '2026-08-21T09:00:00Z', created_by: 'uid:1000' },
        { decision_id: 'n0000000000000042', seq: 42, ref: 'REPO-EXPORT-FILES', aspect: 'business_logic', title: 'Exports are downloadable files', body: 'People asked to keep their data, so every export produces a file they can save.', technical_note: 'csv via streaming writer', superseded_by: null, created_at: '2026-08-22T09:00:00Z', created_by: 'uid:1000' },
        { decision_id: 'n0000000000000043', seq: 43, ref: null, aspect: 'testing', title: 'Every page gets a browser test', body: 'Each page is exercised in a real browser before a release is delivered.', technical_note: null, superseded_by: null, created_at: '2026-08-23T09:00:00Z', created_by: 'uid:1000' },
        { decision_id: 'n0000000000000044', seq: 44, ref: null, aspect: 'ui', title: 'Buttons are green now', body: 'After trying the preview the owner preferred green buttons.', technical_note: null, superseded_by: null, created_at: '2026-08-24T09:00:00Z', created_by: 'uid:1000' }],
      has_more: !scenario.empty, unsummarized_count: scenario.empty ? 0 : 4, summary_due: false,
    },
    'decision.search': { repository_id: REPO, query: 'export', decisions: scenario.empty ? [] : [
      { decision_id: 'n0000000000000042', seq: 42, ref: 'REPO-EXPORT-FILES', aspect: 'business_logic', title: 'Exports are downloadable files', body: 'People asked to keep their data, so every export produces a file they can save.', technical_note: 'csv via streaming writer', superseded_by: null, created_at: '2026-08-22T09:00:00Z', created_by: 'uid:1000' }], has_more: false },
  };
};

const SCENARIOS = {
  populated: { identity: 'owner@example.test', admin: true },
  empty: { identity: 'owner@example.test', admin: true, empty: true },
  error: { identity: 'owner@example.test', admin: true, error: true },
  loading: { identity: 'owner@example.test', admin: true, delayMs: 4000 },
  denied: { identity: 'dev@example.test', admin: false, denied: true },
  applying: { identity: 'owner@example.test', admin: true, applying: true },
  usageComplete: { identity: 'owner@example.test', admin: true, usageComplete: true, targetedOnly: true },
  usageUnavailable: { identity: 'owner@example.test', admin: true, usageUnavailable: true, targetedOnly: true },
  progressPartial: { identity: 'owner@example.test', admin: true, partial: true, targetedOnly: true },
};
const VIEWS = ['#/deployments', `#/deployments/${DEP}`, '#/plan', `#/plan/${REPO}`, '#/progress', `#/progress/${REPO}`, '#/usage', `#/usage/${REPO}`, '#/decisions', `#/decisions/${REPO}`, '#/tests', '#/health', '#/health/containers', '#/bugs', '#/admin'];
const VIEWPORTS = { wide: { width: 1280, height: 800 }, narrow: { width: 390, height: 844 } };
const destinationHref = (view) => {
  if (view.startsWith('#/deployments')) return '#/deployments';
  if (view.startsWith('#/plan')) return '#/plan';
  if (view.startsWith('#/progress')) return '#/progress';
  if (view.startsWith('#/usage')) return '#/usage';
  if (view.startsWith('#/decisions')) return '#/decisions';
  if (view.startsWith('#/tests')) return '#/tests';
  if (view.startsWith('#/health')) return '#/health';
  if (view.startsWith('#/bugs')) return '#/bugs';
  return '#/admin';
};
const PROJECT_DETAIL_VIEWS = new Set([`#/plan/${REPO}`, `#/progress/${REPO}`, `#/usage/${REPO}`, `#/decisions/${REPO}`]);
const ADMIN_ONLY = ['health.summary', 'health.containers', 'health.container_remove', 'user.list', 'user.invite', 'user.remove', 'grant.set', 'grant.remove', 'test.list', 'test.start', 'test.stop', 'test.output', 'deployment.apply', 'deployment.rollback', 'deployment.remove', 'deployment.set_domain', 'task.create', 'task.update', 'release.create', 'release.update', 'release.request', 'release.deliver', 'decision.record', 'decision.summarize'];
const OPERATOR_ONLY = ['usage.repositories', 'usage.repository', 'progress.repositories', 'progress.repository'];

async function startFakeDaemon(dir) {
  const socketPath = path.join(dir, 'daemon.sock');
  let scenario = SCENARIOS.populated;
  const calls = [];
  const mutable = { stopped: false, serviceStopped: false, taskUpdates: new Map(), createdTasks: [], previewRequested: false, failNextTaskUpdate: false };
  const planOverview = () => {
    const result = fixtures(scenario)['plan.overview'];
    result.tasks = result.tasks
      .map((task) => ({ ...task, ...(mutable.taskUpdates.get(task.task_id) || {}) }))
      .filter((task) => task.status !== 'dropped')
      .concat(mutable.createdTasks);
    result.elaboration_requests = result.tasks.filter((task) => task.elaboration_needed).map((task) => ({
      task_id: task.task_id, title: task.title, outcome: task.outcome || task.title,
      status: task.status, kind: task.kind, requested_at: '2026-08-30T12:00:00Z',
    }));
    if (mutable.previewRequested) result.preview_requested = [{ release_id: 'v0000000000000099', name: 'Preview', requested_at: new Date().toISOString(), note: null }];
    return result;
  };
  const server = net.createServer({ allowHalfOpen: true }, (socket) => {
    let buf = '';
    socket.on('data', async (c) => {
      buf += c; if (!buf.endsWith('\n')) return;
      const req = JSON.parse(buf); calls.push(req);
      const reply = (payload) => socket.end(`${JSON.stringify({ protocol: 1, id: req.id, ...payload })}\n`);
      if (scenario.delayMs) await new Promise((r) => setTimeout(r, scenario.delayMs));
      const cmd = req.command;
      if (cmd === 'user.whoami' && process.env.CONSOLE_VERIFY_RESET_PLAN_ON_SESSION === '1') {
        mutable.taskUpdates.clear();
        mutable.createdTasks.length = 0;
        mutable.previewRequested = false;
        mutable.failNextTaskUpdate = false;
      }
      if (scenario.error && cmd !== 'user.whoami') return reply({ ok: false, error: { code: 'internal_error', message: 'simulated daemon fault', detail: '' } });
      if (scenario.denied && ADMIN_ONLY.includes(cmd)) return reply({ ok: false, error: { code: 'permission_denied', message: `${cmd} requires administrator`, detail: '' } });
      if (scenario.denied && OPERATOR_ONLY.includes(cmd)) return reply({ ok: false, error: { code: 'permission_denied', message: `${cmd} requires operator`, detail: '' } });
      if (cmd === 'deployment.stop' && req.args.component === 'stack/projection-worker') { mutable.serviceStopped = true; return reply({ ok: true, result: { state: 'degraded' } }); }
      if (cmd === 'deployment.start' && req.args.component === 'stack/projection-worker') { mutable.serviceStopped = false; return reply({ ok: true, result: { state: 'running' } }); }
      if (cmd === 'deployment.stop') { mutable.stopped = true; return reply({ ok: true, result: { state: 'stopped' } }); }
      if (cmd === 'deployment.start') { mutable.stopped = false; return reply({ ok: true, result: { state: 'running' } }); }
      if (cmd === 'deployment.status' && req.args.deployment_id === OBS) return reply({ ok: true, result: fixtures({ ...scenario, stopped: mutable.stopped, serviceStopped: mutable.serviceStopped })['deployment.observed-status'] });
      if (cmd === 'deployment.set_domain') return reply({ ok: true, result: { deployment_id: req.args.deployment_id, domain: req.args.domain, public: !!req.args.public } });
      if (cmd === 'task.update') {
        if (mutable.failNextTaskUpdate) { mutable.failNextTaskUpdate = false; return reply({ ok: false, error: { code: 'simulated_failure', message: 'simulated task update failure', detail: '' } }); }
        const previous = mutable.taskUpdates.get(req.args.task_id) || {};
        const update = { ...previous };
        for (const key of ['estimated_loc', 'status', 'release_id', 'parent_task_id', 'position', 'title', 'outcome', 'elaboration_needed']) if (key in req.args) update[key] = req.args[key];
        mutable.taskUpdates.set(req.args.task_id, update);
        return reply({ ok: true, result: { task_id: req.args.task_id, ...update, state: 'done', status: update.status || 'planned' } });
      }
      if (cmd === 'task.create') {
        const task = { task_id: `pnew${String(mutable.createdTasks.length + 1).padStart(13, '0')}`, parent_task_id: null, release_id: null, seq: 100 + mutable.createdTasks.length, position: 100 + mutable.createdTasks.length, title: req.args.title, impact: req.args.impact || null, status: 'planned', kind: req.args.kind, estimated_loc: null, elaboration_needed: false };
        mutable.createdTasks.push(task);
        return reply({ ok: true, result: { task_id: task.task_id, status: task.status } });
      }
      if (cmd === 'release.request') { mutable.previewRequested = true; return reply({ ok: true, result: { status: 'requested' } }); }
      if (['deployment.restart', 'deployment.apply', 'deployment.rollback', 'deployment.remove', 'bug.report', 'bug.close', 'user.invite', 'user.remove', 'grant.set', 'grant.remove', 'telegram.link', 'telegram.subscribe', 'telegram.unsubscribe', 'test.stop', 'test.start', 'health.container_remove'].includes(cmd)) return reply({ ok: true, result: { state: 'done', status: 'done' } });
      if (cmd === 'plan.overview' && !req.args.repository_id) return reply({ ok: true, result: fixtures(scenario)['plan.overview-list'] });
      if (cmd === 'plan.overview') return reply({ ok: true, result: planOverview() });
      if (cmd === 'progress.repository') return reply({ ok: true, result: progressFixture(scenario, req.args.period || 'day') });
      if (cmd === 'progress.repositories') return reply({ ok: true, result: fixtures(scenario)['progress.repositories'] });
      if (cmd === 'usage.repository') {
        const result = structuredClone(fixtures({ ...scenario, stopped: mutable.stopped,
          serviceStopped: mutable.serviceStopped })['usage.repository']);
        result.range = req.args.range || '24h';
        const count = result.range === '30d' ? 30 : result.range === '7d' ? 28 : 24;
        const step = result.range === '30d' ? 86400000 : result.range === '7d' ? 21600000 : 3600000;
        if (result.series.length) result.series = Array.from({ length: count }, (_, index) => {
          const source = result.series[index % result.series.length];
          const start = Date.UTC(2026, 7, 29) - (count - index) * step;
          return { ...source, bucket_start_ms: start, bucket_end_ms: start + step };
        });
        return reply({ ok: true, result });
      }
      if (cmd === 'usage.repositories') {
        const result = structuredClone(fixtures(scenario)['usage.repositories']);
        result.range = req.args.range || '24h';
        return reply({ ok: true, result });
      }
      const data = fixtures({ ...scenario, stopped: mutable.stopped, serviceStopped: mutable.serviceStopped })[cmd];
      if (data === undefined) return reply({ ok: false, error: { code: 'command_unknown', message: cmd, detail: '' } });
      return reply({ ok: true, result: data });
    });
  });
  await new Promise((r) => server.listen(socketPath, r));
  return {
    socketPath,
    calls,
    setScenario: (s) => { scenario = s; mutable.stopped = false; mutable.serviceStopped = false; mutable.taskUpdates.clear(); mutable.createdTasks.length = 0; mutable.previewRequested = false; mutable.failNextTaskUpdate = false; calls.length = 0; },
    failNextTaskUpdate: () => { mutable.failNextTaskUpdate = true; },
    close: () => new Promise((r) => server.close(r)),
  };
}

async function main() {
  await fs.mkdir(OUT, { recursive: true });
  const tmp = await fs.mkdtemp(path.join(os.tmpdir(), 'dc2-verify-'));
  const daemon = await startFakeDaemon(tmp);
  const payload = { generation: 1, published_at: 'x', domain: BASE, routes: [], access: { owners: ['owner@example.test'], grants: [] } };
  await fs.writeFile(path.join(tmp, 'routes.json'), JSON.stringify({ schema: 1, payload_sha256: crypto.createHash('sha256').update(canonicalJson(payload)).digest('hex'), ...payload }));
  const secret = 'console-verify-secret-0123456789';
  const edge = await createEdge({ baseDomain: BASE, consoleHost: HOST, httpPort: 0, httpOnly: true, sessionSecret: secret, oidcIssuer: 'http://127.0.0.1:1/', oidcClientId: '', oidcClientSecret: '', routesFile: path.join(tmp, 'routes.json'), stateDir: path.join(tmp, 'edge-state'), daemonSocket: daemon.socketPath, consoleDir: path.resolve(path.dirname(new URL(import.meta.url).pathname)) }, { log: { info() {}, warn() {}, error: (...a) => console.error('edge', ...a), debug() {} } });
  const [port] = await edge.listen();
  const sessions = createSessionManager({ secret, ttlMs: 3600000, cookieName: 'dc2_session', cookieDomain: `.${BASE}`, secure: false });
  const holdFile = process.env.CONSOLE_VERIFY_HOLD_FILE;
  if (holdFile) {
    const scenarioName = process.env.CONSOLE_VERIFY_HOLD_SCENARIO || 'populated';
    const scenario = SCENARIOS[scenarioName];
    if (!scenario) throw new Error(`unknown hold scenario ${scenarioName}`);
    daemon.setScenario(scenario);
    const { cookie } = sessions.issue({ sub: 'sub', email: scenario.identity, name: 'Verifier' });
    const holdView = process.env.CONSOLE_VERIFY_HOLD_VIEW || `#/deployments/${DEP}`;
    const targetUrl = `http://${HOST}:${port}/${holdView}`;
    let previewServer = null;
    let previewUrl = targetUrl;
    if (process.env.CONSOLE_VERIFY_SHARE_PREVIEW === '1') {
      const [sessionCookie] = cookie.split(';');
      previewServer = http.createServer((_request, response) => {
        response.writeHead(302, {
          location: targetUrl,
          'set-cookie': `${sessionCookie}; Path=/; HttpOnly; SameSite=Lax`,
          'cache-control': 'no-store',
        });
        response.end();
      });
      await new Promise((resolve) => previewServer.listen(0, '127.0.0.1', resolve));
      previewUrl = `http://127.0.0.1:${previewServer.address().port}/`;
    }
    const receipt = {
      url: previewUrl,
      target_url: targetUrl,
      cookie: cookie.split(';')[0],
      scenario: scenarioName,
    };
    await fs.writeFile(holdFile, JSON.stringify(receipt));
    console.log(JSON.stringify({ fixture: holdFile, ...receipt, cookie: 'redacted' }));
    await new Promise((resolve) => {
      process.once('SIGINT', resolve);
      process.once('SIGTERM', resolve);
    });
    if (previewServer) await new Promise((resolve) => previewServer.close(resolve));
    await edge.close();
    await daemon.close();
    return;
  }
  const browser = await pw.chromium.launch({ args: [`--host-resolver-rules=MAP *.${BASE} 127.0.0.1`] });
  const report = { checks: [], failures: [] };
  const check = (name, ok, detail = '') => { report.checks.push({ name, ok, detail }); if (!ok) report.failures.push(`${name}: ${detail}`); };

  if (!process.env.CONSOLE_VERIFY_INTERACTIONS_ONLY) for (const [scenarioName, scenario] of Object.entries(SCENARIOS).filter(([, scenario]) => !scenario.targetedOnly)) {
    daemon.setScenario(scenario);
    for (const [vpName, viewport] of Object.entries(VIEWPORTS)) {
      const context = await browser.newContext({ viewport, ignoreHTTPSErrors: true });
      const { cookie } = sessions.issue({ sub: 'sub', email: scenario.identity, name: 'Verifier' });
      const [nameValue] = cookie.split(';');
      await context.addCookies([{ name: 'dc2_session', value: nameValue.split('=')[1], domain: `.${BASE}`, path: '/' }]);
      const page = await context.newPage();
      page.on('dialog', (d) => d.accept());
      for (const view of VIEWS) {
        const label = `${scenarioName}-${view.replace(/[#/]+/g, '_').replace(/^_/, '')}-${vpName}`;
        await page.goto(`http://${HOST}:${port}/${view}`);
        if (scenario.delayMs) { await page.waitForTimeout(500); }
        else { await page.waitForFunction(() => !document.querySelector('.skeleton'), null, { timeout: 15000 }).catch(() => {}); await page.waitForTimeout(300); }
        await page.screenshot({ path: path.join(OUT, `${label}.png`), fullPage: true });
        const metrics = await page.evaluate((expectedDestinationHref) => {
          const doc = document.documentElement;
          const overflow = doc.scrollWidth - window.innerWidth;
          const clipped = [...document.querySelectorAll('.tile .v, .health-capacity-value strong, .health-status-item > span, .health-storage-breakdown dt, .health-storage-breakdown dd, h1, .page-heading > strong, .toast')].filter((el) => el.scrollWidth > el.clientWidth + 1).map((el) => el.textContent.slice(0, 40));
          const buttons = [...document.querySelectorAll('button')].map((b) => ({ text: b.textContent.trim(), visible: b.offsetParent !== null, disabled: b.disabled, x: b.getBoundingClientRect().right, scrollable: !!b.closest('.tablewrap, .gantt-viewport, .usage-chart-scroll') }));
          const offscreen = buttons.filter((b) => b.visible && b.x > window.innerWidth + 1 && !b.scrollable);
          const planControls = document.querySelectorAll('[data-move-task], [data-drag-task], #comment-form, [data-cmd="release.request"]').length;
          const elaborationControls = document.querySelectorAll('[data-elaborate-task]').length;
          const headingLink = document.querySelector('main h1 a.destination-link');
          const header = document.querySelector('header.top');
          const visibleHeaderChildren = [...header.children].filter((el) => el.offsetParent !== null);
          const centers = visibleHeaderChildren.map((el) => {
            const rect = el.getBoundingClientRect();
            return rect.top + rect.height / 2;
          });
          const headerRect = header.getBoundingClientRect();
          const healthSummary = document.querySelector('.health-summary');
          const healthStorageItems = [...document.querySelectorAll('.health-storage-breakdown > div')];
          const healthCapacityRects = [...document.querySelectorAll('.health-capacity-card')].map((element) => element.getBoundingClientRect());
          const healthCapacityRows = new Map();
          for (const rect of healthCapacityRects) {
            const top = Math.round(rect.top);
            if (!healthCapacityRows.has(top)) healthCapacityRows.set(top, []);
            healthCapacityRows.get(top).push(rect.height);
          }
          const healthTable = document.querySelector('.health-repository-table');
          const healthTableWrap = healthTable?.closest('.tablewrap');
          const healthIncidentHeading = document.querySelector('#health-incidents-title');
          const healthSummaryRect = healthSummary?.getBoundingClientRect();
          const healthStatusRect = document.querySelector('.health-status-panel')?.getBoundingClientRect();
          return {
            overflow, clipped, buttons: buttons.length, offscreen: offscreen.length,
            text: document.body.innerText.slice(0, 4000), skeleton: !!document.querySelector('.skeleton'),
            notice: document.querySelector('.notice')?.textContent || '', planControls,
            elaborationControls,
            headingHref: headingLink?.getAttribute('href') || '',
            expectedDestinationHref,
            headerHeight: headerRect.height,
            headerCenterSpread: centers.length ? Math.max(...centers) - Math.min(...centers) : 0,
            navToggleVisible: document.querySelector('#nav-toggle')?.offsetParent !== null,
            navVisible: document.querySelector('#nav')?.offsetParent !== null,
            projectPickerCount: document.querySelectorAll('[data-project-picker]').length,
            projectNativeSelectCount: document.querySelectorAll('[data-project-picker] select').length,
            health: healthSummary ? {
              capacityCards: healthCapacityRects.length,
              statusItems: document.querySelectorAll('.health-status-item').length,
              capacityRowHeightSpreads: [...healthCapacityRows.values()].map((heights) => Math.max(...heights) - Math.min(...heights)),
              primaryInInitialViewport: healthSummaryRect.top >= 0 && healthStatusRect.bottom <= window.innerHeight,
              primaryBeforeIncidents: healthSummaryRect.bottom <= healthIncidentHeading.getBoundingClientRect().top + 1,
              storageItems: healthStorageItems.map((element) => ({
                text: element.innerText,
                clipped: element.scrollWidth > element.clientWidth + 1 || element.scrollHeight > element.clientHeight + 1,
                left: element.getBoundingClientRect().left,
                right: element.getBoundingClientRect().right,
              })),
              tableDisplay: healthTable ? getComputedStyle(healthTable).display : '',
              tableScroll: healthTableWrap ? healthTableWrap.scrollWidth - healthTableWrap.clientWidth : 0,
            } : null,
          };
        }, destinationHref(view));
        check(`${label}: no horizontal document overflow`, metrics.overflow <= 0, `overflow ${metrics.overflow}px`);
        check(`${label}: no clipped headline text`, metrics.clipped.length === 0, metrics.clipped.join(' | '));
        check(`${label}: no off-canvas controls outside scroll containers`, metrics.offscreen === 0, `${metrics.offscreen} off-canvas`);
        check(`${label}: destination title links to its collection`, metrics.headingHref === metrics.expectedDestinationHref, `${metrics.headingHref} != ${metrics.expectedDestinationHref}`);
        check(`${label}: global header stays on one row`, metrics.headerHeight <= 64 && metrics.headerCenterSpread <= 2, `height ${metrics.headerHeight}px, center spread ${metrics.headerCenterSpread}px`);
        if (vpName === 'wide') {
          check(`${label}: wide header shows inline navigation`, metrics.navVisible && !metrics.navToggleVisible, JSON.stringify({ navVisible: metrics.navVisible, toggle: metrics.navToggleVisible }));
        } else {
          check(`${label}: narrow header collapses navigation`, !metrics.navVisible && metrics.navToggleVisible, JSON.stringify({ navVisible: metrics.navVisible, toggle: metrics.navToggleVisible }));
        }
        if (PROJECT_DETAIL_VIEWS.has(view) && ['populated', 'empty', 'applying'].includes(scenarioName)) {
          check(`${label}: project context uses one custom DOM picker`, metrics.projectPickerCount === 1 && metrics.projectNativeSelectCount === 0, JSON.stringify({ pickers: metrics.projectPickerCount, nativeSelects: metrics.projectNativeSelectCount }));
        }
        if (scenarioName === 'loading') check(`${label}: loading state visible`, metrics.skeleton || /Loading/.test(metrics.text));
        if (scenarioName === 'empty' && !view.includes(DEP) && view !== '#/admin') check(`${label}: explicit empty state`, /No (deployments|test runs|open bugs|containers|repositories|plan|decisions|provider-reported|open work)/.test(metrics.text), metrics.text.slice(0, 120));
        if (scenarioName === 'error') check(`${label}: error state with retry`, /Could not load|Cannot reach/.test(metrics.text) && /Retry/.test(metrics.text), metrics.text.slice(0, 120));
        if (scenarioName === 'denied' && (view === '#/admin' || view === '#/tests' || view === '#/health/containers')) check(`${label}: permission denied shown`, /Permission denied/.test(metrics.notice), metrics.notice.slice(0, 120));
        if (scenarioName === 'denied' && view.startsWith('#/usage')) check(`${label}: usage requires operator access`, /Permission denied/.test(metrics.notice), metrics.notice.slice(0, 120));
        if (scenarioName === 'denied' && view.startsWith('#/progress')) check(`${label}: progress requires operator access`, /Permission denied/.test(metrics.notice), metrics.notice.slice(0, 120));
        if (scenarioName === 'denied' && view === '#/health') check(`${label}: host health denied but repositories visible`, /administrator-only/.test(metrics.text) && /repo-one/.test(metrics.text));
        if (scenarioName === 'applying' && (view === '#/deployments' || view === `#/deployments/${DEP}`)) {
          const surface = view === '#/deployments'
            ? page.locator(`a[href="#/deployments/${DEP}"]`).locator('xpath=ancestor::tr')
            : page.locator('main');
          const enabledMutation = await surface.locator('[data-cmd^="deployment."]:not(:disabled)').count();
          check(`${label}: applying deployment has no enabled conflicting mutation`, enabledMutation === 0, `${enabledMutation} enabled`);
          if (view === `#/deployments/${DEP}`) check(`${label}: applying journey explains lost replies`, /Closing this page does not cancel/.test(metrics.text));
        }
        if (scenarioName === 'populated' && ['#/deployments', '#/tests', '#/health'].includes(view)) check(`${label}: long names rendered`, /going-and-going/.test(metrics.text), metrics.text.slice(0, 80));
        if (scenarioName === 'populated' && ['#/tests', '#/health'].includes(view)) check(`${label}: large numbers humanized`, /MiB|GiB|TiB/.test(metrics.text), metrics.text.slice(0, 80));
        if (scenarioName === 'populated' && view === '#/health') {
          check(`${label}: host capacity and operational status lead as one aligned summary`,
            metrics.health?.capacityCards === 4
            && metrics.health?.statusItems === 4
            && metrics.health?.capacityRowHeightSpreads.every((spread) => spread <= 1)
            && metrics.health?.primaryInInitialViewport
            && metrics.health?.primaryBeforeIncidents,
            JSON.stringify(metrics.health));
          check(`${label}: every shared storage category is visible and contained`,
            metrics.health?.storageItems.length === 5
            && metrics.health.storageItems.every((item) => !item.clipped && item.left >= -1 && item.right <= viewport.width + 1)
            && ['Docker shared', 'Docker images', 'Docker build cache', 'Docker shared volumes', 'Other'].every((name) => metrics.health.storageItems.some((item) => item.text.includes(name))),
            JSON.stringify(metrics.health?.storageItems));
          check(`${label}: repository attribution does not need horizontal scrolling`, metrics.health?.tableScroll <= 1, `scroll ${metrics.health?.tableScroll}px`);
        }
        if (scenarioName === 'populated' && view === `#/plan/${REPO}`) {
          check(`${label}: plan shows owner controls for the administrator`, metrics.planControls >= 3, `${metrics.planControls} controls`);
          check(`${label}: every visible task offers an elaboration request to the administrator`, metrics.elaborationControls >= 10, `${metrics.elaborationControls} controls`);
          check(`${label}: plan speaks plainly (no agent-facing technical note)`, !metrics.text.includes('tech-note-marker-must-not-render'));
          check(`${label}: sizes are in lines, statuses in plain words`, /lines/.test(metrics.text) && /being built/.test(metrics.text), metrics.text.slice(0, 120));
        }
        if (scenarioName === 'denied' && view === `#/plan/${REPO}`) check(`${label}: read-only plan for viewers (no owner controls)`, metrics.planControls === 0 && metrics.elaborationControls === 0, `${metrics.planControls} plan controls, ${metrics.elaborationControls} elaboration controls`);
        if (scenarioName === 'populated' && view === `#/usage/${REPO}`) {
          check(`${label}: repository and primary usage trend lead the page`, /Codex Usage/.test(metrics.text) && /repo-one/.test(metrics.text) && await page.locator('[data-ui-region="usage-primary-trend"] svg').count() === 1);
          check(`${label}: usage never exposes collector or captured-content identities`, !/agent-private|request-private|\/home\//.test(metrics.text));
          check(`${label}: usage provides exact chart values`, await page.locator('.usage-exact table').count() === 1);
        }
        if (scenarioName === 'populated' && view === `#/progress/${REPO}`) {
          check(`${label}: progress leads with the release forecast and delivery pulse`,
            /likely release date/i.test(metrics.text) && /Delivery pulse/.test(metrics.text)
            && await page.locator('[data-ui-region="progress-forecast"]').count() === 1
            && await page.locator('.progress-pulse-chart').count() === 1);
          check(`${label}: progress keeps both selected design modes available`,
            await page.locator('[data-progress-mode="pulse"]').count() === 1
            && await page.locator('[data-progress-mode="priorities"]').count() === 1
            && /Priority queue/.test(metrics.text));
          check(`${label}: progress labels estimated lines truthfully`,
            /Planned lines completed/.test(metrics.text)
            && /Current task estimates/.test(metrics.text)
            && !/Git lines completed/.test(metrics.text));
        }
      }
      await context.close();
    }
  }

  // Interaction proofs (populated, wide): controls call the API and the view re-renders.
  daemon.setScenario(SCENARIOS.populated);
  const context = await browser.newContext({ viewport: VIEWPORTS.wide });
  const { cookie } = sessions.issue({ sub: 'sub', email: 'owner@example.test' });
  await context.addCookies([{ name: 'dc2_session', value: cookie.split(';')[0].split('=')[1], domain: `.${BASE}`, path: '/' }]);
  const page = await context.newPage();
  page.on('dialog', (d) => d.accept());
  const pointerDrag = async (sourceSelector, targetSelector, targetPosition = {}) => {
    const source = page.locator(sourceSelector);
    const target = page.locator(targetSelector);
    await source.scrollIntoViewIfNeeded();
    const sourceBox = await source.boundingBox();
    const targetBox = await target.boundingBox();
    const start = { x: sourceBox.x + sourceBox.width / 2,
      y: sourceBox.y + sourceBox.height / 2 };
    const finish = { x: targetBox.x + (targetPosition.x ?? targetBox.width / 2),
      y: targetBox.y + (targetPosition.y ?? targetBox.height / 2) };
    await page.mouse.move(start.x, start.y);
    await page.mouse.down();
    await page.mouse.move(start.x + 4, start.y + 2, { steps: 2 });
    await page.mouse.move(finish.x, finish.y, { steps: 8 });
    await page.mouse.up();
  };
  await page.goto(`http://${HOST}:${port}/#/deployments/${DEP}`);
  await page.waitForSelector('button[data-cmd="deployment.stop"]');
  await page.click('h1 ~ .actions button[data-cmd="deployment.stop"]');
  await page.waitForFunction(() => [...document.querySelectorAll('h1 .badge')].some((b) => b.textContent === 'stopped'), null, { timeout: 10000 });
  check('interaction: stop calls deployment.stop and the header shows stopped', daemon.calls.some((c) => c.command === 'deployment.stop' && c.args.deployment_id === DEP && c.client.identity === 'owner@example.test'));
  await page.click('h1 ~ .actions button[data-cmd="deployment.start"]');
  await page.waitForFunction(() => [...document.querySelectorAll('h1 .badge')].some((b) => b.textContent === 'running'), null, { timeout: 10000 });
  check('interaction: start restores running', true);
  daemon.calls.length = 0;
  const projectionRow = page.locator('[data-compose-service="stack/projection-worker"]');
  await projectionRow.locator('[data-cmd="deployment.stop"]').click();
  await page.waitForFunction(() => [...document.querySelectorAll('h1 .badge')].some((b) => b.textContent === 'degraded'), null, { timeout: 10000 });
  check('interaction: independent Compose stop targets only the reviewed service',
    daemon.calls.some((c) => c.command === 'deployment.stop' && c.args.component === 'stack/projection-worker'));
  check('interaction: stopped Compose service remains visible and the route stays published',
    /stopped/.test(await projectionRow.innerText()) && /20002/.test(await page.innerText('main')));
  await projectionRow.locator('[data-cmd="deployment.start"]').click();
  await page.waitForFunction(() => [...document.querySelectorAll('h1 .badge')].some((b) => b.textContent === 'running'), null, { timeout: 10000 });
  check('interaction: independent Compose start restores running without a stack apply',
    daemon.calls.some((c) => c.command === 'deployment.start' && c.args.component === 'stack/projection-worker')
    && !daemon.calls.some((c) => c.command === 'deployment.apply'));
  await page.click('button[data-logs="api"]');
  await page.waitForSelector('pre.log');
  check('interaction: logs load on demand', daemon.calls.some((c) => c.command === 'deployment.logs' && c.args.component === 'api'));
  await page.click('button[data-cmd="deployment.remove"]');
  await page.waitForTimeout(500);
  const removeCall = daemon.calls.find((c) => c.command === 'deployment.remove');
  check('interaction: remove asks for confirmation and passes delete_data explicitly', removeCall && typeof removeCall.args.delete_data === 'boolean');
  // Domain editing (DC2-2026-08-24: administrators edit the routed domain in place).
  await page.goto(`http://${HOST}:${port}/#/deployments/${DEP}`);
  await page.waitForSelector('#edit-domain');
  await page.click('#edit-domain');
  await page.waitForSelector('dialog#domain-dialog[open]');
  await page.fill('#domain-form [name=domain]', 'renamed-app');
  await page.click('#domain-form button[type=submit]');
  await page.waitForTimeout(500);
  const domainCall = daemon.calls.find((c) => c.command === 'deployment.set_domain');
  check('interaction: domain pop-up calls deployment.set_domain with the new label',
    domainCall && domainCall.args.deployment_id === DEP && domainCall.args.domain === 'renamed-app');
  await page.goto(`http://${HOST}:${port}/#/deployments`);
  await page.waitForSelector('tr.grouphead');
  const groupHeads = await page.locator('tr.grouphead').allInnerTexts();
  check('deployments list groups rows under repository headers',
    groupHeads.length === 2 && groupHeads.some((t) => /repo-one/.test(t)) && groupHeads.some((t) => /legacy-repo/.test(t)),
    groupHeads.join(' | '));
  daemon.calls.length = 0;
  await page.click(`[data-edit-domain="${OBS}"]`);
  await page.waitForSelector('dialog#domain-dialog[open]');
  await page.fill('#domain-form [name=domain]', 'from-list');
  await page.click('#domain-form button[type=submit]');
  await page.waitForTimeout(500);
  const listDomainCall = daemon.calls.find((c) => c.command === 'deployment.set_domain');
  check('interaction: list-row ✎ opens the pop-up and edits that deployment',
    listDomainCall && listDomainCall.args.deployment_id === OBS && listDomainCall.args.domain === 'from-list');
  await page.goto(`http://${HOST}:${port}/#/deployments`);
  const observedRow = page.locator(`a[href="#/deployments/${OBS}"]`).locator('xpath=ancestor::tr');
  await observedRow.waitFor();
  check('observed deployment list row offers start/stop/restart',
    await observedRow.locator('[data-cmd="deployment.restart"]').count() === 1);
  check('observed deployment list row offers no apply',
    await observedRow.locator('[data-cmd="deployment.apply"]').count() === 0);
  check('observed deployment list row is explicitly labelled', /observed/i.test(await observedRow.innerText()));
  await page.goto(`http://${HOST}:${port}/#/deployments/${OBS}`);
  await page.waitForSelector('text=exact recorded containers');
  check('observed deployment detail offers lifecycle and log controls but no configuration authority',
    await page.locator('[data-cmd="deployment.restart"]').count() >= 1
    && await page.locator('[data-logs]').count() >= 1
    && await page.locator('[data-cmd="deployment.apply"], [data-cmd="deployment.rollback"], [data-cmd="deployment.remove"]').count() === 0);
  await page.click('h1 ~ .actions button[data-cmd="deployment.restart"]');
  await page.waitForTimeout(500);
  check('interaction: observed restart calls deployment.restart on the observed id',
    daemon.calls.some((c) => c.command === 'deployment.restart' && c.args.deployment_id === OBS));
  await page.goto(`http://${HOST}:${port}/#/tests`);
  await page.waitForSelector('button[data-out="stderr"]');
  await page.click('button[data-out="stderr"]');
  await page.waitForSelector('pre.log');
  check('interaction: test output loads on demand with bounded tail', daemon.calls.some((c) => c.command === 'test.output' && c.args.tail_bytes === 16384));
  await page.goto(`http://${HOST}:${port}/#/bugs`);
  await page.waitForSelector('#bug-form');
  for (const [f, v] of [['component', 'api'], ['summary', 'verify'], ['expected', 'a'], ['actual', 'b'], ['steps', 'c']]) await page.fill(`#bug-form [name=${f}]`, v);
  await page.click('#bug-form button[type=submit]');
  await page.waitForTimeout(500);
  check('interaction: bug report form calls bug.report', daemon.calls.some((c) => c.command === 'bug.report' && c.args.summary === 'verify'));
  await page.goto(`http://${HOST}:${port}/#/admin`);
  await page.waitForSelector('#invite-form');
  await page.fill('#invite-form [name=email]', 'new2@example.test');
  await page.click('#invite-form button[type=submit]');
  await page.waitForTimeout(500);
  check('interaction: invite form calls user.invite', daemon.calls.some((c) => c.command === 'user.invite' && c.args.email === 'new2@example.test'));
  await page.waitForFunction(() => /daemon 0\.1\.0/.test(document.querySelector('#server')?.textContent || ''), null, { timeout: 10000 });
  check('admin: the Server line renders daemon version, schema, and route generation',
    /daemon 0\.1\.0 · schema 11 · route document generation 1/.test(await page.innerText('#server')),
    await page.innerText('#server'));
  await page.goto(`http://${HOST}:${port}/#/health/containers`);
  await page.waitForSelector('button[data-cmd="health.container_remove"]');
  const removable = await page.$$('button[data-cmd="health.container_remove"]');
  check('interaction: only orphaned/test containers offer removal', removable.length === 1);
  const observedContainer = page.locator('tr', { hasText: 'existing-compose-stack-app-1' });
  check('observed container is labelled and non-removable',
    /observed-current/.test(await observedContainer.innerText()) &&
    await observedContainer.locator('button[data-cmd="health.container_remove"]').count() === 0);
  await removable[0].click();
  await page.waitForTimeout(500);
  check('interaction: container removal calls health.container_remove with the exact id', daemon.calls.some((c) => c.command === 'health.container_remove' && c.args.container_id === 'c'.repeat(64)));
  await page.goto(`http://${HOST}:${port}/#/health`);
  await page.waitForSelector('.card.bad-edge');
  const healthText = await page.innerText('body');
  check('health names the unhealthy component and its cause',
    /worker/.test(healthText) && /exited 1: boom/.test(healthText) && /healthcheck failing/.test(healthText));
  check('health offers actions on unhealthy deployments',
    await page.locator('.card.bad-edge [data-cmd="deployment.restart"]').count() >= 1);
  check('health keeps the container inventory and incident details directly reachable',
    await page.locator('.health-page-heading a[href="#/health/containers"]').count() === 1
    && await page.locator('.health-incident-card a[href^="#/deployments/"]').count() >= 2);
  await page.waitForSelector('.chartbox svg.chart');
  check('health charts render host history with a min–max band',
    await page.locator('.chartbox svg.chart .band').count() >= 3);
  daemon.calls.length = 0;
  await page.click('[data-health-range="7d"]');
  await page.waitForSelector('.chartbox svg.chart');
  await page.waitForTimeout(400);
  check('interaction: the 7d range requests a downsampled week of host history',
    daemon.calls.some((c) => c.command === 'health.history' && c.args.minutes === 10080 && c.args.points > 0));
  daemon.calls.length = 0;
  await page.click('[data-health-range="30d"]');
  await page.waitForSelector('.chartbox svg.chart');
  await page.waitForTimeout(400);
  check('interaction: the 30d range requests a downsampled month of host history',
    daemon.calls.some((c) => c.command === 'health.history' && c.args.minutes === 43200 && c.args.points > 0));
  for (const action of ['start', 'stop', 'restart']) {
    daemon.calls.length = 0;
    await page.click(`.health-incident-card [data-cmd="deployment.${action}"]`);
    await page.waitForSelector('.health-incident-card');
    await page.waitForTimeout(300);
    check(`interaction: Health ${action} acts on the selected unhealthy deployment`,
      daemon.calls.some((call) => call.command === `deployment.${action}` && call.args.deployment_id === 'd1111111111111111'));
  }
  await page.click('.health-page-heading a[href="#/health/containers"]');
  await page.waitForURL(/#\/health\/containers$/);
  await page.waitForFunction(() => document.querySelector('main h1 strong')?.textContent === 'Containers');
  await page.waitForSelector('table');
  check('interaction: View containers opens the real container inventory', /Containers/.test(await page.innerText('main h1')));
  await page.click('main a[href="#/health"]');
  await page.waitForURL(/#\/health$/);
  await page.waitForSelector('.health-incident-card');
  check('interaction: the container inventory returns to Health through its visible link', await page.locator('.health-summary').count() === 1);
  await page.click('.health-incident-card a.btn[href^="#/deployments/"]');
  await page.waitForURL(/#\/deployments\//);
  await page.waitForSelector('main h1 a[href="#/deployments"]');
  check('interaction: incident details opens the deployment detail route', await page.locator('main h1 a[href="#/deployments"]').count() === 1);
  daemon.setScenario(SCENARIOS.error);
  await page.goto(`http://${HOST}:${port}/#/health`);
  await page.waitForSelector('.notice .btn');
  daemon.setScenario(SCENARIOS.populated);
  await page.click('.notice .btn');
  await page.waitForSelector('.health-summary');
  check('interaction: Health recovers from a load failure through Retry', await page.locator('.health-summary').count() === 1);

  // Codex Usage: linked navigation, repository selection, truthful phase
  // chart, keyboard-preserving range changes, and accessible exact values.
  await page.goto(`http://${HOST}:${port}/#/usage`);
  await page.waitForSelector(`a[href="#/usage/${REPO}"]`);
  check('usage: the global navigation remains ordinary links',
    await page.locator('#nav a[href="#/usage"]').count() === 1
    && await page.locator('#nav button').count() === 0);
  const usageCollectionText = await page.innerText('[data-ui-region="codex-usage-repositories"]');
  check('usage: repository collection describes data inclusion without implementation jargon',
    /Data included/.test(usageCollectionText)
    && /All 4 configured Codex environments included/.test(usageCollectionText)
    && /Some usage may be missing/.test(usageCollectionText)
    && !/\bcollectors?\b|Partial coverage|Complete coverage/.test(usageCollectionText));
  await page.click(`a[href="#/usage/${REPO}"]`);
  await page.waitForSelector('.usage-phase-chart');
  check('usage: the destination title is a real collection link',
    await page.locator('main h1 a.destination-link[href="#/usage"]').count() === 1);
  check('usage: project choice is a custom DOM menu, never a native select',
    await page.locator('[data-project-picker]').count() === 1
    && await page.locator('[data-project-picker] select').count() === 0);
  const usageProjectToggle = page.locator('[data-project-picker-toggle]');
  await usageProjectToggle.click();
  await page.waitForSelector('[data-project-picker-menu]:not([hidden])');
  await page.waitForFunction(() => document.activeElement?.matches('[data-project-picker-menu] [role="menuitem"]'));
  check('interaction: the project menu lists every visible project as a real link',
    await page.locator('[data-project-picker-menu] [role="menuitem"]').count() === 2
    && await page.locator('[data-project-picker-menu] a[href="#/usage/r2"]').count() === 1);
  await page.keyboard.press('ArrowDown');
  check('interaction: arrow keys move focus through the project menu',
    await page.evaluate(() => document.activeElement?.getAttribute('href')) === '#/usage/r2');
  await page.keyboard.press('Escape');
  check('interaction: Escape closes the project menu and returns focus',
    await page.locator('[data-project-picker-menu][hidden]').count() === 1
    && await page.locator('[data-project-picker-toggle]:focus').count() === 1);
  await usageProjectToggle.click();
  await page.click('[data-project-picker-menu] a[href="#/usage/r2"]');
  await page.waitForURL(/#\/usage\/r2$/);
  await page.waitForFunction(() => /going-and-going/.test(document.querySelector('.project-picker-current')?.textContent || ''));
  check('interaction: choosing a project changes the usage route and visible project',
    /going-and-going/.test(await page.innerText('.project-picker-current')));
  await page.click('main h1 a.destination-link[href="#/usage"]');
  await page.waitForURL(/#\/usage$/);
  await page.waitForSelector('[data-ui-region="codex-usage-repositories"]');
  check('interaction: the linked destination title returns to the repository collection',
    await page.locator('[data-ui-region="codex-usage-repositories"]').count() === 1);
  await page.goto(`http://${HOST}:${port}/#/usage/${REPO}`);
  await page.waitForSelector('.usage-phase-chart');
  check('usage: provider total tokens are stacked by the six work phases',
    await page.locator('.usage-phase-chart rect').count() >= 6
    && /Planning/.test(await page.innerText('.usage-legend'))
    && /Unattributed/.test(await page.innerText('.usage-legend')));
  const axisTitleBox = await page.locator('.usage-axis-title').boundingBox();
  const topTickBox = await page.locator('.usage-y-label').last().boundingBox();
  check('usage: large scale labels cannot overlap the y-axis title',
    axisTitleBox && topTickBox && axisTitleBox.x + axisTitleBox.width + 4 <= topTickBox.x,
    JSON.stringify({ axisTitleBox, topTickBox }));
  check('usage: overlapping time measures remain separate rails',
    await page.locator('.usage-rail').count() === 3
    && /not added together/.test(await page.innerText('.usage-lower')));
  const usageContextText = await page.innerText('.usage-context');
  check('usage: missing data is explained before the chart in plain language',
    /Some usage may be missing/.test(usageContextText)
    && /data from 3 of 4 configured Codex environments/.test(usageContextText)
    && /separately configured local Codex setup with its own usage history/.test(usageContextText)
    && /excluded, never counted as zero/.test(usageContextText)
    && !/\bcollectors?\b|Partial coverage|measured values only|configured histories/.test(usageContextText));
  daemon.calls.length = 0;
  await page.click('[data-codex-range="7d"]');
  await page.waitForSelector('.usage-phase-chart');
  await page.waitForTimeout(400);
  check('interaction: the 7d range re-reads the selected repository and restores focus',
    daemon.calls.some((c) => c.command === 'usage.repository' && c.args.repository_id === REPO && c.args.range === '7d')
    && await page.locator('[data-codex-range="7d"]:focus').count() === 1);
  await page.click('.usage-provenance summary');
  check('interaction: coverage and provenance expands in place',
    await page.locator('.usage-provenance[open]').count() === 1);
  check('interaction: exact bucket values are available without hover',
    await page.locator('.usage-provenance[open] .usage-exact tbody tr').count() > 0);
  const completenessText = await page.innerText('.usage-provenance');
  check('usage: expanded details explain completeness and bucket status without collector jargon',
    /Data completeness/.test(completenessText)
    && /Counting method/.test(completenessText)
    && /Data status/.test(completenessText)
    && /Measured with gaps/.test(completenessText)
    && !/\bcollectors?\b|Partial coverage/.test(completenessText));

  for (const [scenarioName, scenario, expected, explanation] of [
    ['complete', SCENARIOS.usageComplete, 'All 4 configured Codex environments included', 'Every configured environment supplied measurable data for this repository and period.'],
    ['no-measurement', SCENARIOS.empty, 'No usage measured in this period', 'The configured environments contained no measured usage for this repository and period.'],
    ['unavailable', SCENARIOS.usageUnavailable, 'Usage unavailable · no configured Codex environment supplied data', 'No configured environment supplied data for this repository and period.'],
  ]) {
    daemon.setScenario(scenario);
    await page.reload();
    await page.waitForSelector('.usage-coverage-note');
    await page.waitForFunction((expectedText) =>
      (document.querySelector('.usage-context')?.innerText || '').includes(expectedText), expected);
    const contextText = await page.innerText('.usage-context');
    check(`usage: ${scenarioName} state explains what data is included`,
      contextText.includes(expected) && contextText.includes(explanation)
      && !/\bcollectors?\b|Partial coverage|Complete coverage|measured values only|configured histories/.test(contextText),
      contextText.slice(0, 240));
  }
  daemon.setScenario(SCENARIOS.populated);

  // Progress: both selected dashboard modes, truthful comparisons, local
  // prioritization, period reads, partial coverage, and Plan continuation.
  await page.goto(`http://${HOST}:${port}/#/progress`);
  await page.waitForSelector(`a[href="#/progress/${REPO}"]`);
  check('progress: the operator navigation and repository collection are available',
    await page.locator('#nav a[href="#/progress"]').count() === 1
    && /Release 1/.test(await page.innerText('main')));
  await page.click(`a[href="#/progress/${REPO}"]`);
  await page.waitForSelector('.progress-pulse-chart');
  check('progress: destination title links to its collection',
    await page.locator('main h1 a.destination-link[href="#/progress"]').count() === 1);
  await page.click('main h1 a.destination-link[href="#/progress"]');
  await page.waitForURL(/#\/progress$/);
  await page.waitForSelector(`a[href="#/progress/${REPO}"]`);
  check('interaction: the Progress title returns to the repository collection',
    await page.locator(`a[href="#/progress/${REPO}"]`).count() >= 1);
  await page.click(`a[href="#/progress/${REPO}"]`);
  await page.waitForSelector('.progress-pulse-chart');
  check('progress: the repository project menu lists real same-destination links',
    await page.locator('[data-project-picker-menu] a[href="#/progress/r2"]').count() === 1);
  await page.click('[data-project-picker-toggle]');
  await page.click('[data-project-picker-menu] a[href="#/progress/r2"]');
  await page.waitForURL(/#\/progress\/r2$/);
  await page.waitForFunction(() => /going-and-going/.test(document.querySelector('.project-picker-current')?.textContent || ''));
  check('interaction: the Progress project menu switches within Progress',
    /going-and-going/.test(await page.innerText('.project-picker-current')));
  await page.goto(`http://${HOST}:${port}/#/progress/${REPO}`);
  await page.waitForSelector('.progress-pulse-chart, .progress-priority-view');
  daemon.calls.length = 0;
  await page.click('[data-progress-mode="priorities"]');
  await page.waitForSelector('.progress-priority-view');
  check('interaction: switching to Priorities is local and exposes the full ranked view',
    !daemon.calls.some((call) => call.command === 'progress.repository')
    && await page.locator('.progress-priority-full [data-progress-task]').count() === 3
    && /Selection changes the comparison only/.test(await page.innerText('.progress-priority-view')));
  await page.click('[data-progress-mode="pulse"]');
  await page.waitForSelector('.progress-pulse-view');
  check('interaction: switching back to Delivery pulse is local',
    !daemon.calls.some((call) => call.command === 'progress.repository')
    && await page.locator('.progress-pulse-chart').count() === 1);
  await page.click('[data-progress-show-priorities]');
  await page.waitForSelector('.progress-priority-view');
  check('interaction: View full ranked table opens the complete Priorities mode',
    await page.locator('[data-progress-mode="priorities"].active').count() === 1);
  daemon.calls.length = 0;
  for (const taskId of [P_C2, P_UNSIZED, P_G1]) {
    await page.click(`[data-progress-task="${taskId}"]`);
    check(`interaction: priority row ${taskId} is selectable`,
      await page.locator(`[data-progress-task="${taskId}"].selected`).count() === 1);
  }
  check('interaction: selecting priority work updates the scenarios without mutating the plan',
    await page.locator(`[data-progress-task="${P_G1}"].selected`).count() === 1
    && !daemon.calls.some((call) => call.command === 'task.update')
    && /nothing is changed automatically/.test(await page.innerText('.progress-scenarios')));
  daemon.calls.length = 0;
  await page.click('[data-progress-period="hour"]');
  await page.waitForTimeout(300);
  check('interaction: hourly progress reads hourly repository buckets',
    daemon.calls.some((call) => call.command === 'progress.repository'
      && call.args.repository_id === REPO && call.args.period === 'hour')
    && await page.locator('[data-progress-period="hour"]:focus').count() === 1);
  daemon.calls.length = 0;
  await page.click('[data-progress-period="day"]');
  await page.waitForTimeout(300);
  check('interaction: daily progress reads daily repository buckets',
    daemon.calls.some((call) => call.command === 'progress.repository'
      && call.args.repository_id === REPO && call.args.period === 'day')
    && await page.locator('[data-progress-period="day"]:focus').count() === 1);
  daemon.calls.length = 0;
  await page.click('[data-progress-period="week"]');
  await page.waitForSelector('.progress-pulse-chart, .progress-priority-view');
  await page.waitForTimeout(400);
  check('interaction: weekly progress re-reads aligned repository buckets and restores focus',
    daemon.calls.some((call) => call.command === 'progress.repository'
      && call.args.repository_id === REPO && call.args.period === 'week')
    && await page.locator('[data-progress-period="week"]:focus').count() === 1);
  await page.click('.progress-exact summary');
  check('interaction: exact progress values and counting rules expand in place',
    await page.locator('.progress-exact[open] tbody tr').count() === 8
    && /not measured Git changes/.test(await page.innerText('.progress-exact')));
  await page.click(`.progress-actions a[href="#/plan/${REPO}"]`);
  await page.waitForURL(new RegExp(`#\\/plan\\/${REPO}$`));
  await page.waitForSelector('.gantt');
  check('interaction: Open full plan navigates to the repository plan',
    await page.locator('.gantt').count() === 1);
  await page.goto(`http://${HOST}:${port}/#/progress/${REPO}`);
  await page.waitForSelector('.progress-pulse-chart, .progress-priority-view');
  daemon.setScenario(SCENARIOS.progressPartial);
  await page.reload();
  await page.waitForSelector('.progress-coverage');
  check('progress: partial evidence is visible and never described as zero',
    /partial data/.test(await page.innerText('.progress-coverage'))
    && /Gaps stay blank rather than becoming zero/.test(await page.innerText('.progress-coverage')));
  daemon.setScenario(SCENARIOS.populated);
  await page.reload();
  await page.waitForSelector('.progress-pulse-chart');
  await page.click('[data-progress-mode="priorities"]');
  await page.waitForSelector('.progress-priority-view');
  await page.click(`[data-progress-task="${P_C2}"]`);
  await page.click('[data-progress-open-task]');
  await page.waitForURL(new RegExp(`#\\/plan\\/${REPO}$`));
  await page.waitForSelector(`[data-task-row="${P_C2}"].selected`);
  check('interaction: Open selected in plan continues to the exact real task',
    await page.locator(`[data-task-row="${P_C2}"].selected`).count() === 1);

  // Plan: immersive Gantt navigation, direct manipulation, accessible
  // alternatives, persistence, recovery, preview request, feedback, and drop.
  await page.goto(`http://${HOST}:${port}/#/plan/${REPO}`);
  await page.waitForSelector('.gantt');
  await page.click('[data-project-picker-toggle]');
  await page.click('[data-project-picker-menu] a[href="#/plan/r2"]');
  await page.waitForURL(/#\/plan\/r2$/);
  await page.waitForFunction(() => /going-and-going/.test(document.querySelector('.project-picker-current')?.textContent || ''));
  check('interaction: the Plan project menu switches the project route',
    /going-and-going/.test(await page.innerText('.project-picker-current')));
  await page.goto(`http://${HOST}:${port}/#/plan/${REPO}`);
  await page.waitForSelector('.gantt');
  check('plan: delivered preview links to the running app',
    await page.locator(`a[href="https://app-dev.${BASE}"]`).count() === 1);
  check('plan: a delivered preview without a domain names its server port',
    /runs on server port 20005/.test(await page.innerText('body')));
  check('plan: the selected task tray is visible without displacing the chart',
    await page.locator('[data-ui-region="selected-task"]').count() === 1
    && await page.locator('[data-ui-region="plan-primary"]').count() === 1);
  check('plan: the delivered task exposes no move, resize, or drag control',
    await page.locator(`[data-task-row="${P_D1}"] [data-drag-task], [data-resize-handle="${P_D1}"], [data-move-task="${P_D1}"]`).count() === 0);
  check('plan: work without an estimate still has a selectable chart mark',
    await page.locator(`.gbar.unsized[data-select-task="${P_UNSIZED}"]`).count() === 1
    && /Not estimated/.test(await page.innerText('.gaxis')));
  check('plan: every task row has an elaboration action, including completed work',
    await page.locator('.gtask [data-elaborate-task]').count() === await page.locator('.gtask').count());
  check('plan: an outstanding request is visible and cannot be submitted twice',
    await page.locator(`[data-task-row="${P_D1}"] [data-elaborate-task][aria-disabled="true"]`).count() === 1
    && /Requested/.test(await page.innerText(`[data-task-row="${P_D1}"]`)));

  daemon.calls.length = 0;
  await page.locator('.plan-workspace').evaluate((element) => { element.dataset.identityProof = 'same-workspace'; });
  await page.click(`[data-task-row="${P_C2}"] [data-elaborate-task]`);
  await page.waitForFunction((taskId) => document.querySelector(
    `[data-task-row="${taskId}"] [data-elaborate-task]`)?.getAttribute(
    'aria-disabled') === 'true', P_C2);
  check('interaction: requesting elaboration persists the flag and updates the existing Plan in place',
    daemon.calls.some((call) => call.command === 'task.update' && call.args.task_id === P_C2 && call.args.elaboration_needed === true)
    && !daemon.calls.some((call) => call.command === 'plan.overview')
    && await page.locator('.plan-workspace[data-identity-proof="same-workspace"]').count() === 1
    && /Requested/.test(await page.innerText(`[data-task-row="${P_C2}"]`))
    && /2 tasks need a clearer explanation/.test(await page.innerText('[data-plan-elaboration-notice]')),
    JSON.stringify(daemon.calls));
  daemon.calls.length = 0; daemon.failNextTaskUpdate();
  await page.click(`[data-task-row="${P_G1}"] [data-elaborate-task]`);
  await page.waitForTimeout(500);
  check('interaction: a failed elaboration request reports the failure and stays retryable',
    await page.locator(`[data-task-row="${P_G1}"] [data-elaborate-task]:not(:disabled)`).count() === 1
    && !/elaboration needed/.test(await page.innerText(`[data-task-row="${P_G1}"]`))
    && (await page.locator('.toast.bad').allInnerTexts()).some((text) => /Could not request elaboration/.test(text)));
  daemon.calls.length = 0;
  await page.locator(`[data-task-row="${P_UNSIZED}"] .plan-task-select`).scrollIntoViewIfNeeded();
  const selectionScrollBefore = await page.locator('[data-plan-viewport]').evaluate((element) => ({ left: element.scrollLeft, top: element.scrollTop }));
  await page.click(`[data-task-row="${P_UNSIZED}"] .plan-task-select`);
  await page.waitForSelector(`[data-task-row="${P_UNSIZED}"].selected`);
  const selectionScrollAfter = await page.locator('[data-plan-viewport]').evaluate((element) => ({ left: element.scrollLeft, top: element.scrollTop }));
  check('interaction: selecting a task updates in place without re-reading or replacing the Plan',
    !daemon.calls.some((call) => call.command === 'plan.overview')
    && await page.locator('.plan-workspace[data-identity-proof="same-workspace"]').count() === 1
    && selectionScrollAfter.left === selectionScrollBefore.left
    && await page.locator('.plan-loading,.skeleton').count() === 0,
    JSON.stringify({ calls: daemon.calls, selectionScrollBefore, selectionScrollAfter }));
  check('interaction: an unsized task offers an honest estimate action',
    /Not estimated yet/.test(await page.innerText('[data-ui-region="selected-task"]'))
    && await page.locator(`[data-resize-task="${P_UNSIZED}"]`, { hasText: 'Add estimate' }).count() === 1);

  await page.click('[data-plan-selection-toggle]');
  await page.waitForSelector('.plan-selection.collapsed');
  await page.click('[data-plan-selection-toggle]');
  await page.waitForSelector('.plan-selection:not(.collapsed)');
  check('interaction: collapsing selected details is local and keeps the same workspace',
    !daemon.calls.some((call) => call.command === 'plan.overview')
    && await page.locator('.plan-workspace[data-identity-proof="same-workspace"]').count() === 1);

  await page.hover(`[data-hover-task="${P_G1}"]`);
  await page.waitForSelector('#plan-tooltip:not([hidden])');
  check('interaction: hovering a task bar reveals its anchored detail badge',
    /E-mail field checks its spelling/.test(await page.innerText('#plan-tooltip'))
    && /100/.test(await page.innerText('#plan-tooltip')));

  await page.click(`[data-task-row="${P_C2}"] .plan-task-select`);
  await page.waitForSelector(`[data-task-row="${P_C2}"].selected`);
  check('interaction: selecting a task synchronizes the row, bar, and detail tray',
    await page.locator(`[data-hover-task="${P_C2}"].selected`).count() === 1
    && /Wrong password message/.test(await page.innerText('[data-ui-region="selected-task"]')));
  await page.click('[data-plan-selection-toggle]');
  await page.waitForSelector('.plan-selection.collapsed');
  check('interaction: the selected-task tray collapses in place', await page.locator('.plan-selection.collapsed').count() === 1);
  await page.click('[data-plan-selection-toggle]');
  await page.waitForSelector('.plan-selection:not(.collapsed)');

  const chartWidthBefore = await page.locator('.plan-workspace').evaluate((el) => Number.parseFloat(getComputedStyle(el).getPropertyValue('--chart-width')));
  await page.click('[data-plan-zoom="in"]');
  const chartWidthAfter = await page.locator('.plan-workspace').evaluate((el) => Number.parseFloat(getComputedStyle(el).getPropertyValue('--chart-width')));
  check('interaction: zoom in expands the lines-of-code canvas', chartWidthAfter > chartWidthBefore, `${chartWidthBefore} -> ${chartWidthAfter}`);
  await page.click('[data-plan-zoom="fit"]');
  check('interaction: fit makes the complete chart match the visible canvas', (await page.innerText('#plan-zoom-value')) === 'Fit');
  await page.click('[data-plan-zoom="in"]');
  const planViewport = page.locator('[data-plan-viewport]');
  const viewportBox = await planViewport.boundingBox();
  await page.click('[data-plan-mode="pan"]');
  const scrollBeforePan = await planViewport.evaluate((el) => el.scrollLeft);
  await page.mouse.move(viewportBox.x + viewportBox.width - 120, viewportBox.y + 150);
  await page.mouse.down();
  await page.mouse.move(viewportBox.x + viewportBox.width - 360, viewportBox.y + 150, { steps: 5 });
  await page.mouse.up();
  const scrollAfterPan = await planViewport.evaluate((el) => el.scrollLeft);
  check('interaction: hand mode pans the timeline without moving the document', scrollAfterPan > scrollBeforePan, `${scrollBeforePan} -> ${scrollAfterPan}`);
  await page.click('[data-plan-mode="select"]');
  await planViewport.evaluate((el) => { el.scrollLeft = 0; });
  await page.waitForFunction(() => Number.parseFloat(document.querySelector('[data-plan-minimap-thumb]')?.style.left || '0') === 0);
  const scrollBeforeMinimap = await planViewport.evaluate((el) => el.scrollLeft);
  const minimapControl = page.locator('[data-plan-minimap]');
  const minimapBox = await minimapControl.boundingBox();
  await minimapControl.click({ position: { x: minimapBox.width * 0.85, y: minimapBox.height / 2 } });
  const scrollAfterMinimap = await planViewport.evaluate((el) => el.scrollLeft);
  check('interaction: the minimap changes horizontal position', scrollAfterMinimap !== scrollBeforeMinimap, `${scrollBeforeMinimap} -> ${scrollAfterMinimap}`);

  await page.click('[data-plan-nav-toggle]');
  await page.waitForFunction(() => { const el = document.querySelector('.plan-workspace'); return !!el && Number.parseFloat(getComputedStyle(el).getPropertyValue('--glabel')) === 0; });
  check('interaction: the task navigator collapses to give the chart the full width',
    await page.locator('.plan-workspace').evaluate((el) => Number.parseFloat(getComputedStyle(el).getPropertyValue('--glabel'))) === 0);
  await page.click('[data-plan-nav-toggle]');
  await page.waitForFunction(() => { const el = document.querySelector('.plan-workspace'); return !!el && Number.parseFloat(getComputedStyle(el).getPropertyValue('--glabel')) > 0; });
  const navigatorBefore = await page.locator('.plan-workspace').evaluate((el) => Number.parseFloat(getComputedStyle(el).getPropertyValue('--glabel')));
  await page.focus('[data-plan-nav-resizer]');
  await page.keyboard.press('ArrowRight');
  const navigatorAfter = await page.locator('.plan-workspace').evaluate((el) => Number.parseFloat(getComputedStyle(el).getPropertyValue('--glabel')));
  check('interaction: the navigator divider resizes with the keyboard', navigatorAfter > navigatorBefore, `${navigatorBefore} -> ${navigatorAfter}`);

  const rowsBefore = await page.locator('.gtask:not(.ghidden)').count();
  daemon.calls.length = 0;
  await page.click(`[data-collapse="${P_C1}"]`);
  await page.waitForSelector('.gtask.ghidden', { state: 'attached' });
  check('plan: collapsing a parent hides its subtree rows',
    (await page.locator('.gtask:not(.ghidden)').count()) === rowsBefore - 1);
  await page.click(`[data-collapse="${P_C1}"]`);
  check('interaction: expanding and collapsing a task is local and keeps the same workspace',
    !daemon.calls.some((call) => call.command === 'plan.overview')
    && await page.locator('.plan-workspace[data-identity-proof="same-workspace"]').count() === 1);

  daemon.calls.length = 0;
  await page.click(`[data-task-row="${P_UNSIZED}"] .plan-task-select`);
  await page.click(`[data-resize-task="${P_UNSIZED}"]`);
  await page.waitForSelector('dialog#estimate-dialog[open]');
  await page.fill('#estimate-form [name=estimated_loc]', '240');
  await page.click('#estimate-form button[type=submit]');
  await page.waitForTimeout(500);
  check('interaction: adding an estimate persists it and moves the task onto the measured scale',
    daemon.calls.some((call) => call.command === 'task.update' && call.args.task_id === P_UNSIZED && call.args.estimated_loc === 240)
    && /~240 lines/.test(await page.innerText(`[data-task-row="${P_UNSIZED}"]`))
    && await page.locator(`.gbar.unsized[data-select-task="${P_UNSIZED}"]`).count() === 0);

  // Pointer resize persists through a full reload.
  await page.click(`[data-task-row="${P_C2}"] .plan-task-select`);
  await page.waitForSelector(`[data-resize-handle="${P_C2}"]`);
  daemon.calls.length = 0;
  let resizeBox = await page.locator(`[data-resize-handle="${P_C2}"]`).boundingBox();
  await page.mouse.move(resizeBox.x + resizeBox.width / 2, resizeBox.y + resizeBox.height / 2);
  await page.mouse.down();
  await page.mouse.move(resizeBox.x + resizeBox.width / 2 + 70, resizeBox.y + resizeBox.height / 2, { steps: 5 });
  await page.mouse.up();
  await page.waitForTimeout(500);
  const pointerResizeCall = daemon.calls.find((c) => c.command === 'task.update' && c.args.task_id === P_C2 && c.args.estimated_loc > 200);
  check('interaction: dragging the selected bar handle updates its real estimate', !!pointerResizeCall, JSON.stringify(pointerResizeCall?.args));
  const persistedEstimate = pointerResizeCall?.args.estimated_loc;
  await page.reload();
  await page.waitForSelector(`[data-task-row="${P_C2}"]`);
  check('interaction: resized task estimate survives reload', persistedEstimate && (await page.innerText(`[data-task-row="${P_C2}"]`)).includes(persistedEstimate.toLocaleString('en-US')));

  // Escape cancels pointer resize without an API write.
  await page.click(`[data-task-row="${P_C2}"] .plan-task-select`);
  await page.waitForSelector(`[data-resize-handle="${P_C2}"]`);
  daemon.calls.length = 0;
  resizeBox = await page.locator(`[data-resize-handle="${P_C2}"]`).boundingBox();
  await page.mouse.move(resizeBox.x + resizeBox.width / 2, resizeBox.y + resizeBox.height / 2);
  await page.mouse.down();
  await page.mouse.move(resizeBox.x + resizeBox.width / 2 + 40, resizeBox.y + resizeBox.height / 2, { steps: 3 });
  await page.keyboard.press('Escape');
  await page.mouse.up();
  await page.waitForTimeout(400);
  check('interaction: Escape cancels a pointer resize without changing the task', !daemon.calls.some((c) => c.command === 'task.update'));

  // Failed resize keeps the persisted value and reports the error.
  await page.click(`[data-task-row="${P_C2}"] .plan-task-select`);
  await page.waitForSelector(`[data-resize-handle="${P_C2}"]`);
  daemon.calls.length = 0; daemon.failNextTaskUpdate();
  resizeBox = await page.locator(`[data-resize-handle="${P_C2}"]`).boundingBox();
  await page.mouse.move(resizeBox.x + resizeBox.width / 2, resizeBox.y + resizeBox.height / 2);
  await page.mouse.down();
  await page.mouse.move(resizeBox.x + resizeBox.width / 2 + 30, resizeBox.y + resizeBox.height / 2, { steps: 3 });
  await page.mouse.up();
  await page.waitForTimeout(800);
  const resizeFailureText = await page.locator('.toast.bad').allInnerTexts();
  check('interaction: a failed resize reports failure and keeps the task available', resizeFailureText.some((text) => /resize failed/.test(text)) && await page.locator(`[data-task-row="${P_C2}"]`).count() === 1, resizeFailureText.join(' | '));

  // The numeric dialog is the keyboard/touch resize path.
  await page.click(`[data-task-row="${P_C2}"] .plan-task-select`);
  await page.waitForSelector(`[data-resize-task="${P_C2}"]`);
  daemon.calls.length = 0;
  await page.click(`[data-resize-task="${P_C2}"]`);
  await page.waitForSelector('dialog#estimate-dialog[open]');
  await page.fill('#estimate-form [name=estimated_loc]', '275');
  await page.click('#estimate-form button[type=submit]');
  await page.waitForTimeout(400);
  check('interaction: the resize dialog saves an exact estimate', daemon.calls.some((c) => c.command === 'task.update' && c.args.task_id === P_C2 && c.args.estimated_loc === 275));

  if (await page.locator('.plan-selection:not(.collapsed) [data-plan-selection-toggle]').count()) {
    await page.click('[data-plan-selection-toggle]');
    await page.waitForSelector('.plan-selection.collapsed');
  }
  daemon.calls.length = 0;
  await page.evaluate(() => {
    window.__planDragEvents = [];
    for (const type of ['dragstart', 'dragenter', 'dragover', 'drop', 'dragend']) {
      document.addEventListener(type, (event) => window.__planDragEvents.push({
        type, target: event.target?.className || event.target?.tagName || '',
        row: event.target?.closest?.('[data-task-row]')?.dataset.taskRow || null,
      }), true);
    }
  });
  const dragSourceHit = await page.locator(`[data-drag-task="${P_C2}"]`).evaluate((element) => {
    const box = element.getBoundingClientRect();
    const hit = document.elementFromPoint(box.left + box.width / 2, box.top + box.height / 2);
    return { box: { x: box.x, y: box.y, width: box.width, height: box.height },
      hit: hit?.className || hit?.tagName || '' };
  });
  await pointerDrag(`[data-drag-task="${P_C2}"]`, `[data-task-row="${P_C1}"]`, { x: 100, y: 28 });
  await page.waitForTimeout(400);
  const reorderCall = daemon.calls.find((c) => c.command === 'task.update');
  const dragEvents = await page.evaluate(() => window.__planDragEvents);
  const reorderedInView = reorderCall ? await page.waitForFunction(({ movedId, targetId }) => {
    const rows = [...document.querySelectorAll('[data-task-row]')].map((row) => row.dataset.taskRow);
    return rows.indexOf(movedId) >= 0 && rows.indexOf(movedId) < rows.indexOf(targetId);
  }, { movedId: P_C2, targetId: P_C1 }, { timeout: 2500 }).then(() => true).catch(() => false) : false;
  check('interaction: dragging a task above a sibling reorders it in place',
    reorderCall && reorderCall.args.task_id === P_C2 && reorderCall.args.position === 0
    && !('release_id' in reorderCall.args) && reorderedInView,
    JSON.stringify({ args: reorderCall?.args, reorderedInView, dragSourceHit, dragEvents }));
  daemon.calls.length = 0;
  await page.evaluate(({ sourceId, releaseId }) => {
    const viewport = document.querySelector('[data-plan-viewport]');
    const source = document.querySelector(`[data-drag-task="${sourceId}"]`);
    const sourceRow = source?.closest('[data-task-row]');
    const target = document.querySelector(`[data-drop-release="${releaseId}"]`);
    if (viewport && sourceRow && target) viewport.scrollTop = Math.max(0, ((sourceRow.offsetTop + target.offsetTop) / 2) - (viewport.clientHeight / 2));
  }, { sourceId: P_C2, releaseId: V_R2 });
  await pointerDrag(`[data-drag-task="${P_C2}"]`, `[data-drop-release="${V_R2}"]`);
  await page.waitForTimeout(400);
  const dragMoveCall = daemon.calls.find((c) => c.command === 'task.update' && c.args.task_id === P_C2);
  check('interaction: dropping a task on a release header moves it into that release',
    dragMoveCall && dragMoveCall.args.task_id === P_C2 && dragMoveCall.args.release_id === V_R2,
    JSON.stringify(daemon.calls.filter((call) => call.command === 'task.update').map((call) => call.args)));

  // Cancel is truthful before exercising the successful move path.
  daemon.calls.length = 0;
  await page.click(`[data-task-row="${P_G1}"] .plan-task-select`);
  await page.click(`[data-move-task="${P_G1}"]`);
  await page.waitForSelector('dialog#move-dialog[open]');
  await page.click('#move-cancel');
  check('interaction: cancelling the move dialog makes no API call', !daemon.calls.some((c) => c.command === 'task.update'));
  daemon.calls.length = 0;
  await page.click(`[data-move-task="${P_G1}"]`);
  await page.waitForSelector('dialog#move-dialog[open]');
  await page.selectOption('#move-form [name=release_id]', V_R2);
  await page.click('#move-form button[type=submit]');
  await page.waitForTimeout(400);
  const dialogMoveCall = daemon.calls.find((c) => c.command === 'task.update');
  check('interaction: the move pop-up posts the chosen release',
    dialogMoveCall && dialogMoveCall.args.task_id === P_G1 && dialogMoveCall.args.release_id === V_R2,
    JSON.stringify(dialogMoveCall?.args));
  daemon.calls.length = 0;
  await page.click('button[data-cmd="release.request"]');
  await page.waitForTimeout(400);
  check('interaction: Request preview now calls release.request for the repository',
    daemon.calls.some((c) => c.command === 'release.request' && c.args.repository_id === REPO));
  check('interaction: preview request re-renders as a pending notice', /Preview requested/.test(await page.innerText('main')));
  daemon.calls.length = 0;
  await page.click('[data-plan-feedback]');
  await page.waitForSelector('dialog#feedback-dialog[open]');
  await page.fill('#comment-form [name=title]', 'The export button fails for me');
  await page.click('#comment-form button[type=submit]');
  await page.waitForTimeout(400);
  const feedbackCall = daemon.calls.find((c) => c.command === 'task.create');
  check('interaction: the ask-for-a-change form creates a user_feedback task',
    feedbackCall && feedbackCall.args.title === 'The export button fails for me'
    && feedbackCall.args.kind === 'user_feedback' && feedbackCall.args.repository_id === REPO,
    JSON.stringify(feedbackCall?.args));
  check('interaction: submitted owner feedback appears in the plan', /The export button fails for me/.test(await page.innerText('main')));
  daemon.calls.length = 0;
  await page.click(`[data-task-row="${P_G1}"] .plan-task-select`);
  await page.click(`.plan-selection [data-cmd="task.update"]`);
  await page.waitForTimeout(400);
  const dropCall = daemon.calls.find((c) => c.command === 'task.update');
  check('interaction: drop asks for confirmation and marks the task dropped',
    dropCall && dropCall.args.task_id === P_G1 && dropCall.args.status === 'dropped',
    JSON.stringify(dropCall?.args));
  check('interaction: dropped task leaves the current plan after the state re-read', await page.locator(`[data-task-row="${P_G1}"]`).count() === 0);

  daemon.setScenario({ ...SCENARIOS.populated, densePlan: true });
  await page.reload();
  await page.waitForFunction(() => document.querySelectorAll('.gtask').length >= 120, null, { timeout: 15000 });
  check('plan: a verification-heavy plan renders every unknown-size job in the chart',
    await page.locator('.gtask .gbar.unsized:not(.parent)').count() === 111
    && await page.locator('.gtask').count() >= 120,
    `${await page.locator('.gtask .gbar.unsized:not(.parent)').count()} unknown marks for ${await page.locator('.gtask').count()} rows`);
  check('plan: the dense unknown-size plan keeps one honest non-proportional band',
    /Not estimated/.test(await page.innerText('.gaxis'))
    && /111 jobs not estimated/.test(await page.innerText('.plan-total-progress')));

  // Decisions: story, aspect filter, search, paging, superseded handling.
  daemon.setScenario(SCENARIOS.populated);
  await page.goto(`http://${HOST}:${port}/#/decisions/${REPO}`);
  await page.waitForSelector('.decision');
  await page.click('[data-project-picker-toggle]');
  await page.click('[data-project-picker-menu] a[href="#/decisions/r2"]');
  await page.waitForURL(/#\/decisions\/r2$/);
  await page.waitForFunction(() => /going-and-going/.test(document.querySelector('.project-picker-current')?.textContent || ''));
  check('interaction: the Decisions project menu switches the project route',
    /going-and-going/.test(await page.innerText('.project-picker-current')));
  await page.goto(`http://${HOST}:${port}/#/decisions/${REPO}`);
  await page.waitForSelector('.decision');
  check('decisions: the story so far and the plain entries render',
    /The story so far/.test(await page.innerText('body')) && /Buttons are green now/.test(await page.innerText('body')));
  check('decisions: superseded entries are collapsed', await page.locator('details.decision.superseded:not([open])').count() === 1);
  daemon.calls.length = 0;
  await page.click('#decisions-older');
  await page.waitForTimeout(400);
  check('interaction: Show older pages the tail with before_seq of the oldest shown decision',
    daemon.calls.some((c) => c.command === 'decision.tail' && c.args.before_seq === 41));
  daemon.calls.length = 0;
  await page.click('[data-decision-aspect="ui"]');
  await page.waitForTimeout(400);
  check('interaction: the aspect filter is applied server-side',
    daemon.calls.some((c) => c.command === 'decision.tail' && c.args.aspect === 'ui' && !('before_seq' in c.args)));
  daemon.calls.length = 0;
  await page.fill('#decision-search [name=q]', 'export');
  await page.click('#decision-search button[type=submit]');
  await page.waitForSelector('text=REPO-EXPORT-FILES');
  check('interaction: search calls decision.search with the typed query',
    daemon.calls.some((c) => c.command === 'decision.search' && c.args.query === 'export' && c.args.aspect === 'ui'));
  await context.close();

  // Health layout: preserve the user-marked 856 px surface, mobile, desktop,
  // and the exact responsive transition where the repository table becomes
  // labelled cards. Every storage category must remain visible without a
  // horizontal discovery path.
  daemon.setScenario(SCENARIOS.populated);
  const healthContext = await browser.newContext({ viewport: { width: 1440, height: 1024 } });
  const { cookie: healthCookie } = sessions.issue({ sub: 'sub', email: 'owner@example.test' });
  await healthContext.addCookies([{ name: 'dc2_session', value: healthCookie.split(';')[0].split('=')[1], domain: `.${BASE}`, path: '/' }]);
  const healthPage = await healthContext.newPage();
  const healthSamples = [
    { width: 390, height: 844 },
    { width: 856, height: 915 },
    { width: 959, height: 900 },
    { width: 960, height: 900 },
    { width: 961, height: 900 },
    { width: 1440, height: 1024 },
  ];
  for (const viewport of healthSamples) {
    await healthPage.setViewportSize(viewport);
    await healthPage.goto(`http://${HOST}:${port}/#/health`);
    await healthPage.waitForSelector('.health-storage-breakdown');
    await healthPage.waitForSelector('.chartbox svg.chart');
    await healthPage.screenshot({ path: path.join(OUT, `health-${viewport.width}.png`), fullPage: true });
    const layout = await healthPage.evaluate(() => {
      const summary = document.querySelector('.health-summary').getBoundingClientRect();
      const status = document.querySelector('.health-status-panel').getBoundingClientRect();
      const table = document.querySelector('.health-repository-table');
      const wrap = table.closest('.tablewrap');
      const storageItems = [...document.querySelectorAll('.health-storage-breakdown > div')].map((element) => {
        const rect = element.getBoundingClientRect();
        return { text: element.innerText, left: rect.left, right: rect.right, clipped: element.scrollWidth > element.clientWidth + 1 || element.scrollHeight > element.clientHeight + 1 };
      });
      const capacityRows = new Map();
      for (const card of document.querySelectorAll('.health-capacity-card')) {
        const rect = card.getBoundingClientRect();
        const top = Math.round(rect.top);
        if (!capacityRows.has(top)) capacityRows.set(top, []);
        capacityRows.get(top).push(rect.height);
      }
      const incidentList = document.querySelector('.health-incident-list');
      return {
        documentOverflow: document.documentElement.scrollWidth - innerWidth,
        summaryInInitialViewport: summary.top >= 0 && status.bottom <= innerHeight,
        summaryBeforeIncidents: summary.bottom <= document.querySelector('#health-incidents-title').getBoundingClientRect().top + 1,
        capacityCards: document.querySelectorAll('.health-capacity-card').length,
        capacityRowHeightSpreads: [...capacityRows.values()].map((heights) => Math.max(...heights) - Math.min(...heights)),
        incidentAlignment: getComputedStyle(incidentList).alignItems,
        tableDisplay: getComputedStyle(table).display,
        tableScroll: wrap.scrollWidth - wrap.clientWidth,
        storageItems,
        rawStorageKeysVisible: /docker_(shared|images|build_cache|shared_volumes)/.test(document.body.innerText),
      };
    });
    const expectedTableDisplay = viewport.width <= 960 ? 'block' : 'table';
    check(`health-${viewport.width}: summary leads, aligns, and incidents keep natural height`,
      layout.summaryInInitialViewport
      && layout.summaryBeforeIncidents
      && layout.capacityCards === 4
      && layout.capacityRowHeightSpreads.every((spread) => spread <= 1)
      && layout.incidentAlignment === 'start', JSON.stringify(layout));
    check(`health-${viewport.width}: repository layout switches at 960px without horizontal overflow`,
      layout.documentOverflow <= 0 && layout.tableDisplay === expectedTableDisplay && layout.tableScroll <= 1,
      JSON.stringify({ overflow: layout.documentOverflow, tableDisplay: layout.tableDisplay, tableScroll: layout.tableScroll }));
    check(`health-${viewport.width}: all storage attribution is readable`,
      layout.storageItems.length === 5
      && layout.storageItems.every((item) => !item.clipped && item.left >= -1 && item.right <= viewport.width + 1)
      && !layout.rawStorageKeysVisible,
      JSON.stringify(layout.storageItems));
  }
  await healthContext.close();

  // Responsive global shell: the header never wraps. At and below the
  // breakpoint the same navigation links move into a keyboard-operable menu.
  const navContext = await browser.newContext({ viewport: { width: 799, height: 964 } });
  const { cookie: navCookie } = sessions.issue({ sub: 'sub', email: 'owner@example.test' });
  await navContext.addCookies([{ name: 'dc2_session', value: navCookie.split(';')[0].split('=')[1], domain: `.${BASE}`, path: '/' }]);
  const navPage = await navContext.newPage();
  await navPage.goto(`http://${HOST}:${port}/#/usage/${REPO}`);
  await navPage.waitForSelector('.usage-phase-chart');
  const compactHeader = await navPage.evaluate(() => {
    const header = document.querySelector('header.top');
    const rect = header.getBoundingClientRect();
    return { height: rect.height, overflow: document.documentElement.scrollWidth - innerWidth };
  });
  check('responsive navigation: 799px header stays on one row without document overflow',
    compactHeader.height <= 64 && compactHeader.overflow <= 0, JSON.stringify(compactHeader));
  check('responsive navigation: hamburger replaces the inline menu at 799px',
    await navPage.locator('#nav-toggle:visible').count() === 1
    && await navPage.locator('#nav:visible').count() === 0);
  await navPage.click('#nav-toggle');
  await navPage.waitForSelector('#nav:visible');
  check('interaction: hamburger opens the original navigation links in a DOM menu',
    await navPage.locator('#nav-toggle[aria-expanded="true"]').count() === 1
    && await navPage.locator('#nav a[href="#/usage"]').count() === 1
    && await navPage.locator('#nav a[href="#/progress"]').count() === 1
    && await navPage.locator('#nav button').count() === 0);
  await navPage.screenshot({ path: path.join(OUT, 'navigation-799-open.png'), fullPage: true });
  await navPage.keyboard.press('Escape');
  check('interaction: Escape closes the hamburger menu and restores focus',
    await navPage.locator('#nav:visible').count() === 0
    && await navPage.locator('#nav-toggle:focus').count() === 1);
  await navPage.click('#nav-toggle');
  await navPage.click('#nav a[href="#/decisions"]');
  await navPage.waitForURL(/#\/decisions$/);
  await navPage.waitForSelector('main h1 a[href="#/decisions"]');
  check('interaction: a hamburger link navigates and closes the menu',
    await navPage.locator('#nav:visible').count() === 0
    && await navPage.locator('main h1 a[href="#/decisions"]').count() === 1);
  await navPage.setViewportSize({ width: 1240, height: 800 });
  await navPage.waitForFunction(() => document.querySelector('#nav-toggle')?.offsetParent !== null
    && document.querySelector('#nav')?.offsetParent === null);
  check('responsive navigation: the menu is collapsed at the declared breakpoint',
    await navPage.locator('#nav-toggle:visible').count() === 1
    && await navPage.locator('#nav:visible').count() === 0);
  await navPage.setViewportSize({ width: 1241, height: 800 });
  await navPage.waitForFunction(() => document.querySelector('#nav-toggle')?.offsetParent === null
    && document.querySelector('#nav')?.offsetParent !== null);
  const desktopBoundary = await navPage.evaluate(() => ({
    headerHeight: document.querySelector('header.top').getBoundingClientRect().height,
    overflow: document.documentElement.scrollWidth - innerWidth,
  }));
  check('responsive navigation: inline links return one pixel above the breakpoint without wrapping',
    await navPage.locator('#nav-toggle:visible').count() === 0
    && await navPage.locator('#nav:visible').count() === 1
    && desktopBoundary.headerHeight <= 64 && desktopBoundary.overflow <= 0,
    JSON.stringify(desktopBoundary));
  await navContext.close();

  await browser.close();
  await edge.close();
  await daemon.close();
  report.summary = { checks: report.checks.length, failures: report.failures.length, screenshots: OUT };
  await fs.writeFile(path.join(OUT, 'report.json'), JSON.stringify(report, null, 2));
  console.log(JSON.stringify(report.summary));
  for (const f of report.failures) console.log('FAIL', f);
  process.exit(report.failures.length ? 1 : 0);
}

main().catch((error) => { console.error(error); process.exit(2); });
