//! The UI and ops routers against the demo data.

mod common;

use axum::http::{StatusCode, header};

use chart_notes::web::ops_router;
use common::{demo_app, demo_state, get};

#[tokio::test]
async fn index_lists_charts_looked_up_and_the_renovate_config() {
    let (state, _) = demo_state();
    let app = chart_notes::web::router(state);
    let (status, headers, body) = get(app.clone(), "/", &[]).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        headers[header::CONTENT_SECURITY_POLICY]
            .to_str()
            .unwrap()
            .contains("script-src 'self'")
    );
    assert_eq!(headers[header::CACHE_CONTROL], "no-store");
    assert!(body.contains("No charts looked up yet"));
    for expected in [
        "[![release notes](https://chart-notes.example.com/{{{packageName}}}/badge.svg?from={{{currentVersion}}}&#38;to={{{newVersion}}}&#38;source={{{sourceUrl}}})]",
        "(https://chart-notes.example.com/{{{packageName}}}?from={{{currentVersion}}}&#38;to={{{newVersion}}}&#38;source={{{sourceUrl}}})",
        "<code class=\"text-xs\">kube-prometheus-stack</code>",
    ] {
        assert!(body.contains(expected), "missing {expected}");
    }

    get(app.clone(), "/alloy?from=1.3.0&to=1.4.0&full=1", &[]).await;
    let (_, _, body) = get(app, "/", &[]).await;
    assert!(body.contains(
        "href=\"/alloy?source=https%3A%2F%2Fgithub.com%2Fgrafana%2Fhelm-charts&#38;repo=https%3A%2F%2Fgrafana.github.io%2Fhelm-charts\""
    ));
    assert!(body.contains("grafana/alloy"));
}

#[tokio::test]
async fn the_lookup_form_goes_to_the_chart() {
    let (status, headers, _) = get(demo_app(), "/?chart=alloy", &[]).await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(headers[header::LOCATION], "/alloy");
    let (status, _, _) = get(demo_app(), "/?chart=..%2Fetc", &[]).await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn notes_list_stable_releases_in_range() {
    let (status, _, body) = get(demo_app(), "/alloy?from=1.3.0&to=1.4.0&full=1", &[]).await;
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
async fn chart_changes_come_from_the_index_annotations() {
    let (_, _, body) = get(demo_app(), "/alloy?from=1.3.0&to=1.4.0&full=1", &[]).await;
    assert!(body.contains("Chart <span class=\"tab-count\">· 2</span>"));
    assert!(body.contains("alloy-1.4.0"));
    assert!(body.contains("Add <code>alloy.extraPorts</code> to expose extra container ports."));
    assert!(body.contains("data-kind=\"fixed\""));
    assert!(body.contains("href=\"https://github.com/grafana/helm-charts/pull/3911\""));
    assert!(body.contains("https://github.com/grafana/helm-charts/releases/tag/alloy-1.4.0"));
    // Not in the bump.
    assert!(!body.contains("alloy-1.3.0<"));
}

#[tokio::test]
async fn chart_changes_fall_back_to_the_chart_repositorys_releases() {
    let (status, _, body) = get(demo_app(), "/traefik?from=37.1.2&to=37.2.0&full=1", &[]).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("honour <code>gateway.listeners.*.hostname</code>"));
    assert!(
        body.contains("https://github.com/traefik/traefik-helm-chart/releases/tag/traefik-37.2.0")
    );
}

#[tokio::test]
async fn same_app_version_needs_no_github() {
    let (status, _, body) = get(demo_app(), "/alloy?from=1.2.0&to=1.2.1&full=1", &[]).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("Same app version"));
    assert!(body.contains("No changes listed for this chart version."));
    assert!(!body.contains("Full app diff"));
}

#[tokio::test]
async fn downgrade_lists_what_it_rolls_back() {
    let (status, _, body) = get(demo_app(), "/alloy?from=1.4.0&to=1.3.1&full=1", &[]).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains(">downgrade<"));
    assert!(body.contains("the app releases below are rolled back"));
    assert!(body.contains("releases/tag/v1.11.2\""));
}

#[tokio::test]
async fn overrides_set_the_app_and_a_note() {
    let (_, _, body) = get(
        demo_app(),
        "/kube-prometheus-stack?from=78.5.0&to=79.1.0&full=1",
        &[],
    )
    .await;
    assert!(body.contains("Alertmanager and the other bundled images"));
    // Not prometheus/prometheus, which the chart's sources list first.
    assert!(body.contains("href=\"https://github.com/prometheus-operator/prometheus-operator\""));
}

