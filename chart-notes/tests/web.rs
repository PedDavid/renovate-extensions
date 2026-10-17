//! The UI and ops routers against the demo data.

mod common;

use axum::http::{StatusCode, header};

use chart_notes::web::ops_router;
use common::{demo_app, demo_state, get};

#[tokio::test]
async fn index_lists_charts_and_the_renovate_config() {
    let (status, headers, body) = get(demo_app(), "/", &[]).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        headers[header::CONTENT_SECURITY_POLICY]
            .to_str()
            .unwrap()
            .contains("script-src 'self'")
    );
    assert_eq!(headers[header::CACHE_CONTROL], "no-store");
    for expected in [
        "href=\"/alloy\"",
        "grafana/alloy",
        "kube-prometheus-stack",
        "https://chart-notes.example.com/{{{packageName}}}?from={{{currentVersion}}}&#38;to={{{newVersion}}}",
    ] {
        assert!(body.contains(expected), "missing {expected}");
    }
}

#[tokio::test]
async fn notes_list_stable_releases_in_range() {
    let (status, _, body) = get(demo_app(), "/alloy?from=1.3.0&to=1.4.0", &[]).await;
    assert_eq!(status, StatusCode::OK);
    // v1.10.2 → v1.11.2, newest first, without the release candidate or the
    // release the old version already had.
    let order: Vec<usize> = ["v1.11.2", "v1.11.1", "v1.11.0", "v1.10.3"]
        .iter()
        .map(|t| {
            body.find(&format!("releases/tag/{t}\""))
                .unwrap_or_else(|| panic!("missing {t}"))
        })
        .collect();
    assert!(order.is_sorted(), "{order:?}");
    assert!(!body.contains("rc.1"));
    assert!(!body.contains("releases/tag/v1.10.2\""));
    assert!(body.contains("https://github.com/grafana/alloy/compare/v1.10.2...v1.11.2"));
    assert!(body.contains(">upgrade<"));
}

#[tokio::test]
async fn same_app_version_needs_no_github() {
    let (status, _, body) = get(demo_app(), "/alloy?from=1.2.0&to=1.2.1", &[]).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("Same grafana/alloy version"));
    assert!(!body.contains("Full diff"));
}

#[tokio::test]
async fn downgrade_lists_what_it_rolls_back() {
    let (status, _, body) = get(demo_app(), "/alloy?from=1.4.0&to=1.3.1", &[]).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains(">downgrade<"));
    assert!(body.contains("Rolled back"));
    assert!(body.contains("releases/tag/v1.11.2\""));
}

#[tokio::test]
async fn chart_note_is_shown() {
    let (_, _, body) = get(
        demo_app(),
        "/kube-prometheus-stack?from=78.5.0&to=79.1.0",
        &[],
    )
    .await;
    assert!(body.contains("Alertmanager and the other bundled images"));
}

#[tokio::test]
async fn github_down_still_shows_versions_and_diff() {
    let (status, _, body) = get(demo_app(), "/traefik?from=37.1.2&to=37.2.0", &[]).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body.contains("Couldn&#39;t list the releases")
            || body.contains("Couldn't list the releases")
    );
    assert!(body.contains("rate limit"));
    assert!(body.contains("https://github.com/traefik/traefik/compare/v3.5.2...v3.5.3"));
}

#[tokio::test]
async fn unknown_chart_and_version_are_404s_with_a_way_back() {
    let (status, _, body) = get(demo_app(), "/nope?from=1&to=2", &[]).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(
        body.contains("doesn&#39;t know a chart named")
            || body.contains("doesn't know a chart named")
    );

    let (status, _, body) = get(demo_app(), "/alloy?from=1.3.0&to=9.9.9", &[]).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(body.contains("9.9.9"));
    assert!(body.contains("href=\"/alloy\""));
}

#[tokio::test]
async fn unreachable_helm_repository_is_a_502() {
    let (status, _, body) = get(demo_app(), "/offline?from=1&to=2", &[]).await;
    assert_eq!(status, StatusCode::BAD_GATEWAY);
    assert!(body.contains("https://charts.invalid/index.yaml"));
}

#[tokio::test]
async fn the_old_path_form_is_gone() {
    let (status, _, _) = get(demo_app(), "/alloy/1.3.0/1.4.0", &[]).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn versions_page_links_each_bump_and_preselects_the_latest() {
    let (status, _, body) = get(demo_app(), "/alloy", &[]).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("href=\"/alloy?from=1.3.1&#38;to=1.4.0\""));
    assert!(body.contains("<option selected>1.4.0</option>"));
    assert!(body.contains("<option selected>1.3.1</option>"));
    assert!(body.contains("new app"));
}

#[tokio::test]
async fn search_finds_charts_by_name_or_app() {
    let (_, _, body) = get(demo_app(), "/search?q=operator", &[]).await;
    assert!(body.contains("href=\"/kube-prometheus-stack\""));
    assert!(!body.contains("href=\"/alloy\""));
}

#[tokio::test]
async fn theme_and_palette_come_from_cookies() {
    let (_, _, body) = get(
        demo_app(),
        "/",
        &[(
            "cookie",
            "chart_notes_theme=dark; chart_notes_palette=violet",
        )],
    )
    .await;
    assert!(body.contains("<html lang=\"en\" class=\"dark\" data-palette=\"violet\">"));
    let (_, _, body) = get(
        demo_app(),
        "/",
        &[("cookie", "chart_notes_palette=\"><script>")],
    )
    .await;
    assert!(body.contains("data-palette=\"neutral\""));
}

#[tokio::test]
async fn assets_are_served_immutable() {
    let (status, headers, _) = get(demo_app(), "/assets/js/app.js", &[]).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        headers[header::CACHE_CONTROL]
            .to_str()
            .unwrap()
            .contains("immutable")
    );
    let (status, _, _) = get(demo_app(), "/assets/app.css", &[]).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn notes_pages_are_counted_by_result() {
    let (state, metrics) = demo_state();
    let app = chart_notes::web::router(state);
    for uri in [
        "/alloy?from=1.3.0&to=1.4.0",
        "/alloy?from=1.3.0&to=9.9.9",
        "/nope?from=1&to=2",
        "/traefik?from=37.1.2&to=37.2.0",
        "/alloy",
    ] {
        get(app.clone(), uri, &[]).await;
    }
    let (status, _, body) = get(ops_router(metrics), "/metrics", &[]).await;
    assert_eq!(status, StatusCode::OK);
    for expected in [
        r#"chart_notes_pages_total{result="ok"} 1"#,
        r#"chart_notes_pages_total{result="unknown_version"} 1"#,
        r#"chart_notes_pages_total{result="unknown_chart"} 1"#,
        r#"chart_notes_pages_total{result="upstream_error"} 1"#,
        "chart_notes_charts 4",
    ] {
        assert!(body.contains(expected), "missing {expected} in\n{body}");
    }
}

#[tokio::test]
async fn health_endpoints() {
    let (_, metrics) = demo_state();
    let (status, _, _) = get(ops_router(metrics.clone()), "/healthz", &[]).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _, _) = get(ops_router(metrics), "/readyz", &[]).await;
    assert_eq!(status, StatusCode::OK);
}
