//! Where the data comes from: each Helm repository's `index.yaml` (which app
//! version every chart version ships, and the chart's own changes), the
//! app's and the chart's GitHub releases, and Artifact Hub for finding which
//! Helm repository a chart is in.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use jiff::Timestamp;
use reqwest::header::{self, HeaderMap, HeaderValue};
use reqwest::{Response, StatusCode};
use serde::Deserialize;
use url::Url;

use crate::cache::{Cache, Cached};
use crate::metrics::Metrics;

pub const GITHUB_API: &str = "https://api.github.com/";
pub const ARTIFACT_HUB: &str = "https://artifacthub.io/";

/// Releases come newest-created first, with backports to older lines mixed
/// in; this many pages of 100 reach well past any bump Renovate would offer.
const RELEASE_PAGES: u32 = 3;

#[derive(Debug, Clone, thiserror::Error)]
pub enum UpstreamError {
    #[error("{url} answered {status}")]
    Status { url: String, status: u16 },
    #[error(
        "GitHub's rate limit is used up{}; a token raises it (github.existingSecret in the Helm chart)",
        reset.map(|t| format!(" until {}", t.strftime("%H:%M UTC"))).unwrap_or_default()
    )]
    RateLimited { reset: Option<Timestamp> },
    #[error("fetching {url}: {message}")]
    Request { url: String, message: String },
    #[error("reading {url}: {message}")]
    Parse { url: String, message: String },
}

/// One chart version in a Helm repository.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ChartVersion {
    pub version: String,
    /// Absent when the chart doesn't declare one.
    pub app_version: Option<String>,
    pub created: Option<Timestamp>,
    pub home: Option<String>,
    pub sources: Vec<String>,
    /// The first download URL.
    pub url: Option<String>,
    /// The raw `artifacthub.io/changes` annotation.
    pub changes: Option<String>,
}

/// One chart's versions in a Helm repository's index, in the index's order
/// (newest first, by convention).
#[derive(Debug, Default)]
pub struct ChartIndex {
    pub versions: Vec<ChartVersion>,
    etag: Option<String>,
}

impl ChartIndex {
    pub fn new(versions: Vec<ChartVersion>) -> Self {
        ChartIndex {
            versions,
            etag: None,
        }
    }

    /// A chart version, with or without its "v". A `+` (build metadata)
    /// reaches us as a space when the PR template doesn't encode it.
    pub fn find(&self, version: &str) -> Option<&ChartVersion> {
        let bare = |v: &str| v.strip_prefix('v').unwrap_or(v).replace(' ', "+");
        let wanted = bare(version);
        self.versions.iter().find(|v| bare(&v.version) == wanted)
    }

    /// The source URL Renovate shows for this chart (its `{{sourceUrl}}`).
    pub fn renovate_source_url(&self) -> Option<String> {
        self.versions
            .first()
            .and_then(crate::resolve::renovate_source_url)
    }
}

/// A Helm repository Artifact Hub lists a chart of the searched name in.
#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    /// The repository's URL, serving `index.yaml`.
    pub repo: Url,
    /// Artifact Hub's name for the repository.
    pub name: String,
    pub display_name: String,
    pub official: bool,
    pub verified: bool,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Release {
    pub tag_name: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub prerelease: bool,
    #[serde(default)]
    pub draft: bool,
    pub html_url: String,
    #[serde(default)]
    pub published_at: Option<Timestamp>,
}

/// An app's releases, as GitHub lists them (newest created first).
#[derive(Debug, Default)]
pub struct Releases {
    pub items: Vec<Release>,
    etag: Option<String>,
}

impl Releases {
    pub fn new(items: Vec<Release>) -> Self {
        Releases { items, etag: None }
    }
}

/// One release's notes, as GitHub renders them, sanitized.
#[derive(Debug, Default)]
pub struct ReleaseNotes {
    /// None when the release has no notes.
    pub html: Option<String>,
    etag: Option<String>,
}