#[tokio::test]
async fn github_down_still_shows_versions_and_diff() {
    let (status, _, body) = get(demo_app(), "/traefik?from=37.1.2&to=37.2.0&full=1", &[]).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body.contains("Couldn&#39;t list the app&#39;s releases")
            || body.contains("Couldn't list the app's releases")
    );
    assert!(body.contains("rate limit"));
    assert!(body.contains("https://github.com/traefik/traefik/compare/v3.5.2...v3.5.3"));
}

#[tokio::test]
async fn renovates_source_url_picks_the_repository() {
    // Renovate rewrites GitHub links in PR bodies to redirect.github.com.
    let (status, _, body) = get(
        demo_app(),
        "/alloy?from=1.3.0&to=1.4.0&source=https://redirect.github.com/grafana/helm-charts&full=1",
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("grafana.github.io"));
}

#[tokio::test]
async fn a_repo_in_the_url_is_only_used_when_it_is_a_candidate() {
    let (status, _, body) = get(
        demo_app(),
        "/alloy?from=1.3.0&to=1.4.0&repo=http://169.254.169.254/latest&full=1",
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("grafana.github.io"));
    assert!(!body.contains("169.254"));
}

#[tokio::test]
async fn several_repositories_ask_which_one() {
    let (status, _, body) = get(demo_app(), "/nginx?from=1.0.0&to=1.1.0&full=1", &[]).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("Which nginx?"));
    assert!(body.contains("Example stable"));
    assert!(body.contains(
        "href=\"/nginx?from=1.0.0&#38;to=1.1.0&#38;repo=https%3A%2F%2Fcharts.example.net%2F\""
    ));
}

#[tokio::test]
async fn unknown_chart_and_version_are_404s_with_a_way_back() {
    let (status, _, body) = get(demo_app(), "/nope?from=1&to=2&full=1", &[]).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(body.contains("No Helm repository with a chart named"));
    assert!(body.contains("overrides"));

    let (status, _, body) = get(demo_app(), "/alloy?from=1.3.0&to=9.9.9&full=1", &[]).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(body.contains("9.9.9"));
    assert!(body.contains("href=\"/alloy\""));
}

#[tokio::test]
async fn unreachable_helm_repository_is_a_502() {
    let (status, _, body) = get(demo_app(), "/offline?from=1&to=2&full=1", &[]).await;
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
    assert!(body.contains(
        "href=\"/alloy?from=1.3.1&#38;to=1.4.0&#38;source=https%3A%2F%2Fgithub.com%2Fgrafana%2Fhelm-charts&#38;repo="
    ));
    assert!(body.contains("<option selected>1.4.0</option>"));
    assert!(body.contains("<option selected>1.3.1</option>"));
    assert!(body.contains("new app"));
    assert!(body.contains("name=\"source\" value=\"https://github.com/grafana/helm-charts\""));
}

