//! Prometheus metrics: pages served, upstream requests and GitHub's rate
//! limit, which is what runs out first without a token.

use prometheus_client::encoding::EncodeLabelSet;
use prometheus_client::metrics::counter::Counter;
use prometheus_client::metrics::family::Family;
use prometheus_client::metrics::gauge::Gauge;
use prometheus_client::metrics::info::Info;
use prometheus_client::registry::Registry;

#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
pub struct ResultLabels {
    pub result: String,
}

#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
pub struct UpstreamLabels {
    pub upstream: String,
    pub result: String,
}

#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
struct BuildLabels {
    version: String,
}

/// Every `result` a notes page can end in.
pub const PAGE_RESULTS: [&str; 4] = ["ok", "unknown_chart", "unknown_version", "upstream_error"];
/// Every `upstream` and `result` of an upstream request.
pub const UPSTREAMS: [&str; 2] = ["helm", "github"];
pub const UPSTREAM_RESULTS: [&str; 4] = ["ok", "not_modified", "rate_limited", "error"];

pub struct Metrics {
    registry: Registry,
    pages: Family<ResultLabels, Counter>,
    upstream: Family<UpstreamLabels, Counter>,
    pub github_rate_limit_remaining: Gauge,
    pub github_rate_limit_reset: Gauge,
}

impl Metrics {
    pub fn new(charts: usize) -> Self {
        let mut registry = Registry::default();
        // Each series starts at 0: `increase()` ignores a counter's first
        // sample, so one that first appears at 1 would hide an event.
        let pages = Family::<ResultLabels, Counter>::default();
        for result in PAGE_RESULTS {
            let _ = pages.get_or_create(&ResultLabels {
                result: result.into(),
            });
        }
        let upstream = Family::<UpstreamLabels, Counter>::default();
        for name in UPSTREAMS {
            for result in UPSTREAM_RESULTS {
                let _ = upstream.get_or_create(&UpstreamLabels {
                    upstream: name.into(),
                    result: result.into(),
                });
            }
        }
        registry.register(
            "chart_notes_pages",
            "Notes pages served, by result (ok, unknown_chart, unknown_version, upstream_error)",
            pages.clone(),
        );
        registry.register(
            "chart_notes_upstream_requests",
            "Requests to Helm repositories and GitHub, by upstream and result",
            upstream.clone(),
        );
        let remaining = Gauge::default();
        remaining.set(-1);
        registry.register(
            "chart_notes_github_rate_limit_remaining",
            "GitHub API requests left in the current window (-1 until the first request)",
            remaining.clone(),
        );
        let reset = Gauge::default();
        registry.register(
            "chart_notes_github_rate_limit_reset_timestamp_seconds",
            "When GitHub's rate limit window resets",
            reset.clone(),
        );
        let configured = Gauge::<i64>::default();
        configured.set(charts as i64);
        registry.register("chart_notes_charts", "Charts chart-notes knows", configured);
        registry.register(
            "chart_notes_build",
            "The running version",
            Info::new(BuildLabels {
                version: env!("CARGO_PKG_VERSION").into(),
            }),
        );
        Metrics {
            registry,
            pages,
            upstream,
            github_rate_limit_remaining: remaining,
            github_rate_limit_reset: reset,
        }
    }

    pub fn page(&self, result: &str) {
        self.pages
            .get_or_create(&ResultLabels {
                result: result.into(),
            })
            .inc();
    }

    pub fn upstream(&self, upstream: &str, result: &str) {
        self.upstream
            .get_or_create(&UpstreamLabels {
                upstream: upstream.into(),
                result: result.into(),
            })
            .inc();
    }

    pub fn encode(&self) -> String {
        let mut out = String::new();
        prometheus_client::encoding::text::encode(&mut out, &self.registry)
            .expect("writing to a String cannot fail");
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn series_exist_before_the_first_event() {
        let m = Metrics::new(3);
        let out = m.encode();
        assert!(out.contains(r#"chart_notes_pages_total{result="upstream_error"} 0"#));
        assert!(out.contains(
            r#"chart_notes_upstream_requests_total{upstream="github",result="rate_limited"} 0"#
        ));
        assert!(out.contains("chart_notes_charts 3"));
        assert!(out.contains("chart_notes_github_rate_limit_remaining -1"));
        assert!(out.contains(r#"chart_notes_build_info{version=""#));
    }
}
