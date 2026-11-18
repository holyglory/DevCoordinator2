use super::*;
use crate::access::{Caller, RouteAccessSection};
use crate::automation_test_support::{Fixture, FixtureClock, START, WEEK};
use crate::control_plane::ControlPlane;
use crate::daemon::OperationExecutor;
use crate::usage_api_fixture::{KEY, Peer, capability, response};
use devcoordinator2_api::work_context::{WorkAttribution, WorkContext, WorkSource};
use serde_json::{Value, json};
use std::sync::Arc;

fn input() -> Prepare {
    Prepare {
        repository_id: "project-alpha".into(),
        workstream_id: Some("spec".into()),
        window_start_ms: START,
        window_end_ms: START + WEEK,
        offset: 0,
        limit: 10,
        before_decision_seq: None,
        outcome_cursor: None,
        outcome_limit: None,
    }
}

fn caller() -> Caller {
    Caller {
        via_edge: false,
        pid: 1,
        uid: rustix::process::getuid().as_raw(),
        gid: rustix::process::getgid().as_raw(),
        client_kind: devcoordinator2_api::ClientKind::Codex,
        model: None,
        effort: None,
        client_session: Some("fixture-thread".into()),
        identity: None,
        work: Some(WorkAttribution {
            source: WorkSource::Request,
            diagnostic: None,
            context: Some(WorkContext {
                version: 1,
                alarm: None,
                native_project_id: "fixture-project".into(),
                thread_id: "fixture-thread".into(),
                turn_id: Some("fixture-turn".into()),
                operation_id: Some("governed-op".into()),
                workstream_id: Some("spec".into()),
                outcome_id: Some("spec".into()),
                experiment_ref: Some("experiment@1".into()),
            }),
        }),
    }
}

fn service(fixture: &Fixture, source: crate::config::CodexUsageSource) -> ReviewService {
    let mut config = fixture.config.clone();
    config.codex_usage_sources = vec![source];
    ReviewService::new(
        fixture.database.clone(),
        UsageService::with_clock(
            config,
            fixture.database.clone(),
            crate::repository::Registry::new(fixture.database.clone()),
            Arc::new(FixtureClock),
        ),
    )
}

fn priced_response(params: &Value) -> Value {
    let mut value = response(params, START + WEEK, 135);
    value["report"]["cost"] = json!({
        "basis":"api_equivalent", "currency":"USD", "status":"complete", "processingTier":"standard",
        "estimatedUsdMicros":135, "inputUsdMicros":135, "cachedInputUsdMicros":0, "cacheWriteUsdMicros":0, "outputUsdMicros":0,
        "inputTokens":135, "uncachedInputTokens":135, "cachedInputTokens":0, "cacheWriteTokens":0, "outputTokens":0, "reasoningTokens":0,
        "providerTotalTokens":135, "pricedObservations":2, "unknownObservations":0, "rateCardRefs":["fixture-standard"]
    });
    value
}

