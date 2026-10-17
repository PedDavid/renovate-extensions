//! Command-line flags, each also settable through a `CHART_NOTES_*` environment variable.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use clap::Parser;
use url::Url;

#[derive(Parser, Debug, Clone)]
#[command(
    name = "chart-notes",
    version,
    about = "The app release notes behind a Helm chart bump, for Renovate PRs"
)]
pub struct Config {
    /// Address for the UI.
    #[arg(long, env = "CHART_NOTES_LISTEN", default_value = "0.0.0.0:8080")]
    pub listen: SocketAddr,

    /// Address for /metrics, /healthz and /readyz.
    #[arg(
        long,
        env = "CHART_NOTES_METRICS_LISTEN",
        default_value = "0.0.0.0:9090"
    )]
    pub metrics_listen: SocketAddr,

    /// URL the UI is reached at, for the Renovate config it shows.
    #[arg(long, env = "CHART_NOTES_PUBLIC_URL")]
    pub public_url: Option<Url>,

    /// Charts (YAML or JSON) on top of the bundled list, replacing entries
    /// with the same name.
    #[arg(long, env = "CHART_NOTES_CHARTS_FILE")]
    pub charts_file: Option<PathBuf>,

    /// File holding a GitHub token. Unauthenticated, GitHub allows 60 API
    /// requests an hour; any token, even without scopes, allows 5000.
    #[arg(long, env = "CHART_NOTES_GITHUB_TOKEN_FILE")]
    pub github_token_file: Option<PathBuf>,

    /// How long Helm indexes and GitHub releases are cached, e.g. 10m.
    #[arg(long, env = "CHART_NOTES_CACHE_TTL", default_value = "10m", value_parser = parse_duration)]
    pub cache_ttl: Duration,

    #[arg(long, env = "CHART_NOTES_GITHUB_API_URL", default_value = crate::sources::GITHUB_API, hide = true)]
    pub github_api_url: Url,

    /// Serve sample data from memory, without network access.
    #[arg(long, env = "CHART_NOTES_DEMO")]
    pub demo: bool,

    /// Log as JSON.
    #[arg(long, env = "CHART_NOTES_LOG_JSON")]
    pub log_json: bool,
}

/// `90s`, `10m`, `1h`, or anything else jiff's friendly format reads.
fn parse_duration(s: &str) -> Result<Duration, String> {
    let d: jiff::SignedDuration = s.parse().map_err(|e| format!("{s:?}: {e}"))?;
    Duration::try_from(d).map_err(|_| format!("{s:?} is negative"))
}

impl Config {
    pub fn github_token(&self) -> anyhow::Result<Option<String>> {
        let Some(path) = &self.github_token_file else {
            return Ok(None);
        };
        let token = std::fs::read_to_string(path)
            .map_err(|e| anyhow::anyhow!("reading {}: {e}", path.display()))?;
        Ok(Some(token.trim().to_string()).filter(|t| !t.is_empty()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_parse() {
        let cfg = Config::try_parse_from(["chart-notes"]).unwrap();
        assert_eq!(cfg.cache_ttl, Duration::from_secs(600));
        assert_eq!(cfg.github_api_url.as_str(), "https://api.github.com/");
        assert!(cfg.github_token().unwrap().is_none());
    }

    #[test]
    fn durations() {
        assert_eq!(parse_duration("90s"), Ok(Duration::from_secs(90)));
        assert_eq!(parse_duration("1h"), Ok(Duration::from_secs(3600)));
        assert!(parse_duration("-1m").is_err());
        assert!(parse_duration("soon").is_err());
    }
}
