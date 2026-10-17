//! The charts chart-notes knows: which Helm repository serves each one and
//! which GitHub repository has its app's releases.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::Context;
use serde::Deserialize;
use url::Url;

/// The list shipped in the image.
const BUNDLED: &str = include_str!("../charts.yaml");

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Chart {
    /// Helm repository serving `index.yaml`.
    pub repo: Url,
    /// GitHub `owner/repo` with the app's releases.
    pub app: String,
    /// Caveat shown on every page for this chart.
    #[serde(default)]
    pub note: Option<String>,
}

impl Chart {
    pub fn index_url(&self) -> String {
        format!("{}/index.yaml", self.repo.as_str().trim_end_matches('/'))
    }

    pub fn app_url(&self) -> String {
        format!("https://github.com/{}", self.app)
    }
}

/// Charts by name (Renovate's `packageName`).
#[derive(Clone, Debug, Default)]
pub struct Charts(pub BTreeMap<String, Chart>);

impl Charts {
    /// The bundled list, with `extra` (YAML or JSON) adding to it or
    /// replacing whole entries.
    pub fn load(extra: Option<&Path>) -> anyhow::Result<Charts> {
        let mut charts = parse(BUNDLED).context("bundled charts.yaml")?;
        if let Some(path) = extra {
            let text = std::fs::read_to_string(path)
                .with_context(|| format!("reading {}", path.display()))?;
            let extra = parse(&text).with_context(|| format!("parsing {}", path.display()))?;
            charts.0.extend(extra.0);
        }
        Ok(charts)
    }

    pub fn get(&self, name: &str) -> Option<&Chart> {
        self.0.get(name)
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&String, &Chart)> {
        self.0.iter()
    }
}

pub fn parse(text: &str) -> anyhow::Result<Charts> {
    // An empty file (or a Helm value rendered as `{}`) is no charts.
    let charts: Option<BTreeMap<String, Chart>> = serde_yaml::from_str(text)?;
    let charts = charts.unwrap_or_default();
    for (name, chart) in &charts {
        validate(name, chart).with_context(|| format!("chart {name:?}"))?;
    }
    Ok(Charts(charts))
}

fn validate(name: &str, chart: &Chart) -> anyhow::Result<()> {
    let plain = |s: &str| {
        !s.is_empty()
            && s != "."
            && s != ".."
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    };
    anyhow::ensure!(plain(name), "name must be letters, digits, '-', '_' or '.'");
    // These paths belong to the UI itself.
    anyhow::ensure!(!matches!(name, "search" | "assets"), "{name:?} is reserved");
    anyhow::ensure!(
        matches!(chart.repo.scheme(), "https" | "http"),
        "repo must be an http(s) URL"
    );
    let parts: Vec<&str> = chart.app.split('/').collect();
    anyhow::ensure!(
        parts.len() == 2 && parts.iter().all(|p| plain(p)),
        "app must be a GitHub owner/repo, got {:?}",
        chart.app
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_charts_are_valid() {
        let charts = Charts::load(None).unwrap();
        assert!(charts.get("alloy").is_some());
        for (_, chart) in charts.iter() {
            assert_eq!(chart.repo.scheme(), "https");
        }
    }

    #[test]
    fn extra_file_adds_and_replaces() {
        let dir = std::env::temp_dir().join(format!("chart-notes-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("charts.json");
        // JSON, as the Helm chart renders it, is YAML too.
        std::fs::write(
            &path,
            r#"{"loki": {"repo": "https://charts.example.com", "app": "x/y"},
                "new": {"repo": "https://charts.example.com", "app": "a/b", "note": "hi"}}"#,
        )
        .unwrap();
        let charts = Charts::load(Some(&path)).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(charts.get("loki").unwrap().app, "x/y");
        assert_eq!(charts.get("new").unwrap().note.as_deref(), Some("hi"));
        assert!(charts.get("alloy").is_some());
    }

    #[test]
    fn empty_extra_is_no_charts() {
        assert!(parse("{}").unwrap().is_empty());
        assert!(parse("").unwrap().is_empty());
    }

    #[test]
    fn rejects_bad_entries() {
        for bad in [
            "x: {repo: 'ftp://example.com', app: a/b}",
            "x: {repo: 'https://example.com', app: 'a/b/c'}",
            "x: {repo: 'https://example.com', app: '../b'}",
            "x: {repo: 'https://example.com', app: a/b, typo: 1}",
            "'a b': {repo: 'https://example.com', app: a/b}",
            "search: {repo: 'https://example.com', app: a/b}",
        ] {
            assert!(parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn index_url_ignores_trailing_slash() {
        let chart = parse("x: {repo: 'https://example.com/charts/', app: a/b}").unwrap();
        assert_eq!(
            chart.get("x").unwrap().index_url(),
            "https://example.com/charts/index.yaml"
        );
    }
}