#[test]
fn performance_cost_components_accept_scalar_null_and_measured_transport_values() {
    let cases = [
        (
            "unknown",
            json!({"measured":0,"exact":null,"unknown":2}),
            None,
            "unavailable",
            2,
        ),
        (
            "exact-zero",
            json!({"measured":0,"exact":0,"unknown":0}),
            Some(0),
            "complete",
            0,
        ),
        (
            "partial",
            json!({"measured":17,"exact":null,"unknown":1}),
            Some(17),
            "partial",
            1,
        ),
        ("scalar", json!(17), Some(17), "complete", 0),
        ("scalar-zero", json!(0), Some(0), "complete", 0),
        ("null", Value::Null, None, "unavailable", 2),
    ];
    let mut failures = Vec::new();
    for (name, component, expected, status, unknown) in cases {
        let mut fixture = Fixture::new();
        let root = fixture._temporary.path();
        let remainder = if component.is_object() {
            if expected.is_none() {
                component.clone()
            } else {
                json!({"measured":0,"exact":0,"unknown":0})
            }
        } else if expected.is_some() {
            json!(0)
        } else {
            Value::Null
        };
        let expected_remainder = expected.map(|_| 0);
        let wire_component = component.clone();
        let wire_remainder = remainder.clone();
        let peer = Peer::start(root, &root.join(name), move |params| {
            let mut value = priced_response(params);
            let cost = &mut value["report"]["cost"];
            cost["status"] = json!(status);
            cost["estimatedUsdMicros"] = json!(expected);
            cost["inputUsdMicros"] = wire_component.clone();
            for key in [
                "cachedInputUsdMicros",
                "cacheWriteUsdMicros",
                "outputUsdMicros",
            ] {
                cost[key] = wire_remainder.clone();
            }
            cost["unknownObservations"] = json!(unknown);
            cost["pricedObservations"] = json!(if expected.is_none() { 0 } else { 2 - unknown });
            cost["rateCardRefs"] = if expected.is_some() {
                json!(["fixture-standard"])
            } else {
                json!([])
            };
            let reasoning_component = if status == "complete" {
                json!({"measured":0,"exact":0,"unknown":0})
            } else {
                json!({"measured":0,"exact":null,"unknown":unknown})
            };
            let review_cost = json!({
                "status":status,"basis":"api_equivalent","currency":"USD","processing_tier":"standard",
                "estimated_usd_micros":expected,"input_usd_micros":wire_component,
                "cached_input_usd_micros":wire_remainder,"cache_write_usd_micros":wire_remainder,"output_usd_micros":wire_remainder,
                "reasoning_usd_micros":reasoning_component,
                "unknown_observations":unknown,"rate_card_refs":["fixture-standard"],"model_requests":2,
            });
            value["review"]["totals"]["cost"] = review_cost.clone();
            value["review"]["activities"][0]["cost"] = review_cost.clone();
            value["review"]["outcomes"]["totals"]["cost"] = review_cost.clone();
            let mut attributed_cost = review_cost.clone();
            let mut unattributed_cost = review_cost;
            attributed_cost["unknown_observations"] = json!(unknown.min(1));
            unattributed_cost["unknown_observations"] = json!(u64::from(expected.is_none()));
            unattributed_cost["status"] = json!(if expected.is_some() {
                "complete"
            } else {
                "unavailable"
            });
            unattributed_cost["estimated_usd_micros"] = json!(expected.map(|_| 0));
            for field in [
                "input_usd_micros",
                "cached_input_usd_micros",
                "cache_write_usd_micros",
                "output_usd_micros",
            ] {
                if expected.is_none() && attributed_cost[field].is_object() {
                    attributed_cost[field]["unknown"] = json!(1);
                }
                unattributed_cost[field] = if expected.is_none() {
                    attributed_cost[field].clone()
                } else if wire_component.is_object() {
                    json!({"measured":0,"exact":0,"unknown":0})
                } else {
                    json!(0)
                };
            }
            value["review"]["outcomes"]["attributed"]["cost"] = attributed_cost.clone();
            value["review"]["outcomes"]["unattributed"]["cost"] = unattributed_cost;
            for row in value["review"]["outcomes"]["rows"].as_array_mut().unwrap() {
                row["effort"]["cost"] = attributed_cost.clone();
            }
            value
        });
        let authorized = caller();
        crate::usage_context::ingest(
            &fixture.database,
            authorized.uid,
            "project-alpha",
            authorized.work.as_ref().unwrap(),
            START + 1,
        )
        .unwrap();
        fixture.service = service(&fixture, peer.source.clone());
        let params = || devcoordinator2_api::performance::Overview {
            repository_id: "project-alpha".into(),
            window_start_ms: START,
            window_end_ms: START + WEEK,
            outcome_cursor: None,
            outcome_limit: Some(5),
            totals_only: false,
            wait_for_refresh: true,
        };
        let _ = fixture
            .service
            .performance_overview(params(), START + WEEK)
            .unwrap();
        assert!(
            fixture
                .service
                .usage
                .wait_for_collector_refresh(Instant::now() + std::time::Duration::from_secs(3))
        );
        let result = fixture
            .service
            .performance_overview(params(), START + WEEK)
            .unwrap();
        if result.coverage.available_collectors != 1 {
            failures.push(format!(
                "{name}: source rejected with {:?}",
                result.coverage.unavailable_reasons
            ));
            continue;
        }
        assert_eq!(result.total_tokens.measured, 135, "{name}");
        assert_eq!(result.cost.input_usd_micros, expected, "{name}");
        assert_eq!(
            result.cost.cached_input_usd_micros, expected_remainder,
            "{name}"
        );
        assert_eq!(result.cost.status, status, "{name}");
        assert_eq!(result.cost.unknown_observations, Some(unknown), "{name}");
        let usage = result.usage.unwrap();
        assert_eq!(usage.activities.len(), 1, "{name}");
        assert_eq!(usage.outcomes.rows.len(), 1, "{name}");
        assert_eq!(
            usage.outcomes.totals.cost.unknown_observations,
            Some(unknown),
            "{name}"
        );
        for cost in [
            &usage.outcomes.attributed.cost,
            &usage.outcomes.rows[0].effort.cost,
        ] {
            assert_eq!(cost.input_usd_micros, expected, "{name}");
            assert_eq!(cost.cached_input_usd_micros, expected_remainder, "{name}");
            assert_eq!(cost.status, status, "{name}");
            assert_eq!(cost.unknown_observations, Some(unknown.min(1)), "{name}");
        }
        let cost = &usage.outcomes.unattributed.cost;
        assert_eq!(cost.input_usd_micros, expected.map(|_| 0), "{name}");
        assert_eq!(cost.cached_input_usd_micros, expected_remainder, "{name}");
        assert_eq!(
            cost.unknown_observations,
            Some(u64::from(expected.is_none())),
            "{name}"
        );
        let requests = peer.requests.lock().unwrap();
        let summary = requests
            .iter()
            .find(|request| request["method"] == "localUsage/summary")
            .unwrap();
        assert_eq!(summary["params"]["includeReview"], true);
        assert_eq!(summary["params"]["fromAt"], START);
        assert_eq!(summary["params"]["toAt"], START + WEEK);
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn performance_headlines_preserve_exact_components_and_nullable_deferred_counts() {
    let mut fixture = Fixture::new();
    let root = fixture._temporary.path();
    let mut cap = capability();
    cap["supportsTokenHeadlines"] = json!(true);
    let peer = Peer::with_capability(root, &root.join("headlines"), Some(cap), |params| {
        if params["includeReview"] == false {
            let mut value = crate::usage_api_fixture::headline_response(params, START + WEEK);
            value["snapshot"]["freshness"] = json!("stale");
            value["snapshot"]["pendingEntries"] = json!(97);
            value
        } else {
            let mut value = response(params, START + WEEK, 135);
            value["snapshot"]["journalWatermark"] = json!(11);
            value
        }
    });
    fixture.config.codex_usage_sources = vec![peer.source.clone()];
    let plane = ControlPlane::with_adapters(
        fixture.config.clone(),
        fixture.database.clone(),
        Arc::new(|_: &RouteAccessSection| Ok(())),
        Arc::new(FixtureClock),
    )
    .unwrap();
    let authorized = Caller {
        work: None,
        ..caller()
    };
    let mut period = Value::Null;
    for start in [START, 0] {
        let result = plane
            .execute(
                "performance.overview",
                json!({
                    "repository_id":"project-alpha","window_start_ms":start,
                    "window_end_ms":START + WEEK,"totals_only":true,
                }),
                &authorized,
            )
            .unwrap();
        assert_eq!(result["total_tokens"]["measured"], 120);
        assert_eq!(result["total_tokens"]["exact"], 120);
        assert_eq!(result["totals"]["input_tokens"], 100);
        assert_eq!(result["totals"]["cached_input_tokens"], 80);
        assert_eq!(result["totals"]["output_tokens"], 20);
        assert_eq!(result["totals"]["reasoning_tokens"], 5);
        for count in ["model_requests", "tool_calls", "operations"] {
            assert!(result["totals"].get(count).is_some_and(Value::is_null));
        }
        assert_eq!(result["cost"]["status"], "unavailable");
        for amount in [
            "estimated_usd_micros",
            "input_usd_micros",
            "cached_input_usd_micros",
            "cache_write_usd_micros",
            "output_usd_micros",
        ] {
            assert!(result["cost"][amount].is_null());
        }
        assert_eq!(result["cost"]["input_tokens"], 100);
        assert_eq!(result["cost"]["uncached_input_tokens"], 20);
        assert_eq!(result["cost"]["cache_write_tokens"], Value::Null);
        assert_eq!(
            result["coverage"]["state"], "complete",
            "deferred counts/cost and an optional unknown cache-write component must not erase a known provider total"
        );
        assert_eq!(result["coverage"]["sources"][0]["format_version"], 3);
        assert_eq!(result["coverage"]["sources"][0]["generation"], 7);
        assert_eq!(result["coverage"]["sources"][0]["watermark"], 42);
        assert_eq!(result["coverage"]["sources"][0]["journal_watermark"], 42);
        assert_eq!(result["coverage"]["sources"][0]["pending_entries"], 97);
        assert!(
            result["coverage"]["sources"][0]["source_watermark"]
                .as_str()
                .unwrap()
                .starts_with("usage-v2:headline:")
        );
        assert_eq!(
            result["coverage"]["snapshot"]["updated_at_ms"],
            START + WEEK
        );
        assert_eq!(result["coverage"]["loading"], Value::Null);
        assert_eq!(
            result["usage"],
            Value::Null,
            "no empty rich graphs are manufactured from a headline"
        );
        if start == START {
            period = result;
        }
    }
    let params = json!({"repository_id":"project-alpha","window_start_ms":START,"window_end_ms":START + WEEK});
    let rich = plane
        .execute("performance.overview", params, &authorized)
        .unwrap();
    assert_eq!(rich["total_tokens"]["measured"], 135);
    assert_eq!(rich["totals"]["model_requests"], 2);
    assert_eq!(rich["totals"]["tool_calls"], 0);
    assert_eq!(rich["totals"]["operations"], 2);
    assert!(rich["usage"].is_object());
    let again = plane.execute("performance.overview", json!({"repository_id":"project-alpha","window_start_ms":START,"window_end_ms":START + WEEK,"totals_only":true}), &authorized).unwrap();
    assert_eq!(
        again, period,
        "rich and headline cache entries never replace each other"
    );
    let summaries = peer
        .requests
        .lock()
        .unwrap()
        .iter()
        .filter(|request| request["method"] == "localUsage/summary")
        .cloned()
        .collect::<Vec<_>>();
    assert_eq!(summaries.len(), 3);
    assert_eq!(summaries[0]["params"]["includeReview"], false);
    assert_eq!(summaries[1]["params"]["includeReview"], false);
    assert_eq!(summaries[2]["params"]["includeReview"], true);
    for (request, start) in summaries.iter().zip([START, 0, START]) {
        assert_eq!(request["params"]["repositoryKey"], KEY);
        assert_eq!(request["params"]["fromAt"], start);
        assert_eq!(request["params"]["toAt"], START + WEEK);
    }
    let schema = (devcoordinator2_api::operation("performance.overview")
        .unwrap()
        .output_schema)();
    for count in ["model_requests", "tool_calls", "operations"] {
        assert!(
            schema["$defs"]["UsageTotals"]["properties"][count]["type"]
                .as_array()
                .is_some_and(|types| types.contains(&json!("null")))
        );
    }
}

#[test]
fn mixed_headline_and_rich_sources_keep_deferred_counts_unknown() {
    for headlined in [false, true] {
        let mut fixture = Fixture::new();
        let root = fixture._temporary.path();
        let modern = root.join("modern");
        let compatible = root.join("compatible");
        std::fs::create_dir(&modern).unwrap();
        std::fs::create_dir(&compatible).unwrap();
        let mut cap = capability();
        cap["supportsTokenHeadlines"] = json!(headlined);
        let modern = Peer::with_capability(&modern, &modern.join("home"), Some(cap), |params| {
            if params["includeReview"] == false {
                crate::usage_api_fixture::headline_response(params, START + WEEK)
            } else {
                response(params, START + WEEK, 135)
            }
        });
        let compatible = Peer::start(&compatible, &compatible.join("home"), |params| {
            assert_eq!(params["includeReview"], true);
            response(params, START + WEEK, 135)
        });
        fixture.config.codex_usage_sources = vec![modern.source.clone(), compatible.source.clone()];
        let plane = ControlPlane::with_adapters(
            fixture.config.clone(),
            fixture.database.clone(),
            Arc::new(|_: &RouteAccessSection| Ok(())),
            Arc::new(FixtureClock),
        )
        .unwrap();
        let result = plane.execute("performance.overview", json!({"repository_id":"project-alpha", "window_start_ms":START,"window_end_ms":START + WEEK,"totals_only":true}), &Caller { work: None, ..caller() }).unwrap();
        assert_eq!(result["coverage"]["available_collectors"], 2);
        assert_eq!(
            result["total_tokens"]["measured"],
            if headlined { 255 } else { 270 }
        );
        assert_eq!(
            result["coverage"]["state"], "partial",
            "the rich source's existing partial token coverage is preserved independently of deferred counts"
        );
        if headlined {
            for count in ["operations", "model_requests", "tool_calls"] {
                assert!(
                    result["totals"][count].is_null(),
                    "{count} cannot present a measured subset as the full aggregate"
                );
            }
        } else {
            assert_eq!(result["totals"]["operations"], 4);
            assert_eq!(result["totals"]["model_requests"], 4);
            assert_eq!(result["totals"]["tool_calls"], 0);
        }
        assert_eq!(result["cost"]["estimated_usd_micros"], Value::Null);
    }
}

#[test]
fn signed_components_survive_headline_and_rich_transport_without_changing_totals_or_prices() {
    for totals_only in [true, false] {
        for (source_count, output) in [(1, -3), (2, -3), (2, i64::MAX)] {
            let mut fixture = Fixture::new();
            let root = fixture._temporary.path();
            let mut peers = Vec::new();
            for index in 0..source_count {
                let path = root.join(format!("signed-{index}"));
                std::fs::create_dir(&path).unwrap();
                let mut cap = capability();
                cap["supportsTokenHeadlines"] = json!(true);
                peers.push(Peer::with_capability(
                    &path,
                    &path.join("home"),
                    Some(cap),
                    move |params| {
                        let mut value = if params["includeReview"] == false {
                            crate::usage_api_fixture::headline_response(params, START + WEEK)
                        } else {
                            priced_response(params)
                        };
                        value["report"]["providerTokens"] = json!([
                        ("total_tokens",135), ("input_tokens",-4),
                        ("input_tokens_details.cached_tokens",-2),
                            ("output_tokens",output), ("output_tokens_details.reasoning_tokens",-1),
                        ("cache_write_input_tokens",7),
                    ].map(|(category, quantity)| json!({
                        "category":category,"repositoryBucket":params["repositoryKey"],
                        "measurementProvenance":"provider_reported","measuredTokens":quantity,
                        "exactTokens":quantity,"unknownObservations":0,"observationCount":1,
                    })));
                        if params["includeReview"] != false {
                            for (field, quantity) in [
                                ("inputTokens", -4),
                                ("cachedInputTokens", -2),
                                ("uncachedInputTokens", -2),
                                ("outputTokens", output),
                                ("reasoningTokens", -1),
                                ("cacheWriteTokens", 7),
                            ] {
                                value["report"]["cost"][field] = json!(quantity);
                            }
                        }
                        value
                    },
                ));
            }
            fixture.config.codex_usage_sources =
                peers.iter().map(|peer| peer.source.clone()).collect();
            let plane = ControlPlane::with_adapters(
                fixture.config.clone(),
                fixture.database.clone(),
                Arc::new(|_: &RouteAccessSection| Ok(())),
                Arc::new(FixtureClock),
            )
            .unwrap();
            let result = plane
                .execute(
                    "performance.overview",
                    json!({
                        "repository_id":"project-alpha","window_start_ms":START,
                        "window_end_ms":START+WEEK,"totals_only":totals_only,
                    }),
                    &Caller {
                        work: None,
                        ..caller()
                    },
                )
                .unwrap();
            assert_eq!(result["total_tokens"]["measured"], source_count * 135);
            for (field, quantity) in [
                ("input_tokens", -4),
                ("cached_input_tokens", -2),
                ("output_tokens", output),
                ("reasoning_tokens", -1),
            ] {
                let expected = quantity
                    .checked_mul(source_count)
                    .map_or(Value::Null, |v| json!(v));
                assert_eq!(result["totals"][field], expected);
                assert_eq!(result["cost"][field], expected);
            }
            assert_eq!(result["cost"]["uncached_input_tokens"], -2 * source_count);
            assert_eq!(result["cost"]["cache_write_tokens"], 7 * source_count);
            assert_eq!(
                result["cost"]["estimated_usd_micros"],
                if totals_only {
                    Value::Null
                } else {
                    json!(135 * source_count)
                }
            );
            assert_eq!(
                result["coverage"]["sources"][0]["updated_at_ms"],
                START + WEEK
            );
            if output == i64::MAX {
                assert_eq!(result["coverage"]["events"]["token_quantity_overflow"], 1);
                if !totals_only {
                    assert_eq!(
                        result["cost"]["unavailable_reasons"]["quantity_overflow"],
                        1
                    );
                }
            }
            for peer in &peers {
                let requests = peer.requests.lock().unwrap();
                let request = requests
                    .iter()
                    .find(|request| request["method"] == "localUsage/summary")
                    .unwrap();
                assert_eq!(request["params"]["fromAt"], START);
                assert_eq!(request["params"]["toAt"], START + WEEK);
                assert_eq!(request["params"]["repositoryKey"], KEY);
                assert_eq!(request["params"]["includeReview"], !totals_only);
            }
        }
    }
    let schema = (devcoordinator2_api::operation("performance.overview")
        .unwrap()
        .output_schema)();
    for (definition, fields) in [
        (
            "UsageTotals",
            &[
                "input_tokens",
                "cached_input_tokens",
                "output_tokens",
                "reasoning_tokens",
            ][..],
        ),
        (
            "UsageCost",
            &[
                "input_tokens",
                "cached_input_tokens",
                "uncached_input_tokens",
                "output_tokens",
                "reasoning_tokens",
            ][..],
        ),
    ] {
        for field in fields {
            let property = &schema["$defs"][definition]["properties"][*field];
            assert_eq!(property["format"], "int64");
            assert!(
                property.get("minimum").is_none(),
                "{definition}.{field} accepts signed corrections"
            );
            assert!(
                property["type"]
                    .as_array()
                    .unwrap()
                    .contains(&json!("null"))
            );
        }
    }
    assert_eq!(
        schema["$defs"]["UsageTotals"]["properties"]["total_tokens"]["minimum"],
        0
    );
    assert_eq!(
        schema["$defs"]["UsageCost"]["properties"]["cache_write_tokens"]["minimum"],
        0
    );
}

#[test]
fn rich_execution_unions_preserve_source_measurements_without_summing_sources() {
    // The source supplies scalar unions, not interval positions. A second
    // positive union can overlap the first; a known empty union adds nothing.
    for (sources, expected_wall, expected_elapsed, expected_exact, expected_unknown) in [
        (vec![(1000, 0, 800, Some(800), 0)], 1000, 800, Some(800), 0),
        (vec![(0, 0, 0, Some(0), 0)], 0, 0, Some(0), 0),
        (vec![(0, 3, 0, None, 3)], 0, 0, None, 3),
        (vec![(1000, 2, 800, None, 2)], 1000, 800, None, 2),
        (
            vec![(1000, 0, 800, Some(800), 0), (1500, 0, 1200, Some(1200), 0)],
            1500,
            1200,
            None,
            1,
        ),
        (
            vec![(1000, 0, 800, Some(800), 0), (0, 0, 0, Some(0), 0)],
            1000,
            800,
            Some(800),
            0,
        ),
        (
            vec![(0, 0, 0, Some(0), 0), (1000, 0, 800, Some(800), 0)],
            1000,
            800,
            Some(800),
            0,
        ),
        (
            vec![(0, 0, 0, Some(0), 0), (0, 0, 0, Some(0), 0)],
            0,
            0,
            Some(0),
            0,
        ),
        (
            vec![(1000, 0, 800, Some(800), 0), (0, 2, 0, None, 2)],
            1000,
            800,
            None,
            2,
        ),
    ] {
        let mut fixture = Fixture::new();
        let root = fixture._temporary.path();
        let mut peers = Vec::new();
        for (index, (wall, unknown_wall, elapsed, exact, unknown_elapsed)) in
            sources.iter().copied().enumerate()
        {
            let path = root.join(format!("union-{index}"));
            std::fs::create_dir(&path).unwrap();
            peers.push(Peer::start(&path, &path.join("home"), move |params| {
                let mut value = response(params, START + WEEK, 135);
                value["review"]["time"]["execution_wall"] =
                    json!({"measured_ms":wall,"unknown_intervals":unknown_wall});
                value["review"]["time"]["summed_agent_active"] =
                    json!({"measured_ms":wall * 2,"unknown_intervals":unknown_wall});
                value["review"]["outcomes"]["totals"]["elapsedExecutionMs"] =
                    json!({"measured":elapsed,"exact":exact,"unknown":unknown_elapsed});
                value["review"]["outcomes"]["unattributed"]["elapsedExecutionMs"] =
                    json!({"measured":elapsed,"exact":exact,"unknown":unknown_elapsed});
                value["report"]["measuredExecutionIntervalSumMs"] = json!(9_000);
                value
            }));
        }
        fixture.config.codex_usage_sources = peers.iter().map(|peer| peer.source.clone()).collect();
        let plane = ControlPlane::with_adapters(
            fixture.config.clone(),
            fixture.database.clone(),
            Arc::new(|_: &RouteAccessSection| Ok(())),
            Arc::new(FixtureClock),
        )
        .unwrap();
        let prepared: Prepared = serde_json::from_value(
            plane
                .execute(
                    "review.prepare",
                    serde_json::to_value(input()).unwrap(),
                    &Caller {
                        work: None,
                        ..caller()
                    },
                )
                .unwrap(),
        )
        .unwrap();
        assert_eq!(
            prepared.usage.time.execution_wall.measured_ms,
            expected_wall
        );
        assert_eq!(
            prepared.usage.time.execution_wall.unknown_intervals,
            expected_unknown
        );
        assert_eq!(
            prepared.usage.time.summed_agent_active.measured_ms,
            sources.iter().map(|source| source.0 * 2).sum::<u64>()
        );
        assert_eq!(
            prepared.usage.time.summed_agent_active.unknown_intervals,
            sources.iter().map(|source| source.1).sum::<u64>()
        );
        assert_eq!(
            prepared.usage.outcomes.totals.active_agent_ms.measured, 0,
            "repository active time is not allocated into an outcome/workstream aggregate"
        );
        let measured = &prepared.usage.outcomes.totals.elapsed_execution_ms;
        assert_eq!(measured.measured, expected_elapsed);
        assert_eq!(measured.exact, expected_exact);
        assert_eq!(measured.unknown, expected_unknown);
        assert!(
            prepared
                .usage
                .coverage
                .sources
                .iter()
                .all(|source| source.measured_execution_interval_sum_ms == Some(9_000)),
            "diagnostic operation sums remain separate from wall unions"
        );
        let performance = plane.execute("performance.overview", json!({
            "repository_id":"project-alpha","window_start_ms":START,"window_end_ms":START + WEEK
        }), &Caller { work: None, ..caller() }).unwrap();
        assert_eq!(
            performance["usage"]["time"]["execution_wall"]["measured_ms"],
            expected_wall
        );
        assert_eq!(
            performance["usage"]["outcomes"]["totals"]["elapsedExecutionMs"]["measured"],
            expected_elapsed
        );
        assert_eq!(
            performance["usage"]["outcomes"]["totals"]["elapsedExecutionMs"]["exact"],
            json!(expected_exact)
        );
        assert_eq!(
            performance["usage"]["outcomes"]["totals"]["elapsedExecutionMs"]["unknown"],
            expected_unknown
        );
    }
}

#[test]
fn rich_cost_reasons_and_observation_units_survive_real_transport() {
    for source_count in [1, 2] {
        let mut fixture = Fixture::new();
        let root = fixture._temporary.path();
        let mut peers = Vec::new();
        for index in 0..source_count {
            let path = root.join(format!("cost-{index}"));
            std::fs::create_dir(&path).unwrap();
            peers.push(Peer::start(&path, &path.join("home"), |params| {
                let mut value = response(params, START + WEEK, 135);
                value["report"]["cost"] = json!({
                    "basis":"api_equivalent","currency":"USD","status":"unavailable","processingTier":"standard",
                    "estimatedUsdMicros":null,"inputUsdMicros":null,"cachedInputUsdMicros":null,"cacheWriteUsdMicros":null,"outputUsdMicros":null,
                    "inputTokens":135,"uncachedInputTokens":135,"cachedInputTokens":0,"cacheWriteTokens":0,"outputTokens":0,"reasoningTokens":0,"providerTotalTokens":135,
                    "pricedObservations":0,"unknownObservations":3,"unknownRequests":3,
                    "rateCardRefs":[],"unavailableReasons":{"rate_card_missing":3}
                }); value
            }));
        }
        fixture.config.codex_usage_sources = peers.iter().map(|peer| peer.source.clone()).collect();
        let plane = ControlPlane::with_adapters(
            fixture.config.clone(),
            fixture.database.clone(),
            Arc::new(|_: &RouteAccessSection| Ok(())),
            Arc::new(FixtureClock),
        )
        .unwrap();
        let prepared = plane
            .execute(
                "review.prepare",
                serde_json::to_value(input()).unwrap(),
                &Caller {
                    work: None,
                    ..caller()
                },
            )
            .unwrap();
        let cost = &prepared["usage"]["totals"]["cost"];
        assert_eq!(cost["status"], "unavailable");
        assert_eq!(cost["estimated_usd_micros"], Value::Null);
        assert_eq!(
            cost["unavailable_reasons"]["rate_card_missing"],
            source_count * 3
        );
        assert_eq!(
            cost["model_requests"],
            source_count * 2,
            "only the exact repository report count establishes unique model requests"
        );
        assert_eq!(cost["priced_requests"], Value::Null);
        assert_eq!(
            cost["unknown_requests"],
            Value::Null,
            "pricing observations are not unique requests"
        );
        assert_eq!(cost["priced_observations"], 0);
        assert_eq!(cost["unknown_observations"], source_count * 3);
        let schema = (devcoordinator2_api::operation("review.prepare")
            .unwrap()
            .output_schema)();
        for counter in [
            "model_requests",
            "priced_requests",
            "unknown_requests",
            "priced_observations",
            "unknown_observations",
        ] {
            assert!(
                schema["$defs"]["UsageCost"]["properties"][counter]["type"]
                    .as_array()
                    .is_some_and(|types| types.contains(&json!("null"))),
                "{counter} must represent unavailable counts in the public schema"
            );
        }
        assert_eq!(
            prepared["usage"]["outcomes"]["totals"]["cost"]["model_requests"],
            Value::Null,
            "repository request counts cannot allocate an outcome/workstream cost denominator"
        );
    }
}

#[test]
fn rich_scoped_cost_preserves_measured_request_counts_and_known_zero_estimate() {
    use devcoordinator2_api::results::UsageCost;
    let mut fixture = Fixture::new();
    let authorized = caller();
    crate::usage_context::ingest(
        &fixture.database,
        authorized.uid,
        "project-alpha",
        authorized.work.as_ref().unwrap(),
        START + 1,
    )
    .unwrap();
    let root = fixture._temporary.path();
    let peer = Peer::start(root, &root.join("known-cost"), |params| {
        let mut value = response(params, START + WEEK, 135);
        assert_eq!(
            value["review"]["outcomes"]["rows"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        value["review"]["outcomes"]["rows"][0]["effort"]["cost"] =
            serde_json::to_value(UsageCost {
                status: "complete".into(),
                basis: "api_equivalent".into(),
                currency: "USD".into(),
                processing_tier: "standard".into(),
                estimated_usd_micros: Some(0),
                input_usd_micros: Some(0),
                cached_input_usd_micros: Some(0),
                cache_write_usd_micros: Some(0),
                output_usd_micros: Some(0),
                input_tokens: Some(120),
                uncached_input_tokens: Some(120),
                cached_input_tokens: Some(0),
                cache_write_tokens: Some(0),
                output_tokens: Some(0),
                model_requests: Some(1),
                priced_requests: Some(1),
                unknown_requests: Some(0),
                priced_observations: Some(3),
                unknown_observations: Some(0),
                rate_card_ref: Some("fixture-free-rate".into()),
                rate_card_refs: vec!["fixture-free-rate".into()],
                ..Default::default()
            })
            .unwrap();
        value
    });
    fixture.config.codex_usage_sources = vec![peer.source.clone()];
    let plane = ControlPlane::with_adapters(
        fixture.config.clone(),
        fixture.database.clone(),
        Arc::new(|_: &RouteAccessSection| Ok(())),
        Arc::new(FixtureClock),
    )
    .unwrap();
    let result = plane
        .execute(
            "review.prepare",
            serde_json::to_value(input()).unwrap(),
            &Caller {
                work: None,
                ..authorized
            },
        )
        .unwrap();
    let cost = &result["usage"]["outcomes"]["rows"][0]["effort"]["cost"];
    assert_eq!(cost["model_requests"], 1);
    assert_eq!(cost["priced_requests"], 1);
    assert_eq!(cost["unknown_requests"], 0);
    assert_eq!(
        cost["priced_observations"], 3,
        "unique requests and pricing observations retain their separate units"
    );
    assert_eq!(cost["unknown_observations"], 0);
    assert_eq!(
        cost["estimated_usd_micros"], 0,
        "a measured zero estimate must not become unavailable"
    );
    assert_eq!(
        result["usage"]["totals"]["cost"]["estimated_usd_micros"],
        Value::Null
    );
}

#[test]
fn rich_cost_reason_labels_and_count_map_remain_bounded_and_private() {
    for reasons in [
        json!({"/private/source/secret":1}),
        json!({"private\nsource":1}),
        Value::Object(
            (0..65)
                .map(|index| (format!("reason_{index}"), json!(1)))
                .collect(),
        ),
    ] {
        let directory = tempfile::tempdir().unwrap();
        let peer = Peer::start(
            directory.path(),
            &directory.path().join("home"),
            move |params| {
                let mut value = priced_response(params);
                value["report"]["cost"]["unavailableReasons"] = reasons.clone();
                value
            },
        );
        let fixture = Fixture::new();
        let prepared = service(&fixture, peer.source.clone())
            .prepare(input(), START + WEEK)
            .unwrap();
        assert_eq!(prepared.usage.totals.total_tokens, None);
        assert_eq!(
            prepared.usage.coverage.unavailable_reasons["api_contract_unsupported"],
            1
        );
        let wire = serde_json::to_string(&prepared).unwrap();
        assert!(!wire.contains("/private/source/secret"));
    }
}

fn activity_response(params: &Value, count: usize) -> Value {
    use devcoordinator2_api::outcomes::OutcomeMeasurement;
    use devcoordinator2_api::results::{UsageActivity, UsageCost};

    let total = count as u64 * 20;
    let mut value = response(params, START + WEEK, total);
    let phases = [
        "planning",
        "implementation",
        "testing",
        "deployment",
        "reporting",
    ];
    let activities = [
        "coding",
        "research",
        "requirements",
        "repository_analysis",
        "unit_testing",
        "integration_testing",
        "build_validation",
        "documentation_authoring",
    ];
    let rows = (0..count).map(|index| {
        let activity = if count == 40 { activities[index % activities.len()].to_owned() } else { format!("activity_{index:03}") };
        json!({"phase":phases[index / activities.len() % phases.len()], "activity":activity, "attributionProvenance":"declared", "measuredTokens":20, "exactTokens":null, "unknownObservations":0})
    }).collect::<Vec<_>>();
    let mut review: ReviewUsage = serde_json::from_value(value["review"].clone()).unwrap();
    review.activities = rows
        .iter()
        .map(|row| UsageActivity {
            phase: row["phase"].as_str().unwrap().into(),
            activity: row["activity"].as_str().unwrap().into(),
            total_tokens: 20,
            share: None,
            operations: None,
            provenance: [("declared".into(), 20)].into(),
            cost: UsageCost::default(),
        })
        .collect();
    if let Some(first) = review.outcomes.rows.first_mut() {
        first.effort.provider_total_tokens = OutcomeMeasurement {
            measured: total - 25,
            exact: Some(total - 25),
            unknown: 0,
        };
        let mut next = first.clone();
        next.outcome_id = "other-outcome".into();
        next.effort.provider_total_tokens = OutcomeMeasurement {
            measured: 10,
            exact: Some(10),
            unknown: 0,
        };
        review.outcomes.rows.push(next);
        review.outcomes.total_rows = 2;
        review.outcomes.attributed.operations = 2;
        review.outcomes.totals.operations = 3;
    }
    if params["fromAt"] == START + 2 {
        review.outcomes.totals.activities.insert(
            "coding".into(),
            OutcomeMeasurement {
                measured: 7,
                exact: None,
                unknown: 0,
            },
        );
    }
    if params["workstreamId"].as_str().is_some() {
        review.outcomes.totals.provider_total_tokens = OutcomeMeasurement {
            measured: 100,
            exact: None,
            unknown: 0,
        };
    }
    value["review"] = serde_json::to_value(review).unwrap();
    value["report"]["providerTokensByActivity"] = json!(rows);
    value["snapshot"]["freshness"] = json!("stale");
    value["snapshot"]["pendingEntries"] = json!(97);
    value
}

#[test]
fn performance_activity_breakdowns_survive_paging_and_keep_review_scope() {
    let mut fixture = Fixture::new();
    let saved = fixture
        .service
        .record(super::tests::record(&fixture), "fixture", START + WEEK)
        .unwrap();
    let root = fixture._temporary.path();
    let peer = Peer::start(root, &root.join("activity-source"), |params| {
        activity_response(params, 40)
    });
    let authorized = caller();
    crate::usage_context::ingest(
        &fixture.database,
        authorized.uid,
        "project-alpha",
        authorized.work.as_ref().unwrap(),
        START + 1,
    )
    .unwrap();
    fixture.config.codex_usage_sources = vec![peer.source.clone()];
    let plane = ControlPlane::with_adapters(
        fixture.config.clone(),
        fixture.database.clone(),
        Arc::new(|_: &RouteAccessSection| Ok(())),
        Arc::new(FixtureClock),
    )
    .unwrap();
    let authorized = Caller {
        work: None,
        ..authorized
    };
    let params = json!({"repository_id":"project-alpha", "window_start_ms":START, "window_end_ms":START + WEEK, "outcome_limit":1});
    let result = plane
        .execute("performance.overview", params.clone(), &authorized)
        .unwrap();
    let shown: devcoordinator2_api::performance::OverviewResult =
        serde_json::from_value(result.clone()).unwrap();
    let usage = shown.usage.unwrap();
    assert_eq!(
        usage.activities.len(),
        40,
        "Performance must retain the source's activity matrix"
    );
    assert!(
        serde_json::to_vec(&usage).unwrap().len() > 16 * 1024,
        "fixture must cross the compact review ceiling"
    );
    assert!(serde_json::to_vec(&usage).unwrap().len() <= 64 * 1024);
    assert_eq!(shown.total_tokens.measured, 800);
    assert_eq!(usage.outcomes.totals.activities.len(), 8);
    for measurement in usage.outcomes.totals.activities.values() {
        assert_eq!(measurement.measured, 100);
        assert_eq!(measurement.exact, None);
    }
    assert!(!usage.coverage.events.is_empty());
    assert!(!usage.coverage.token_observations.is_empty());
    assert!(usage.coverage.snapshot.as_ref().unwrap().refreshing);
    assert!(!usage.coverage.snapshot.as_ref().unwrap().refresh_failed);
    assert!(
        usage
            .activities
            .iter()
            .all(|activity| activity.cost.status == "unavailable"
                && activity.cost.estimated_usd_micros.is_none())
    );
    for activity in &usage.activities {
        assert_eq!(activity.operations, None);
        assert_eq!(
            activity.cost.model_requests, None,
            "activity token observations do not establish unique request counts"
        );
        assert_eq!(activity.cost.priced_requests, None);
        assert_eq!(activity.cost.unknown_requests, None);
    }
    assert_eq!(
        usage.totals.model_requests,
        Some(2),
        "the independently measured repository request count remains available"
    );
    assert_eq!(shown.cost.model_requests, Some(2));
    assert_eq!(shown.cost.estimated_usd_micros, None);
    let envelope =
        devcoordinator2_api::ResponseEnvelope::success("activity-regression", result.clone())
            .unwrap();
    let wire = devcoordinator2_api::encode_response(&envelope);
    assert!(wire.len() < devcoordinator2_api::MAX_RESPONSE_BYTES);
    let envelope: Value = serde_json::from_slice(&wire).unwrap();
    assert_eq!(
        envelope["data"]["usage"]["activities"]
            .as_array()
            .unwrap()
            .len(),
        40
    );

    let mut next_params = params.clone();
    next_params["outcome_cursor"] = json!(usage.outcomes.next_cursor.unwrap());
    let next: devcoordinator2_api::performance::OverviewResult = serde_json::from_value(
        plane
            .execute("performance.overview", next_params, &authorized)
            .unwrap(),
    )
    .unwrap();
    let next = next.usage.unwrap();
    assert_eq!(next.activities.len(), 40);
    assert_eq!(next.outcomes.totals.activities.len(), 8);
    assert_eq!(next.outcomes.rows[0].outcome_id, "other-outcome");

    let packet: Prepared = serde_json::from_value(
        plane
            .execute(
                "review.prepare",
                serde_json::to_value(input()).unwrap(),
                &authorized,
            )
            .unwrap(),
    )
    .unwrap();
    assert!(serde_json::to_vec(&packet.usage).unwrap().len() <= 16 * 1024);
    assert!(packet.usage.outcomes.totals.activities.is_empty());
    let scoped: devcoordinator2_api::performance::ReviewResult = serde_json::from_value(plane.execute("performance.review", json!({"repository_id":"project-alpha", "reference":saved.reference, "outcome_limit":1}), &authorized).unwrap()).unwrap();
    assert_eq!(scoped.usage.activities.len(), 40);
    assert!(
        scoped.usage.outcomes.totals.activities.is_empty(),
        "repository activity cannot allocate named-workstream effort"
    );
    assert_eq!(
        scoped.usage.outcomes.totals.provider_total_tokens.measured,
        100
    );

    let mut explicit_params = params;
    explicit_params["window_start_ms"] = json!(START + 2);
    let explicit: devcoordinator2_api::performance::OverviewResult = serde_json::from_value(
        plane
            .execute("performance.overview", explicit_params, &authorized)
            .unwrap(),
    )
    .unwrap();
    let activities = explicit.usage.unwrap().outcomes.totals.activities;
    assert_eq!(
        activities.len(),
        1,
        "an explicit producer map must not be overwritten"
    );
    assert_eq!(activities["coding"].measured, 7);
}

#[test]
fn performance_activity_breakdowns_fail_explicitly_beyond_display_bound() {
    let mut fixture = Fixture::new();
    let root = fixture._temporary.path();
    let peer = Peer::start(root, &root.join("large-source"), |params| {
        activity_response(params, 200)
    });
    fixture.config.codex_usage_sources = vec![peer.source.clone()];
    let plane = ControlPlane::with_adapters(
        fixture.config.clone(),
        fixture.database.clone(),
        Arc::new(|_: &RouteAccessSection| Ok(())),
        Arc::new(FixtureClock),
    )
    .unwrap();
    let error = plane.execute("performance.overview", json!({"repository_id":"project-alpha", "window_start_ms":START, "window_end_ms":START + WEEK, "outcome_limit":1}), &Caller {work:None, ..caller()}).unwrap_err();
    assert_eq!(error.code, ErrorCode::ParamsInvalid);
    assert!(error.message.contains("exceeds the supported size"));
}

#[test]
fn performance_and_repository_totals_resolve_cold_identity_without_sibling_requests() {
    use crate::usage_api_fixture::ResponseGate;

    for operation in ["performance.overview", "usage.repositories"] {
        let mut fixture = Fixture::new();
        let root = fixture._temporary.path();
        let gate = ResponseGate::default();
        let response_gate = gate.clone();
        let peer = Peer::start(root, &root.join("usage-home"), move |params| {
            response_gate.wait();
            priced_response(params)
        });
        let release = gate.release_on_drop();
        fixture.config.codex_usage_sources = vec![peer.source.clone()];
        let plane = ControlPlane::with_adapters(
            fixture.config.clone(),
            fixture.database.clone(),
            Arc::new(|_: &RouteAccessSection| Ok(())),
            Arc::new(FixtureClock),
        )
        .unwrap();
        let mut params = if operation == "performance.overview" {
            json!({"repository_id":"project-alpha","window_start_ms":START,"window_end_ms":START + WEEK})
        } else {
            json!({"range":"24h"})
        };
        let authorized = Caller {
            work: None,
            ..caller()
        };
        let cold = plane
            .execute(operation, params.clone(), &authorized)
            .unwrap();
        if operation == "performance.overview" {
            assert_eq!(cold["total_tokens"]["exact"], Value::Null);
            assert_eq!(cold["totals"]["total_tokens"], Value::Null);
            assert_eq!(cold["coverage"]["available_collectors"], 0);
        } else {
            assert_eq!(cold["repositories"][0]["total_tokens"], Value::Null);
        }
        release.release();
        params["wait_for_refresh"] = json!(true);
        let populated = plane.execute(operation, params, &authorized).unwrap();
        let (total, totals, coverage, cost) = if operation == "performance.overview" {
            (
                &populated["total_tokens"]["measured"],
                Some(&populated["totals"]),
                &populated["coverage"],
                &populated["cost"],
            )
        } else {
            let row = &populated["repositories"][0];
            (&row["total_tokens"], None, &row["coverage"], &row["cost"])
        };
        assert_eq!(total, 135, "{operation} must finish its own cold mapping");
        assert_eq!(coverage["available_collectors"], 1, "{operation}");
        assert_eq!(cost["status"], "complete", "{operation}");
        assert!(
            cost["estimated_usd_micros"]
                .as_u64()
                .is_some_and(|value| value > 0)
        );
        if let Some(totals) = totals {
            assert_eq!(totals["input_tokens"], 135);
            assert!(
                populated["usage"]["activities"]
                    .as_array()
                    .is_some_and(|rows| !rows.is_empty())
            );
        }
        let requests = peer.requests.lock().unwrap();
        assert_eq!(
            requests
                .iter()
                .filter(|request| request["method"] == "localUsage/repositoryIdentity")
                .count(),
            1,
            "{operation} resolves and retains its own identity"
        );
        assert!(
            requests
                .iter()
                .any(|request| request["method"] == "localUsage/summary")
        );
    }
}

#[test]
fn performance_overview_retains_the_owned_read_past_foreground_and_recency_budgets() {
    use crate::usage_api_fixture::ResponseGate;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    let mut fixture = Fixture::new();
    let root = fixture._temporary.path();
    let gate = ResponseGate::default();
    let response_gate = gate.clone();
    let calls = Arc::new(AtomicUsize::new(0));
    let observed = calls.clone();
    let (entered_source, entered) = std::sync::mpsc::channel();
    let peer = Peer::start(root, &root.join("slow-source"), move |params| {
        observed.fetch_add(1, Ordering::SeqCst);
        entered_source.send(()).unwrap();
        response_gate.wait_for(Duration::from_secs(45));
        let mut value = priced_response(params);
        let mut next = value["review"]["outcomes"]["rows"][0].clone();
        next["outcomeId"] = json!("other-outcome");
        next["effort"]["providerTotalTokens"] = json!({"measured":10,"exact":10,"unknown":0});
        value["review"]["outcomes"]["rows"][0]["effort"]["providerTotalTokens"] =
            json!({"measured":110,"exact":110,"unknown":0});
        value["review"]["outcomes"]["rows"]
            .as_array_mut()
            .unwrap()
            .push(next);
        value["review"]["outcomes"]["totalRows"] = json!(2);
        value["review"]["outcomes"]["attributed"]["operations"] = json!(2);
        value["review"]["outcomes"]["totals"]["operations"] = json!(3);
        value["review"]["totals"]["operations"] = json!(3);
        value["report"]["counts"]["operations"] = json!(3);
        value
    });
    let release = gate.release_on_drop();
    let authorized = caller();
    crate::usage_context::ingest(
        &fixture.database,
        authorized.uid,
        "project-alpha",
        authorized.work.as_ref().unwrap(),
        START + 1,
    )
    .unwrap();
    fixture.service = service(&fixture, peer.source.clone());
    let params = || devcoordinator2_api::performance::Overview {
        repository_id: "project-alpha".into(),
        window_start_ms: START,
        window_end_ms: START + WEEK,
        outcome_cursor: None,
        outcome_limit: Some(1),
        totals_only: false,
        wait_for_refresh: true,
    };
    let began = Instant::now();
    let cold = fixture
        .service
        .performance_overview(params(), START + WEEK)
        .unwrap();
    assert!(began.elapsed() < Duration::from_secs(1));
    assert_eq!(cold.total_tokens.exact, None);
    assert_eq!(cold.totals.unwrap().total_tokens, None);
    assert_eq!(cold.coverage.available_collectors, 0);
    assert!(cold.coverage.snapshot.unwrap().refreshing);
    entered.recv_timeout(Duration::from_secs(2)).unwrap();

    // Observe the real request's terminal-state condition through the former
    // 30-second transport deadline. A dropped socket would finish the owned
    // read and wake this observer early; it must remain in flight until the
    // explicit response release below. Advancing request time also crosses
    // the former two-minute routing cutoff without a two-minute wall wait.
    assert!(
        !fixture
            .service
            .usage
            .wait_for_collector_refresh(Instant::now() + Duration::from_secs(31)),
        "the owned source read must remain in flight past its former deadline"
    );
    let later = START + WEEK + 180_000;
    let began = Instant::now();
    let pending = fixture
        .service
        .performance_overview(params(), later)
        .unwrap();
    assert!(began.elapsed() < Duration::from_secs(1));
    assert!(pending.coverage.snapshot.unwrap().refreshing);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    release.release();
    assert!(
        fixture
            .service
            .usage
            .wait_for_collector_refresh(Instant::now() + Duration::from_secs(5))
    );

    let populated = fixture
        .service
        .performance_overview(params(), later)
        .unwrap();
    assert_eq!(populated.total_tokens.measured, 135);
    assert_eq!(populated.totals.unwrap().input_tokens, Some(135));
    assert_eq!(populated.cost.estimated_usd_micros, Some(135));
    let usage = populated.usage.unwrap();
    assert_eq!(usage.outcomes.rows.len(), 1);
    assert_eq!(usage.outcomes.rows[0].outcome_id, "spec");
    assert_eq!(
        usage.outcomes.rows[0].title.as_deref(),
        Some("Specify the workflow")
    );
    assert_eq!(usage.outcomes.rows[0].kind.as_deref(), Some("improvement"));
    assert_eq!(usage.outcomes.kinds["improvement"].measured, 110);
    assert_eq!(usage.outcomes.kinds["unknown"].measured, 10);
    assert_eq!(
        usage.outcomes.unattributed.provider_total_tokens.measured,
        15
    );
    assert!(!usage.activities.is_empty());
    let next = fixture
        .service
        .performance_overview(
            devcoordinator2_api::performance::Overview {
                outcome_cursor: usage.outcomes.next_cursor,
                ..params()
            },
            later + 90 * 86_400_000,
        )
        .unwrap()
        .usage
        .unwrap();
    assert_eq!(next.outcomes.rows.len(), 1);
    assert_eq!(next.outcomes.rows[0].outcome_id, "other-outcome");
    assert_eq!(
        next.outcomes.rows[0].effort.provider_total_tokens.measured,
        10
    );
    assert!(next.outcomes.next_cursor.is_none());
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "retry must consume the completed exact-window read"
    );
}

#[test]
fn review_prepare_uses_authorized_operation_context_over_real_transport_with_legacy_source_separate()
 {
    let mut fixture = Fixture::new();
    let root = fixture._temporary.path();
    let home = root.join("fresh");
    let peer = Peer::start(root, &home, |params| response(params, START + WEEK, 135));
    let legacy = root.join("retired");
    std::fs::create_dir_all(legacy.join("usage")).unwrap();
    let sentinel = legacy.join("usage/usage.sqlite3");
    std::fs::write(&sentinel, "retired-source-must-never-open").unwrap();
    let original = std::fs::metadata(&sentinel).unwrap();
    fixture.config.codex_usage_sources = vec![
        peer.source.clone(),
        crate::config::CodexUsageSource {
            uid: peer.source.uid + 1,
            codex_home: legacy.clone(),
            api_socket: None,
            executable: "/must-not-execute-retired-cli".into(),
        },
    ];
    // An old link must not mask the new negotiated repository identity.
    fixture.database.transaction(|tx| { tx.execute("INSERT INTO codex_usage_repository_links(source_uid,repository_id,codex_repository_id,source_schema,taxonomy_version,resolved_at) VALUES($1,'project-alpha',$2,9,1,'t')",crate::database::params![peer.source.uid,"b".repeat(64)])?;Ok(()) }).unwrap();
    fixture.service = ReviewService::new(
        fixture.database.clone(),
        UsageService::with_clock(
            fixture.config.clone(),
            fixture.database.clone(),
            crate::repository::Registry::new(fixture.database.clone()),
            Arc::new(FixtureClock),
        ),
    );
    let missing = fixture.service.prepare(input(), START + WEEK).unwrap();
    assert!(missing.usage.outcomes.rows.is_empty());
    assert_eq!(missing.usage.outcomes.unattributed.operations, 2);
    let now =
        time::OffsetDateTime::from_unix_timestamp_nanos(i128::from(START + WEEK - 1) * 1_000_000)
            .unwrap();
    let plane = ControlPlane::with_adapters(
        fixture.config.clone(),
        fixture.database.clone(),
        Arc::new(|_: &RouteAccessSection| Ok(())),
        Arc::new(crate::platform::FixedClock(now)),
    )
    .unwrap();
    let authorized = caller();
    plane
        .execute(
            "plan.overview",
            json!({"repository_id":"project-alpha"}),
            &authorized,
        )
        .unwrap();
    let review_plane = ControlPlane::with_adapters(
        fixture.config.clone(),
        fixture.database.clone(),
        Arc::new(|_: &RouteAccessSection| Ok(())),
        Arc::new(FixtureClock),
    )
    .unwrap();
    let prepared: Prepared = serde_json::from_value(
        review_plane
            .execute(
                "review.prepare",
                serde_json::to_value(input()).unwrap(),
                &Caller {
                    work: None,
                    ..authorized.clone()
                },
            )
            .unwrap(),
    )
    .unwrap();
    assert_eq!(prepared.usage.coverage.available_collectors, 1);
    assert_eq!(prepared.usage.coverage.configured_collectors, 2);
    assert_eq!(
        prepared
            .usage
            .coverage
            .unavailable_reasons
            .get("legacy_source_unsupported"),
        Some(&1)
    );
    assert_eq!(prepared.usage.totals.total_tokens, Some(135));
    assert_eq!(prepared.usage.outcomes.rows.len(), 1);
    assert_eq!(prepared.usage.outcomes.rows[0].outcome_id, "spec");
    assert_eq!(
        prepared.usage.outcomes.rows[0].title.as_deref(),
        Some("Specify the workflow")
    );
    let metadata = &prepared.usage.coverage.sources[0];
    assert_eq!(
        (
            metadata.contract_version,
            metadata.format_version,
            metadata.generation,
            metadata.journal_watermark
        ),
        (1, 3, 7, 10)
    );
    assert_eq!(metadata.freshness, "fresh");
    assert_eq!(metadata.pending_entries, 0);
    let requests = peer.requests.lock().unwrap();
    assert!(
        requests
            .iter()
            .any(|r| r["method"] == "localUsage/repositoryIdentity")
    );
    let binding = requests
        .iter()
        .filter_map(|r| r["params"]["workContextBindings"].as_array())
        .flat_map(|b| b.iter())
        .next()
        .expect("actual persisted context sent");
    assert_eq!(binding["operationId"], "governed-op");
    assert_eq!(binding["workContext"]["id"], "C1");
    for (field, expected) in [
        ("repositoryId", "project-alpha"),
        ("nativeProjectId", "fixture-project"),
        ("threadId", "fixture-thread"),
        ("turnId", "fixture-turn"),
        ("workstreamId", "spec"),
        ("outcomeId", "spec"),
        ("experimentRef", "experiment@1"),
    ] {
        assert_eq!(binding["workContext"][field], expected);
    }
    drop(requests);
    let wire = serde_json::to_string(&prepared).unwrap();
    for private in [
        home.to_str().unwrap(),
        legacy.to_str().unwrap(),
        KEY,
        "fixture-thread",
        "fixture-turn",
        "fixture-project",
        "governed-op",
    ] {
        assert!(
            !wire.contains(private),
            "private context escaped: {private}"
        );
    }
    assert_eq!(
        std::fs::metadata(&sentinel).unwrap().accessed().unwrap(),
        original.accessed().unwrap()
    );
    assert_eq!(
        std::fs::read(&sentinel).unwrap(),
        b"retired-source-must-never-open"
    );
    // An unchanged, evidence-backed decision is conclusive without inventing
    // before/after gains or exact timing that the collector did not capture.
    let mut request = super::tests::record(&fixture);
    request.record.experiment.baseline.evidence_refs = prepared.source_refs.clone();
    request.record.experiment.baseline.missing_measurements = prepared.coverage_gaps.clone();
    let saved = fixture
        .service
        .record(request, "fixture", START + WEEK)
        .unwrap();
    assert!(saved.completed);
    assert_eq!(saved.record.experiment.disposition, Disposition::Unchanged);
    assert_eq!(
        fixture
            .service
            .receipt(Reference {
                reference: saved.reference.clone()
            })
            .unwrap(),
        saved
    );
}

#[test]
fn review_prepare_fails_closed_on_missing_or_unsupported_handshake_and_snapshot_metadata() {
    for mode in 0..5 {
        let fixture = Fixture::new();
        let root = fixture._temporary.path();
        let home = root.join("fresh");
        let mut cap = capability();
        if mode == 1 {
            cap["version"] = json!(2);
        }
        let peer = Peer::with_capability(root, &home, (mode != 0).then_some(cap), move |params| {
            let mut value = response(params, START + WEEK, 135);
            match mode {
                2 => {
                    value.as_object_mut().unwrap().remove("snapshot");
                }
                3 => {
                    value["snapshot"]
                        .as_object_mut()
                        .unwrap()
                        .remove("pendingEntries");
                }
                4 => value["snapshot"]["journalWatermark"] = json!(9),
                _ => {}
            }
            value
        });
        let prepared = service(&fixture, peer.source.clone())
            .prepare(input(), START + WEEK)
            .unwrap();
        assert_eq!(
            prepared.usage.coverage.available_collectors, 0,
            "mode {mode}"
        );
        assert!(prepared.usage.outcomes.rows.is_empty());
        assert_eq!(prepared.usage.totals.total_tokens, None);
        let expected = if mode == 4 {
            "api_snapshot_inconsistent"
        } else {
            "api_contract_unsupported"
        };
        assert!(
            prepared
                .usage
                .coverage
                .unavailable_reasons
                .contains_key(expected),
            "mode {mode}: {:?}",
            prepared.usage.coverage.unavailable_reasons
        );
    }
}

#[test]
fn review_context_window_is_indexed_bounded_uid_scoped_and_conflicts_stay_unattributed() {
    let fixture = Fixture::new();
    let root = fixture._temporary.path();
    let home = root.join("fresh");
    let peer = Peer::start(root, &home, |params| response(params, START + WEEK, 135));
    let call = caller();
    let work = call.work.clone().unwrap();
    crate::usage_context::ingest(
        &fixture.database,
        call.uid,
        "project-alpha",
        &work,
        START + 1,
    )
    .unwrap();
    crate::usage_context::ingest(
        &fixture.database,
        call.uid,
        "project-alpha",
        &work,
        START + 2,
    )
    .unwrap();
    let selected = crate::usage_context::read(
        &fixture.database,
        call.uid,
        "project-alpha",
        START,
        START + WEEK,
    )
    .unwrap();
    assert_eq!(selected.bindings.len(), 1);
    assert_eq!(selected.bindings[0].captured_at_ms, START + 1);
    assert_eq!(
        crate::usage_context::read(
            &fixture.database,
            call.uid + 1,
            "project-alpha",
            START,
            START + WEEK
        )
        .unwrap()
        .bindings
        .len(),
        0
    );
    assert_eq!(
        crate::usage_context::read(
            &fixture.database,
            call.uid,
            "project-alpha",
            START + 2,
            START + WEEK
        )
        .unwrap()
        .bindings
        .len(),
        0
    );
    let mut conflict = work.clone();
    conflict.context.as_mut().unwrap().outcome_id = Some("other".into());
    crate::usage_context::ingest(
        &fixture.database,
        call.uid,
        "project-alpha",
        &conflict,
        START + 3,
    )
    .unwrap();
    // The read-only identical-declaration fast path must never clear a
    // previously recorded conflict or replace its original declaration.
    crate::usage_context::ingest(
        &fixture.database,
        call.uid,
        "project-alpha",
        &work,
        START + 4,
    )
    .unwrap();
    let rejected = crate::usage_context::read(
        &fixture.database,
        call.uid,
        "project-alpha",
        START,
        START + WEEK,
    )
    .unwrap();
    assert!(rejected.bindings.is_empty());
    assert_eq!(rejected.rejected, 1);
    let prepared = service(&fixture, peer.source.clone())
        .prepare(input(), START + WEEK)
        .unwrap();
    assert!(prepared.usage.outcomes.rows.is_empty());
    assert_eq!(
        prepared
            .usage
            .outcomes
            .unattributed_reasons
            .get("work_context_binding_conflict"),
        Some(&1)
    );
    // Populate a new finite window beyond the rejection; the reader may fetch
    // one sentinel row beyond its cap, never a collector history scan.
    for index in 0..257 {
        let mut other = work.clone();
        other.context.as_mut().unwrap().operation_id = Some(format!("operation-{index}"));
        crate::usage_context::ingest(
            &fixture.database,
            call.uid,
            "project-alpha",
            &other,
            START + 10,
        )
        .unwrap();
    }
    let window = crate::usage_context::read(
        &fixture.database,
        call.uid,
        "project-alpha",
        START + 10,
        START + WEEK,
    )
    .unwrap();
    assert!(window.truncated);
    assert_eq!(window.bindings.len(), 256);
    let prepared = service(&fixture, peer.source.clone())
        .prepare(input(), START + WEEK)
        .unwrap();
    assert_eq!(
        prepared
            .usage
            .outcomes
            .unattributed_reasons
            .get("work_context_bindings_truncated"),
        Some(&1)
    );
    assert_ne!(prepared.usage.outcomes.coverage, "complete");
}

#[test]
#[ignore = "requires an explicit candidate binary and isolated synthetic usage-v2 fixture"]
fn review_prepare_against_candidate_usage_v2_app_server() {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    use std::os::unix::process::CommandExt;
    use std::path::Path;
    use std::process::{Child, Command, Stdio};
    use std::time::{Duration, Instant};
    struct Candidate(Child);
    impl Drop for Candidate {
        fn drop(&mut self) {
            // Only this exact child and its process group belong to this test.
            unsafe {
                libc::kill(-(self.0.id() as i32), libc::SIGKILL);
            }
            let _ = self.0.wait();
        }
    }
    fn copy_fixture(source: &Path, target: &Path) {
        std::fs::create_dir_all(target).unwrap();
        for entry in std::fs::read_dir(source).unwrap() {
            let entry = entry.unwrap();
            let kind = entry.file_type().unwrap();
            assert!(
                !kind.is_symlink(),
                "synthetic fixture contains an unsupported symlink"
            );
            if kind.is_dir() {
                copy_fixture(&entry.path(), &target.join(entry.file_name()));
            } else {
                assert!(kind.is_file());
                std::fs::copy(entry.path(), target.join(entry.file_name())).unwrap();
            }
        }
    }
    let binary =
        std::env::var_os("CODEX_USAGE_V2_APP_SERVER").expect("candidate app-server binary");
    let source = std::path::PathBuf::from(
        std::env::var_os("CODEX_USAGE_V2_FIXTURE_HOME").expect("isolated synthetic fixture home"),
    );
    let mut fixture = Fixture::new();
    let mut request = super::tests::record(&fixture);
    let info: serde_json::Value = serde_json::from_slice(
        &std::fs::read(source.join("fixture-info.json")).expect("synthetic fixture description"),
    )
    .unwrap();
    let start = info["windowStartMs"]
        .as_u64()
        .expect("fixture window start");
    let end = info["windowEndMs"].as_u64().expect("fixture window end");
    let captured = info["capturedAtMs"]
        .as_u64()
        .expect("fixture context timestamp");
    let fixture_tokens = info["providerTokens"]
        .as_u64()
        .expect("fixture token total");
    let additional_operations = info["additionalOperations"]
        .as_array()
        .expect("fixture operation rows");
    let unattributed_tokens = additional_operations
        .iter()
        .map(|operation| {
            operation["providerTokens"]
                .as_u64()
                .expect("operation tokens")
        })
        .sum::<u64>();
    let unattributed_operations = additional_operations.len() as u64;
    assert_eq!(fixture_tokens, 120 + unattributed_tokens);
    let repository = std::env::var("CODEX_USAGE_V2_FIXTURE_REPOSITORY")
        .ok()
        .or_else(|| info["repositoryPath"].as_str().map(str::to_owned))
        .expect("fixture repository path");
    assert!(Path::new(&repository).is_absolute());
    let home = fixture._temporary.path().join("candidate-home");
    copy_fixture(&source.join("usage-v2"), &home.join("usage-v2"));
    let copied_info: serde_json::Value =
        serde_json::from_slice(&std::fs::read(source.join("fixture-info.json")).unwrap()).unwrap();
    assert_eq!(
        info, copied_info,
        "synthetic fixture changed while being copied"
    );
    fixture.database.transaction(|tx| {
        tx.execute("UPDATE repositories SET root_path=$1 WHERE repository_id='project-alpha'",[&repository])?;
        tx.execute_batch("INSERT INTO tasks(task_id,repository_id,seq,position,title,outcome,kind,status,created_at,created_by,updated_at) VALUES('fixture-outcome','project-alpha',2,2,'Review scoped usage','Prove explicit operation attribution','improvement','in_progress','1970-01-01T00:00:00Z','fixture','1970-01-01T00:00:00Z')")?;
        Ok(())
    }).unwrap();
    let socket_directory = fixture._temporary.path().join("private-socket");
    std::fs::create_dir(&socket_directory).unwrap();
    std::fs::set_permissions(&socket_directory, std::fs::Permissions::from_mode(0o700)).unwrap();
    let socket_metadata = std::fs::metadata(&socket_directory).unwrap();
    assert_eq!(socket_metadata.uid(), rustix::process::geteuid().as_raw());
    assert_eq!(socket_metadata.permissions().mode() & 0o777, 0o700);
    let socket = socket_directory.join("candidate.sock");
    let child = Command::new(binary)
        .arg("--listen")
        .arg(format!("unix://{}", socket.display()))
        .env("CODEX_HOME", &home)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
        .expect("start isolated candidate app-server");
    let mut child = Candidate(child);
    let deadline = Instant::now() + Duration::from_secs(15);
    while !socket.exists() {
        assert!(
            child.0.try_wait().unwrap().is_none(),
            "candidate app-server exited before binding"
        );
        assert!(
            Instant::now() < deadline,
            "candidate app-server socket readiness deadline"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    let legacy = fixture
        ._temporary
        .path()
        .join("retired-source-must-not-open");
    fixture.config.codex_usage_sources = vec![
        crate::config::CodexUsageSource {
            uid: rustix::process::getuid().as_raw(),
            codex_home: home,
            api_socket: Some(socket),
            executable: "/must-not-execute-retired-cli".into(),
        },
        crate::config::CodexUsageSource {
            uid: rustix::process::getuid().as_raw() + 1,
            codex_home: legacy.clone(),
            api_socket: None,
            executable: "/must-not-execute-retired-cli".into(),
        },
    ];
    let clock = Arc::new(crate::platform::FixedClock(
        time::OffsetDateTime::from_unix_timestamp_nanos(i128::from(captured) * 1_000_000).unwrap(),
    ));
    let plane = ControlPlane::with_adapters(
        fixture.config.clone(),
        fixture.database.clone(),
        Arc::new(|_: &RouteAccessSection| Ok(())),
        clock,
    )
    .unwrap();
    let mut operation = caller();
    let context = operation.work.as_mut().unwrap().context.as_mut().unwrap();
    context.operation_id = Some("fixture-model".into());
    context.workstream_id = Some("fixture-workstream".into());
    context.outcome_id = Some("fixture-outcome".into());
    context.experiment_ref = Some("fixture-experiment@1".into());
    plane
        .execute(
            "plan.overview",
            json!({"repository_id":"project-alpha"}),
            &operation,
        )
        .unwrap();
    fixture.service = ReviewService::new(
        fixture.database.clone(),
        UsageService::with_clock(
            fixture.config.clone(),
            fixture.database.clone(),
            crate::repository::Registry::new(fixture.database.clone()),
            Arc::new(crate::platform::FixedClock(
                time::OffsetDateTime::from_unix_timestamp_nanos(i128::from(end) * 1_000_000)
                    .unwrap(),
            )),
        ),
    );
    let scope = Prepare {
        window_start_ms: start,
        window_end_ms: end,
        workstream_id: Some("fixture-workstream".into()),
        ..input()
    };
    let began = Instant::now();
    let cold = fixture.service.prepare(scope.clone(), end).unwrap();
    eprintln!(
        "candidate_cold_prepare_ms={} reasons={:?}",
        began.elapsed().as_millis(),
        cold.usage.coverage.unavailable_reasons
    );
    assert!(
        began.elapsed() < Duration::from_millis(750),
        "cold review exceeded its bounded foreground window"
    );
    if cold.usage.coverage.available_collectors == 0 {
        assert_eq!(cold.usage.totals.total_tokens, None);
        assert_eq!(
            cold.usage
                .coverage
                .unavailable_reasons
                .get("api_refresh_pending"),
            Some(&1)
        );
        let snapshot = cold
            .usage
            .coverage
            .snapshot
            .as_ref()
            .expect("explicit pending snapshot");
        assert!(snapshot.refreshing);
        assert!(!snapshot.refresh_failed);
        assert!(
            fixture
                .service
                .usage
                .wait_for_collector_refresh(Instant::now() + Duration::from_secs(15)),
            "finite source read did not finish"
        );
    }
    let warm_began = Instant::now();
    let prepared = fixture.service.prepare(scope, end).unwrap();
    eprintln!(
        "candidate_warm_prepare_ms={}",
        warm_began.elapsed().as_millis()
    );
    assert!(
        warm_began.elapsed() < Duration::from_millis(500),
        "cached review exceeded foreground budget"
    );
    assert_eq!(
        prepared.usage.coverage.available_collectors, 1,
        "{:?}",
        prepared.usage.coverage.unavailable_reasons
    );
    assert_eq!(prepared.usage.coverage.configured_collectors, 2);
    assert_eq!(
        prepared
            .usage
            .coverage
            .unavailable_reasons
            .get("legacy_source_unsupported"),
        Some(&1)
    );
    assert!(!legacy.exists());
    assert!(
        prepared
            .usage
            .totals
            .total_tokens
            .is_some_and(|value| value >= 120)
    );
    assert_eq!(prepared.usage.coverage.sources[0].source, "usage-v2");
    assert!(
        prepared
            .usage
            .outcomes
            .rows
            .iter()
            .any(|row| row.outcome_id == "fixture-outcome"
                && row.effort.provider_total_tokens.measured == 120)
    );
    // This request captures only fixture-model. The retained bound-summary
    // also bound fixture-other to other-stream and excluded its 40 tokens;
    // here all four additional operations must remain unattributed.
    assert_eq!(
        prepared
            .usage
            .outcomes
            .totals
            .provider_total_tokens
            .measured,
        fixture_tokens
    );
    assert_eq!(
        prepared
            .usage
            .outcomes
            .attributed
            .provider_total_tokens
            .measured,
        120
    );
    assert_eq!(
        prepared
            .usage
            .outcomes
            .unattributed
            .provider_total_tokens
            .measured,
        unattributed_tokens
    );
    assert_eq!(
        prepared.usage.outcomes.unattributed.operations,
        unattributed_operations
    );
    assert!(
        prepared
            .usage
            .outcomes
            .totals
            .elapsed_execution_ms
            .exact
            .is_none()
    );
    request.record.window_start_ms = start;
    request.record.window_end_ms = end;
    request.record.workstream_id = Some("fixture-workstream".into());
    request.record.outcome_id = Some("fixture-outcome".into());
    request.record.experiment.baseline.evidence_refs = prepared.source_refs;
    request.record.experiment.baseline.missing_measurements = prepared.coverage_gaps;
    request.record.experiment.baseline.interpretation="The candidate collector reports actual synthetic provider measurements with explicit operation attribution; unavailable timing remains acknowledged.".into();
    request.record.experiment.observations=vec![Observation {
        kind:ObservationKind::IntentionalValidation,
        interpretation:"The same candidate transport and review checks preserve the required quality boundary. These necessary checks are not treated as wasted work.".into(),
        evidence_refs:vec![EvidenceRef { kind:EvidenceKind::Outcome,reference:"fixture-outcome".into() }],
    }];
    request.record.experiment.reason="The available scoped evidence supports retaining the current workflow. There is no claimed performance gain and the unrecorded timing remains unknown.".into();
    let saved = fixture.service.record(request, "fixture", end).unwrap();
    assert!(saved.completed);
    assert_eq!(saved.record.experiment.disposition, Disposition::Unchanged);
    assert_eq!(
        fixture
            .service
            .receipt(Reference {
                reference: saved.reference.clone()
            })
            .unwrap(),
        saved
    );
}

#[test]
fn delayed_collector_finishes_once_after_bounded_cold_review_and_supplies_attributed_receipt() {
    use crate::usage_api_fixture::ResponseGate;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{Duration, Instant};
    let mut fixture = Fixture::new();
    let root = fixture._temporary.path();
    let home = root.join("slow-source");
    let calls = Arc::new(AtomicUsize::new(0));
    let observed = calls.clone();
    let (entered_source, entered) = std::sync::mpsc::channel();
    let gates = (0..5).map(|_| ResponseGate::default()).collect::<Vec<_>>();
    let response_gates = gates.clone();
    let peer = Peer::start(root, &home, move |params| {
        let call = observed.fetch_add(1, Ordering::SeqCst);
        entered_source.send(call).unwrap();
        response_gates[call].wait();
        let mut value = response(params, START + WEEK - 1000, [135, 150, 150, 180, 210][call]);
        value["generatedAt"] = json!(START + WEEK);
        if call == 2 {
            value.as_object_mut().unwrap().remove("contractVersion");
        }
        value
    });
    // Release on assertion failure as well, before Peer joins its worker.
    let mut releases = gates
        .iter()
        .map(|gate| Some(gate.release_on_drop()))
        .collect::<Vec<_>>();
    let caller = caller();
    crate::usage_context::ingest(
        &fixture.database,
        caller.uid,
        "project-alpha",
        caller.work.as_ref().unwrap(),
        START + 1,
    )
    .unwrap();
    fixture.service = service(&fixture, peer.source.clone());
    let began = Instant::now();
    let cold = fixture.service.prepare(input(), START + WEEK).unwrap();
    assert!(began.elapsed() < Duration::from_millis(750));
    assert_eq!(cold.usage.totals.total_tokens, None);
    assert_eq!(
        cold.usage
            .coverage
            .unavailable_reasons
            .get("api_refresh_pending"),
        Some(&1)
    );
    let snapshot = cold.usage.coverage.snapshot.unwrap();
    assert!(snapshot.refreshing);
    assert!(!snapshot.refresh_failed);
    entered
        .recv_timeout(Duration::from_secs(5))
        .expect("the owned source request must enter its response gate");
    // A second request joins the same source read instead of poisoning the
    // source with a timeout or starting a duplicate request.
    let pending = fixture.service.prepare(input(), START + WEEK).unwrap();
    assert_eq!(pending.usage.coverage.available_collectors, 0);
    assert!(pending.usage.coverage.snapshot.unwrap().refreshing);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    releases[0].take().unwrap().release();
    assert!(
        fixture
            .service
            .usage
            .wait_for_collector_refresh(Instant::now() + Duration::from_secs(5))
    );
    // Operators may return after the five-second freshness period, including
    // before consuming this completed read for the first time.
    fixture.service.usage.expire_collector_cache();
    let began = Instant::now();
    let warm = fixture
        .service
        .prepare(input(), START + WEEK + 60_000)
        .unwrap();
    assert_eq!(warm.usage.coverage.available_collectors, 1);
    assert_eq!(warm.usage.totals.total_tokens, Some(135));
    assert!(began.elapsed() < Duration::from_millis(500));
    assert_eq!(warm.usage.outcomes.rows.len(), 1);
    assert_eq!(warm.usage.outcomes.rows[0].outcome_id, "spec");
    let snapshot = warm.usage.coverage.snapshot.unwrap();
    assert!(snapshot.refreshing);
    assert!(!snapshot.refresh_failed);
    assert_eq!(snapshot.updated_at_ms, Some(START + WEEK));
    assert_eq!(warm.generated_at_ms, START + WEEK + 60_000);
    assert_eq!(
        warm.usage.coverage.sources[0].updated_at_ms,
        START + WEEK - 1000
    );
    assert_eq!(warm.usage.coverage.sources[0].freshness, "fresh");
    assert_eq!(warm.usage.coverage.events["api_snapshot_stale"], 1);
    assert_eq!(entered.recv_timeout(Duration::from_secs(2)).unwrap(), 1);
    for _ in 0..2 {
        let refreshing = fixture.service.prepare(input(), START + WEEK).unwrap();
        assert_eq!(refreshing.usage.totals.total_tokens, Some(135));
        assert!(refreshing.usage.coverage.snapshot.unwrap().refreshing);
    }
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    for other in [
        Prepare {
            workstream_id: Some("other".into()),
            ..input()
        },
        Prepare {
            window_start_ms: START + 1,
            ..input()
        },
    ] {
        let different = fixture.service.prepare(other, START + WEEK).unwrap();
        assert_eq!(different.usage.coverage.available_collectors, 0);
        assert_eq!(different.usage.totals.total_tokens, None);
        assert_eq!(
            different.usage.coverage.unavailable_reasons["api_source_busy"],
            1
        );
    }
    releases[1].take().unwrap().release();
    assert!(
        fixture
            .service
            .usage
            .wait_for_collector_refresh(Instant::now() + Duration::from_secs(5))
    );
    let refreshed = fixture.service.prepare(input(), START + WEEK).unwrap();
    assert_eq!(refreshed.usage.totals.total_tokens, Some(150));
    assert!(!refreshed.usage.coverage.snapshot.unwrap().refreshing);
    assert!(
        !refreshed
            .usage
            .coverage
            .events
            .contains_key("api_snapshot_stale")
    );

    fixture.service.usage.expire_collector_cache();
    let refreshing = fixture.service.prepare(input(), START + WEEK).unwrap();
    assert_eq!(refreshing.usage.totals.total_tokens, Some(150));
    assert_eq!(entered.recv_timeout(Duration::from_secs(2)).unwrap(), 2);
    releases[2].take().unwrap().release();
    assert!(
        fixture
            .service
            .usage
            .wait_for_collector_refresh(Instant::now() + Duration::from_secs(5))
    );
    let failed = fixture.service.prepare(input(), START + WEEK).unwrap();
    assert_eq!(failed.usage.coverage.available_collectors, 1);
    assert_eq!(failed.usage.totals.total_tokens, Some(150));
    let snapshot = failed.usage.coverage.snapshot.unwrap();
    assert!(!snapshot.refreshing);
    assert!(snapshot.refresh_failed);
    assert_eq!(failed.usage.coverage.events["api_contract_unsupported"], 1);
    let repeated = fixture.service.prepare(input(), START + WEEK).unwrap();
    assert_eq!(repeated.usage.totals.total_tokens, Some(150));
    assert!(repeated.usage.coverage.snapshot.unwrap().refresh_failed);
    assert_eq!(calls.load(Ordering::SeqCst), 3);

    fixture.service.usage.expire_collector_cache();
    let recovering = fixture.service.prepare(input(), START + WEEK).unwrap();
    assert_eq!(recovering.usage.totals.total_tokens, Some(150));
    assert_eq!(entered.recv_timeout(Duration::from_secs(2)).unwrap(), 3);
    releases[3].take().unwrap().release();
    assert!(
        fixture
            .service
            .usage
            .wait_for_collector_refresh(Instant::now() + Duration::from_secs(5))
    );
    let recovered = fixture.service.prepare(input(), START + WEEK).unwrap();
    assert_eq!(recovered.usage.totals.total_tokens, Some(180));
    let snapshot = recovered.usage.coverage.snapshot.unwrap();
    assert!(!snapshot.refreshing);
    assert!(!snapshot.refresh_failed);
    assert_eq!(calls.load(Ordering::SeqCst), 4);
    let request = super::tests::record(&fixture);
    let saved = fixture
        .service
        .record(request, "fixture", START + WEEK)
        .unwrap();
    assert!(saved.completed);
    assert_eq!(saved.record.experiment.disposition, Disposition::Unchanged);

    let other = Prepare {
        workstream_id: Some("other".into()),
        ..input()
    };
    let other = fixture.service.prepare(other, START + WEEK).unwrap();
    assert_eq!(other.usage.totals.total_tokens, None);
    assert_eq!(entered.recv_timeout(Duration::from_secs(2)).unwrap(), 4);
    let other_root = fixture._temporary.path().join("other-repository");
    let other_key = KEY.replace('b', "c");
    fixture.database.transaction(|tx| {
        tx.execute("INSERT INTO repositories(repository_id,root_path,display_name,registered_at,registered_by_uid,last_seen_at) VALUES('project-beta',$1,'Project Beta','1970-01-01T00:16:40Z',1,'1970-01-01T00:16:40Z')", [other_root.to_string_lossy().as_ref()])?;
        tx.execute("INSERT INTO codex_usage_repository_links(source_uid,repository_id,codex_repository_id,source_schema,taxonomy_version,resolved_at) VALUES($1,'project-beta',$2,3,1,'t')", crate::database::params![peer.source.uid,other_key])?;
        Ok(())
    }).unwrap();
    let different = fixture
        .service
        .prepare(
            Prepare {
                repository_id: "project-beta".into(),
                ..input()
            },
            START + WEEK,
        )
        .unwrap();
    assert_eq!(different.usage.totals.total_tokens, None);
    assert_eq!(
        different.usage.coverage.unavailable_reasons["api_source_busy"],
        1
    );

    let mut changed = caller.clone();
    changed
        .work
        .as_mut()
        .unwrap()
        .context
        .as_mut()
        .unwrap()
        .operation_id = Some("new-operation-binding".into());
    crate::usage_context::ingest(
        &fixture.database,
        changed.uid,
        "project-alpha",
        changed.work.as_ref().unwrap(),
        START + 2,
    )
    .unwrap();
    let different = fixture.service.prepare(input(), START + WEEK).unwrap();
    assert_eq!(different.usage.totals.total_tokens, None);
    assert_eq!(
        different.usage.coverage.unavailable_reasons["api_source_busy"],
        1
    );
    assert_eq!(calls.load(Ordering::SeqCst), 5);
    releases[4].take().unwrap().release();
    assert!(
        fixture
            .service
            .usage
            .wait_for_collector_refresh(Instant::now() + Duration::from_secs(5))
    );
}

#[test]
fn source_rpc_failure_retains_measured_review_clears_loading_and_recovers() {
    use crate::usage_api_fixture::ResponseGate;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    let mut fixture = Fixture::new();
    let root = fixture._temporary.path();
    let private = "never-return-private /private/accounting secret-token hostile instructions";
    let gates = [ResponseGate::default(), ResponseGate::default()];
    let response_gates = gates.clone();
    let calls = Arc::new(AtomicUsize::new(0));
    let observed = calls.clone();
    let peer = Peer::start_rpc(root, &root.join("source"), move |params| {
        match observed.fetch_add(1, Ordering::SeqCst) {
            0 => json!({"id":2,"result":response(params, START + WEEK, 135)}),
            1 => {
                response_gates[0].wait();
                json!({"id":2,"error":{"code":-32603,"message":private,"data":{"raw":private}}})
            }
            2 => {
                response_gates[1].wait();
                json!({"id":2,"result":response(params, START + WEEK, 180)})
            }
            _ => panic!("an exact cached read must not start a duplicate source request"),
        }
    });
    let releases = gates
        .iter()
        .map(ResponseGate::release_on_drop)
        .collect::<Vec<_>>();
    fixture.service = service(&fixture, peer.source.clone());
    fixture.service.prepare(input(), START + WEEK).unwrap();
    assert!(
        fixture
            .service
            .usage
            .wait_for_collector_refresh(Instant::now() + Duration::from_secs(2))
    );
    let initial = fixture.service.prepare(input(), START + WEEK).unwrap();
    assert_eq!(initial.usage.totals.total_tokens, Some(135));

    fixture.service.usage.expire_collector_cache();
    let refreshing = fixture.service.prepare(input(), START + WEEK).unwrap();
    assert_eq!(refreshing.usage.totals.total_tokens, Some(135));
    assert!(refreshing.usage.coverage.loading.is_some());
    let mut releases = releases.into_iter();
    releases.next().unwrap().release();
    assert!(
        fixture
            .service
            .usage
            .wait_for_collector_refresh(Instant::now() + Duration::from_secs(2))
    );
    let failed = fixture.service.prepare(input(), START + WEEK).unwrap();
    assert_eq!(failed.usage.totals.total_tokens, Some(135));
    assert_eq!(failed.usage.coverage.events["api_source_unavailable"], 1);
    assert!(
        !failed
            .usage
            .coverage
            .events
            .contains_key("api_contract_unsupported")
    );
    assert_eq!(failed.usage.coverage.loading, None);
    let snapshot = failed.usage.coverage.snapshot.as_ref().unwrap();
    assert!(snapshot.refresh_failed);
    assert!(!snapshot.refreshing);
    assert_eq!(snapshot.updated_at_ms, Some(START + WEEK));
    let repeated = fixture.service.prepare(input(), START + WEEK).unwrap();
    assert_eq!(repeated.usage.totals.total_tokens, Some(135));
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    for packet in [&failed, &repeated] {
        let wire = serde_json::to_string(packet).unwrap();
        for forbidden in [
            private,
            "/private/accounting",
            "secret-token",
            "hostile instructions",
        ] {
            assert!(!wire.contains(forbidden));
        }
    }

    fixture.service.usage.expire_collector_cache();
    let recovering = fixture.service.prepare(input(), START + WEEK).unwrap();
    assert_eq!(recovering.usage.totals.total_tokens, Some(135));
    assert!(recovering.usage.coverage.loading.is_some());
    releases.next().unwrap().release();
    assert!(
        fixture
            .service
            .usage
            .wait_for_collector_refresh(Instant::now() + Duration::from_secs(2))
    );
    let recovered = fixture.service.prepare(input(), START + WEEK).unwrap();
    assert_eq!(recovered.usage.totals.total_tokens, Some(180));
    assert_eq!(recovered.usage.coverage.loading, None);
    assert!(
        !recovered
            .usage
            .coverage
            .snapshot
            .as_ref()
            .unwrap()
            .refresh_failed
    );
    assert_eq!(calls.load(Ordering::SeqCst), 3);
    for request in peer
        .requests
        .lock()
        .unwrap()
        .iter()
        .filter(|request| request["method"] == "localUsage/summary")
    {
        assert_eq!(request["params"]["repositoryKey"], KEY);
        assert_eq!(request["params"]["fromAt"], START);
        assert_eq!(request["params"]["toAt"], START + WEEK);
        assert_eq!(request["params"]["includeReview"], true);
    }
}
