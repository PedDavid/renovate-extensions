//! Where the data comes from: each Helm repository's `index.yaml` (which app
//! version every chart version ships) and the app's GitHub releases.

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
use crate::charts::Chart;
use crate::metrics::Metrics;

pub const GITHUB_API: &str = "https://api.github.com/";

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
#[derive(Debug, Clone, PartialEq)]
pub struct ChartVersion {
    pub version: String,
    /// Absent when the chart doesn't declare one.
    pub app_version: Option<String>,
    pub created: Option<Timestamp>,
}

/// A Helm repository's index: every chart's versions, in the index's order.
#[derive(Debug, Default)]
pub struct HelmIndex {
    pub charts: HashMap<String, Vec<ChartVersion>>,
    etag: Option<String>,
}

impl HelmIndex {
    pub fn new(charts: HashMap<String, Vec<ChartVersion>>) -> Self {
        HelmIndex { charts, etag: None }
    }

    pub fn versions(&self, chart: &str) -> &[ChartVersion] {
        self.charts
            .get(chart)
            .map(Vec::as_slice)
            .unwrap_or_default()
    }

    /// A chart version, with or without its "v". A `+` (build metadata)
    /// reaches us as a space when the PR template doesn't encode it.
    pub fn find(&self, chart: &str, version: &str) -> Option<&ChartVersion> {
        let bare = |v: &str| v.strip_prefix('v').unwrap_or(v).replace(' ', "+");
        let wanted = bare(version);
        self.versions(chart)
            .iter()
            .find(|v| bare(&v.version) == wanted)
    }
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
    async fn index(&self, chart: &Chart) -> Result<Cached<HelmIndex>, UpstreamError>;
    async fn releases(&self, app: &str) -> Result<Cached<Releases>, UpstreamError>;
    /// The notes of the release tagged `tag`.
    async fn notes(&self, app: &str, tag: &str) -> Result<Cached<ReleaseNotes>, UpstreamError>;
}

/// The real thing, over HTTP, cached.
pub struct HttpSources {
    http: reqwest::Client,
    github_api: Url,
    github_token: Option<String>,
    indexes: Cache<HelmIndex>,
    releases: Cache<Releases>,
    notes: Cache<ReleaseNotes>,
    metrics: Arc<Metrics>,
}

impl HttpSources {
    pub fn new(
        github_api: Url,
        github_token: Option<String>,
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
            indexes: Cache::new(ttl),
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
        previous: Option<Arc<HelmIndex>>,
    ) -> Result<HelmIndex, UpstreamError> {
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
            return Ok(HelmIndex {
                charts: previous.charts.clone(),
                etag: previous.etag.clone(),
            });
        }
        let etag = etag(res.headers());
        let body = res.bytes().await.map_err(|e| UpstreamError::Request {
            url: url.to_string(),
            message: error_chain(&e),
        })?;
        // Some indexes run to tens of megabytes: parse off the async runtime.
        let charts = tokio::task::spawn_blocking(move || parse_index(&body))
            .await
            .map_err(|e| UpstreamError::Parse {
                url: url.to_string(),
                message: e.to_string(),
            })?
            .map_err(|e| UpstreamError::Parse {
                url: url.to_string(),
                message: e.to_string(),
            })?;
        Ok(HelmIndex { charts, etag })
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

#[async_trait]
impl Sources for HttpSources {
    async fn index(&self, chart: &Chart) -> Result<Cached<HelmIndex>, UpstreamError> {
        let url = chart.index_url();
        self.indexes
            .get(&url, |previous| self.fetch_index(&url, previous))
            .await
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
}

/// An unquoted `appVersion: 2` comes through as a number.
fn scalar(v: Option<serde_yaml::Value>) -> Option<String> {
    match v? {
        serde_yaml::Value::String(s) => Some(s),
        serde_yaml::Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

pub fn parse_index(body: &[u8]) -> Result<HashMap<String, Vec<ChartVersion>>, serde_yaml::Error> {
    let raw: RawIndex = serde_yaml::from_slice(body)?;
    Ok(raw
        .entries
        .into_iter()
        .map(|(name, entries)| {
            let versions = entries
                .into_iter()
                .filter_map(|e| {
                    Some(ChartVersion {
                        version: scalar(e.version)?,
                        app_version: scalar(e.app_version),
                        created: e.created.and_then(|c| c.parse().ok()),
                    })
                })
                .collect();
            (name, versions)
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &[u8] = include_bytes!("../tests/fixtures/index.yaml");

    fn app_versions(chart: &str) -> Vec<(String, Option<String>)> {
        let index = HelmIndex::new(parse_index(FIXTURE).unwrap());
        index
            .versions(chart)
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
        let numbers =
            parse_index(b"entries:\n  x:\n  - version: 1.0.0\n    appVersion: 2\n").unwrap();
        assert_eq!(numbers["x"][0].app_version.as_deref(), Some("2"));
    }

    #[test]
    fn created_and_lookup() {
        let index = HelmIndex::new(parse_index(FIXTURE).unwrap());
        let v = index.find("alloy", "v1.13.1").unwrap();
        assert_eq!(v.created, Some("2026-10-08T17:41:54.542Z".parse().unwrap()));
        assert!(index.find("alloy", "9.9.9").is_none());
        let plus =
            HelmIndex::new(parse_index(b"entries:\n  x:\n  - version: 1.0.0+up2\n").unwrap());
        assert!(plus.find("x", "1.0.0 up2").is_some());
        assert!(index.versions("tempo").is_empty());
    }
}
