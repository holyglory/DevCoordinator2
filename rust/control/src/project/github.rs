//! Daemon-private, bounded GitHub access for the Project Explorer.
//!
//! Production credentials can reach only api.github.com. Pagination links are
//! inspected, never followed, and URLs returned to readers are reconstructed.
//! A failed refresh is not evidence that a branch disappeared. Local cleanup
//! and its durable audit receipt remain the calling service's responsibility.

use std::fmt;
use std::io::Read;
use std::os::unix::fs::MetadataExt;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use reqwest::header::{AUTHORIZATION, CACHE_CONTROL, HeaderMap, HeaderValue, LINK, RETRY_AFTER};
use reqwest::{Method, StatusCode, Url};
use rustix::fs::{Mode, OFlags, fstat, open, openat};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

pub const GITHUB_API_VERSION: &str = "2026-03-10";
const MAX_RESPONSE_BYTES: usize = 2 * 1024 * 1024;
const MAX_TOKEN_BYTES: u64 = 4096;
const MAX_PAGE: u32 = 10_000;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(12);
const PROOF_TIMEOUT: Duration = Duration::from_secs(60);
const ZERO_OID: &str = "0000000000000000000000000000000000000000";

/// Errors deliberately contain no upstream body, request URL, token, or path.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GithubError {
    InvalidInput,
    MissingCredential,
    UnsafeCredential,
    Authentication,
    Forbidden,
    NotFound,
    RateLimited {
        retry_after_seconds: u64,
    },
    Timeout,
    Unavailable,
    InvalidResponse,
    ResponseTooLarge,
    Conflict,
    ProtectedBranch,
    MergeNotProven,
    CleanupNotVerified,
    /// A mutation may have reached GitHub; refresh before deciding what to do.
    DeletionUnverified,
}

impl GithubError {
    pub fn retryable(self) -> bool {
        matches!(
            self,
            Self::RateLimited { .. } | Self::Timeout | Self::Unavailable
        )
    }

    pub fn retry_after_seconds(self) -> Option<u64> {
        match self {
            Self::RateLimited {
                retry_after_seconds,
            } => Some(retry_after_seconds),
            _ => None,
        }
    }
}

impl fmt::Display for GithubError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidInput => "the GitHub repository, ref, or page is invalid",
            Self::MissingCredential => "the Coordinator GitHub credential is not configured",
            Self::UnsafeCredential => {
                "the Coordinator GitHub credential is not a safe private file"
            }
            Self::Authentication => "GitHub rejected the Coordinator credential",
            Self::Forbidden => "the Coordinator credential cannot perform this GitHub operation",
            Self::NotFound => "GitHub did not expose the requested repository or ref",
            Self::RateLimited { .. } => {
                "GitHub asked the Coordinator to wait before another request"
            }
            Self::Timeout => "the GitHub request did not finish before its deadline",
            Self::Unavailable => "GitHub is unavailable",
            Self::InvalidResponse => "GitHub returned incomplete or invalid metadata",
            Self::ResponseTooLarge => "the GitHub response exceeded the metadata limit",
            Self::Conflict => "the GitHub branch changed; refresh its evidence before retrying",
            Self::ProtectedBranch => "the GitHub branch is protected or is an integration branch",
            Self::MergeNotProven => "the exact feature head has no verified merged pull request",
            Self::CleanupNotVerified => {
                "verified local cleanup is required before remote branch deletion"
            }
            Self::DeletionUnverified => {
                "the remote deletion result is uncertain; refresh GitHub before retrying"
            }
        })
    }
}

impl std::error::Error for GithubError {}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct GitHubRepository {
    owner: String,
    name: String,
}

impl GitHubRepository {
    pub fn new(owner: impl Into<String>, name: impl Into<String>) -> Result<Self, GithubError> {
        let owner = owner.into();
        let name = name.into();
        if !valid_owner(&owner) || !valid_repository_name(&name) {
            return Err(GithubError::InvalidInput);
        }
        Ok(Self { owner, name })
    }

