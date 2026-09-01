use std::env;
use std::fs;
use std::path::Path;

use devcoordinator2_executor_protocol::ExecutionPlan;

fn load_plan(path: &Path) -> Result<ExecutionPlan, String> {
    let input = fs::read(path).map_err(|error| format!("cannot read plan: {error}"))?;
    if path.extension().and_then(|value| value.to_str()) == Some("toml") {
        let text = std::str::from_utf8(&input)
            .map_err(|error| format!("executor TOML is not UTF-8: {error}"))?;
        ExecutionPlan::from_toml(text).map_err(|error| error.to_string())
    } else {
        ExecutionPlan::from_json(&input).map_err(|error| error.to_string())
    }
}

fn main() {
    let mut args = env::args_os();
    let program = args
        .next()
        .and_then(|value| Path::new(&value).file_name().map(|name| name.to_owned()))
        .unwrap_or_else(|| "devcoordinator2-executor".into());
    let Some(command) = args.next() else {
        eprintln!(
            "usage: {} validate PLAN.json",
            Path::new(&program).display()
        );
        std::process::exit(2);
    };
    let Some(plan_path) = args.next() else {
        eprintln!(
            "usage: {} validate PLAN.json",
            Path::new(&program).display()
        );
        std::process::exit(2);
    };
    if command != "validate" || args.next().is_some() {
        eprintln!(
            "usage: {} validate PLAN.json",
            Path::new(&program).display()
        );
        std::process::exit(2);
    }
    match load_plan(Path::new(&plan_path)) {
        Ok(plan) => println!(
            "validated schema 2 plan for {} ({} declared checks)",
            plan.test,
            plan.checks.len()
        ),
        Err(error) => {
            eprintln!("executor plan rejected: {error}");
            std::process::exit(2);
        }
    }
}
