//! Rendered HTML of each page of the demo app, pinned with insta. Review
//! changes with `cargo insta review`.

mod common;

use axum::http::StatusCode;

async fn render(uri: &str, headers: &[(&str, &str)], status: StatusCode) -> String {
    let (got, _, body) = common::get(common::demo_app(), uri, headers).await;
    assert_eq!(got, status, "{uri}");
    body
}

/// The asset hash changes with every CSS or JS edit; keep it out of the
/// snapshots so they only move when the markup does.
fn assert_html(name: &str, html: String) {
    let mut settings = insta::Settings::clone_current();
    settings.add_filter(r"\?v=[0-9a-f]+", "?v=[hash]");
    settings.bind(|| insta::assert_snapshot!(name, html));
}

#[tokio::test]
async fn index() {
    assert_html("index", render("/", &[], StatusCode::OK).await);
}

#[tokio::test]
async fn index_dark_with_palette() {
    let html = render(
        "/",
        &[(
            "cookie",
            "chart_notes_theme=dark; chart_notes_palette=violet",
        )],
        StatusCode::OK,
    )
    .await;
    assert_html("index_dark_violet", html);
}

#[tokio::test]
async fn versions() {
    assert_html(
        "versions_alloy",
        render("/alloy", &[], StatusCode::OK).await,
    );
}

#[tokio::test]
async fn notes_upgrade() {
    let html = render("/alloy?from=1.3.0&to=1.4.0", &[], StatusCode::OK).await;
    assert_html("notes_alloy_upgrade", html);
}

#[tokio::test]
async fn notes_same_app_version() {
    let html = render("/alloy?from=1.2.0&to=1.2.1", &[], StatusCode::OK).await;
    assert_html("notes_alloy_same", html);
}

#[tokio::test]
async fn notes_with_note() {
    let html = render(
        "/kube-prometheus-stack?from=78.5.0&to=79.1.0",
        &[],
        StatusCode::OK,
    )
    .await;
    assert_html("notes_kube_prometheus_stack", html);
}

#[tokio::test]
async fn notes_github_down() {
    let html = render("/traefik?from=37.1.2&to=37.2.0", &[], StatusCode::OK).await;
    assert_html("notes_traefik_github_down", html);
}

#[tokio::test]
async fn search_results() {
    assert_html("search_empty", render("/search", &[], StatusCode::OK).await);
    let html = render("/search?q=graf", &[], StatusCode::OK).await;
    assert_html("search_graf", html);
}

#[tokio::test]
async fn not_found() {
    let html = render("/alloy?from=1.3.0&to=9.9.9", &[], StatusCode::NOT_FOUND).await;
    assert_html("unknown_version", html);
}

#[tokio::test]
async fn upstream_error() {
    let html = render("/offline", &[], StatusCode::BAD_GATEWAY).await;
    assert_html("upstream_error", html);
}

#[tokio::test]
async fn several_repositories() {
    let html = render("/nginx?from=1.0.0&to=1.1.0", &[], StatusCode::OK).await;
    assert_html("ambiguous_nginx", html);
}
