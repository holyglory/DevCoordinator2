//! These tests drive the real reqwest HTTP and response parsing boundary. They
//! are fixture evidence, not evidence of access to a live GitHub repository.
//! No existing GitHub journey fixture existed to extend. Credential/ref syntax
//! assertions stay here because they support the same HTTP admission boundary.

use super::*;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::sync::Mutex;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

const TOKEN: &str = "fixture-auth-access";
const FEATURE: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const PARENT: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const MERGE: &str = "cccccccccccccccccccccccccccccccccccccccc";
const TARGET: &str = "dddddddddddddddddddddddddddddddddddddddd";
const CHANGED: &str = "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee";
const BRANCH: &str = "feature/demo";

struct Step {
    method: &'static str,
    target: String,
    status: u16,
    headers: Vec<(String, String)>,
    body: String,
    echo_mutation: bool,
    declared_length: Option<usize>,
    chunked: bool,
    delay: Duration,
    expected_token: &'static str,
}

impl Step {
    fn get(target: impl Into<String>, value: Value) -> Self {
        Self {
            method: "GET",
            target: target.into(),
            status: 200,
            headers: vec![],
            body: value.to_string(),
            echo_mutation: false,
            declared_length: None,
            chunked: false,
            delay: Duration::ZERO,
            expected_token: TOKEN,
        }
    }

    fn status(mut self, status: u16) -> Self {
        self.status = status;
        self
    }
    fn header(mut self, key: &str, value: impl Into<String>) -> Self {
        self.headers.push((key.into(), value.into()));
        self
    }
    fn body(mut self, value: &str) -> Self {
        self.body = value.into();
        self
    }

    fn mutation(value: Option<Value>) -> Self {
        Self {
            method: "POST",
            target: "/graphql".into(),
            status: 200,
            headers: vec![],
            body: value.as_ref().map(Value::to_string).unwrap_or_default(),
            echo_mutation: value.is_none(),
            declared_length: None,
            chunked: false,
            delay: Duration::ZERO,
            expected_token: TOKEN,
        }
    }
}

#[derive(Clone, Debug)]
struct Captured {
    method: String,
    target: String,
    authorized: bool,
    pinned_version: bool,
    body: Option<Value>,
}

struct Fixture {
    _private: tempfile::TempDir,
    token_file: PathBuf,
    address: std::net::SocketAddr,
    captured: Arc<Mutex<Vec<Captured>>>,
    server: Option<tokio::task::JoinHandle<()>>,
}