impl ReleaseNotes {
    /// Sanitizes `html`, resolving relative links against `base`.
    pub fn new(html: Option<&str>, base: &str) -> Self {
        ReleaseNotes {
            html: html.and_then(|h| crate::notes::sanitize(h, base)),
            etag: None,
        }
    }
}

#[derive(Deserialize)]
struct RawNotes {
    #[serde(default)]
    body_html: Option<String>,
    html_url: String,
}

#[async_trait]
pub trait Sources: Send + Sync {
    /// `chart`'s versions in the Helm repository at `repo`.
    async fn index(&self, repo: &Url, chart: &str) -> Result<Cached<ChartIndex>, UpstreamError>;
    /// The Helm repositories Artifact Hub lists a chart named `chart` in.
    async fn search(&self, chart: &str) -> Result<Cached<Vec<Candidate>>, UpstreamError>;
    async fn releases(&self, app: &str) -> Result<Cached<Releases>, UpstreamError>;
    /// The notes of the release tagged `tag`.
    async fn notes(&self, app: &str, tag: &str) -> Result<Cached<ReleaseNotes>, UpstreamError>;
}

/// The real thing, over HTTP, cached.
pub struct HttpSources {
    http: reqwest::Client,
    github_api: Url,
    github_token: Option<String>,
    artifact_hub: Url,
    indexes: Cache<ChartIndex>,
    searches: Cache<Vec<Candidate>>,
    releases: Cache<Releases>,
    notes: Cache<ReleaseNotes>,
    metrics: Arc<Metrics>,
}

