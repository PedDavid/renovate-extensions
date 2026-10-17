//! HTTP layer: the UI router, security headers, static assets and the
//! separate metrics/health router. Everything is read-only and public: there
//! are no POST routes, sessions or CSRF tokens.

mod pages;
mod views;

use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::Arc;

use askama::Template;
use axum::Router;
use axum::body::Body;
use axum::extract::{Path, Request};
use axum::http::{HeaderValue, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::get;
use jiff::Timestamp;
use rust_embed::RustEmbed;
use tower_http::trace::TraceLayer;
use url::Url;

use crate::charts::Charts;
use crate::metrics::Metrics;
use crate::sources::Sources;

#[derive(Clone)]
pub struct AppState {
    pub inner: Arc<Inner>,
}

pub struct Inner {
    pub charts: Charts,
    pub sources: Arc<dyn Sources>,
    pub metrics: Arc<Metrics>,
    pub public_url: Option<Url>,
    pub demo: bool,
    pub asset_version: String,
    /// Pins the clock the pages render relative dates against (tests only).
    pub fixed_now: Option<Timestamp>,
}

impl AppState {
    pub fn new(
        charts: Charts,
        sources: Arc<dyn Sources>,
        metrics: Arc<Metrics>,
        public_url: Option<Url>,
        demo: bool,
    ) -> Self {
        AppState {
            inner: Arc::new(Inner {
                charts,
                sources,
                metrics,
                public_url,
                demo,
                asset_version: asset_version(),
                fixed_now: None,
            }),
        }
    }

    /// Renders every page as if it were `now`, so output is reproducible.
    pub fn with_fixed_now(mut self, now: Timestamp) -> Self {
        Arc::get_mut(&mut self.inner)
            .expect("fixed_now is set before the state is shared")
            .fixed_now = Some(now);
        self
    }

    fn now(&self) -> Timestamp {
        self.inner.fixed_now.unwrap_or_else(Timestamp::now)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    /// A page that doesn't exist, with where to go instead.
    #[error("{message}")]
    NotFound { message: String, back: Back },
    #[error("{message}")]
    Upstream { message: String, back: Back },
    #[error("{0}")]
    Internal(String),
}

/// The error page's way out.
#[derive(Debug, Clone)]
pub struct Back {
    pub href: String,
    pub label: String,
}

impl Back {
    pub fn home() -> Back {
        Back {
            href: "/".into(),
            label: "All charts".into(),
        }
    }

    pub fn chart(name: &str) -> Back {
        Back {
            href: format!("/{name}"),
            label: format!("All {name} versions"),
        }
    }
}

impl AppError {
    pub fn not_found(message: impl Into<String>) -> AppError {
        AppError::NotFound {
            message: message.into(),
            back: Back::home(),
        }
    }
}

#[derive(Template)]
#[template(path = "error.html")]
struct ErrorPage<'a> {
    status: u16,
    title: &'a str,
    message: &'a str,
    back: Back,
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, title, back) = match &self {
            AppError::NotFound { back, .. } => (StatusCode::NOT_FOUND, "Not found", back.clone()),
            AppError::Upstream { back, .. } => (
                StatusCode::BAD_GATEWAY,
                "Upstream unavailable",
                back.clone(),
            ),
            AppError::Internal(_) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Something went wrong",
                Back::home(),
            ),
        };
        if status.is_server_error() {
            tracing::error!(error = %self, "request failed");
        }
        let message = self.to_string();
        let page = ErrorPage {
            status: status.as_u16(),
            title,
            message: &message,
            back,
        };
        match page.render() {
            Ok(html) => (status, Html(html)).into_response(),
            Err(_) => (status, message).into_response(),
        }
    }
}

impl From<askama::Error> for AppError {
    fn from(e: askama::Error) -> Self {
        AppError::Internal(format!("template error: {e}"))
    }
}

#[derive(RustEmbed)]
#[folder = "assets/"]
#[include = "dist/*"]
#[include = "js/*"]
#[include = "vendor/htmx/*.js"]
struct Assets;

fn asset_version() -> String {
    let mut hasher = DefaultHasher::new();
    for file in Assets::iter() {
        if let Some(content) = Assets::get(&file) {
            content.data.hash(&mut hasher);
        }
    }
    format!("{:x}", hasher.finish() & 0xffff_ffff)
}

async fn asset(Path(path): Path<String>) -> Response {
    match Assets::get(&path) {
        Some(content) => {
            let mime = mime_guess::from_path(&path).first_or_octet_stream();
            (
                [
                    (header::CONTENT_TYPE, mime.as_ref().to_string()),
                    (
                        header::CACHE_CONTROL,
                        "public, max-age=31536000, immutable".to_string(),
                    ),
                ],
                Body::from(content.data.into_owned()),
            )
                .into_response()
        }
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

/// Images in release notes are served from GitHub; nothing else is remote.
const CSP: &str = "default-src 'self'; script-src 'self'; style-src 'self'; \
img-src 'self' data: https://github.com https://*.githubusercontent.com; \
connect-src 'self'; font-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'self'";

/// Security headers on every response; `no-store` on everything but assets.
async fn security_headers(req: Request, next: Next) -> Response {
    let is_asset = req.uri().path().starts_with("/assets/");
    let mut res = next.run(req).await;
    let h = res.headers_mut();
    h.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(CSP),
    );
    h.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    h.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    h.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("same-origin"),
    );
    if !is_asset {
        h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    }
    res
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/", get(pages::index))
        .route("/search", get(pages::search))
        .route("/assets/{*path}", get(asset))
        .route("/{chart}", get(pages::chart))
        .fallback(|| async { AppError::not_found("page not found") })
        .layer(middleware::from_fn(security_headers))
        .layer(TraceLayer::new_for_http().make_span_with(|req: &Request| {
            tracing::info_span!("http", method = %req.method(), path = %req.uri().path())
        }))
        .with_state(state)
}

/// `/metrics`, `/healthz` and `/readyz`, served on their own port. There is
/// nothing to sync before serving, so ready means up.
pub fn ops_router(metrics: Arc<Metrics>) -> Router {
    Router::new()
        .route(
            "/metrics",
            get(move || {
                let metrics = metrics.clone();
                async move {
                    (
                        [(
                            header::CONTENT_TYPE,
                            "application/openmetrics-text; version=1.0.0; charset=utf-8",
                        )],
                        metrics.encode(),
                    )
                }
            }),
        )
        .route("/healthz", get(|| async { "ok" }))
        .route("/readyz", get(|| async { "ready" }))
}