impl Fixture {
    async fn start(steps: Vec<Step>) -> Self {
        let private = tempfile::tempdir().unwrap();
        let token_file = private.path().join("github.token");
        std::fs::write(&token_file, format!("{TOKEN}\n")).unwrap();
        std::fs::set_permissions(&token_file, std::fs::Permissions::from_mode(0o600)).unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let origin = format!("http://{address}");
        let captured = Arc::new(Mutex::new(Vec::new()));
        let records = captured.clone();
        let server = tokio::spawn(async move {
            for step in steps {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut bytes = Vec::new();
                let header_end = loop {
                    let mut buffer = [0_u8; 4096];
                    let count = stream.read(&mut buffer).await.unwrap();
                    assert!(count > 0, "fixture request ended before HTTP headers");
                    bytes.extend_from_slice(&buffer[..count]);
                    assert!(bytes.len() < 64 * 1024, "fixture request exceeds bound");
                    if let Some(offset) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
                        break offset + 4;
                    }
                };
                let headers = std::str::from_utf8(&bytes[..header_end])
                    .unwrap()
                    .to_owned();
                let request_line = headers.lines().next().unwrap();
                let mut request_line = request_line.split_whitespace();
                let method = request_line.next().unwrap().to_owned();
                let target = request_line.next().unwrap().to_owned();
                let fields = headers
                    .lines()
                    .skip(1)
                    .filter_map(|line| line.split_once(':'))
                    .map(|(key, value)| (key.to_ascii_lowercase(), value.trim().to_owned()))
                    .collect::<std::collections::HashMap<_, _>>();
                let content_length = fields
                    .get("content-length")
                    .map(|length| length.parse::<usize>().unwrap())
                    .unwrap_or_default();
                while bytes.len() < header_end + content_length {
                    let mut buffer = [0_u8; 4096];
                    let count = stream.read(&mut buffer).await.unwrap();
                    assert!(count > 0, "fixture request body truncated");
                    bytes.extend_from_slice(&buffer[..count]);
                }
                let body: Option<Value> = if content_length == 0 {
                    None
                } else {
                    Some(
                        serde_json::from_slice(&bytes[header_end..header_end + content_length])
                            .unwrap(),
                    )
                };
                let authorization = fields
                    .get("authorization")
                    .is_some_and(|value| value == &format!("Bearer {}", step.expected_token));
                records.lock().unwrap().push(Captured {
                    method: method.clone(),
                    target: target.clone(),
                    authorized: authorization,
                    pinned_version: fields
                        .get("x-github-api-version")
                        .is_some_and(|value| value == GITHUB_API_VERSION),
                    body: body.clone(),
                });
                assert_eq!(method, step.method, "HTTP method");
                assert_eq!(target, step.target, "exact repository/ref/page request");
                assert!(authorization, "fixture expected a private bearer header");
                let response_body = if step.echo_mutation {
                    json!({"data":{"updateRefs":{"clientMutationId":body.as_ref().unwrap().pointer("/variables/input/clientMutationId").unwrap()}}}).to_string()
                } else {
                    step.body
                };
                let declared_length = step.declared_length.unwrap_or(response_body.len());
                let mut response = format!(
                    "HTTP/1.1 {} Fixture\r\nContent-Type: application/json\r\nConnection: close\r\n",
                    step.status
                );
                if step.chunked {
                    response.push_str("Transfer-Encoding: chunked\r\n");
                } else {
                    response.push_str(&format!("Content-Length: {declared_length}\r\n"));
                }
                for (key, value) in step.headers {
                    response.push_str(&format!(
                        "{key}: {}\r\n",
                        value.replace("{origin}", &origin)
                    ));
                }
                response.push_str("\r\n");
                tokio::time::sleep(step.delay).await;
                if stream.write_all(response.as_bytes()).await.is_ok() {
                    if step.chunked {
                        let _ = stream
                            .write_all(format!("{:x}\r\n", response_body.len()).as_bytes())
                            .await;
                        let _ = stream.write_all(response_body.as_bytes()).await;
                        let _ = stream.write_all(b"\r\n0\r\n\r\n").await;
                    } else {
                        let _ = stream.write_all(response_body.as_bytes()).await;
                    }
                }
                let _ = stream.shutdown().await;
            }
        });
        Self {
            _private: private,
            token_file,
            address,
            captured,
            server: Some(server),
        }
    }

    fn client(&self) -> GithubClient {
        GithubClient::fixture(self.token_file.clone(), self.address).unwrap()
    }

    async fn finish(mut self) -> Vec<Captured> {
        let task = self.server.as_mut().unwrap();
        tokio::time::timeout(Duration::from_secs(3), task)
            .await
            .expect("HTTP fixture still had unconsumed requests")
            .expect("HTTP fixture failed");
        self.server.take();
        self.captured.lock().unwrap().clone()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if let Some(server) = self.server.take() {
            server.abort();
        }
    }
}

fn repository() -> GitHubRepository {
    GitHubRepository::new("fixture-owner", "fixture-project").unwrap()
}
fn root() -> &'static str {
    "/repos/fixture-owner/fixture-project"
}
fn repo_json() -> Value {
    json!({"owner":{"login":"fixture-owner"},"name":"fixture-project","default_branch":"main","node_id":"R_fixture","html_url":"https://userinfo:fixture@example.invalid/?token=upstream-value"})
}
fn branch_json(name: &str, sha: &str) -> Value {
    json!({"name":name,"commit":{"sha":sha},"protected":false})
}
fn ref_json(name: &str, sha: &str) -> Value {
    json!({"ref":format!("refs/heads/{name}"),"object":{"type":"commit","sha":sha}})
}
fn pull_json() -> Value {
    json!({"number":12,"state":"closed","merged":true,"merged_at":"2026-11-17T10:00:00Z","merge_commit_sha":MERGE,"head":{"ref":BRANCH,"sha":FEATURE,"repo":{"owner":{"login":"fixture-owner"},"name":"fixture-project"}},"base":{"ref":"main","sha":TARGET,"repo":{"owner":{"login":"fixture-owner"},"name":"fixture-project"}}})
}
fn compare_json() -> Value {
    json!({"status":"ahead","ahead_by":3,"behind_by":0,"base_commit":{"sha":MERGE},"merge_base_commit":{"sha":MERGE}})
}
fn commit_json(sha: &str, parents: &[&str]) -> Value {
    json!({"sha":sha,"parents":parents.iter().map(|sha|json!({"sha":sha})).collect::<Vec<_>>(),"commit":{"message":"Support Project Explorer\n\nPrivate commit body is not a subject.","author":{"name":"Fixture Author","date":"2026-11-17T09:00:00Z","email":"private-address@example.invalid"}},"author":{"login":"fixture-author","email":"private-address@example.invalid"},"html_url":"https://other.invalid/?access_token=untrusted"})
}
fn proof_steps() -> Vec<Step> {
    vec![
        Step::get(root(), repo_json()),
        Step::get(
            format!("{}/branches/feature%2Fdemo", root()),
            branch_json(BRANCH, FEATURE),
        ),
        Step::get(format!("{}/pulls/12", root()), pull_json()),
        Step::get(
            format!("{}/branches/main", root()),
            branch_json("main", TARGET),
        ),
        Step::get(
            format!("{}/compare/{MERGE}...{TARGET}?per_page=1&page=1", root()),
            compare_json(),
        ),
        Step::get(
            format!("{}/git/ref/heads%2Ffeature%2Fdemo", root()),
            ref_json(BRANCH, FEATURE),
        ),
        Step::get(
            format!("{}/git/ref/heads%2Fmain", root()),
            ref_json("main", TARGET),
        ),
    ]
}