impl HttpSources {
    pub fn new(
        github_api: Url,
        github_token: Option<String>,
        artifact_hub: Url,
        ttl: Duration,
        metrics: Arc<Metrics>,
    ) -> anyhow::Result<Self> {
        let http = reqwest::Client::builder()
            .user_agent(concat!("chart-notes/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(60))
            .build()?;
        Ok(HttpSources {
            http,
            github_api,
            github_token: github_token.filter(|t| !t.trim().is_empty()),
            artifact_hub,
            // Keyed by repository and chart: drop charts nobody asks about.
            indexes: Cache::new(ttl).forgetting_after(Duration::from_secs(7 * 24 * 3600)),
            // Which repositories carry a chart changes rarely.
            searches: Cache::new(ttl * 36).forgetting_after(Duration::from_secs(7 * 24 * 3600)),
            releases: Cache::new(ttl),
            // One entry per release looked at: drop the ones nobody reads.
            notes: Cache::new(ttl).forgetting_after(Duration::from_secs(7 * 24 * 3600)),
            metrics,
        })
    }

    async fn send(
        &self,
        upstream: &str,
        url: &str,
        headers: HeaderMap,
    ) -> Result<Response, UpstreamError> {
        let res = self
            .http
            .get(url)
            .headers(headers)
            .send()
            .await
            .map_err(|e| {
                self.metrics.upstream(upstream, "error");
                UpstreamError::Request {
                    url: url.to_string(),
                    message: error_chain(&e),
                }
            })?;
        if upstream == "github" {
            self.track_rate_limit(res.headers());
        }
        let status = res.status();
        let result = match status {
            StatusCode::NOT_MODIFIED => "not_modified",
            s if s.is_success() => "ok",
            StatusCode::FORBIDDEN | StatusCode::TOO_MANY_REQUESTS
                if upstream == "github" && rate_limited(res.headers()) =>
            {
                self.metrics.upstream(upstream, "rate_limited");
                return Err(UpstreamError::RateLimited {
                    reset: header_i64(res.headers(), "x-ratelimit-reset")
                        .and_then(|s| Timestamp::from_second(s).ok()),
                });
            }
            _ => "error",
        };
        self.metrics.upstream(upstream, result);
        if result == "error" {
            return Err(UpstreamError::Status {
                url: url.to_string(),
                status: status.as_u16(),
            });
        }
        Ok(res)
    }

    fn track_rate_limit(&self, headers: &HeaderMap) {
        if let Some(n) = header_i64(headers, "x-ratelimit-remaining") {
            self.metrics.github_rate_limit_remaining.set(n);
        }
        if let Some(t) = header_i64(headers, "x-ratelimit-reset") {
            self.metrics.github_rate_limit_reset.set(t);
        }
    }

    async fn fetch_index(
        &self,
        url: &str,
        chart: &str,
        previous: Option<Arc<ChartIndex>>,
    ) -> Result<ChartIndex, UpstreamError> {
        let mut headers = HeaderMap::new();
        if let Some(etag) = previous.as_ref().and_then(|p| p.etag.as_deref())
            && let Ok(v) = HeaderValue::from_str(etag)
        {
            headers.insert(header::IF_NONE_MATCH, v);
        }
        let res = self.send("helm", url, headers).await?;
        if res.status() == StatusCode::NOT_MODIFIED
            && let Some(previous) = previous
        {
            return Ok(ChartIndex {
                versions: previous.versions.clone(),
                etag: previous.etag.clone(),
            });
        }
        let etag = etag(res.headers());
        let body = res.bytes().await.map_err(|e| UpstreamError::Request {
            url: url.to_string(),
            message: error_chain(&e),
        })?;
        // Some indexes run to tens of megabytes: parse off the async runtime,
        // and keep only the chart asked for.
        let chart = chart.to_string();
        let versions = tokio::task::spawn_blocking(move || parse_index(&body, &chart))
            .await
            .map_err(|e| UpstreamError::Parse {
                url: url.to_string(),
                message: e.to_string(),
            })?
            .map_err(|e| UpstreamError::Parse {
                url: url.to_string(),
                message: e.to_string(),
            })?;
        Ok(ChartIndex { versions, etag })
    }

    fn github_headers(&self, accept: &'static str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(header::ACCEPT, HeaderValue::from_static(accept));
        headers.insert(
            "x-github-api-version",
            HeaderValue::from_static("2022-11-28"),
        );
        if let Some(token) = &self.github_token
            && let Ok(mut v) = HeaderValue::from_str(&format!("Bearer {}", token.trim()))
        {
            v.set_sensitive(true);
            headers.insert(header::AUTHORIZATION, v);
        }
        headers
    }

    async fn fetch_releases(
        &self,
        app: &str,
        previous: Option<Arc<Releases>>,
    ) -> Result<Releases, UpstreamError> {
        let mut items = Vec::new();
        let mut first_etag = None;
        for page in 1..=RELEASE_PAGES {
            let url = self
                .github_api
                .join(&format!("repos/{app}/releases?per_page=100&page={page}"))
                .map_err(|e| UpstreamError::Request {
                    url: app.to_string(),
                    message: e.to_string(),
                })?
                .to_string();
            let mut headers = self.github_headers("application/vnd.github+json");
            // Conditional requests that come back 304 don't count against
            // the rate limit. Only the first page is asked: when it hasn't
            // changed, no release was added or edited lately.
            if page == 1
                && let Some(etag) = previous.as_ref().and_then(|p| p.etag.as_deref())
                && let Ok(v) = HeaderValue::from_str(etag)
            {
                headers.insert(header::IF_NONE_MATCH, v);
            }
            let res = self.send("github", &url, headers).await?;
            if res.status() == StatusCode::NOT_MODIFIED
                && let Some(previous) = &previous
            {
                return Ok(Releases {
                    items: previous.items.clone(),
                    etag: previous.etag.clone(),
                });
            }
            if page == 1 {
                first_etag = etag(res.headers());
            }
            let batch: Vec<Release> = res.json().await.map_err(|e| UpstreamError::Parse {
                url: url.clone(),
                message: error_chain(&e),
            })?;
            let last = batch.len() < 100;
            items.extend(batch);
            if last {
                break;
            }
        }
        Ok(Releases {
            items,
            etag: first_etag,
        })
    }

    async fn fetch_notes(
        &self,
        app: &str,
        tag: &str,
        previous: Option<Arc<ReleaseNotes>>,
    ) -> Result<ReleaseNotes, UpstreamError> {
        let mut url = self
            .github_api
            .join(&format!("repos/{app}/releases/tags/"))
            .map_err(|e| UpstreamError::Request {
                url: app.to_string(),
                message: e.to_string(),
            })?;
        // Tags can hold `/`: one path segment, encoded.
        url.path_segments_mut()
            .map_err(|_| UpstreamError::Request {
                url: app.to_string(),
                message: "GitHub API URL cannot have a path".into(),
            })?
            .pop_if_empty()
            .push(tag);
        let url = url.to_string();
        // GitHub renders the notes (links to issues, mentions, commits).
        let mut headers = self.github_headers("application/vnd.github.html+json");
        if let Some(etag) = previous.as_ref().and_then(|p| p.etag.as_deref())
            && let Ok(v) = HeaderValue::from_str(etag)
        {
            headers.insert(header::IF_NONE_MATCH, v);
        }
        let res = self.send("github", &url, headers).await?;
        if res.status() == StatusCode::NOT_MODIFIED
            && let Some(previous) = previous
        {
            return Ok(ReleaseNotes {
                html: previous.html.clone(),
                etag: previous.etag.clone(),
            });
        }
        let etag = etag(res.headers());
        let raw: RawNotes = res.json().await.map_err(|e| UpstreamError::Parse {
            url: url.clone(),
            message: error_chain(&e),
        })?;
        Ok(ReleaseNotes {
            etag,
            ..ReleaseNotes::new(raw.body_html.as_deref(), &raw.html_url)
        })
    }
}

impl HttpSources {
    async fn fetch_search(&self, chart: &str) -> Result<Vec<Candidate>, UpstreamError> {
        let mut url = self
            .artifact_hub
            .join("api/v1/packages/search")
            .map_err(|e| UpstreamError::Request {
                url: self.artifact_hub.to_string(),
                message: e.to_string(),
            })?;
        // kind 0: Helm charts.
        url.query_pairs_mut()
            .append_pair("ts_query_web", chart)
            .append_pair("kind", "0")
            .append_pair("limit", "60");
        let url = url.to_string();
        let mut headers = HeaderMap::new();
        headers.insert(header::ACCEPT, HeaderValue::from_static("application/json"));
        let res = self.send("artifacthub", &url, headers).await?;
        let raw: RawSearch = res.json().await.map_err(|e| UpstreamError::Parse {
            url: url.clone(),
            message: error_chain(&e),
        })?;
        Ok(candidates(raw, chart))
    }
}

#[derive(Deserialize)]
struct RawSearch {
    #[serde(default)]
    packages: Vec<RawPackage>,
}

#[derive(Deserialize)]
struct RawPackage {
    name: String,
    repository: RawRepository,
}

#[derive(Deserialize)]
struct RawRepository {
    url: String,
    name: String,
    #[serde(default)]
    display_name: Option<String>,
    #[serde(default)]
    official: bool,
    #[serde(default)]
    verified_publisher: bool,
}

/// The search's exact-name matches in plain http(s) Helm repositories
/// (OCI ones are out of scope), official and verified ones first.
fn candidates(raw: RawSearch, chart: &str) -> Vec<Candidate> {
    let mut found: Vec<Candidate> = raw
        .packages
        .into_iter()
        .filter(|p| p.name == chart)
        .filter_map(|p| {
            let repo = Url::parse(&p.repository.url).ok()?;
            matches!(repo.scheme(), "https" | "http").then(|| Candidate {
                repo,
                display_name: p
                    .repository
                    .display_name
                    .filter(|d| !d.trim().is_empty())
                    .unwrap_or_else(|| p.repository.name.clone()),
                name: p.repository.name,
                official: p.repository.official,
                verified: p.repository.verified_publisher,
            })
        })
        .collect();
    found.sort_by_key(|c| (!c.official, !c.verified));
    found.dedup_by(|a, b| a.repo == b.repo);
    found
}

#[async_trait]
impl Sources for HttpSources {
    async fn index(&self, repo: &Url, chart: &str) -> Result<Cached<ChartIndex>, UpstreamError> {
        let url = index_url(repo);
        self.indexes
            .get(&format!("{url}#{chart}"), |previous| {
                self.fetch_index(&url, chart, previous)
            })
            .await
    }

    async fn search(&self, chart: &str) -> Result<Cached<Vec<Candidate>>, UpstreamError> {
        self.searches.get(chart, |_| self.fetch_search(chart)).await
    }

    async fn releases(&self, app: &str) -> Result<Cached<Releases>, UpstreamError> {
        self.releases
            .get(app, |previous| self.fetch_releases(app, previous))
            .await
    }

    async fn notes(&self, app: &str, tag: &str) -> Result<Cached<ReleaseNotes>, UpstreamError> {
        self.notes
            .get(&format!("{app}@{tag}"), |previous| {
                self.fetch_notes(app, tag, previous)
            })
            .await
    }
}

/// Where a Helm repository serves its index.
pub fn index_url(repo: &Url) -> String {
    format!("{}/index.yaml", repo.as_str().trim_end_matches('/'))
}

fn header_i64(headers: &HeaderMap, name: &str) -> Option<i64> {
    headers.get(name)?.to_str().ok()?.trim().parse().ok()
}

fn etag(headers: &HeaderMap) -> Option<String> {
    Some(headers.get(header::ETAG)?.to_str().ok()?.to_string())
}

fn rate_limited(headers: &HeaderMap) -> bool {
    header_i64(headers, "x-ratelimit-remaining") == Some(0) || headers.contains_key("retry-after")
}

/// reqwest's error and its causes, which hold the useful part ("dns error",
/// "connection refused").
fn error_chain(e: &dyn std::error::Error) -> String {
    let mut out = e.to_string();
    let mut source = e.source();
    while let Some(s) = source {
        out.push_str(": ");
        out.push_str(&s.to_string());
        source = s.source();
    }
    out
}

#[derive(Deserialize)]
struct RawIndex {
    #[serde(default)]
    entries: HashMap<String, Vec<RawEntry>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawEntry {
    #[serde(default)]
    version: Option<serde_yaml::Value>,
    #[serde(default)]
    app_version: Option<serde_yaml::Value>,
    #[serde(default)]
    created: Option<String>,
    #[serde(default)]
    home: Option<serde_yaml::Value>,
    #[serde(default)]
    sources: Option<Vec<serde_yaml::Value>>,
    #[serde(default)]
    urls: Option<Vec<serde_yaml::Value>>,
    #[serde(default)]
    annotations: Option<HashMap<String, serde_yaml::Value>>,
}

/// An unquoted `appVersion: 2` comes through as a number.
fn scalar(v: Option<serde_yaml::Value>) -> Option<String> {
    match v? {
        serde_yaml::Value::String(s) => Some(s),
        serde_yaml::Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

/// `chart`'s versions in a Helm repository index.
pub fn parse_index(body: &[u8], chart: &str) -> Result<Vec<ChartVersion>, serde_yaml::Error> {
    let mut raw: RawIndex = serde_yaml::from_slice(body)?;
    let strings = |v: Option<Vec<serde_yaml::Value>>| -> Vec<String> {
        v.unwrap_or_default()
            .into_iter()
            .filter_map(|s| scalar(Some(s)))
            .collect()
    };
    Ok(raw
        .entries
        .remove(chart)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|e| {
            Some(ChartVersion {
                version: scalar(e.version)?,
                app_version: scalar(e.app_version),
                created: e.created.and_then(|c| c.parse().ok()),
                home: scalar(e.home),
                sources: strings(e.sources),
                url: strings(e.urls).into_iter().next(),
                changes: e
                    .annotations
                    .and_then(|mut a| a.remove("artifacthub.io/changes"))
                    .and_then(|c| scalar(Some(c))),
            })
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &[u8] = include_bytes!("../tests/fixtures/index.yaml");

    fn app_versions(chart: &str) -> Vec<(String, Option<String>)> {
        parse_index(FIXTURE, chart)
            .unwrap()
            .iter()
            .map(|v| (v.version.clone(), v.app_version.clone()))
            .collect()
    }

    fn pair(v: &str, app: &str) -> (String, Option<String>) {
        (v.into(), Some(app.into()))
    }

    #[test]
    fn reads_entry_keys_not_nested_ones() {
        // alloy's dependency also has a `version:`, one level deeper.
        assert_eq!(
            app_versions("alloy"),
            [pair("1.13.1", "v1.20.1"), pair("1.13.0", "v1.20.0")]
        );
    }

    #[test]
    fn quoted_and_unquoted_values() {
        assert_eq!(
            app_versions("pyroscope"),
            [pair("2.4.0", "2.4.0"), pair("2.3.1", "2.3.1")]
        );
        assert_eq!(app_versions("loki"), [pair("18.17.0", "3.7.8")]);
        let numbers = parse_index(
            b"entries:\n  x:\n  - version: 1.0.0\n    appVersion: 2\n",
            "x",
        )
        .unwrap();
        assert_eq!(numbers[0].app_version.as_deref(), Some("2"));
    }

    #[test]
    fn created_and_lookup() {
        let index = ChartIndex::new(parse_index(FIXTURE, "alloy").unwrap());
        let v = index.find("v1.13.1").unwrap();
        assert_eq!(v.created, Some("2026-10-08T17:41:54.542Z".parse().unwrap()));
        assert!(index.find("9.9.9").is_none());
        let plus =
            ChartIndex::new(parse_index(b"entries:\n  x:\n  - version: 1.0.0+up2\n", "x").unwrap());
        assert!(plus.find("1.0.0 up2").is_some());
        assert!(parse_index(FIXTURE, "tempo").unwrap().is_empty());
    }

    #[test]
    fn keeps_what_resolution_and_chart_notes_need() {
        let alloy = parse_index(FIXTURE, "alloy").unwrap();
        let v = &alloy[0];
        assert_eq!(v.home.as_deref(), Some("https://grafana.com/oss/alloy"));
        assert_eq!(
            v.sources,
            [
                "https://github.com/grafana/alloy",
                "https://github.com/grafana/helm-charts"
            ]
        );
        assert!(
            v.url
                .as_deref()
                .unwrap()
                .contains("/releases/download/alloy-1.13.1/")
        );
        assert!(v.changes.as_deref().unwrap().contains("extraPorts"));
        assert!(alloy[1].changes.is_none());
    }

    #[test]
    fn search_keeps_exact_http_matches_trusted_first() {
        let raw: RawSearch = serde_json::from_str(
            r#"{"packages": [
                {"name": "alloy-operator", "repository": {"url": "https://a.example", "name": "a"}},
                {"name": "alloy", "repository": {"url": "https://fork.example/charts", "name": "fork"}},
                {"name": "alloy", "repository": {"url": "oci://ghcr.io/x", "name": "oci"}},
                {"name": "alloy", "repository": {"url": "https://grafana.github.io/helm-charts", "name": "grafana",
                  "display_name": "Grafana", "verified_publisher": true, "official": true}}
            ]}"#,
        )
        .unwrap();
        let found = candidates(raw, "alloy");
        let names: Vec<_> = found.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["grafana", "fork"]);
        assert_eq!(found[0].display_name, "Grafana");
    }
}
