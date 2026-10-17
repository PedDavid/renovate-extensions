//! The Helm and GitHub clients against fake servers: caching, conditional
//! requests, pagination, rate limits and serving stale copies.

use std::sync::Arc;
use std::time::Duration;

use serde_json::json;
use wiremock::matchers::{header, header_exists, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

use chart_notes::metrics::Metrics;
use chart_notes::sources::{HttpSources, Sources, UpstreamError};
use url::Url;

const INDEX: &str = include_str!("fixtures/index.yaml");

fn repo(server: &MockServer) -> Url {
    format!("{}/charts", server.uri()).parse().unwrap()
}

fn sources(server: &MockServer, token: Option<&str>, ttl: Duration) -> (HttpSources, Arc<Metrics>) {
    let metrics = Arc::new(Metrics::new(1));
    let sources = HttpSources::new(
        format!("{}/", server.uri()).parse().unwrap(),
        token.map(str::to_string),
        format!("{}/hub/", server.uri()).parse().unwrap(),
        ttl,
        metrics.clone(),
    )
    .unwrap();
    (sources, metrics)
}

fn release(tag: &str) -> serde_json::Value {
    json!({
        "tag_name": tag,
        "name": tag,
        "prerelease": false,
        "draft": false,
        "html_url": format!("https://github.com/grafana/alloy/releases/tag/{tag}"),
        "published_at": "2026-10-01T00:00:00Z",
    })
}

#[tokio::test]
async fn index_is_cached_within_the_ttl() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/charts/index.yaml"))
        .respond_with(ResponseTemplate::new(200).set_body_string(INDEX))
        .expect(1)
        .mount(&server)
        .await;
    let (sources, metrics) = sources(&server, None, Duration::from_secs(60));
    for _ in 0..3 {
        let index = sources.index(&repo(&server), "alloy").await.unwrap();
        assert_eq!(
            index.value.find("1.13.1").unwrap().app_version.as_deref(),
            Some("v1.20.1")
        );
    }
    assert!(
        metrics
            .encode()
            .contains(r#"chart_notes_upstream_requests_total{upstream="helm",result="ok"} 1"#)
    );
}

#[tokio::test]
async fn index_refresh_is_conditional() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/charts/index.yaml"))
        .and(header("if-none-match", "\"v1\""))
        .respond_with(ResponseTemplate::new(304))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/charts/index.yaml"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("etag", "\"v1\"")
                .set_body_string(INDEX),
        )
        .expect(1)
        .mount(&server)
        .await;
    let (sources, metrics) = sources(&server, None, Duration::ZERO);
    sources.index(&repo(&server), "alloy").await.unwrap();
    let again = sources.index(&repo(&server), "alloy").await.unwrap();
    assert!(again.stale.is_none());
    assert!(again.value.find("1.13.0").is_some());
    assert!(
        metrics
            .encode()
            .contains(r#"upstream="helm",result="not_modified"} 1"#)
    );
}

#[tokio::test]
async fn failed_refresh_serves_the_last_copy() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/charts/index.yaml"))
        .respond_with(ResponseTemplate::new(200).set_body_string(INDEX))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/charts/index.yaml"))
        .respond_with(ResponseTemplate::new(503))
        .mount(&server)
        .await;
    let (sources, _) = sources(&server, None, Duration::ZERO);
    sources.index(&repo(&server), "alloy").await.unwrap();
    let stale = sources.index(&repo(&server), "alloy").await.unwrap();
    assert!(stale.stale.as_deref().unwrap().contains("503"));
    assert!(stale.value.find("1.13.0").is_some());
}

#[tokio::test]
async fn bad_index_is_a_parse_error() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/charts/index.yaml"))
        .respond_with(ResponseTemplate::new(200).set_body_string("entries: [unclosed"))
        .mount(&server)
        .await;
    let (sources, _) = sources(&server, None, Duration::ZERO);
    let err = sources.index(&repo(&server), "alloy").await.unwrap_err();
    assert!(matches!(err, UpstreamError::Parse { .. }), "{err}");
}

