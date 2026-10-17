//! Sample charts, indexes and releases for `--demo` and the tests: no network.

use std::sync::Arc;

use async_trait::async_trait;
use jiff::Timestamp;

use url::Url;

use crate::cache::Cached;
use crate::overrides::{self, Overrides};
use crate::sources::{
    Candidate, ChartIndex, ChartVersion, Release, ReleaseNotes, Releases, Sources, UpstreamError,
};

/// The demo's overrides: an app the lookup can't guess, with a note, and a
/// repository that can't be reached.
pub fn overrides() -> Overrides {
    overrides::parse(
        r#"
kube-prometheus-stack:
  app: prometheus-operator/prometheus-operator
  note: >-
    This chart's appVersion is prometheus-operator's. Prometheus,
    Alertmanager and the other bundled images can change without it.
offline:
  repo: https://charts.invalid
  app: example/offline
"#,
    )
    .expect("demo overrides are valid")
}

fn ts(s: &str) -> Timestamp {
    s.parse().expect("demo timestamps are valid")
}

/// Where each demo chart is released from, and its app.
struct Meta {
    repo: &'static str,
    source: &'static str,
    app: &'static str,
}

fn meta(chart: &str) -> Option<Meta> {
    Some(match chart {
        "alloy" => Meta {
            repo: "https://grafana.github.io/helm-charts",
            source: "grafana/helm-charts",
            app: "grafana/alloy",
        },
        "kube-prometheus-stack" => Meta {
            repo: "https://prometheus-community.github.io/helm-charts",
            source: "prometheus-community/helm-charts",
            app: "prometheus/prometheus",
        },
        "traefik" => Meta {
            repo: "https://traefik.github.io/charts",
            source: "traefik/traefik-helm-chart",
            app: "traefik/traefik",
        },
        _ => return None,
    })
}

const ALLOY_1_4_0: &str = "- kind: added
  description: Add `alloy.extraPorts` to expose extra container ports.
  links:
    - name: \"PR #3911\"
      url: https://github.com/grafana/helm-charts/pull/3911
- kind: changed
  description: Bump Alloy to v1.11.2.
";

const ALLOY_1_3_1: &str = "- kind: fixed
  description: Fix the PodDisruptionBudget template when `replicas` is 1.
  links:
    - name: \"PR #3890\"
      url: https://github.com/grafana/helm-charts/pull/3890
- kind: changed
  description: Bump Alloy to v1.11.0.
";

fn versions(chart: &str, list: &[(&str, &str, &str, Option<&str>)]) -> Vec<ChartVersion> {
    let m = meta(chart).expect("demo chart");
    list.iter()
        .map(|(version, app, created, changes)| ChartVersion {
            version: (*version).into(),
            app_version: Some((*app).into()),
            created: Some(ts(created)),
            home: None,
            sources: vec![
                format!("https://github.com/{}", m.app),
                format!("https://github.com/{}", m.source),
            ],
            url: Some(format!(
                "https://github.com/{}/releases/download/{chart}-{version}/{chart}-{version}.tgz",
                m.source
            )),
            changes: changes.map(str::to_string),
        })
        .collect()
}

fn index(chart: &str) -> Vec<ChartVersion> {
    match chart {
        "alloy" => versions(
            chart,
            &[
                (
                    "1.4.0",
                    "v1.11.2",
                    "2026-10-08T17:41:54Z",
                    Some(ALLOY_1_4_0),
                ),
                (
                    "1.3.1",
                    "v1.11.0",
                    "2026-09-30T09:12:00Z",
                    Some(ALLOY_1_3_1),
                ),
                ("1.3.0", "v1.10.2", "2026-09-10T14:03:00Z", None),
                ("1.2.1", "v1.10.1", "2026-08-21T08:30:00Z", None),
                ("1.2.0", "v1.10.1", "2026-08-14T11:45:00Z", None),
            ],
        ),
        "kube-prometheus-stack" => versions(
            chart,
            &[
                ("79.1.0", "v0.86.1", "2026-10-12T10:00:00Z", None),
                ("79.0.2", "v0.86.1", "2026-10-05T10:00:00Z", None),
                ("78.5.0", "v0.86.0", "2026-09-28T10:00:00Z", None),
            ],
        ),
        // No `artifacthub.io/changes`: the chart's notes are its GitHub
        // releases.
        "traefik" => versions(
            chart,
            &[
                ("37.2.0", "v3.5.3", "2026-10-01T10:00:00Z", None),
                ("37.1.2", "v3.5.2", "2026-09-17T10:00:00Z", None),
            ],
        ),
        _ => Vec::new(),
    }
}