#[tokio::test]
async fn search_finds_charts_looked_up() {
    let (state, _) = demo_state();
    let app = chart_notes::web::router(state);
    get(app.clone(), "/alloy?from=1.3.0&to=1.4.0&full=1", &[]).await;
    get(app.clone(), "/kube-prometheus-stack", &[]).await;
    let (_, _, body) = get(app, "/search?q=operator", &[]).await;
    assert!(body.contains("href=\"/kube-prometheus-stack?"));
    assert!(!body.contains("href=\"/alloy?"));
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
        "/alloy?from=1.3.0&to=1.4.0&full=1",
        "/alloy?from=1.3.0&to=9.9.9&full=1",
        "/nope?from=1&to=2&full=1",
        "/traefik?from=37.1.2&to=37.2.0&full=1",
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
        "chart_notes_overrides 2",
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

#[tokio::test]
async fn notes_show_each_release_with_its_release_notes() {
    let (status, _, body) = get(demo_app(), "/alloy?from=1.3.0&to=1.4.0&full=1", &[]).await;
    assert_eq!(status, StatusCode::OK);
    // GitHub's rendering, sanitized, with links opening in a new tab.
    assert!(body.contains("<code>otelcol.receiver.tcplog</code>"));
    assert!(body.contains(
        r#"<a href="https://github.com/grafana/alloy/pull/4501" target="_blank" rel="noopener noreferrer nofollow">#4501</a>"#
    ));
    assert!(body.contains("<h3>Breaking changes</h3>"));
    // v1.10.3 was published without notes.
    assert!(body.contains("Published without notes."));
    // Two chart releases and four app releases, the newest five open.
    assert_eq!(body.matches("<details class=\"release\"").count(), 6);
    assert_eq!(body.matches("<details class=\"release\" open>").count(), 5);
}

#[tokio::test]
async fn release_note_images_from_github_are_allowed() {
    let (_, headers, _) = get(demo_app(), "/alloy?from=1.3.0&to=1.4.0&full=1", &[]).await;
    let csp = headers[header::CONTENT_SECURITY_POLICY].to_str().unwrap();
    assert!(
        csp.contains("img-src 'self' data: https://github.com https://*.githubusercontent.com;")
    );
    assert!(csp.contains("script-src 'self';"));
}

#[tokio::test]
async fn badge_summarises_and_warms_the_page() {
    let (state, metrics) = demo_state();
    let app = chart_notes::web::router(state);
    let (status, headers, body) = get(
        app.clone(),
        "/alloy/badge.svg?from=1.3.0&to=1.4.0&source=https://github.com/grafana/helm-charts",
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        headers[header::CONTENT_TYPE],
        "image/svg+xml; charset=utf-8"
    );
    assert_eq!(headers[header::CACHE_CONTROL], "no-store");
    assert!(body.starts_with("<svg"));
    assert!(
        body.contains("app v1.10.2 → v1.11.2 · 4 releases"),
        "{body}"
    );
    // The lookup put the chart on the front page.
    let (_, _, index) = get(app, "/", &[]).await;
    assert!(index.contains("href=\"/alloy?"));
    let (_, _, out) = get(ops_router(metrics), "/metrics", &[]).await;
    assert!(out.contains(r#"chart_notes_badges_total{result="ready"} 1"#));
}

#[tokio::test]
async fn badge_for_what_cant_be_looked_up() {
    let (_, _, body) = get(demo_app(), "/nope/badge.svg?from=1&to=2", &[]).await;
    assert!(body.contains("unknown chart"));
    let (_, _, body) = get(demo_app(), "/alloy/badge.svg?from=1.3.0&to=9.9.9", &[]).await;
    assert!(body.contains("unknown version"));
    let (_, _, body) = get(demo_app(), "/alloy/badge.svg", &[]).await;
    assert!(body.contains("unknown chart"));
    let (_, _, body) = get(demo_app(), "/alloy/badge.svg?from=1.2.0&to=1.2.1", &[]).await;
    assert!(
        body.contains("app v1.10.1 unchanged · 1 chart release"),
        "{body}"
    );
}

/// The demo, with every Helm index taking a minute.
struct Slow(chart_notes::demo::DemoSources);

#[async_trait::async_trait]
impl chart_notes::sources::Sources for Slow {
    async fn index(
        &self,
        repo: &url::Url,
        chart: &str,
    ) -> Result<
        chart_notes::cache::Cached<chart_notes::sources::ChartIndex>,
        chart_notes::sources::UpstreamError,
    > {
        tokio::time::sleep(std::time::Duration::from_secs(60)).await;
        self.0.index(repo, chart).await
    }
    async fn search(
        &self,
        chart: &str,
    ) -> Result<
        chart_notes::cache::Cached<Vec<chart_notes::sources::Candidate>>,
        chart_notes::sources::UpstreamError,
    > {
        self.0.search(chart).await
    }
    async fn values(
        &self,
        repo: &url::Url,
        chart: &str,
        version: &str,
        package: &str,
    ) -> Result<
        chart_notes::cache::Cached<chart_notes::sources::ChartValues>,
        chart_notes::sources::UpstreamError,
    > {
        self.0.values(repo, chart, version, package).await
    }
    async fn releases(
        &self,
        app: &str,
    ) -> Result<
        chart_notes::cache::Cached<chart_notes::sources::Releases>,
        chart_notes::sources::UpstreamError,
    > {
        self.0.releases(app).await
    }
    async fn notes(
        &self,
        app: &str,
        tag: &str,
    ) -> Result<
        chart_notes::cache::Cached<chart_notes::sources::ReleaseNotes>,
        chart_notes::sources::UpstreamError,
    > {
        self.0.notes(app, tag).await
    }
}

#[tokio::test(start_paused = true)]
async fn badge_answers_before_slow_upstreams_and_finishes_in_the_background() {
    use std::sync::Arc;
    let sources: Arc<dyn chart_notes::sources::Sources> =
        Arc::new(Slow(chart_notes::demo::DemoSources));
    let metrics = Arc::new(chart_notes::metrics::Metrics::new(0));
    let resolver = Arc::new(chart_notes::resolve::Resolver::new(
        chart_notes::demo::overrides(),
        sources.clone(),
        std::time::Duration::from_secs(3600),
    ));
    let state = chart_notes::web::AppState::new(resolver, sources, metrics.clone(), None, true);
    let app = chart_notes::web::router(state);
    let uri = "/alloy/badge.svg?from=1.3.0&to=1.4.0";
    let (_, _, body) = get(app.clone(), uri, &[]).await;
    assert!(body.contains("1.3.0 → 1.4.0 · looking up"), "{body}");
    // The lookup carried on; a later view shows it.
    tokio::time::sleep(std::time::Duration::from_secs(120)).await;
    let (_, _, body) = get(app, uri, &[]).await;
    assert!(
        body.contains("app v1.10.2 → v1.11.2 · 4 releases"),
        "{body}"
    );
    let (_, _, out) = get(ops_router(metrics), "/metrics", &[]).await;
    assert!(out.contains(r#"chart_notes_badges_total{result="pending"} 1"#));
    assert!(out.contains(r#"chart_notes_badges_total{result="ready"} 1"#));
}

#[tokio::test]
async fn search_offers_overrides_and_a_lookup() {
    let (_, _, body) = get(demo_app(), "/search", &[]).await;
    assert!(body.contains("href=\"/kube-prometheus-stack\""));
    assert!(body.contains("overridden"));
    let (_, _, body) = get(demo_app(), "/search?q=traefik", &[]).await;
    assert!(body.contains("href=\"/traefik\""));
    assert!(body.contains("look up"));
    let (_, _, body) = get(demo_app(), "/search?q=a%20b", &[]).await;
    assert!(!body.contains("look up"));
}

#[tokio::test]
async fn the_page_loads_its_content_and_works_without_javascript() {
    let (status, _, body) = get(demo_app(), "/alloy?from=1.3.0&to=1.4.0&tab=app", &[]).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("hx-get=\"/alloy/timeline?from=1.3.0&#38;to=1.4.0&#38;source="));
    assert!(body.contains("&#38;tab=app&#38;first=1\""));
    assert!(body.contains("<noscript>"));
    assert!(body.contains("&#38;tab=app&#38;full=1\""));
    assert!(!body.contains("<details"));
}

#[tokio::test]
async fn tabs_filter_the_timeline() {
    let base = "/alloy/timeline?from=1.3.0&to=1.4.0";
    let (_, _, app) = get(demo_app(), &format!("{base}&tab=app"), &[]).await;
    assert!(!app.contains("<html"));
    assert!(app.contains("data-kind=\"app\""));
    assert!(!app.contains("data-kind=\"chart\""));
    assert!(app.contains("data-tab=\"app\" href=\"/alloy?from=1.3.0&#38;to=1.4.0&#38;source="));
    let (_, _, chart) = get(demo_app(), &format!("{base}&tab=chart"), &[]).await;
    assert!(chart.contains("data-kind=\"chart\""));
    assert!(!chart.contains("kind-badge\" data-kind=\"app\""));
    let (_, _, all) = get(demo_app(), base, &[]).await;
    // Newest first, chart and app releases interleaved by date.
    let pos = |s: &str| all.find(s).unwrap_or_else(|| panic!("missing {s}"));
    assert!(pos("alloy-1.4.0") < pos(">v1.11.2<"));
    assert!(pos(">v1.11.2<") < pos("alloy-1.3.1"));
    assert!(pos("alloy-1.3.1") < pos(">v1.11.0<"));
}

#[tokio::test]
async fn values_diff_tab() {
    let (status, _, body) = get(demo_app(), "/alloy/values?from=1.3.0&to=1.4.0", &[]).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("data-tab=\"values\""));
    assert!(body.contains("aria-selected=\"true\""));
    // 1.3.0 → 1.4.0: the image tag, extraPorts, and the PDB reshaped.
    assert!(body.contains("<span class=\"diff-text\">  tag: v1.11.2</span>"));
    assert!(body.contains("extraPorts: []"));
    assert!(body.contains("1 key removed: check your values"));
    assert!(body.contains("<code class=\"text-xs\">controller.pdb</code>"));
    assert!(body.contains("@@ alloy @@"));
    let (_, _, same) = get(demo_app(), "/traefik/values?from=37.1.2&to=37.2.0", &[]).await;
    assert!(same.contains("tag: v3.5.3"));
}

#[tokio::test]
async fn only_the_pages_own_load_is_counted() {
    let (state, metrics) = demo_state();
    let app = chart_notes::web::router(state);
    get(app.clone(), "/alloy?from=1.3.0&to=1.4.0", &[]).await;
    get(
        app.clone(),
        "/alloy/timeline?from=1.3.0&to=1.4.0&tab=all&first=1",
        &[],
    )
    .await;
    get(
        app.clone(),
        "/alloy/timeline?from=1.3.0&to=1.4.0&tab=app",
        &[],
    )
    .await;
    get(app.clone(), "/alloy/values?from=1.3.0&to=1.4.0", &[]).await;
    let (_, _, out) = get(ops_router(metrics), "/metrics", &[]).await;
    assert!(
        out.contains(r#"chart_notes_pages_total{result="ok"} 1"#),
        "{out}"
    );
}