    /// Only a credential-free HTTPS github.com repository URL is admitted.
    /// Enterprise hosts require a separately reviewed explicit configuration.
    pub fn from_url(value: &str) -> Result<Self, GithubError> {
        if value.trim() != value
            || value.chars().any(char::is_control)
            || value.contains(['%', '\\', '@'])
            || value.contains("/../")
            || value.contains("/./")
        {
            return Err(GithubError::InvalidInput);
        }
        let url = Url::parse(value).map_err(|_| GithubError::InvalidInput)?;
        if url.scheme() != "https"
            || url.host_str() != Some("github.com")
            || !url.username().is_empty()
            || url.password().is_some()
            || url.port().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return Err(GithubError::InvalidInput);
        }
        let mut parts = url
            .path()
            .strip_suffix('/')
            .unwrap_or(url.path())
            .split('/');
        if parts.next() != Some("") {
            return Err(GithubError::InvalidInput);
        }
        let owner = parts.next().ok_or(GithubError::InvalidInput)?;
        let name = parts.next().ok_or(GithubError::InvalidInput)?;
        if parts.next().is_some() {
            return Err(GithubError::InvalidInput);
        }
        Self::new(owner, name.strip_suffix(".git").unwrap_or(name))
    }

    pub fn owner(&self) -> &str {
        &self.owner
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn html_url(&self) -> String {
        format!("https://github.com/{}/{}", self.owner, self.name)
    }

    fn matches(&self, owner: &str, name: &str) -> bool {
        self.owner.eq_ignore_ascii_case(owner) && self.name.eq_ignore_ascii_case(name)
    }

    fn browser_url(&self, segments: &[&str]) -> String {
        let mut url = Url::parse(&self.html_url()).expect("validated GitHub repository URL");
        url.path_segments_mut().expect("HTTPS URL").extend(segments);
        url.to_string()
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct GithubRepositoryInfo {
    pub owner: String,
    pub name: String,
    pub default_branch: String,
    pub html_url: String,
    pub node_id: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct GithubBranch {
    pub name: String,
    pub head_sha: String,
    pub protected: bool,
    pub html_url: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct GithubCommit {
    pub sha: String,
    pub parent_shas: Vec<String>,
    pub subject: String,
    pub author: String,
    pub authored_at: String,
    pub html_url: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct GithubPullRequest {
    pub number: u64,
    pub html_url: String,
    pub state: String,
    pub merged: bool,
    pub head_ref: String,
    pub head_sha: String,
    pub base_ref: String,
    pub merged_at: Option<String>,
    pub merge_commit_sha: Option<String>,
}

/// One bounded remote page, not a claim that every repository commit was read.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct GithubPage<T> {
    pub items: Vec<T>,
    pub next_cursor: Option<String>,
    /// The current page reached the remote collection's end. Earlier pages
    /// still have to be retained and concurrent ref changes remain possible.
    pub complete: bool,
    pub fetched_at: String,
}

/// Only a fresh network verification can create this proof. Persist its
/// serialized projection as evidence; do not deserialize it into authority.
#[derive(Clone, Debug, Serialize)]
pub struct GithubIntegrationProof {
    repository: GitHubRepository,
    repository_node_id: String,
    branch_name: String,
    head_sha: String,
    integration_branch: String,
    integration_head_sha: String,
    merge_commit_sha: String,
    pull_request_number: u64,
    observed_at: String,
}

impl GithubIntegrationProof {
    pub fn repository(&self) -> &GitHubRepository {
        &self.repository
    }
    pub fn branch_name(&self) -> &str {
        &self.branch_name
    }
    pub fn head_sha(&self) -> &str {
        &self.head_sha
    }
    pub fn integration_branch(&self) -> &str {
        &self.integration_branch
    }
    pub fn integration_head_sha(&self) -> &str {
        &self.integration_head_sha
    }
    pub fn merge_commit_sha(&self) -> &str {
        &self.merge_commit_sha
    }
    pub fn pull_request_number(&self) -> u64 {
        self.pull_request_number
    }
    pub fn observed_at(&self) -> &str {
        &self.observed_at
    }

    pub fn is_fresh(&self) -> bool {
        OffsetDateTime::parse(&self.observed_at, &Rfc3339)
            .ok()
            .map(|observed| (OffsetDateTime::now_utc() - observed).whole_seconds())
            .is_some_and(|age| (0..=300).contains(&age))
    }
}

#[derive(Clone, Debug)]
pub struct GithubDeleteRequest {
    pub repository_id: String,
    pub branch_name: String,
    pub expected_head_sha: String,
    pub pull_request_number: u64,
    /// None means the fresh GitHub default. Some must come from the service's
    /// approved integration branch, never an unvalidated browser assertion.
    pub integration_branch: Option<String>,
}

/// Returned only after the exact remote ref is observed absent, with repository
/// access rechecked. The service must persist this alongside its cleanup audit.
#[derive(Clone, Debug, Serialize)]
pub struct GithubDeleteReceipt {
    pub repository_id: String,
    pub repository: GitHubRepository,
    pub branch_name: String,
    pub removed_head_sha: String,
    pub integration_branch: String,
    pub integration_head_sha: String,
    pub merge_commit_sha: String,
    pub pull_request_number: u64,
    pub local_cleanup_receipt_id: String,
    pub worktree_id: String,
    pub mutation_id: String,
    pub observed_at: String,
}

#[derive(Clone)]
pub struct GithubClient {
    http: reqwest::Client,
    api_base: Url,
    token_file: PathBuf,
    rate_limited_until: Arc<AtomicU64>,
}

impl fmt::Debug for GithubClient {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("GithubClient { credential: [private] }")
    }
}

impl GithubClient {
    /// Call with Config::secrets_dir().join("github.token"). Missing credentials
    /// are reported when an operation is attempted, so cached reads stay usable.
    pub fn new(token_file: PathBuf) -> Result<Self, GithubError> {
        Self::build(
            token_file,
            Url::parse("https://api.github.com/").expect("fixed URL"),
            true,
        )
    }

    fn build(token_file: PathBuf, api_base: Url, https_only: bool) -> Result<Self, GithubError> {
        if !token_file.is_absolute() {
            return Err(GithubError::UnsafeCredential);
        }
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .https_only(https_only)
            .no_proxy()
            .connect_timeout(Duration::from_secs(4))
            .timeout(REQUEST_TIMEOUT)
            .user_agent("DevCoordinator2-ProjectExplorer")
            .build()
            .map_err(|_| GithubError::Unavailable)?;
        Ok(Self {
            http,
            api_base,
            token_file,
            rate_limited_until: Arc::new(AtomicU64::new(0)),
        })
    }

    // This is compiled only into the test harness. Production has no arbitrary
    // origin option that could forward the Coordinator credential elsewhere.
    #[cfg(test)]
    fn fixture(token_file: PathBuf, address: std::net::SocketAddr) -> Result<Self, GithubError> {
        if !address.ip().is_loopback() {
            return Err(GithubError::InvalidInput);
        }
        Self::build(
            token_file,
            Url::parse(&format!("http://{address}/")).expect("socket URL"),
            false,
        )
    }

    fn session(&self) -> Result<Session<'_>, GithubError> {
        self.check_rate_limit()?;
        let token = read_private_token(&self.token_file)?;
        let mut authorization = HeaderValue::from_str(&format!("Bearer {token}"))
            .map_err(|_| GithubError::UnsafeCredential)?;
        authorization.set_sensitive(true);
        Ok(Session {
            client: self,
            token,
            authorization,
        })
    }

    fn check_rate_limit(&self) -> Result<(), GithubError> {
        let remaining = self
            .rate_limited_until
            .load(Ordering::Relaxed)
            .saturating_sub(epoch_seconds());
        if remaining > 0 {
            return Err(GithubError::RateLimited {
                retry_after_seconds: remaining,
            });
        }
        Ok(())
    }

    pub async fn repository(
        &self,
        repository: &GitHubRepository,
    ) -> Result<GithubRepositoryInfo, GithubError> {
        self.session()?.repository(repository).await
    }

    pub async fn list_branches(
        &self,
        repository: &GitHubRepository,
        cursor: Option<&str>,
        page_size: u32,
    ) -> Result<GithubPage<GithubBranch>, GithubError> {
        self.session()?
            .list_branches(repository, cursor, page_size)
            .await
    }

    pub async fn commits(
        &self,
        repository: &GitHubRepository,
        head_sha: &str,
        cursor: Option<&str>,
        page_size: u32,
    ) -> Result<GithubPage<GithubCommit>, GithubError> {
        self.session()?
            .commits(repository, head_sha, cursor, page_size)
            .await
    }

    pub async fn pull_requests(
        &self,
        repository: &GitHubRepository,
        branch: &str,
        cursor: Option<&str>,
        page_size: u32,
    ) -> Result<GithubPage<GithubPullRequest>, GithubError> {
        self.session()?
            .pull_requests(repository, branch, cursor, page_size)
            .await
    }

    pub async fn verify_integration(
        &self,
        repository: &GitHubRepository,
        branch: &str,
        expected_head_sha: &str,
        pull_request_number: u64,
        integration_branch: Option<&str>,
    ) -> Result<GithubIntegrationProof, GithubError> {
        let session = self.session()?;
        tokio::time::timeout(
            PROOF_TIMEOUT,
            session.verify_integration(
                repository,
                branch,
                expected_head_sha,
                pull_request_number,
                integration_branch,
            ),
        )
        .await
        .map_err(|_| GithubError::Timeout)?
    }

    /// The caller must first verify all remaining local copies/consumers. This
    /// opaque receipt proves the exact checkout was removed; it cannot be
    /// supplied by a browser or reconstructed from a claimed success flag.
    pub(crate) async fn delete_branch(
        &self,
        repository: &GitHubRepository,
        request: &GithubDeleteRequest,
        cleanup: &super::cleanup::VerifiedCheckoutRemoval,
    ) -> Result<GithubDeleteReceipt, GithubError> {
        if request.repository_id != cleanup.repository_id()
            || request.branch_name != cleanup.branch_name()
            || request.expected_head_sha != cleanup.head_sha()
            || cleanup.receipt_id().is_empty()
            || cleanup.worktree_id().is_empty()
        {
            return Err(GithubError::CleanupNotVerified);
        }
        let session = self.session()?;
        let proof = tokio::time::timeout(
            PROOF_TIMEOUT,
            session.verify_integration(
                repository,
                &request.branch_name,
                &request.expected_head_sha,
                request.pull_request_number,
                request.integration_branch.as_deref(),
            ),
        )
        .await
        .map_err(|_| GithubError::Timeout)??;
        let mutation_id = mutation_id()?;
        session.delete_verified(&proof, &mutation_id).await?;
        Ok(GithubDeleteReceipt {
            repository_id: request.repository_id.clone(),
            repository: repository.clone(),
            branch_name: proof.branch_name,
            removed_head_sha: proof.head_sha,
            integration_branch: proof.integration_branch,
            integration_head_sha: proof.integration_head_sha,
            merge_commit_sha: proof.merge_commit_sha,
            pull_request_number: proof.pull_request_number,
            local_cleanup_receipt_id: cleanup.receipt_id().into(),
            worktree_id: cleanup.worktree_id().into(),
            mutation_id,
            observed_at: timestamp(),
        })
    }
}

struct Session<'a> {
    client: &'a GithubClient,
    token: String,
    authorization: HeaderValue,
}

struct Document {
    value: Value,
    headers: HeaderMap,
    url: Url,
}

impl Session<'_> {
    fn url(&self, segments: &[&str], query: &[(&str, String)]) -> Url {
        let mut url = self.client.api_base.clone();
        url.path_segments_mut()
            .expect("fixed HTTP origin")
            .extend(segments);
        if !query.is_empty() {
            url.query_pairs_mut()
                .extend_pairs(query.iter().map(|(name, value)| (*name, value.as_str())));
        }
        url
    }

    async fn request(
        &self,
        method: Method,
        segments: &[&str],
        query: &[(&str, String)],
        body: Option<Value>,
    ) -> Result<Document, GithubError> {
        self.client.check_rate_limit()?;
        let url = self.url(segments, query);
        let mut request = self
            .client
            .http
            .request(method, url.clone())
            .header(AUTHORIZATION, self.authorization.clone())
            .header("X-GitHub-Api-Version", GITHUB_API_VERSION)
            .header("Accept", "application/vnd.github+json")
            .header(CACHE_CONTROL, "no-cache");
        if let Some(body) = body {
            request = request.json(&body);
        }
        let mut response = request.send().await.map_err(http_error)?;
        let status = response.status();
        let headers = response.headers().clone();
        if status == StatusCode::UNAUTHORIZED {
            return Err(GithubError::Authentication);
        }
        if status == StatusCode::NOT_FOUND {
            return Err(GithubError::NotFound);
        }
        if status == StatusCode::CONFLICT || status == StatusCode::PRECONDITION_FAILED {
            return Err(GithubError::Conflict);
        }
        if status.is_redirection() {
            return Err(GithubError::InvalidResponse);
        }
        let exhausted = header_u64(&headers, "x-ratelimit-remaining") == Some(0);
        if status == StatusCode::TOO_MANY_REQUESTS
            || (status == StatusCode::FORBIDDEN && (exhausted || headers.contains_key(RETRY_AFTER)))
        {
            return Err(self.record_rate_limit(&headers));
        }
        if exhausted && status.is_success() {
            self.record_rate_limit(&headers);
        }
        if response
            .content_length()
            .is_some_and(|size| size > MAX_RESPONSE_BYTES as u64)
        {
            return Err(GithubError::ResponseTooLarge);
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(http_error)? {
            if bytes.len().saturating_add(chunk.len()) > MAX_RESPONSE_BYTES {
                return Err(GithubError::ResponseTooLarge);
            }
            bytes.extend_from_slice(&chunk);
        }
        let secondary = status == StatusCode::FORBIDDEN
            && serde_json::from_slice::<Value>(&bytes)
                .ok()
                .and_then(|body| {
                    body.get("message")
                        .and_then(Value::as_str)
                        .map(str::to_ascii_lowercase)
                })
                .is_some_and(|message| {
                    message.contains("rate limit") || message.contains("abuse detection")
                });
        if secondary {
            return Err(self.record_rate_limit(&headers));
        }
        if status == StatusCode::FORBIDDEN {
            return Err(GithubError::Forbidden);
        }
        if status == StatusCode::UNPROCESSABLE_ENTITY {
            return Err(GithubError::Conflict);
        }
        if !status.is_success() {
            return Err(GithubError::Unavailable);
        }
        let value: Value =
            serde_json::from_slice(&bytes).map_err(|_| GithubError::InvalidResponse)?;
        // Refuse credential echoes in metadata. GraphQL error messages are
        // discarded in full; only their typed error class is interpreted.
        let graphql_error =
            segments == ["graphql"] && value.get("errors").is_some_and(Value::is_array);
        if !graphql_error
            && bytes
                .windows(self.token.len())
                .any(|window| window == self.token.as_bytes())
        {
            return Err(GithubError::InvalidResponse);
        }
        Ok(Document {
            value,
            headers,
            url,
        })
    }

    fn record_rate_limit(&self, headers: &HeaderMap) -> GithubError {
        let retry_after_seconds = retry_delay(headers).max(1);
        self.client.rate_limited_until.fetch_max(
            epoch_seconds().saturating_add(retry_after_seconds),
            Ordering::Relaxed,
        );
        GithubError::RateLimited {
            retry_after_seconds,
        }
    }

    async fn repository(
        &self,
        repository: &GitHubRepository,
    ) -> Result<GithubRepositoryInfo, GithubError> {
        let document = self
            .request(
                Method::GET,
                &["repos", repository.owner(), repository.name()],
                &[],
                None,
            )
            .await?;
        let raw: RawRepository = decode(document.value)?;
        if !repository.matches(&raw.owner.login, &raw.name)
            || !valid_node_id(&raw.node_id)
            || !valid_branch(&raw.default_branch)
        {
            return Err(GithubError::InvalidResponse);
        }
        Ok(GithubRepositoryInfo {
            owner: raw.owner.login,
            name: raw.name,
            default_branch: raw.default_branch,
            html_url: repository.html_url(),
            node_id: raw.node_id,
        })
    }

    async fn list_branches(
        &self,
        repository: &GitHubRepository,
        cursor: Option<&str>,
        page_size: u32,
    ) -> Result<GithubPage<GithubBranch>, GithubError> {
        let (page, query) = page_query(cursor, page_size)?;
        let document = self
            .request(
                Method::GET,
                &["repos", repository.owner(), repository.name(), "branches"],
                &query,
                None,
            )
            .await?;
        let raw: Vec<RawBranch> = decode(document.value)?;
        let next_cursor =
            next_cursor(&document.headers, &document.url, page, page_size, raw.len())?;
        let mut items = Vec::with_capacity(raw.len());
        for branch in raw {
            items.push(branch.into_branch(repository)?);
        }
        if has_duplicates(items.iter().map(|branch| branch.name.as_str())) {
            return Err(GithubError::InvalidResponse);
        }
        Ok(page_result(items, next_cursor))
    }

    async fn branch(
        &self,
        repository: &GitHubRepository,
        branch: &str,
    ) -> Result<GithubBranch, GithubError> {
        if !valid_branch(branch) {
            return Err(GithubError::InvalidInput);
        }
        let document = self
            .request(
                Method::GET,
                &[
                    "repos",
                    repository.owner(),
                    repository.name(),
                    "branches",
                    branch,
                ],
                &[],
                None,
            )
            .await?;
        let raw: RawBranch = decode(document.value)?;
        let value = raw.into_branch(repository)?;
        if value.name != branch {
            return Err(GithubError::InvalidResponse);
        }
        Ok(value)
    }

    async fn commits(
        &self,
        repository: &GitHubRepository,
        head_sha: &str,
        cursor: Option<&str>,
        page_size: u32,
    ) -> Result<GithubPage<GithubCommit>, GithubError> {
        if !valid_sha(head_sha) {
            return Err(GithubError::InvalidInput);
        }
        let (page, mut query) = page_query(cursor, page_size)?;
        query.push(("sha", head_sha.into()));
        let document = self
            .request(
                Method::GET,
                &["repos", repository.owner(), repository.name(), "commits"],
                &query,
                None,
            )
            .await?;
        let raw: Vec<RawCommit> = decode(document.value)?;
        let next_cursor =
            next_cursor(&document.headers, &document.url, page, page_size, raw.len())?;
        let mut items = Vec::with_capacity(raw.len());
        for raw in raw {
            if !valid_sha(&raw.sha)
                || raw.parents.len() > 64
                || raw
                    .parents
                    .iter()
                    .any(|parent| !valid_sha(&parent.sha) || parent.sha == raw.sha)
                || has_duplicates(raw.parents.iter().map(|parent| parent.sha.as_str()))
                || !valid_timestamp(&raw.commit.author.date)
            {
                return Err(GithubError::InvalidResponse);
            }
            let sha = raw.sha;
            let author = raw
                .author
                .map(|author| author.login)
                .filter(|login| valid_owner(login))
                .unwrap_or_else(|| display_author(&raw.commit.author.name));
            items.push(GithubCommit {
                html_url: repository.browser_url(&["commit", &sha]),
                sha,
                parent_shas: raw.parents.into_iter().map(|parent| parent.sha).collect(),
                subject: display_text(raw.commit.message.lines().next().unwrap_or_default(), 500),
                author,
                authored_at: raw.commit.author.date,
            });
        }
        if has_duplicates(items.iter().map(|commit| commit.sha.as_str())) {
            return Err(GithubError::InvalidResponse);
        }
        if page == 1 && items.first().is_some_and(|commit| commit.sha != head_sha) {
            return Err(GithubError::InvalidResponse);
        }
        Ok(page_result(items, next_cursor))
    }

    async fn pull_requests(
        &self,
        repository: &GitHubRepository,
        branch: &str,
        cursor: Option<&str>,
        page_size: u32,
    ) -> Result<GithubPage<GithubPullRequest>, GithubError> {
        if !valid_branch(branch) {
            return Err(GithubError::InvalidInput);
        }
        let (page, mut query) = page_query(cursor, page_size)?;
        query.extend([
            ("state", "all".into()),
            ("head", format!("{}:{branch}", repository.owner())),
            ("sort", "updated".into()),
            ("direction", "desc".into()),
        ]);
        let document = self
            .request(
                Method::GET,
                &["repos", repository.owner(), repository.name(), "pulls"],
                &query,
                None,
            )
            .await?;
        let raw: Vec<RawPullRequest> = decode(document.value)?;
        let next_cursor =
            next_cursor(&document.headers, &document.url, page, page_size, raw.len())?;
        let mut items = Vec::with_capacity(raw.len());
        for raw in raw {
            if let Some(pull) = raw.into_pull(repository)? {
                if pull.head_ref != branch {
                    return Err(GithubError::InvalidResponse);
                }
                items.push(pull);
            }
        }
        if has_duplicates(items.iter().map(|pull| pull.number)) {
            return Err(GithubError::InvalidResponse);
        }
        Ok(page_result(items, next_cursor))
    }

    async fn exact_ref(
        &self,
        repository: &GitHubRepository,
        branch: &str,
    ) -> Result<String, GithubError> {
        let document = self
            .request(
                Method::GET,
                &[
                    "repos",
                    repository.owner(),
                    repository.name(),
                    "git",
                    "ref",
                    &format!("heads/{branch}"),
                ],
                &[],
                None,
            )
            .await?;
        let raw: RawRef = decode(document.value)?;
        if raw.reference != format!("refs/heads/{branch}")
            || raw.object.kind != "commit"
            || !valid_sha(&raw.object.sha)
        {
            return Err(GithubError::InvalidResponse);
        }
        Ok(raw.object.sha)
    }

    async fn verify_integration(
        &self,
        repository: &GitHubRepository,
        branch: &str,
        expected_head_sha: &str,
        pull_request_number: u64,
        integration_branch: Option<&str>,
    ) -> Result<GithubIntegrationProof, GithubError> {
        if !valid_branch(branch)
            || !valid_sha(expected_head_sha)
            || pull_request_number == 0
            || integration_branch.is_some_and(|value| !valid_branch(value))
        {
            return Err(GithubError::InvalidInput);
        }
        let info = self.repository(repository).await?;
        let target = integration_branch.unwrap_or(&info.default_branch);
        if branch == info.default_branch || branch == target {
            return Err(GithubError::ProtectedBranch);
        }
        let feature = self.branch(repository, branch).await?;
        if feature.protected {
            return Err(GithubError::ProtectedBranch);
        }
        if feature.head_sha != expected_head_sha {
            return Err(GithubError::Conflict);
        }
        let document = self
            .request(
                Method::GET,
                &[
                    "repos",
                    repository.owner(),
                    repository.name(),
                    "pulls",
                    &pull_request_number.to_string(),
                ],
                &[],
                None,
            )
            .await?;
        let raw: RawPullRequest = decode(document.value)?;
        if raw.merged != Some(true) {
            return Err(GithubError::MergeNotProven);
        }
        let pull = raw
            .into_pull(repository)?
            .ok_or(GithubError::MergeNotProven)?;
        if pull.number != pull_request_number
            || !pull.merged
            || pull.state != "closed"
            || pull.head_ref != branch
            || pull.head_sha != expected_head_sha
            || pull.base_ref != target
        {
            return Err(GithubError::MergeNotProven);
        }
        let merge_commit_sha = pull.merge_commit_sha.ok_or(GithubError::MergeNotProven)?;
        let integration = self.branch(repository, target).await?;
        let comparison = format!("{merge_commit_sha}...{}", integration.head_sha);
        let document = self
            .request(
                Method::GET,
                &[
                    "repos",
                    repository.owner(),
                    repository.name(),
                    "compare",
                    &comparison,
                ],
                &[("per_page", "1".into()), ("page", "1".into())],
                None,
            )
            .await?;
        let compare: RawCompare = decode(document.value)?;
        let contained = compare.base_commit.sha == merge_commit_sha
            && compare.merge_base_commit.sha == merge_commit_sha
            && compare.behind_by == 0
            && ((compare.status == "identical"
                && compare.ahead_by == 0
                && integration.head_sha == merge_commit_sha)
                || (compare.status == "ahead"
                    && compare.ahead_by > 0
                    && integration.head_sha != merge_commit_sha));
        if !contained {
            return Err(GithubError::MergeNotProven);
        }
        if self.exact_ref(repository, branch).await? != expected_head_sha
            || self.exact_ref(repository, target).await? != integration.head_sha
        {
            return Err(GithubError::Conflict);
        }
        Ok(GithubIntegrationProof {
            repository: repository.clone(),
            repository_node_id: info.node_id,
            branch_name: branch.into(),
            head_sha: expected_head_sha.into(),
            integration_branch: target.into(),
            integration_head_sha: integration.head_sha,
            merge_commit_sha,
            pull_request_number,
            observed_at: timestamp(),
        })
    }

    async fn delete_verified(
        &self,
        proof: &GithubIntegrationProof,
        mutation_id: &str,
    ) -> Result<(), GithubError> {
        if !proof.is_fresh() {
            return Err(GithubError::MergeNotProven);
        }
        // updateRefs compares beforeOid atomically. The no-op target update also
        // rejects a target force-push between the merge proof and deletion.
        let body = json!({"query":"mutation RetireExplorerBranch($input: UpdateRefsInput!) { updateRefs(input: $input) { clientMutationId } }", "variables":{"input":{
        "repositoryId":proof.repository_node_id,"clientMutationId":mutation_id,"refUpdates":[
            {"name":format!("refs/heads/{}",proof.branch_name),"beforeOid":proof.head_sha,"afterOid":ZERO_OID,"force":false},
            {"name":format!("refs/heads/{}",proof.integration_branch),"beforeOid":proof.integration_head_sha,"afterOid":proof.integration_head_sha,"force":false}
        ]}}});
        let document = self
            .request(Method::POST, &["graphql"], &[], Some(body))
            .await
            .map_err(|error| match error {
                GithubError::Timeout
                | GithubError::Unavailable
                | GithubError::InvalidResponse
                | GithubError::ResponseTooLarge => GithubError::DeletionUnverified,
                other => other,
            })?;
        if let Some(errors) = document.value.get("errors") {
            let errors = errors
                .as_array()
                .filter(|errors| !errors.is_empty())
                .ok_or(GithubError::DeletionUnverified)?;
            let kind = errors
                .iter()
                .filter_map(|error| error.get("type").and_then(Value::as_str))
                .collect::<Vec<_>>();
            if kind.contains(&"RATE_LIMITED") {
                self.client
                    .rate_limited_until
                    .fetch_max(epoch_seconds().saturating_add(60), Ordering::Relaxed);
                return Err(GithubError::RateLimited {
                    retry_after_seconds: 60,
                });
            }
            if kind.contains(&"FORBIDDEN") {
                return Err(GithubError::Forbidden);
            }
            return Err(GithubError::Conflict);
        }
        if document
            .value
            .pointer("/data/updateRefs/clientMutationId")
            .and_then(Value::as_str)
            != Some(mutation_id)
        {
            return Err(GithubError::DeletionUnverified);
        }
        let repository = self
            .repository(&proof.repository)
            .await
            .map_err(|_| GithubError::DeletionUnverified)?;
        if repository.node_id != proof.repository_node_id {
            return Err(GithubError::DeletionUnverified);
        }
        if self
            .exact_ref(&proof.repository, &proof.integration_branch)
            .await
            .map_err(|_| GithubError::DeletionUnverified)?
            != proof.integration_head_sha
        {
            return Err(GithubError::DeletionUnverified);
        }
        match self.exact_ref(&proof.repository, &proof.branch_name).await {
            Err(GithubError::NotFound) => Ok(()),
            _ => Err(GithubError::DeletionUnverified),
        }
    }
}

#[derive(Deserialize)]
struct RawOwner {
    login: String,
}
#[derive(Deserialize)]
struct RawRepository {
    owner: RawOwner,
    name: String,
    default_branch: String,
    node_id: String,
}
#[derive(Deserialize)]
struct RawSha {
    sha: String,
}
#[derive(Deserialize)]
struct RawBranch {
    name: String,
    commit: RawSha,
    protected: bool,
}
impl RawBranch {
    fn into_branch(self, repository: &GitHubRepository) -> Result<GithubBranch, GithubError> {
        if !valid_branch(&self.name) || !valid_sha(&self.commit.sha) {
            return Err(GithubError::InvalidResponse);
        }
        Ok(GithubBranch {
            html_url: repository.browser_url(&["tree", &self.name]),
            name: self.name,
            head_sha: self.commit.sha,
            protected: self.protected,
        })
    }
}
#[derive(Deserialize)]
struct RawCommitAuthor {
    name: String,
    date: String,
}
#[derive(Deserialize)]
struct RawCommitBody {
    author: RawCommitAuthor,
    message: String,
}
#[derive(Deserialize)]
struct RawCommit {
    sha: String,
    parents: Vec<RawSha>,
    commit: RawCommitBody,
    author: Option<RawOwner>,
}
#[derive(Deserialize)]
struct RawPullRepository {
    name: String,
    owner: RawOwner,
}
#[derive(Deserialize)]
struct RawPullRef {
    #[serde(rename = "ref")]
    reference: String,
    sha: String,
    repo: Option<RawPullRepository>,
}
#[derive(Deserialize)]
struct RawPullRequest {
    number: u64,
    state: String,
    head: RawPullRef,
    base: RawPullRef,
    merged: Option<bool>,
    merged_at: Option<String>,
    merge_commit_sha: Option<String>,
}
impl RawPullRequest {
    fn into_pull(
        self,
        repository: &GitHubRepository,
    ) -> Result<Option<GithubPullRequest>, GithubError> {
        let matching = |value: &Option<RawPullRepository>| {
            value
                .as_ref()
                .is_some_and(|repo| repository.matches(&repo.owner.login, &repo.name))
        };
        if !matching(&self.head.repo) || !matching(&self.base.repo) {
            return Ok(None);
        }
        if self.number == 0
            || !matches!(self.state.as_str(), "open" | "closed")
            || !valid_branch(&self.head.reference)
            || !valid_branch(&self.base.reference)
            || !valid_sha(&self.head.sha)
            || !valid_sha(&self.base.sha)
            || self
                .merge_commit_sha
                .as_ref()
                .is_some_and(|sha| !valid_sha(sha))
            || self
                .merged_at
                .as_ref()
                .is_some_and(|date| !valid_timestamp(date))
            || (self.merged_at.is_some() && (self.state != "closed" || self.merged == Some(false)))
        {
            return Err(GithubError::InvalidResponse);
        }
        Ok(Some(GithubPullRequest {
            number: self.number,
            html_url: repository.browser_url(&["pull", &self.number.to_string()]),
            state: self.state,
            merged: self.merged_at.is_some(),
            head_ref: self.head.reference,
            head_sha: self.head.sha,
            base_ref: self.base.reference,
            merged_at: self.merged_at,
            merge_commit_sha: self.merge_commit_sha,
        }))
    }
}
#[derive(Deserialize)]
struct RawRefObject {
    sha: String,
    #[serde(rename = "type")]
    kind: String,
}
#[derive(Deserialize)]
struct RawRef {
    #[serde(rename = "ref")]
    reference: String,
    object: RawRefObject,
}
#[derive(Deserialize)]
struct RawCompare {
    status: String,
    ahead_by: u64,
    behind_by: u64,
    base_commit: RawSha,
    merge_base_commit: RawSha,
}

fn decode<T: for<'de> Deserialize<'de>>(value: Value) -> Result<T, GithubError> {
    serde_json::from_value(value).map_err(|_| GithubError::InvalidResponse)
}

fn page_query(
    cursor: Option<&str>,
    page_size: u32,
) -> Result<(u32, Vec<(&'static str, String)>), GithubError> {
    if !(1..=100).contains(&page_size) {
        return Err(GithubError::InvalidInput);
    }
    let page = match cursor {
        Some(value) if !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit()) => {
            value
                .parse::<u32>()
                .map_err(|_| GithubError::InvalidInput)?
        }
        Some(_) => return Err(GithubError::InvalidInput),
        None => 1,
    };
    if !(1..=MAX_PAGE).contains(&page) {
        return Err(GithubError::InvalidInput);
    }
    Ok((
        page,
        vec![
            ("per_page", page_size.to_string()),
            ("page", page.to_string()),
        ],
    ))
}

fn next_cursor(
    headers: &HeaderMap,
    current: &Url,
    page: u32,
    page_size: u32,
    count: usize,
) -> Result<Option<String>, GithubError> {
    if count > page_size as usize {
        return Err(GithubError::InvalidResponse);
    }
    let mut next = None;
    let mut any_link = false;
    for header in headers.get_all(LINK) {
        any_link = true;
        let header = header.to_str().map_err(|_| GithubError::InvalidResponse)?;
        if header.len() > 8192 {
            return Err(GithubError::InvalidResponse);
        }
        for entry in header.split(',') {
            let (address, attributes) = entry
                .trim()
                .split_once('>')
                .ok_or(GithubError::InvalidResponse)?;
            let address = address
                .strip_prefix('<')
                .ok_or(GithubError::InvalidResponse)?;
            let target = Url::parse(address).map_err(|_| GithubError::InvalidResponse)?;
            if target.origin() != current.origin()
                || target.path() != current.path()
                || !target.username().is_empty()
                || target.password().is_some()
                || target.fragment().is_some()
            {
                return Err(GithubError::InvalidResponse);
            }
            let mut target_query = target
                .query_pairs()
                .map(|(key, value)| (key.into_owned(), value.into_owned()))
                .collect::<Vec<_>>();
            let mut current_query = current
                .query_pairs()
                .map(|(key, value)| (key.into_owned(), value.into_owned()))
                .collect::<Vec<_>>();
            let pages = target_query
                .iter()
                .filter(|(key, _)| key == "page")
                .map(|(_, value)| value.as_str())
                .collect::<Vec<_>>();
            if pages.len() != 1 {
                return Err(GithubError::InvalidResponse);
            }
            let target_page = pages[0]
                .parse::<u32>()
                .map_err(|_| GithubError::InvalidResponse)?;
            if target_page == 0 {
                return Err(GithubError::InvalidResponse);
            }
            target_query.retain(|(key, _)| key != "page");
            current_query.retain(|(key, _)| key != "page");
            target_query.sort();
            current_query.sort();
            if target_query != current_query {
                return Err(GithubError::InvalidResponse);
            }
            let relations = attributes
                .split(';')
                .filter_map(|attribute| attribute.trim().strip_prefix("rel="))
                .collect::<Vec<_>>();
            if relations.len() != 1
                || !matches!(
                    relations[0],
                    "\"next\"" | "\"prev\"" | "\"first\"" | "\"last\""
                )
            {
                return Err(GithubError::InvalidResponse);
            }
            if relations[0] == "\"next\"" {
                if next.is_some()
                    || target_page != page.saturating_add(1)
                    || target_page > MAX_PAGE
                    || count == 0
                {
                    return Err(GithubError::InvalidResponse);
                }
                next = Some(target_page.to_string());
            }
        }
    }
    // An exactly full page with no pagination metadata cannot prove completion.
    if !any_link && count == page_size as usize {
        if page >= MAX_PAGE {
            return Err(GithubError::InvalidResponse);
        }
        next = Some((page + 1).to_string());
    }
    Ok(next)
}

fn page_result<T>(items: Vec<T>, next_cursor: Option<String>) -> GithubPage<T> {
    GithubPage {
        complete: next_cursor.is_none(),
        items,
        next_cursor,
        fetched_at: timestamp(),
    }
}

fn valid_owner(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 39
        && !value.starts_with('-')
        && !value.ends_with('-')
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
}

fn valid_repository_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 100
        && !matches!(value, "." | "..")
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

fn valid_branch(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 1024
        && value != "@"
        && !value.starts_with('-')
        && !value.ends_with('.')
        && !value.contains("..")
        && !value.contains("@{")
        && !value.chars().any(|character| {
            character.is_control()
                || character.is_whitespace()
                || matches!(character, '\\' | ':' | '?' | '*' | '[' | '^' | '~')
        })
        && value
            .split('/')
            .all(|part| !part.is_empty() && !part.starts_with('.') && !part.ends_with(".lock"))
}

fn valid_sha(value: &str) -> bool {
    value.len() == 40
        && value != ZERO_OID
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn valid_node_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'+' | b'/' | b'=')
        })
}

