//! Test-only process fixture for executor integration tests.

use std::env;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::os::fd::FromRawFd;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::thread;
use std::time::Duration;

use serde_json::json;

fn argument(index: usize, label: &str) -> Result<String, String> {
    env::args()
        .nth(index)
        .ok_or_else(|| format!("missing {label}"))
}

fn environment(name: &str) -> Result<String, String> {
    env::var(name).map_err(|_| format!("missing {name}"))
}

fn descriptor(name: &str) -> Result<File, String> {
    let raw = environment(name)?
        .parse::<i32>()
        .map_err(|_| format!("invalid {name}"))?;
    // SAFETY: the executor intentionally passes this owned descriptor to the
    // fixture process. Each action opens it once and exits after the write.
    Ok(unsafe { File::from_raw_fd(raw) })
}

fn write_descriptor(name: &str, payload: &[u8]) -> Result<(), String> {
    descriptor(name)?
        .write_all(payload)
        .map_err(|error| format!("cannot write {name}: {error}"))
}

fn sleep_seconds(seconds: u64) {
    thread::sleep(Duration::from_secs(seconds));
}

fn diagnostics_path(name: &str) -> Result<PathBuf, String> {
    Ok(PathBuf::from(environment("DEVCOORDINATOR_DIAGNOSTICS_DIR")?).join(name))
}

fn write_valid_junit(path: &Path) -> Result<(), String> {
    fs::write(
        path,
        "<testsuite><testcase name=\"rejects\" classname=\"Parser\"><failure file=\"src/parser.rs\" line=\"17\" column=\"3\" expected=\"ready\" actual=\"pending\"/></testcase></testsuite>",
    )
    .map_err(|error| error.to_string())
}