#[tokio::test]
async fn http_journey_reads_remote_pages_parents_and_atomically_retires_squash_head() {
    let mut steps = vec![
        Step::get(root(), repo_json()),
        Step::get(
            format!("{}/branches?per_page=2&page=1", root()),
            json!([branch_json("main", TARGET), branch_json(BRANCH, FEATURE)]),
        )
        .header(
            "Link",
            format!(
                "<{{origin}}{}/branches?per_page=2&page=2>; rel=\"next\"",
                root()
            ),
        ),
        Step::get(
            format!("{}/branches?per_page=2&page=2", root()),
            json!([branch_json("release", PARENT)]),
        ),
        Step::get(
            format!("{}/commits?per_page=3&page=1&sha={FEATURE}", root()),
            json!([
                commit_json(FEATURE, &[PARENT, TARGET]),
                commit_json(PARENT, &[])
            ]),
        ),
        Step::get(
            format!(
                "{}/pulls?per_page=2&page=1&state=all&head=fixture-owner%3Afeature%2Fdemo&sort=updated&direction=desc",
                root()
            ),
            json!([pull_json()]),
        ),
    ];
    steps.extend(proof_steps());
    steps.extend([
        Step::mutation(None),
        Step::get(root(), repo_json()),
        Step::get(
            format!("{}/git/ref/heads%2Fmain", root()),
            ref_json("main", TARGET),
        ),
        Step::get(
            format!("{}/git/ref/heads%2Ffeature%2Fdemo", root()),
            json!({"message":"Not Found"}),
        )
        .status(404),
    ]);
    let fixture = Fixture::start(steps).await;
    let client = fixture.client();
    let repository = repository();
    let info = client.repository(&repository).await.unwrap();
    assert_eq!(info.html_url, repository.html_url());
    let first = client.list_branches(&repository, None, 2).await.unwrap();
    assert!(!first.complete);
    let last = client
        .list_branches(&repository, first.next_cursor.as_deref(), 2)
        .await
        .unwrap();
    assert!(last.complete);
    assert_eq!(last.items[0].name, "release");
    let commits = client.commits(&repository, FEATURE, None, 3).await.unwrap();
    assert_eq!(commits.items[0].parent_shas, vec![PARENT, TARGET]);
    assert!(commits.items[1].parent_shas.is_empty());
    assert_eq!(commits.items[0].subject, "Support Project Explorer");
    let projection = serde_json::to_string(&commits).unwrap();
    for private in [
        "private-address",
        "Private commit body",
        "access_token",
        "other.invalid",
        TOKEN,
    ] {
        assert!(!projection.contains(private));
    }
    let pulls = client
        .pull_requests(&repository, BRANCH, None, 2)
        .await
        .unwrap();
    assert!(pulls.items[0].merged);
    let session = client.session().unwrap();
    let proof = session
        .verify_integration(&repository, BRANCH, FEATURE, 12, None)
        .await
        .unwrap();
    assert_eq!(proof.merge_commit_sha(), MERGE);
    assert_ne!(proof.head_sha(), proof.merge_commit_sha());
    session
        .delete_verified(&proof, "fixture-deletion-1")
        .await
        .unwrap();
    let captured = fixture.finish().await;
    assert!(captured.iter().all(|request| request.target == "/graphql" || request.target.starts_with(root())));
    assert!(
        captured
            .iter()
            .all(|request| request.authorized && request.pinned_version)
    );
    let mutation = captured
        .iter()
        .find(|request| request.method == "POST")
        .unwrap()
        .body
        .as_ref()
        .unwrap();
    assert_eq!(
        mutation.pointer("/variables/input/repositoryId").unwrap(),
        "R_fixture"
    );
    assert_eq!(
        mutation
            .pointer("/variables/input/refUpdates/0/name")
            .unwrap(),
        "refs/heads/feature/demo"
    );
    assert_eq!(
        mutation
            .pointer("/variables/input/refUpdates/0/beforeOid")
            .unwrap(),
        FEATURE
    );
    assert_eq!(
        mutation
            .pointer("/variables/input/refUpdates/0/afterOid")
            .unwrap(),
        ZERO_OID
    );
    assert_eq!(
        mutation
            .pointer("/variables/input/refUpdates/1/beforeOid")
            .unwrap(),
        TARGET
    );
    assert_eq!(
        mutation
            .pointer("/variables/input/refUpdates/1/afterOid")
            .unwrap(),
        TARGET
    );
    assert_eq!(
        mutation
            .pointer("/variables/input/refUpdates/0/force")
            .unwrap(),
        false
    );
    assert!(!captured.iter().any(|request| request.method == "DELETE"));
}