fn valid_timestamp(value: &str) -> bool {
    value.len() <= 64 && OffsetDateTime::parse(value, &Rfc3339).is_ok()
}

fn display_text(value: &str, maximum: usize) -> String {
    let mut characters = value.chars().filter(|character| !character.is_control());
    let mut text = characters.by_ref().take(maximum).collect::<String>();
    if characters.next().is_some() {
        text.push('…');
    }
    text
}

fn display_author(value: &str) -> String {
    if value.contains('@') || value.contains('<') || value.contains('>') || value.trim().is_empty()
    {
        "Unknown author".into()
    } else {
        display_text(value, 160)
    }
}

fn has_duplicates<T: Eq + std::hash::Hash>(values: impl Iterator<Item = T>) -> bool {
    let mut seen = std::collections::HashSet::new();
    values.into_iter().any(|value| !seen.insert(value))
}

fn http_error(error: reqwest::Error) -> GithubError {
    if error.is_timeout() {
        GithubError::Timeout
    } else {
        GithubError::Unavailable
    }
}

fn header_u64(headers: &HeaderMap, name: &str) -> Option<u64> {
    headers.get(name)?.to_str().ok()?.parse().ok()
}

fn retry_delay(headers: &HeaderMap) -> u64 {
    let now = epoch_seconds();
    let retry_after = headers.get(RETRY_AFTER).and_then(|value| value.to_str().ok()).and_then(|value| {
        value.parse::<u64>().ok().or_else(|| {
            // HTTP-date uses GMT and the IMF-fixdate spelling. Keep parsing
            // bounded and fail conservatively to the normal backoff.
            let format = time::macros::format_description!("[weekday repr:short], [day] [month repr:short] [year] [hour]:[minute]:[second] GMT");
            time::PrimitiveDateTime::parse(value, &format).ok().and_then(|date| u64::try_from(date.assume_utc().unix_timestamp()).ok()).map(|deadline| deadline.saturating_sub(now))
        })
    });
    let reset =
        header_u64(headers, "x-ratelimit-reset").map(|deadline| deadline.saturating_sub(now));
    match (retry_after, reset) {
        (Some(retry), Some(reset)) => retry.max(reset),
        (Some(retry), _) => retry,
        (_, Some(reset)) if reset > 0 => reset,
        _ => 60,
    }
}