fn candidate(repo: &str, name: &str, display: &str, official: bool) -> Candidate {
    Candidate {
        repo: Url::parse(repo).expect("demo URLs are valid"),
        name: name.into(),
        display_name: display.into(),
        official,
        verified: official,
    }
}

/// What Artifact Hub lists. "nginx" is in two repositories, neither
/// official: the page asks which one.
fn search(chart: &str) -> Vec<Candidate> {
    match chart {
        "nginx" => vec![
            candidate(
                "https://charts.example.org/stable",
                "example-stable",
                "Example stable",
                false,
            ),
            candidate(
                "https://charts.example.net",
                "example-net",
                "Example.net charts",
                false,
            ),
        ],
        "alloy" => vec![
            candidate(meta(chart).unwrap().repo, "grafana", "Grafana", true),
            candidate(
                "https://charts.example.org/forks",
                "forks",
                "Some forks",
                false,
            ),
        ],
        _ => match meta(chart) {
            Some(m) => vec![candidate(m.repo, chart, chart, true)],
            None => Vec::new(),
        },
    }
}

fn release(app: &str, tag: &str, published: &str, prerelease: bool) -> Release {
    Release {
        tag_name: tag.into(),
        name: Some(tag.into()),
        prerelease,
        draft: false,
        html_url: format!("https://github.com/{app}/releases/tag/{tag}"),
        published_at: Some(ts(published)),
    }
}

fn releases(app: &str) -> Option<Releases> {
    let list: &[(&str, &str, bool)] = match app {
        "grafana/alloy" => &[
            ("v1.11.2", "2026-10-07T12:00:00Z", false),
            ("v1.11.1", "2026-10-02T12:00:00Z", false),
            ("v1.11.0", "2026-09-29T12:00:00Z", false),
            ("v1.11.0-rc.1", "2026-09-22T12:00:00Z", true),
            ("v1.10.3", "2026-09-25T12:00:00Z", false),
            ("v1.10.2", "2026-09-09T12:00:00Z", false),
            ("v1.10.1", "2026-08-13T12:00:00Z", false),
        ],
        "prometheus-operator/prometheus-operator" => &[
            ("v0.86.1", "2026-10-03T12:00:00Z", false),
            ("v0.86.0", "2026-09-26T12:00:00Z", false),
        ],
        // Traefik's releases are what GitHub returns when it is down.
        _ => return None,
    };
    Some(Releases::new(
        list.iter()
            .map(|(tag, published, pre)| release(app, tag, published, *pre))
            .collect(),
    ))
}

