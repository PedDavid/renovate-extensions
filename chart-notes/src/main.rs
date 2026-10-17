use std::sync::Arc;

use clap::Parser;
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;

use chart_notes::charts::Charts;
use chart_notes::config::Config;
use chart_notes::demo;
use chart_notes::metrics::Metrics;
use chart_notes::sources::{HttpSources, Sources};
use chart_notes::web::{self, AppState};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cfg = Config::parse();
    init_tracing(cfg.log_json);

    let charts = if cfg.demo {
        warn!("demo mode: serving sample data from memory");
        demo::charts()
    } else {
        Charts::load(cfg.charts_file.as_deref())?
    };
    info!(charts = charts.len(), "charts loaded");
    let metrics = Arc::new(Metrics::new(charts.len()));
    let sources: Arc<dyn Sources> = if cfg.demo {
        Arc::new(demo::DemoSources)
    } else {
        let token = cfg.github_token()?;
        if token.is_none() {
            info!("no GitHub token: limited to 60 API requests an hour");
        }
        Arc::new(HttpSources::new(
            cfg.github_api_url.clone(),
            token,
            cfg.cache_ttl,
            metrics.clone(),
        )?)
    };

    let ops = web::ops_router(metrics.clone());
    let app = web::router(AppState::new(
        charts,
        sources,
        metrics,
        cfg.public_url.clone(),
        cfg.demo,
    ));

    let ui_listener = tokio::net::TcpListener::bind(cfg.listen).await?;
    let ops_listener = tokio::net::TcpListener::bind(cfg.metrics_listen).await?;
    info!(ui = %cfg.listen, ops = %cfg.metrics_listen, "listening");
    tokio::select! {
        r = axum::serve(ui_listener, app).with_graceful_shutdown(shutdown()) => r?,
        r = axum::serve(ops_listener, ops).with_graceful_shutdown(shutdown()) => r?,
    }
    Ok(())
}

fn init_tracing(json: bool) {
    let filter = EnvFilter::try_from_env("CHART_NOTES_LOG")
        .unwrap_or_else(|_| EnvFilter::new("info,tower_http=info"));
    let builder = tracing_subscriber::fmt().with_env_filter(filter);
    if json {
        builder.json().init();
    } else {
        builder.init();
    }
}

async fn shutdown() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let term = async {
        if let Ok(mut s) = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        {
            s.recv().await;
        }
    };
    #[cfg(not(unix))]
    let term = std::future::pending::<()>();
    tokio::select! {
        _ = ctrl_c => {},
        _ = term => {},
    }
}
