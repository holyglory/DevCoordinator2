//! Test-only process leaf for executor invalidation and all-settled checks.

fn main() {
    let arguments = std::env::args_os().skip(1).collect::<Vec<_>>();
    match arguments.first().and_then(|value| value.to_str()) {
        Some("pass") if arguments.len() == 1 => {}
        Some("fail") if arguments.len() == 1 => std::process::exit(7),
        Some("write") if arguments.len() == 3 => {
            if let Err(error) =
                std::fs::write(&arguments[1], arguments[2].to_string_lossy().as_bytes())
            {
                eprintln!("fixture write failed: {error}");
                std::process::exit(2);
            }
        }
        _ => {
            eprintln!("fixture supports pass, fail, or write PATH VALUE");
            std::process::exit(2);
        }
    }
}