/// Release notes as GitHub renders them (before sanitizing).
fn notes_html(app: &str, tag: &str) -> Option<&'static str> {
    Some(match (app, tag) {
        ("grafana/alloy", "v1.11.2") => {
            r#"<h3>Bugfixes</h3>
<ul>
<li>Fix <code>loki.source.file</code> missing lines after a log rotation. (<a href="https://github.com/grafana/alloy/pull/4501">#4501</a>)</li>
<li>Fix a panic in <code>otelcol.processor.transform</code> on empty statements. (<a href="https://github.com/grafana/alloy/pull/4498">#4498</a>)</li>
</ul>"#
        }
        ("grafana/alloy", "v1.11.1") => {
            r#"<h3>Bugfixes</h3>
<ul>
<li>Restore the <code>--stability.level</code> flag's default. (<a href="https://github.com/grafana/alloy/pull/4470">#4470</a>)</li>
</ul>"#
        }
        ("grafana/alloy", "v1.11.0") => {
            r#"<h3>Breaking changes</h3>
<ul>
<li><code>prometheus.exporter.windows</code>: the <code>msmq</code> collector was removed upstream. (<a href="https://github.com/grafana/alloy/pull/4402">#4402</a>)</li>
</ul>
<h3>Features</h3>
<ul>
<li>Add <code>otelcol.receiver.tcplog</code>. (<a href="https://github.com/grafana/alloy/pull/4388">#4388</a>)</li>
<li><code>loki.write</code> can now retry on 429 responses with backoff.</li>
</ul>
<h3>Upgrading</h3>
<pre><code>helm upgrade alloy grafana/alloy --version 1.4.0
</code></pre>"#
        }
        ("traefik/traefik-helm-chart", "traefik-37.2.0") => {
            r#"<h2>What's changed</h2>
<ul>
<li>feat(deps): update traefik to v3.5.3 by <a href="https://github.com/renovate-bot">@renovate-bot</a></li>
<li>fix(gateway): honour <code>gateway.listeners.*.hostname</code> by <a href="https://github.com/mloiseleur">@mloiseleur</a></li>
</ul>"#
        }
        ("prometheus-operator/prometheus-operator", "v0.86.1") => {
            r#"<ul>
<li>[BUGFIX] Fix the <code>ScrapeConfig</code> reconciliation when the namespace selector is empty. <a href="https://github.com/prometheus-operator/prometheus-operator/pull/7900">#7900</a></li>
</ul>"#
        }
        // v1.10.3 was published without notes.
        _ => return None,
    })
}

/// When the demo data was "fetched".
pub fn fetched_at() -> Timestamp {
    ts("2026-10-17T08:00:00Z")
}

pub struct DemoSources;

#[async_trait]
impl Sources for DemoSources {
    async fn index(&self, repo: &Url, chart: &str) -> Result<Cached<ChartIndex>, UpstreamError> {
        // The "offline" chart's repository is unreachable.
        if repo.host_str() == Some("charts.invalid") {
            return Err(UpstreamError::Request {
                url: crate::sources::index_url(repo),
                message: "dns error: failed to lookup address information".into(),
            });
        }
        // Each demo chart is only in its own repository.
        let here = meta(chart).is_some_and(|m| repo.as_str().trim_end_matches('/') == m.repo);
        Ok(Cached {
            value: Arc::new(ChartIndex::new(if here {
                index(chart)
            } else {
                Vec::new()
            })),
            fetched_at: fetched_at(),
            stale: None,
        })
    }

    async fn search(&self, chart: &str) -> Result<Cached<Vec<Candidate>>, UpstreamError> {
        Ok(Cached {
            value: Arc::new(search(chart)),
            fetched_at: fetched_at(),
            stale: None,
        })
    }

    async fn releases(&self, app: &str) -> Result<Cached<Releases>, UpstreamError> {
        match releases(app) {
            Some(r) => Ok(Cached {
                value: Arc::new(r),
                fetched_at: fetched_at(),
                stale: None,
            }),
            None => Err(UpstreamError::RateLimited {
                reset: Some(ts("2026-10-17T09:00:00Z")),
            }),
        }
    }

    async fn notes(&self, app: &str, tag: &str) -> Result<Cached<ReleaseNotes>, UpstreamError> {
        let base = format!("https://github.com/{app}/releases/tag/{tag}");
        Ok(Cached {
            value: Arc::new(ReleaseNotes::new(notes_html(app, tag), &base)),
            fetched_at: fetched_at(),
            stale: None,
        })
    }
}