fn read_private_token(path: &Path) -> Result<String, GithubError> {
    if !path.is_absolute() {
        return Err(GithubError::UnsafeCredential);
    }
    let mut components = path.components();
    if components.next() != Some(Component::RootDir) {
        return Err(GithubError::UnsafeCredential);
    }
    let mut parts = components.peekable();
    let mut directory = open(
        "/",
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|_| GithubError::UnsafeCredential)?;
    let descriptor = loop {
        let Some(Component::Normal(part)) = parts.next() else {
            return Err(GithubError::UnsafeCredential);
        };
        if parts.peek().is_some() {
            directory = openat(
                &directory,
                part,
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::empty(),
            )
            .map_err(|error| {
                if error == rustix::io::Errno::NOENT {
                    GithubError::MissingCredential
                } else {
                    GithubError::UnsafeCredential
                }
            })?;
        } else {
            break openat(
                &directory,
                part,
                OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
                Mode::empty(),
            )
            .map_err(|error| {
                if error == rustix::io::Errno::NOENT {
                    GithubError::MissingCredential
                } else {
                    GithubError::UnsafeCredential
                }
            })?;
        }
    };
    let before = fstat(&descriptor).map_err(|_| GithubError::UnsafeCredential)?;
    let effective_uid = rustix::process::geteuid().as_raw();
    if before.st_mode & libc::S_IFMT != libc::S_IFREG
        || before.st_size <= 0
        || before.st_size as u64 > MAX_TOKEN_BYTES
        || before.st_nlink != 1
        || before.st_mode & 0o777 != 0o600
        || (before.st_uid != 0 && before.st_uid != effective_uid)
    {
        return Err(GithubError::UnsafeCredential);
    }
    let mut file = std::fs::File::from(descriptor);
    let mut bytes = Vec::with_capacity(before.st_size as usize);
    (&mut file)
        .take(MAX_TOKEN_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| GithubError::UnsafeCredential)?;
    let after = file.metadata().map_err(|_| GithubError::UnsafeCredential)?;
    if after.dev() != before.st_dev as u64
        || after.ino() != before.st_ino
        || after.size() != before.st_size as u64
        || after.mtime() != before.st_mtime
        || after.mtime_nsec() != before.st_mtime_nsec as i64
        || after.ctime() != before.st_ctime
        || after.ctime_nsec() != before.st_ctime_nsec as i64
        || after.mode() & 0o777 != 0o600
        || after.uid() != before.st_uid
        || after.nlink() != 1
        || bytes.len() as u64 != before.st_size as u64
    {
        return Err(GithubError::UnsafeCredential);
    }
    let token = std::str::from_utf8(&bytes)
        .map_err(|_| GithubError::UnsafeCredential)?
        .trim();
    if token.is_empty()
        || !token
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
    {
        return Err(GithubError::UnsafeCredential);
    }
    Ok(token.into())
}

fn mutation_id() -> Result<String, GithubError> {
    let mut random = [0_u8; 16];
    getrandom::fill(&mut random).map_err(|_| GithubError::Unavailable)?;
    Ok(format!(
        "dc2-gh-{}",
        random
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    ))
}

fn epoch_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
fn timestamp() -> String {
    OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_default()
}

#[cfg(test)]
#[path = "github/tests.rs"]
mod tests;