#[tokio::test]
async fn http_exactly_full_page_is_partial_until_remote_end_is_observed() {
    let fixture = Fixture::start(vec![
        Step::get(
            format!("{}/branches?per_page=1&page=1", root()),
            json!([branch_json(BRANCH, FEATURE)]),
        ),
        Step::get(format!("{}/branches?per_page=1&page=2", root()), json!([])),
    ])
    .await;
    let client = fixture.client();
    let first = client.list_branches(&repository(), None, 1).await.unwrap();
    assert!(!first.complete);
    assert_eq!(first.next_cursor.as_deref(), Some("2"));
    let final_page = client
        .list_branches(&repository(), first.next_cursor.as_deref(), 1)
        .await
        .unwrap();
    assert!(final_page.complete && final_page.items.is_empty());
    fixture.finish().await;
}

#[tokio::test]
async fn http_pagination_rejects_other_hosts_queries_and_skipped_pages() {
    let links = [
        format!(
            "<https://other.invalid{}/branches?per_page=2&page=2>; rel=\"next\"",
            root()
        ),
        format!(
            "<{{origin}}{}/branches?per_page=2&page=2&token=upstream>; rel=\"next\"",
            root()
        ),
        format!(
            "<{{origin}}{}/branches?per_page=2&page=3>; rel=\"next\"",
            root()
        ),
        format!(
            "<{{origin}}{}/commits?per_page=2&page=2>; rel=\"next\"",
            root()
        ),
        format!(
            "<{{origin}}{}/branches?per_page=2&page=2>; type=\"application/json\"",
            root()
        ),
        "malformed".into(),
    ];
    for link in links {
        let fixture = Fixture::start(vec![
            Step::get(
                format!("{}/branches?per_page=2&page=1", root()),
                json!([branch_json(BRANCH, FEATURE)]),
            )
            .header("Link", link),
        ])
        .await;
        assert_eq!(
            fixture
                .client()
                .list_branches(&repository(), None, 2)
                .await
                .unwrap_err(),
            GithubError::InvalidResponse
        );
        fixture.finish().await;
    }
}

#[tokio::test]
async fn http_rejects_invalid_and_truncated_metadata_without_claiming_absence() {
    let branch = branch_json(BRANCH, FEATURE);
    let responses = [
        json!({"message":"unexpected object"}).to_string(),
        json!([{"name":BRANCH,"commit":{"sha":FEATURE}}]).to_string(),
        json!([branch_json(BRANCH, "incomplete-sha")]).to_string(),
        json!([branch.clone(), branch]).to_string(),
        "[{\"name\":".into(),
    ];
    for response in responses {
        let fixture = Fixture::start(vec![
            Step::get(
                format!("{}/branches?per_page=2&page=1", root()),
                Value::Null,
            )
            .body(&response),
        ])
        .await;
        assert_eq!(
            fixture
                .client()
                .list_branches(&repository(), None, 2)
                .await
                .unwrap_err(),
            GithubError::InvalidResponse
        );
        fixture.finish().await;
    }
    let mut truncated = Step::get(format!("{}/branches?per_page=2&page=1", root()), json!([]));
    truncated.declared_length = Some(100);
    let fixture = Fixture::start(vec![truncated]).await;
    assert_eq!(
        fixture
            .client()
            .list_branches(&repository(), None, 2)
            .await
            .unwrap_err(),
        GithubError::Unavailable
    );
    fixture.finish().await;
}

