//! Bounded deployment HTTP and TCP readiness probes.

use std::net::{TcpStream, ToSocketAddrs};
use std::thread;
use std::time::{Duration, Instant};

const POLL: Duration = Duration::from_millis(100);
const HTTP_ATTEMPT: Duration = Duration::from_secs(5);
const TCP_ATTEMPT: Duration = Duration::from_secs(3);

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Readiness {
    pub ready: bool,
    pub note: String,
}

impl Readiness {
    pub fn ready(note: impl Into<String>) -> Self {
        Self {
            ready: true,
            note: note.into(),
        }
    }

    pub fn failed(note: impl Into<String>) -> Self {
        Self {
            ready: false,
            note: note.into(),
        }
    }
}

pub trait DeploymentHealth: Send + Sync + 'static {
    fn http_ready(
        &self,
        port: u16,
        path: &str,
        timeout: Duration,
        terminal: &(dyn Fn() -> Option<String> + Sync),
    ) -> Readiness;

    fn tcp_ready(
        &self,
        host: &str,
        port: u16,
        timeout: Duration,
        terminal: &(dyn Fn() -> Option<String> + Sync),
    ) -> Readiness;

    fn tcp_probe(&self, host: &str, port: u16) -> bool {
        self.tcp_ready(host, port, Duration::from_secs(3), &|| None)
            .ready
    }
}

#[derive(Clone)]
pub struct HostDeploymentHealth {
    client: reqwest::blocking::Client,
}

impl Default for HostDeploymentHealth {
    fn default() -> Self {
        let client = reqwest::blocking::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("static deployment HTTP client configuration is valid");
        Self { client }
    }
}

impl DeploymentHealth for HostDeploymentHealth {
    fn http_ready(
        &self,
        port: u16,
        path: &str,
        timeout: Duration,
        terminal: &(dyn Fn() -> Option<String> + Sync),
    ) -> Readiness {
        if !valid_http_path(path) || timeout.is_zero() {
            return Readiness::failed("invalid HTTP readiness request");
        }
        let url = format!("http://127.0.0.1:{port}{path}");
        let deadline = Instant::now() + timeout;
        let mut last = "no response".to_owned();
        loop {
            if let Some(reason) = terminal() {
                return Readiness::failed(reason);
            }
            let now = Instant::now();
            if now >= deadline {
                return Readiness::failed(last);
            }
            let attempt = HTTP_ATTEMPT.min(deadline.saturating_duration_since(now));
            match self.client.get(&url).timeout(attempt).send() {
                Ok(response) if (200..400).contains(&response.status().as_u16()) => {
                    return Readiness::ready(format!("http {}", response.status().as_u16()));
                }
                Ok(response) => last = format!("http {}", response.status().as_u16()),
                Err(_) => last = "unreachable".into(),
            }
            sleep_until_next(deadline);
        }
    }

    fn tcp_ready(
        &self,
        host: &str,
        port: u16,
        timeout: Duration,
        terminal: &(dyn Fn() -> Option<String> + Sync),
    ) -> Readiness {
        if host.is_empty() || timeout.is_zero() {
            return Readiness::failed("invalid TCP readiness request");
        }
        let addresses = match (host, port).to_socket_addrs() {
            Ok(addresses) => addresses.collect::<Vec<_>>(),
            Err(_) => return Readiness::failed("TCP target could not be resolved"),
        };
        if addresses.is_empty() {
            return Readiness::failed("TCP target could not be resolved");
        }
        let deadline = Instant::now() + timeout;
        let mut last = "connection refused".to_owned();
        loop {
            if let Some(reason) = terminal() {
                return Readiness::failed(reason);
            }
            let now = Instant::now();
            if now >= deadline {
                return Readiness::failed(last);
            }
            let attempt = TCP_ATTEMPT.min(deadline.saturating_duration_since(now));
            if addresses
                .iter()
                .any(|address| TcpStream::connect_timeout(address, attempt).is_ok())
            {
                return Readiness::ready("tcp open");
            }
            last = "tcp closed".into();
            sleep_until_next(deadline);
        }
    }
}

fn sleep_until_next(deadline: Instant) {
    let remaining = deadline.saturating_duration_since(Instant::now());
    if !remaining.is_zero() {
        thread::sleep(POLL.min(remaining));
    }
}

fn valid_http_path(path: &str) -> bool {
    path.starts_with('/')
        && path.len() <= 2_048
        && !path
            .bytes()
            .any(|byte| byte.is_ascii_control() || byte == b' ')
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    #[test]
    fn tcp_and_http_probes_report_real_local_outcomes_without_bodies() {
        let tcp = match TcpListener::bind(("127.0.0.1", 0)) {
            Ok(listener) => listener,
            Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => return,
            Err(error) => panic!("bind TCP fixture: {error}"),
        };
        let tcp_port = tcp.local_addr().unwrap().port();
        assert!(HostDeploymentHealth::default().tcp_probe("127.0.0.1", tcp_port));
        drop(tcp);

        let http = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let http_port = http.local_addr().unwrap().port();
        let server = thread::spawn(move || {
            let (mut connection, _) = http.accept().unwrap();
            let mut request = [0_u8; 1_024];
            let _ = connection.read(&mut request);
            connection
                .write_all(b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\n\r\n")
                .unwrap();
        });
        let result = HostDeploymentHealth::default().http_ready(
            http_port,
            "/ready",
            Duration::from_secs(1),
            &|| None,
        );
        server.join().unwrap();
        assert_eq!(result, Readiness::ready("http 204"));
    }

    #[test]
    fn terminal_probe_aborts_before_network_work() {
        let called = Arc::new(AtomicBool::new(false));
        let observed = Arc::clone(&called);
        let abort = move || {
            observed.store(true, Ordering::SeqCst);
            Some("runtime stopped".to_owned())
        };
        let result = HostDeploymentHealth::default().tcp_ready(
            "127.0.0.1",
            9,
            Duration::from_secs(30),
            &abort,
        );
        assert!(called.load(Ordering::SeqCst));
        assert_eq!(result, Readiness::failed("runtime stopped"));
    }

    #[test]
    fn malformed_http_path_fails_without_a_request() {
        let result = HostDeploymentHealth::default().http_ready(
            80,
            "relative",
            Duration::from_secs(1),
            &|| None,
        );
        assert_eq!(result, Readiness::failed("invalid HTTP readiness request"));
    }
}