#[tokio::test]
async fn releases_page_until_a_short_page_and_send_the_token() {
    let server = MockServer::start().await;
    let full: Vec<_> = (0..100).map(|i| release(&format!("v1.{i}.0"))).collect();
    Mock::given(method("GET"))
        .and(path("/repos/grafana/alloy/releases"))
        .and(query_param("page", "1"))
        .and(header("authorization", "Bearer s3cret"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("x-ratelimit-remaining", "4999")
                .insert_header("x-ratelimit-reset", "1792000000")
                .set_body_json(full),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/repos/grafana/alloy/releases"))
        .and(query_param("page", "2"))
        .respond_with(ResponseTemplate::new(200).set_body_json(vec![release("v0.9.0")]))
        .expect(1)
        .mount(&server)
        .await;
    let (sources, metrics) = sources(&server, Some("s3cret\n"), Duration::from_secs(60));
    let releases = sources.releases("grafana/alloy").await.unwrap();
    assert_eq!(releases.value.items.len(), 101);
    let out = metrics.encode();
    assert!(out.contains("chart_notes_github_rate_limit_remaining 4999"));
    assert!(out.contains("chart_notes_github_rate_limit_reset_timestamp_seconds 1792000000"));
}

#[tokio::test]
async fn releases_refresh_with_the_etag() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/repos/grafana/alloy/releases"))
        .and(header_exists("if-none-match"))
        .respond_with(ResponseTemplate::new(304))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/repos/grafana/alloy/releases"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("etag", "W/\"abc\"")
                .set_body_json(vec![release("v1.0.0")]),
        )
        .expect(1)
        .mount(&server)
        .await;
    let (sources, _) = sources(&server, None, Duration::ZERO);
    sources.releases("grafana/alloy").await.unwrap();
    let again = sources.releases("grafana/alloy").await.unwrap();
    assert_eq!(again.value.items.len(), 1);
}

#[tokio::test]
async fn rate_limit_is_reported_as_such() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/repos/grafana/alloy/releases"))
        .respond_with(
            ResponseTemplate::new(403)
                .insert_header("x-ratelimit-remaining", "0")
                .insert_header("x-ratelimit-reset", "1792000000"),
        )
        .mount(&server)
        .await;
    let (sources, metrics) = sources(&server, None, Duration::from_secs(60));
    let err = sources.releases("grafana/alloy").await.unwrap_err();
    assert!(
        matches!(err, UpstreamError::RateLimited { reset: Some(_) }),
        "{err}"
    );
    assert!(err.to_string().contains("github.existingSecret"));
    let out = metrics.encode();
    assert!(out.contains(r#"upstream="github",result="rate_limited"} 1"#));
    assert!(out.contains("chart_notes_github_rate_limit_remaining 0"));
}

#[tokio::test]
async fn missing_repository_is_a_status_error() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;
    let (sources, _) = sources(&server, None, Duration::from_secs(60));
    let err = sources.releases("nope/nope").await.unwrap_err();
    assert!(
        matches!(err, UpstreamError::Status { status: 404, .. }),
        "{err}"
    );
}

#[tokio::test]
async fn release_notes_are_rendered_by_github_and_sanitized() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        // A tag with a `/` stays one path segment.
        .and(path("/repos/grafana/loki/releases/tags/helm-loki%2F6.0.0"))
        .and(header("accept", "application/vnd.github.html+json"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("etag", "\"n1\"")
                .set_body_json(json!({
                    "html_url": "https://github.com/grafana/loki/releases/tag/helm-loki/6.0.0",
                    "body_html": "<p>Fixed <a href=\"/grafana/loki/pull/1\">#1</a><script>x()</script></p>",
                })),
        )
        .expect(1)
        .mount(&server)
        .await;
    let (sources, _) = sources(&server, None, Duration::from_secs(60));
    let notes = sources
        .notes("grafana/loki", "helm-loki/6.0.0")
        .await
        .unwrap();
    let html = notes.value.html.as_deref().unwrap();
    assert!(
        html.contains(r#"href="https://github.com/grafana/loki/pull/1""#),
        "{html}"
    );
    assert!(!html.contains("script"));
    // Cached: the mock expects a single request.
    sources
        .notes("grafana/loki", "helm-loki/6.0.0")
        .await
        .unwrap();
}

#[tokio::test]
async fn release_without_notes() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/repos/grafana/alloy/releases/tags/v1.0.0"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "html_url": "https://github.com/grafana/alloy/releases/tag/v1.0.0",
            "body_html": null,
        })))
        .mount(&server)
        .await;
    let (sources, _) = sources(&server, None, Duration::from_secs(60));
    let notes = sources.notes("grafana/alloy", "v1.0.0").await.unwrap();
    assert!(notes.value.html.is_none());
}

#[tokio::test]
async fn artifact_hub_search() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/hub/api/v1/packages/search"))
        .and(query_param("ts_query_web", "alloy"))
        .and(query_param("kind", "0"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "packages": [
                {"name": "alloy", "repository": {"url": "https://grafana.github.io/helm-charts",
                  "name": "grafana", "display_name": "Grafana", "official": true, "verified_publisher": true}},
                {"name": "alloy-operator", "repository": {"url": "https://x.example", "name": "x"}}
            ]
        })))
        .expect(1)
        .mount(&server)
        .await;
    let (sources, metrics) = sources(&server, None, Duration::from_secs(60));
    let found = sources.search("alloy").await.unwrap();
    assert_eq!(found.value.len(), 1);
    assert_eq!(
        found.value[0].repo.as_str(),
        "https://grafana.github.io/helm-charts"
    );
    sources.search("alloy").await.unwrap();
    assert!(
        metrics
            .encode()
            .contains(r#"upstream="artifacthub",result="ok"} 1"#)
    );
}
