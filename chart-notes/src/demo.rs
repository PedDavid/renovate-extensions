//! Sample charts, indexes and releases for `--demo` and the tests: no network.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use jiff::Timestamp;

use crate::cache::Cached;
use crate::charts::{self, Chart, Charts};
use crate::sources::{
    ChartVersion, HelmIndex, Release, ReleaseNotes, Releases, Sources, UpstreamError,
};

pub fn charts() -> Charts {
    charts::parse(
        r#"
alloy:
  repo: https://grafana.github.io/helm-charts
  app: grafana/alloy
kube-prometheus-stack:
  repo: https://prometheus-community.github.io/helm-charts
  app: prometheus-operator/prometheus-operator
  note: >-
    This chart's appVersion is prometheus-operator's. Prometheus,
    Alertmanager and the other bundled images can change without it.
traefik:
  repo: https://traefik.github.io/charts
  app: traefik/traefik
offline:
  repo: https://charts.invalid
  app: example/offline
"#,
    )
    .expect("demo charts are valid")
}

fn ts(s: &str) -> Timestamp {
    s.parse().expect("demo timestamps are valid")
}

fn versions(list: &[(&str, &str, &str)]) -> Vec<ChartVersion> {
    list.iter()
        .map(|(version, app, created)| ChartVersion {
            version: (*version).into(),
            app_version: Some((*app).into()),
            created: Some(ts(created)),
        })
        .collect()
}

/// One index with every demo chart, whichever repository is asked.
fn index() -> HelmIndex {
    HelmIndex::new(HashMap::from([
        (
            "alloy".to_string(),
            versions(&[
                ("1.4.0", "v1.11.2", "2026-10-08T17:41:54Z"),
                ("1.3.1", "v1.11.0", "2026-09-30T09:12:00Z"),
                ("1.3.0", "v1.10.2", "2026-09-10T14:03:00Z"),
                ("1.2.1", "v1.10.1", "2026-08-21T08:30:00Z"),
                ("1.2.0", "v1.10.1", "2026-08-14T11:45:00Z"),
            ]),
        ),
        (
            "kube-prometheus-stack".to_string(),
            versions(&[
                ("79.1.0", "v0.86.1", "2026-10-12T10:00:00Z"),
                ("79.0.2", "v0.86.1", "2026-10-05T10:00:00Z"),
                ("78.5.0", "v0.86.0", "2026-09-28T10:00:00Z"),
            ]),
        ),
        (
            "traefik".to_string(),
            versions(&[
                ("37.2.0", "v3.5.3", "2026-10-01T10:00:00Z"),
                ("37.1.2", "v3.5.2", "2026-09-17T10:00:00Z"),
            ]),
        ),
    ]))
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
    async fn index(&self, chart: &Chart) -> Result<Cached<HelmIndex>, UpstreamError> {
        // The "offline" chart's repository is unreachable.
        if chart.repo.host_str() == Some("charts.invalid") {
            return Err(UpstreamError::Request {
                url: chart.index_url(),
                message: "dns error: failed to lookup address information".into(),
            });
        }
        Ok(Cached {
            value: Arc::new(index()),
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