fn run() -> Result<i32, String> {
    let action = argument(1, "action")?;
    match action.as_str() {
        "exit" => argument(2, "exit code")?
            .parse::<i32>()
            .map_err(|_| "invalid exit code".to_owned()),
        "case-status" => {
            let value = argument(2, "case value")?;
            println!("{value}");
            Ok(i32::from(value == "bad"))
        }
        "manifest" => {
            let kind = argument(2, "manifest kind")?;
            let payload: Option<Vec<u8>> = match kind.as_str() {
                "oversized" => Some(vec![b'x'; 2 * 1024 * 1024 + 1]),
                "one-noise" => {
                    print!("ordinary discovery noise");
                    io::stdout().flush().map_err(|error| error.to_string())?;
                    Some(br#"{"schema":2,"cases":[{"id":"one","args":[]}]}"#.to_vec())
                }
                "one-discovery" => {
                    print!("discovery");
                    io::stdout().flush().map_err(|error| error.to_string())?;
                    Some(br#"{"schema":2,"cases":[{"id":"one","args":[]}]}"#.to_vec())
                }
                "missing" => None,
                "invalid" => Some(b"not-json".to_vec()),
                "trailing" => Some(br#"{"schema":2,"cases":[]} {"schema":2,"cases":[]}"#.to_vec()),
                _ => return Err(format!("unknown manifest kind: {kind}")),
            };
            if let Some(payload) = payload {
                write_descriptor("DEVCOORDINATOR_CASE_MANIFEST_FD", &payload)?;
            }
            Ok(0)
        }
        "invalid-manifest-with-child" => {
            let child = Command::new(env::current_exe().map_err(|error| error.to_string())?)
                .args(["sleep", "30"])
                .spawn()
                .map_err(|error| format!("cannot spawn child fixture: {error}"))?;
            let scratch = PathBuf::from(environment("DEVCOORDINATOR_CHECK_SCRATCH")?);
            fs::write(scratch.join("child.pid"), child.id().to_string())
                .map_err(|error| error.to_string())?;
            write_descriptor("DEVCOORDINATOR_CASE_MANIFEST_FD", b"not-json")?;
            Ok(0)
        }
        "sleep" => {
            let seconds = argument(2, "sleep seconds")?
                .parse::<u64>()
                .map_err(|_| "invalid sleep seconds".to_owned())?;
            sleep_seconds(seconds);
            Ok(0)
        }
        "sleep-ignore-term" => {
            // SAFETY: setting SIGTERM to SIG_IGN is process-local and is the
            // exact behavior this disposable fixture exists to exercise.
            unsafe {
                libc::signal(libc::SIGTERM, libc::SIG_IGN);
            }
            sleep_seconds(30);
            Ok(0)
        }
        "event" => {
            let identity = argument(2, "event identity")?;
            let run_id = environment("DEVCOORDINATOR_RUN_ID")?;
            let actual_check = environment("DEVCOORDINATOR_CHECK_NAME")?;
            let check = if identity == "wrong" {
                "wrong"
            } else {
                actual_check.as_str()
            };
            let mut payload = serde_json::to_vec(&json!({
                "schema": 2,
                "run_id": run_id,
                "check": check,
                "status": "passed",
                "reason": null,
            }))
            .map_err(|error| error.to_string())?;
            payload.push(b'\n');
            write_descriptor("DEVCOORDINATOR_EVENT_FD", &payload)?;
            sleep_seconds(30);
            Ok(0)
        }
        "repeat-stdout" => {
            let count = argument(2, "byte count")?
                .parse::<usize>()
                .map_err(|_| "invalid byte count".to_owned())?;
            let chunk = vec![b'x'; 64 * 1024];
            let mut remaining = count;
            let mut output = io::stdout().lock();
            while remaining > 0 {
                let current = remaining.min(chunk.len());
                output
                    .write_all(&chunk[..current])
                    .map_err(|error| error.to_string())?;
                remaining -= current;
            }
            Ok(0)
        }
        "streams" => {
            println!("{}", argument(2, "stdout text")?);
            eprintln!("{}", argument(3, "stderr text")?);
            Ok(0)
        }
        "stderr-exit" => {
            eprintln!("{}", argument(2, "stderr text")?);
            argument(3, "exit code")?
                .parse::<i32>()
                .map_err(|_| "invalid exit code".to_owned())
        }
        "large-output-sentinel" => {
            let count = argument(2, "byte count")?
                .parse::<usize>()
                .map_err(|_| "invalid byte count".to_owned())?;
            let mut output = io::stdout().lock();
            output
                .write_all(&vec![b'x'; count])
                .and_then(|()| output.write_all(b"\nFINAL-SENTINEL\n"))
                .map_err(|error| error.to_string())?;
            Ok(0)
        }
        "write-relative" => {
            fs::write(argument(2, "relative path")?, argument(3, "content")?)
                .map_err(|error| error.to_string())?;
            Ok(0)
        }
        "write-scratch" => {
            let path = PathBuf::from(environment("DEVCOORDINATOR_CHECK_SCRATCH")?)
                .join(argument(2, "scratch filename")?);
            fs::write(path, argument(3, "content")?).map_err(|error| error.to_string())?;
            Ok(0)
        }
        "kill-self" => {
            // SAFETY: the test intentionally terminates only this disposable
            // fixture process to exercise signal reporting.
            unsafe {
                libc::kill(libc::getpid(), libc::SIGKILL);
            }
            Ok(2)
        }
        "break-log-storage" => {
            let diagnostics = PathBuf::from(environment("DEVCOORDINATOR_DIAGNOSTICS_DIR")?);
            let leaf = diagnostics
                .parent()
                .ok_or_else(|| "diagnostics directory has no leaf parent".to_owned())?;
            print!("stored-before-failure");
            io::stdout().flush().map_err(|error| error.to_string())?;
            fs::set_permissions(leaf, fs::Permissions::from_mode(0o500))
                .map_err(|error| error.to_string())?;
            // SAFETY: these are this disposable process's standard streams.
            unsafe {
                libc::close(libc::STDOUT_FILENO);
                libc::close(libc::STDERR_FILENO);
            }
            sleep_seconds(30);
            Ok(0)
        }
        "write-evidence" => {
            let root = PathBuf::from(environment("DEVCOORDINATOR_EVIDENCE_DIR")?);
            fs::create_dir_all(&root).map_err(|error| error.to_string())?;
            fs::write(root.join("journey-evidence.json"), "{\"kind\":\"fixture\"}")
                .map_err(|error| error.to_string())?;
            Ok(0)
        }
        "write-artifact-tree" => {
            let root = PathBuf::from("browser-evidence");
            fs::create_dir_all(root.join("nested")).map_err(|error| error.to_string())?;
            fs::write(root.join("result.json"), "{\"ok\":true}\n")
                .map_err(|error| error.to_string())?;
            fs::write(root.join("nested/capture.png"), b"png")
                .map_err(|error| error.to_string())?;
            Ok(0)
        }
        "print" => {
            println!("{}", argument(2, "text")?);
            Ok(0)
        }
        "diagnostic-event" => {
            let mut payload = serde_json::to_vec(&json!({
                "schema": 2,
                "run_id": environment("DEVCOORDINATOR_RUN_ID")?,
                "check": environment("DEVCOORDINATOR_CHECK_NAME")?,
                "case": null,
                "status": "failed",
                "exit": {"code": 7, "signal": null},
                "termination_reason": null,
                "source": {"file": "src/parser.rs", "line": 81, "column": 9},
                "error_category": "assertion",
                "expected": null,
                "actual": null,
                "log_refs": [],
            }))
            .map_err(|error| error.to_string())?;
            payload.push(b'\n');
            write_descriptor("DEVCOORDINATOR_DIAGNOSTIC_FD", &payload)?;
            Ok(0)
        }
        "junit" => {
            let kind = argument(2, "JUnit kind")?;
            let path = diagnostics_path("junit.xml")?;
            match kind.as_str() {
                "valid" => write_valid_junit(&path)?,
                "oversized" => {
                    OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .open(path)
                        .and_then(|file| file.set_len(16 * 1024 * 1024 + 1))
                        .map_err(|error| error.to_string())?;
                }
                "symlink" => {
                    fs::write(&path, "<testsuite/>").map_err(|error| error.to_string())?;
                    fs::remove_file(&path).map_err(|error| error.to_string())?;
                    symlink("/etc/passwd", path).map_err(|error| error.to_string())?;
                }
                "fifo" => {
                    let encoded = std::ffi::CString::new(path.as_os_str().as_encoded_bytes())
                        .map_err(|_| "FIFO path contains NUL".to_owned())?;
                    // SAFETY: the path is a checked local diagnostics path and
                    // the fixture does not open the FIFO.
                    if unsafe { libc::mkfifo(encoded.as_ptr(), 0o600) } != 0 {
                        return Err(io::Error::last_os_error().to_string());
                    }
                }
                "log-case" => {
                    fs::write(
                        path,
                        "<testsuite name=\"parser\">\n  <testcase classname=\"Parser\" name=\"rejects-final-state\" file=\"src/parser.rs\" line=\"37\" column=\"5\">\n    <failure expected=\"ready\" actual=\"pending\">PRIVATE-JUNIT-PROSE</failure>\n    <failure expected=\"ready\" actual=\"pending\">PRIVATE-JUNIT-PROSE</failure>\n  </testcase>\n</testsuite>",
                    )
                    .map_err(|error| error.to_string())?;
                    println!("structured case output");
                }
                _ => return Err(format!("unknown JUnit kind: {kind}")),
            }
            Ok(0)
        }
        _ => Err(format!("unknown fixture action: {action}")),
    }
}

fn main() -> ExitCode {
    match run() {
        Ok(code) => ExitCode::from(u8::try_from(code).unwrap_or(2)),
        Err(error) => {
            eprintln!("{error}");
            ExitCode::from(2)
        }
    }
}
