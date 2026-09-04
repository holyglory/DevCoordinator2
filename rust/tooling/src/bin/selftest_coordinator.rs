//! Test-only deterministic deployment discovery fixture.

fn main() {
    let all_arguments = std::env::args().collect::<Vec<_>>();
    let legacy = all_arguments
        .first()
        .is_some_and(|value| value.contains("protocol-v1"));
    let arguments = all_arguments.into_iter().skip(1).collect::<Vec<_>>();
    if arguments.len() < 2 || arguments[0] != "deployment" || arguments[1] != "list" {
        eprintln!("fixture supports only deployment list");
        std::process::exit(2);
    }
    let deployments = serde_json::json!({"deployments":[{
        "name":"fixture",
        "repository_name":"fixture",
        "route_port":9,
        "state":"running"
    }]});
    if legacy {
        println!(
            "{}",
            serde_json::json!({
                "protocol":1,"id":"formal-ui-self-test","ok":true,"result":deployments
            })
        );
    } else {
        println!(
            "{}",
            serde_json::json!({
                "protocol":2,
                "id":"formal-ui-self-test",
                "ok":true,
                "data":deployments
            })
        );
    }
}