#[tokio::test]
async fn http_enforces_response_and_page_bounds_before_exposing_rows() {
    let mut oversized = Step::get(format!("{}/branches?per_page=2&page=1", root()), json!([]));
    oversized.declared_length = Some(MAX_RESPONSE_BYTES + 1);
    let fixture = Fixture::start(vec![oversized]).await;
    let client = fixture.client();
    for (cursor, size) in [
        (None, 0),
        (None, 101),
        (Some("0"), 2),
        (Some("https://other.invalid"), 2),
        (Some("10001"), 2),
    ] {
        assert_eq!(
            client
                .list_branches(&repository(), cursor, size)
                .await
                .unwrap_err(),
            GithubError::InvalidInput
        );
    }
    assert_eq!(
        client
            .list_branches(&repository(), None, 2)
            .await
            .unwrap_err(),
        GithubError::ResponseTooLarge
    );
    fixture.finish().await;
    let fixture = Fixture::start(vec![Step::get(
        format!("{}/branches?per_page=1&page=1", root()),
        json!([branch_json("main", TARGET), branch_json(BRANCH, FEATURE)]),
    )])
    .await;
    assert_eq!(
        fixture
            .client()
            .list_branches(&repository(), None, 1)
            .await
            .unwrap_err(),
        GithubError::InvalidResponse
    );
    fixture.finish().await;
    let mut chunked = Step::get(
        format!("{}/branches?per_page=2&page=1", root()),
        Value::Null,
    )
    .body(&"x".repeat(MAX_RESPONSE_BYTES + 1));
    chunked.chunked = true;
    let fixture = Fixture::start(vec![chunked]).await;
    assert_eq!(
        fixture
            .client()
            .list_branches(&repository(), None, 2)
            .await
            .unwrap_err(),
        GithubError::ResponseTooLarge
    );
    fixture.finish().await;
}

#[tokio::test]
async fn http_deadline_does_not_turn_a_slow_remote_into_missing_data() {
    let mut slow = Step::get(root(), repo_json());
    slow.delay = Duration::from_millis(200);
    let fixture = Fixture::start(vec![slow]).await;
    let mut client = fixture.client();
    client.http = reqwest::Client::builder()
        .timeout(Duration::from_millis(50))
        .no_proxy()
        .build()
        .unwrap();
    assert_eq!(
        client.repository(&repository()).await.unwrap_err(),
        GithubError::Timeout
    );
    fixture.finish().await;
}

#[tokio::test]
async fn http_next_operation_reads_a_rotated_private_credential() {
    const ROTATED: &str = "fixture-auth-rotated";
    let mut second = Step::get(root(), repo_json());
    second.expected_token = ROTATED;
    let fixture = Fixture::start(vec![Step::get(root(), repo_json()), second]).await;
    let client = fixture.client();
    client.repository(&repository()).await.unwrap();
    std::fs::write(&fixture.token_file, format!("{ROTATED}\n")).unwrap();
    client.repository(&repository()).await.unwrap();
    assert!(
        fixture
            .finish()
            .await
            .iter()
            .all(|request| request.authorized)
    );
}

#[tokio::test]
async fn http_auth_failures_are_private_and_rate_limits_stop_further_requests() {
    for (status, error) in [
        (401, GithubError::Authentication),
        (403, GithubError::Forbidden),
        (404, GithubError::NotFound),
        (500, GithubError::Unavailable),
    ] {
        let fixture = Fixture::start(vec![
            Step::get(
                root(),
                json!({"message":format!("upstream reflected {TOKEN} /private/credential")}),
            )
            .status(status),
        ])
        .await;
        let observed = fixture
            .client()
            .repository(&repository())
            .await
            .unwrap_err();
        assert_eq!(observed, error);
        assert!(!format!("{observed:?}: {observed}").contains(TOKEN));
        fixture.finish().await;
    }
    for status in [403, 429] {
        let fixture = Fixture::start(vec![
            Step::get(root(), json!({"message":"secondary rate limit"}))
                .status(status)
                .header("Retry-After", "120"),
        ])
        .await;
        let client = fixture.client();
        assert_eq!(
            client.repository(&repository()).await.unwrap_err(),
            GithubError::RateLimited {
                retry_after_seconds: 120
            }
        );
        assert!(matches!(
            client.repository(&repository()).await,
            Err(GithubError::RateLimited { .. })
        ));
        let captured = fixture.finish().await;
        assert_eq!(captured.len(), 1);
    }
}

