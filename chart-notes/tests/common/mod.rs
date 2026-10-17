//! Shared test harness for the HTTP tests.
#![allow(dead_code)]

use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::http::{HeaderMap, Request, StatusCode};
use http_body_util::BodyExt;
use tower::ServiceExt;

use chart_notes::demo;
use chart_notes::metrics::Metrics;
use chart_notes::resolve::Resolver;
use chart_notes::sources::Sources;
use chart_notes::web::{AppState, router};

/// The demo app, rendered as of a fixed time.
pub fn demo_state() -> (AppState, Arc<Metrics>) {
    let overrides = demo::overrides();
    let metrics = Arc::new(Metrics::new(overrides.len()));
    let sources: Arc<dyn Sources> = Arc::new(demo::DemoSources);
    let resolver = Arc::new(Resolver::new(
        overrides,
        sources.clone(),
        std::time::Duration::from_secs(3600),
    ));
    let state = AppState::new(
        resolver,
        sources,
        metrics.clone(),
        Some("https://chart-notes.example.com".parse().unwrap()),
        true,
    )
    .with_fixed_now("2026-10-17T12:00:00Z".parse().unwrap());
    (state, metrics)
}

pub fn demo_app() -> Router {
    router(demo_state().0)
}

pub async fn get(
    app: Router,
    uri: &str,
    headers: &[(&str, &str)],
) -> (StatusCode, HeaderMap, String) {
    let mut req = Request::get(uri);
    for (k, v) in headers {
        req = req.header(*k, *v);
    }
    let res = app.oneshot(req.body(Body::empty()).unwrap()).await.unwrap();
    let status = res.status();
    let headers = res.headers().clone();
    let body = res.into_body().collect().await.unwrap().to_bytes();
    (status, headers, String::from_utf8(body.to_vec()).unwrap())
}
