use devcoordinator2_executor_protocol::{ExecutionPlan, ValidationTier};

#[test]
fn backend_schema_two_golden_plan_is_accepted() {
    let payload = include_bytes!("fixtures/backend-plan.json");
    let plan = ExecutionPlan::from_json(payload).expect("backend fixture must remain compatible");
    assert_eq!(plan.requested_tier, ValidationTier::Release);
    assert_eq!(plan.checks.len(), 3);
    assert_eq!(plan.checks[0].invalidates, ["unit"]);
    assert_eq!(plan.checks[1].requires, ["source-preflight"]);
    assert_eq!(plan.checks[0].timeout_seconds, None);
}