#[tokio::test]
async fn http_success_at_rate_limit_is_returned_then_later_requests_wait() {
    let reset = epoch_seconds() + 120;
    let fixture = Fixture::start(vec![
        Step::get(root(), repo_json())
            .header("X-RateLimit-Remaining", "0")
            .header("X-RateLimit-Reset", reset.to_string()),
    ])
    .await;
    let client = fixture.client();
    assert_eq!(
        client
            .repository(&repository())
            .await
            .unwrap()
            .default_branch,
        "main"
    );
    assert!(matches!(
        client.repository(&repository()).await,
        Err(GithubError::RateLimited { .. })
    ));
    fixture.finish().await;
}

#[tokio::test]
async fn http_rate_limit_headers_apply_even_when_the_error_body_is_oversized() {
    let mut limited = Step::get(root(), Value::Null)
        .status(429)
        .header("Retry-After", "120");
    limited.declared_length = Some(MAX_RESPONSE_BYTES + 1);
    let fixture = Fixture::start(vec![limited]).await;
    let client = fixture.client();
    assert_eq!(
        client.repository(&repository()).await.unwrap_err(),
        GithubError::RateLimited {
            retry_after_seconds: 120
        }
    );
    assert!(matches!(
        client.repository(&repository()).await,
        Err(GithubError::RateLimited { .. })
    ));
    fixture.finish().await;
}

#[tokio::test]
async fn http_redirect_and_credential_echo_are_rejected() {
    let fixture = Fixture::start(vec![
        Step::get(root(), Value::Null)
            .status(302)
            .header("Location", "https://other.invalid/receive"),
    ])
    .await;
    assert_eq!(
        fixture
            .client()
            .repository(&repository())
            .await
            .unwrap_err(),
        GithubError::InvalidResponse
    );
    fixture.finish().await;
    let mut echoed = repo_json();
    echoed["default_branch"] = TOKEN.into();
    let fixture = Fixture::start(vec![Step::get(root(), echoed)]).await;
    assert_eq!(
        fixture
            .client()
            .repository(&repository())
            .await
            .unwrap_err(),
        GithubError::InvalidResponse
    );
    fixture.finish().await;
}

#[tokio::test]
async fn http_merge_proof_rejects_protected_changed_unmerged_forked_or_uncontained_heads() {
    for case in [
        "default",
        "protected",
        "changed",
        "unmerged",
        "wrong-head",
        "fork",
        "no-merge-sha",
        "diverged",
        "feature-race",
        "target-race",
    ] {
        let mut steps = proof_steps();
        let expected = match case {
            "default" => {
                steps.truncate(1);
                GithubError::ProtectedBranch
            }
            "protected" => {
                let mut branch = branch_json(BRANCH, FEATURE);
                branch["protected"] = true.into();
                steps[1].body = branch.to_string();
                steps.truncate(2);
                GithubError::ProtectedBranch
            }
            "changed" => {
                steps[1].body = branch_json(BRANCH, CHANGED).to_string();
                steps.truncate(2);
                GithubError::Conflict
            }
            "unmerged" => {
                let mut pull = pull_json();
                pull["merged"] = false.into();
                pull["merged_at"] = Value::Null;
                steps[2].body = pull.to_string();
                steps.truncate(3);
                GithubError::MergeNotProven
            }
            "wrong-head" => {
                let mut pull = pull_json();
                pull["head"]["sha"] = CHANGED.into();
                steps[2].body = pull.to_string();
                steps.truncate(3);
                GithubError::MergeNotProven
            }
            "fork" => {
                let mut pull = pull_json();
                pull["head"]["repo"]["owner"]["login"] = "other-owner".into();
                steps[2].body = pull.to_string();
                steps.truncate(3);
                GithubError::MergeNotProven
            }
            "no-merge-sha" => {
                let mut pull = pull_json();
                pull["merge_commit_sha"] = Value::Null;
                steps[2].body = pull.to_string();
                steps.truncate(3);
                GithubError::MergeNotProven
            }
            "diverged" => {
                let mut compare = compare_json();
                compare["status"] = "diverged".into();
                compare["behind_by"] = 2.into();
                steps[4].body = compare.to_string();
                steps.truncate(5);
                GithubError::MergeNotProven
            }
            "feature-race" => {
                steps[5].body = ref_json(BRANCH, CHANGED).to_string();
                steps.truncate(6);
                GithubError::Conflict
            }
            "target-race" => {
                steps[6].body = ref_json("main", CHANGED).to_string();
                GithubError::Conflict
            }
            _ => unreachable!(),
        };
        let fixture = Fixture::start(steps).await;
        let actual = fixture
            .client()
            .verify_integration(
                &repository(),
                if case == "default" { "main" } else { BRANCH },
                FEATURE,
                12,
                None,
            )
            .await
            .unwrap_err();
        assert_eq!(actual, expected, "case {case}");
        let captured = fixture.finish().await;
        assert!(captured.iter().all(|request| request.method == "GET"));
    }
}

