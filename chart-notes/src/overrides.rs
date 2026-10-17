//! Per-chart overrides, for charts the automatic lookup gets wrong or can't
//! find (an unlisted repository, an app that isn't among the chart's
//! sources), or that need a note. Every field is optional.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::Context;
use serde::Deserialize;
use url::Url;

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Override {
    /// Helm repository serving `index.yaml`.
    #[serde(default)]
    pub repo: Option<Url>,
    /// GitHub `owner/repo` with the app's releases.
    #[serde(default)]
    pub app: Option<String>,
    /// GitHub `owner/repo` the chart itself is released from.
    #[serde(default)]
    pub source: Option<String>,
    /// Caveat shown on every page for this chart.
    #[serde(default)]
    pub note: Option<String>,
}

/// Overrides by chart name (Renovate's `packageName`).
#[derive(Clone, Debug, Default)]
pub struct Overrides(pub BTreeMap<String, Override>);

impl Overrides {
    /// Reads `path` (YAML or JSON); no file is no overrides.
    pub fn load(path: Option<&Path>) -> anyhow::Result<Overrides> {
        let Some(path) = path else {
            return Ok(Overrides::default());
        };
        let text =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        parse(&text).with_context(|| format!("parsing {}", path.display()))
    }

    pub fn get(&self, name: &str) -> Option<&Override> {
        self.0.get(name)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&String, &Override)> {
        self.0.iter()
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

pub fn parse(text: &str) -> anyhow::Result<Overrides> {
    // An empty file (or a Helm value rendered as `{}`) is no overrides.
    let all: Option<BTreeMap<String, Override>> = serde_yaml::from_str(text)?;
    let all = all.unwrap_or_default();
    for (name, o) in &all {
        validate(name, o).with_context(|| format!("chart {name:?}"))?;
    }
    Ok(Overrides(all))
}

/// Chart names as Helm allows them, minus the paths the UI uses itself.
pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        && !name.starts_with('.')
        && !matches!(name, "search" | "assets")
}

/// `owner/repo`, as GitHub allows them.
pub fn valid_repo(repo: &str) -> bool {
    let plain = |s: &str| {
        !s.is_empty()
            && !s.starts_with('.')
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    };
    matches!(repo.split_once('/'), Some((o, r)) if plain(o) && plain(r))
}

fn validate(name: &str, o: &Override) -> anyhow::Result<()> {
    anyhow::ensure!(
        valid_name(name),
        "name must be letters, digits, '-', '_' or '.' (and not search or assets)"
    );
    if let Some(repo) = &o.repo {
        anyhow::ensure!(
            matches!(repo.scheme(), "https" | "http"),
            "repo must be an http(s) URL"
        );
    }
    for (field, value) in [("app", &o.app), ("source", &o.source)] {
        if let Some(v) = value {
            anyhow::ensure!(
                valid_repo(v),
                "{field} must be a GitHub owner/repo, got {v:?}"
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_partial_overrides() {
        let o = parse(
            r#"
kube-prometheus-stack:
  app: prometheus-operator/prometheus-operator
  note: Only prometheus-operator's version.
private:
  repo: https://charts.internal.example
  app: me/private
"#,
        )
        .unwrap();
        assert_eq!(o.len(), 2);
        let kps = o.get("kube-prometheus-stack").unwrap();
        assert!(kps.repo.is_none());
        assert_eq!(
            kps.app.as_deref(),
            Some("prometheus-operator/prometheus-operator")
        );
    }

    #[test]
    fn empty_is_no_overrides() {
        assert!(parse("{}").unwrap().is_empty());
        assert!(parse("").unwrap().is_empty());
        assert!(Overrides::load(None).unwrap().is_empty());
    }

    #[test]
    fn rejects_bad_entries() {
        for bad in [
            "x: {repo: 'ftp://example.com'}",
            "x: {app: 'a/b/c'}",
            "x: {source: '../b'}",
            "x: {app: a/b, typo: 1}",
            "'a b': {app: a/b}",
            "search: {app: a/b}",
        ] {
            assert!(parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn names_and_repos() {
        assert!(valid_name("kube-prometheus-stack"));
        assert!(!valid_name("../etc"));
        assert!(!valid_name("assets"));
        assert!(valid_repo("grafana/helm-charts"));
        assert!(!valid_repo("grafana"));
        assert!(!valid_repo("grafana/.."));
    }
}