#[tokio::test]
async fn http_atomic_deletion_conflict_or_forbidden_is_never_success() {
    for (kind, expected) in [
        ("UNPROCESSABLE", GithubError::Conflict),
        ("FORBIDDEN", GithubError::Forbidden),
        (
            "RATE_LIMITED",
            GithubError::RateLimited {
                retry_after_seconds: 60,
            },
        ),
    ] {
        let mut steps = proof_steps();
        steps.push(Step::mutation(Some(json!({"data":{"updateRefs":null},"errors":[{"type":kind,"message":format!("ref changed {TOKEN}")}]}))));
        let fixture = Fixture::start(steps).await;
        let client = fixture.client();
        let session = client.session().unwrap();
        let proof = session
            .verify_integration(&repository(), BRANCH, FEATURE, 12, None)
            .await
            .unwrap();
        assert_eq!(
            session
                .delete_verified(&proof, "fixture-race")
                .await
                .unwrap_err(),
            expected
        );
        assert!(!expected.to_string().contains(TOKEN));
        fixture.finish().await;
    }
}

#[tokio::test]
async fn http_remote_deletion_requires_observed_absence_and_continued_repository_access() {
    for case in ["missing-result", "branch-remains", "lost-access"] {
        let mut steps = proof_steps();
        if case == "missing-result" {
            steps.push(Step::mutation(Some(json!({"data":{"updateRefs":null}}))));
        } else {
            steps.push(Step::mutation(None));
            if case == "lost-access" {
                steps.push(Step::get(root(), json!({"message":"Not Found"})).status(404));
            } else {
                steps.extend([
                    Step::get(root(), repo_json()),
                    Step::get(
                        format!("{}/git/ref/heads%2Fmain", root()),
                        ref_json("main", TARGET),
                    ),
                    Step::get(
                        format!("{}/git/ref/heads%2Ffeature%2Fdemo", root()),
                        ref_json(BRANCH, CHANGED),
                    ),
                ]);
            }
        }
        let fixture = Fixture::start(steps).await;
        let client = fixture.client();
        let session = client.session().unwrap();
        let proof = session
            .verify_integration(&repository(), BRANCH, FEATURE, 12, None)
            .await
            .unwrap();
        assert_eq!(
            session
                .delete_verified(&proof, "fixture-unverified")
                .await
                .unwrap_err(),
            GithubError::DeletionUnverified
        );
        fixture.finish().await;
    }
}

#[tokio::test]
async fn http_stale_merge_proof_cannot_start_a_remote_mutation() {
    let fixture = Fixture::start(proof_steps()).await;
    let client = fixture.client();
    let session = client.session().unwrap();
    let mut proof = session
        .verify_integration(&repository(), BRANCH, FEATURE, 12, None)
        .await
        .unwrap();
    assert!(proof.is_fresh());
    proof.observed_at = (OffsetDateTime::now_utc() - time::Duration::minutes(6))
        .format(&Rfc3339)
        .unwrap();
    assert!(!proof.is_fresh());
    assert_eq!(
        session
            .delete_verified(&proof, "fixture-stale")
            .await
            .unwrap_err(),
        GithubError::MergeNotProven
    );
    assert!(
        fixture
            .finish()
            .await
            .iter()
            .all(|request| request.method == "GET")
    );
}

#[tokio::test]
async fn http_commit_graph_keeps_pagination_anchored_and_rejects_missing_parents() {
    let mut no_parents = commit_json(FEATURE, &[]);
    no_parents.as_object_mut().unwrap().remove("parents");
    for value in [
        no_parents,
        commit_json(FEATURE, &["invalid"]),
        commit_json(FEATURE, &[FEATURE]),
        commit_json(PARENT, &[]),
    ] {
        let fixture = Fixture::start(vec![Step::get(
            format!("{}/commits?per_page=2&page=1&sha={FEATURE}", root()),
            json!([value]),
        )])
        .await;
        assert_eq!(
            fixture
                .client()
                .commits(&repository(), FEATURE, None, 2)
                .await
                .unwrap_err(),
            GithubError::InvalidResponse
        );
        fixture.finish().await;
    }
    let fixture = Fixture::start(vec![
        Step::get(
            format!("{}/commits?per_page=1&page=1&sha={FEATURE}", root()),
            json!([commit_json(FEATURE, &[PARENT])]),
        )
        .header(
            "Link",
            format!(
                "<{{origin}}{}/commits?per_page=1&page=2&sha={FEATURE}>; rel=\"next\"",
                root()
            ),
        ),
        Step::get(
            format!("{}/commits?per_page=1&page=2&sha={FEATURE}", root()),
            json!([]),
        ),
    ])
    .await;
    let client = fixture.client();
    let first = client
        .commits(&repository(), FEATURE, None, 1)
        .await
        .unwrap();
    assert!(
        client
            .commits(&repository(), FEATURE, first.next_cursor.as_deref(), 1)
            .await
            .unwrap()
            .complete
    );
    fixture.finish().await;
}

#[tokio::test]
async fn http_pr_pagination_omits_forks_without_claiming_end_of_collection() {
    let mut fork = pull_json();
    fork["head"]["repo"]["owner"]["login"] = "other-owner".into();
    let fixture=Fixture::start(vec![Step::get(format!("{}/pulls?per_page=1&page=1&state=all&head=fixture-owner%3Afeature%2Fdemo&sort=updated&direction=desc",root()),json!([fork]))]).await;
    let page = fixture
        .client()
        .pull_requests(&repository(), BRANCH, None, 1)
        .await
        .unwrap();
    assert!(page.items.is_empty());
    assert!(!page.complete);
    assert_eq!(page.next_cursor.as_deref(), Some("2"));
    fixture.finish().await;
}

#[tokio::test]
async fn missing_or_unsafe_credentials_never_fall_back_to_unauthenticated_http() {
    for case in [
        "missing",
        "mode",
        "symlink",
        "parent-symlink",
        "hardlink",
        "directory",
        "empty",
        "malformed",
    ] {
        let fixture = Fixture::start(vec![]).await;
        let mut token_file = fixture.token_file.clone();
        let error = match case {
            "missing" => {
                std::fs::remove_file(&token_file).unwrap();
                GithubError::MissingCredential
            }
            "mode" => {
                std::fs::set_permissions(&token_file, std::fs::Permissions::from_mode(0o644))
                    .unwrap();
                GithubError::UnsafeCredential
            }
            "symlink" => {
                let link = token_file.with_extension("link");
                symlink(&token_file, &link).unwrap();
                token_file = link;
                GithubError::UnsafeCredential
            }
            "parent-symlink" => {
                let link = fixture._private.path().join("linked-parent");
                symlink(fixture._private.path(), &link).unwrap();
                token_file = link.join("github.token");
                GithubError::UnsafeCredential
            }
            "hardlink" => {
                std::fs::hard_link(&token_file, token_file.with_extension("hardlink")).unwrap();
                GithubError::UnsafeCredential
            }
            "directory" => {
                token_file = fixture._private.path().into();
                GithubError::UnsafeCredential
            }
            "empty" => {
                std::fs::write(&token_file, "").unwrap();
                GithubError::UnsafeCredential
            }
            "malformed" => {
                std::fs::write(&token_file, "not a valid\ncredential").unwrap();
                GithubError::UnsafeCredential
            }
            _ => unreachable!(),
        };
        let client = GithubClient::fixture(token_file, fixture.address).unwrap();
        assert_eq!(
            client.repository(&repository()).await.unwrap_err(),
            error,
            "case {case}"
        );
        assert!(fixture.finish().await.is_empty());
    }
}

#[test]
fn repository_and_ref_admission_cannot_forward_credentials_or_arbitrary_hosts() {
    for address in [
        "http://github.com/owner/repo",
        "https://other.invalid/owner/repo",
        "https://token@github.com/owner/repo",
        "https://github.com/owner/repo?token=secret",
        "https://github.com/owner/repo#secret",
        "https://github.com/owner/repo/extra",
        "https://github.com/owner/%2e%2e",
        "https://github.com/owner/repo/../other",
        "https://github.com/owner/repo///",
        " https://github.com/owner/repo",
        "https://github.com:444/owner/repo",
    ] {
        assert_eq!(
            GitHubRepository::from_url(address).unwrap_err(),
            GithubError::InvalidInput
        );
    }
    assert_eq!(
        GitHubRepository::from_url("https://github.com/owner/repo.git")
            .unwrap()
            .html_url(),
        "https://github.com/owner/repo"
    );
    assert!(valid_branch("feature/проект"));
    for branch in [
        "../main",
        "refs//main",
        "feature/.hidden",
        "x.lock",
        "feature/../main",
        "main@{1}",
        "a\nb",
        "-flag",
    ] {
        assert!(!valid_branch(branch), "invalid ref {branch:?}");
    }
    assert!(!GithubError::DeletionUnverified.retryable());
    let client = GithubClient::new(PathBuf::from("/private/credential/path")).unwrap();
    assert!(!format!("{client:?}").contains("/private/credential/path"));
}
