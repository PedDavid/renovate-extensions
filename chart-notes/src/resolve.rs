//! Which Helm repository a chart comes from, which GitHub repository has its
//! app's releases, and which one releases the chart itself.
//!
//! Renovate can't tell us the Helm repository, but it can send its
//! `{{sourceUrl}}`: the chart's own GitHub repository, worked out from the
//! index. From that, and Artifact Hub's list of repositories carrying a
//! chart of that name, the repository whose index gives the same source URL
//! is the one. Overrides cover what that gets wrong.
//!
//! Only URLs we derive are fetched: a GitHub Pages address built from a
//! GitHub `owner/repo`, the repositories Artifact Hub lists, and overrides.
//! A URL in a request is never fetched as given.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use url::Url;

use crate::cache::Cache;
use crate::overrides::{Override, Overrides, valid_repo};
use crate::sources::{Candidate, ChartIndex, ChartVersion, Sources, UpstreamError};

/// How many candidate repositories' indexes are fetched to match a source.
const MAX_PROBES: usize = 4;

/// A chart, located.
#[derive(Debug, Clone, PartialEq)]
pub struct Resolved {
    pub name: String,
    /// The Helm repository serving `index.yaml`.
    pub repo: Url,
    /// GitHub `owner/repo` with the app's releases, when known.
    pub app: Option<String>,
    /// GitHub `owner/repo` the chart is released from, when known.
    pub source: Option<String>,
    pub note: Option<String>,
}

impl Resolved {
    pub fn app_url(&self) -> Option<String> {
        self.app.as_ref().map(|a| format!("https://github.com/{a}"))
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Resolution {
    Found(Resolved),
    /// Several repositories carry a chart of this name and nothing tells
    /// which one is meant.
    Ambiguous(Vec<Candidate>),
    NotFound,
}

/// What a request says about where the chart comes from.
#[derive(Debug, Clone, Default)]
pub struct Hints {
    /// Renovate's `{{sourceUrl}}`.
    pub source: Option<String>,
    /// A Helm repository URL picked on the disambiguation page. Only used
    /// when it is one of the candidates.
    pub repo: Option<String>,
}

pub struct Resolver {
    overrides: Overrides,
    sources: Arc<dyn Sources>,
    cache: Cache<Resolution>,
    /// Charts found so far, for the front page and the search.
    seen: Mutex<BTreeMap<String, Resolved>>,
}

impl Resolver {
    pub fn new(overrides: Overrides, sources: Arc<dyn Sources>, ttl: Duration) -> Self {
        Resolver {
            overrides,
            sources,
            cache: Cache::new(ttl).forgetting_after(Duration::from_secs(7 * 24 * 3600)),
            seen: Mutex::new(BTreeMap::new()),
        }
    }

    pub fn overrides(&self) -> &Overrides {
        &self.overrides
    }

    /// Charts found so far, by name.
    pub fn seen(&self) -> Vec<Resolved> {
        self.seen
            .lock()
            .expect("seen lock poisoned")
            .values()
            .cloned()
            .collect()
    }

    pub async fn resolve(
        &self,
        name: &str,
        hints: &Hints,
    ) -> Result<Arc<Resolution>, UpstreamError> {
        let source = hints.source.as_deref().and_then(github_repo);
        let key = format!(
            "{name}|{}|{}",
            source.as_deref().unwrap_or_default(),
            hints.repo.as_deref().unwrap_or_default()
        );
        let resolution = self
            .cache
            .get(&key, |_| {
                self.resolve_uncached(name, source.as_deref(), hints)
            })
            .await?
            .value;
        if let Resolution::Found(r) = resolution.as_ref() {
            self.seen
                .lock()
                .expect("seen lock poisoned")
                .insert(r.name.clone(), r.clone());
        }
        Ok(resolution)
    }

    async fn resolve_uncached(
        &self,
        name: &str,
        source: Option<&str>,
        hints: &Hints,
    ) -> Result<Resolution, UpstreamError> {
        let o = self.overrides.get(name).cloned().unwrap_or_default();
        if let Some(repo) = o.repo.clone() {
            let index = self.sources.index(&repo, name).await?;
            return Ok(Resolution::Found(finish(name, repo, &index.value, &o)));
        }

        // Candidates, most likely first: the GitHub Pages site of the
        // source repository (where chart-releaser publishes), then what
        // Artifact Hub lists.
        let mut urls: Vec<Url> = source.and_then(pages_url).into_iter().collect();
        let hub = self.sources.search(name).await;
        let listed: Vec<Candidate> = match &hub {
            Ok(c) => c.value.as_ref().clone(),
            Err(e) if !urls.is_empty() => {
                tracing::warn!(chart = name, error = %e, "Artifact Hub search failed");
                Vec::new()
            }
            Err(e) => return Err(e.clone()),
        };
        for c in &listed {
            if !urls.iter().any(|u| same_url(u, &c.repo)) {
                urls.push(c.repo.clone());
            }
        }
        // A repository picked on the disambiguation page goes first, if it
        // is one of the candidates (never fetched otherwise).
        if let Some(picked) = hints.repo.as_deref().and_then(|r| Url::parse(r).ok())
            && let Some(i) = urls.iter().position(|u| same_url(u, &picked))
        {
            let url = urls.remove(i);
            let index = self.sources.index(&url, name).await?;
            if index.value.versions.is_empty() {
                return Ok(Resolution::NotFound);
            }
            return Ok(Resolution::Found(finish(name, url, &index.value, &o)));
        }

        if let Some(source) = source {
            // The repository whose index gives Renovate's source URL.
            let mut first_with_chart = None;
            for url in urls.iter().take(MAX_PROBES) {
                let Ok(index) = self.sources.index(url, name).await else {
                    continue;
                };
                if index.value.versions.is_empty() {
                    continue;
                }
                let theirs = index.value.renovate_source_url();
                if theirs.as_deref().and_then(github_repo).as_deref() == Some(source) {
                    return Ok(Resolution::Found(finish(
                        name,
                        url.clone(),
                        &index.value,
                        &o,
                    )));
                }
                first_with_chart.get_or_insert((url.clone(), index));
            }
            // Renovate's source URL can be rewritten by its own rules: the
            // likeliest repository that has the chart at all.
            if let Some((url, index)) = first_with_chart {
                return Ok(Resolution::Found(finish(name, url, &index.value, &o)));
            }
            return Ok(Resolution::NotFound);
        }

        let pick = match listed.as_slice() {
            [] => return Ok(Resolution::NotFound),
            [one] => one,
            many => match only(many, |c| c.official).or_else(|| only(many, |c| c.verified)) {
                Some(c) => c,
                None => return Ok(Resolution::Ambiguous(listed.clone())),
            },
        };
        let index = self.sources.index(&pick.repo, name).await?;
        if index.value.versions.is_empty() {
            return Ok(Resolution::NotFound);
        }
        Ok(Resolution::Found(finish(
            name,
            pick.repo.clone(),
            &index.value,
            &o,
        )))
    }
}

fn only(candidates: &[Candidate], keep: impl Fn(&Candidate) -> bool) -> Option<&Candidate> {
    let mut matching = candidates.iter().filter(|c| keep(c));
    match (matching.next(), matching.next()) {
        (Some(c), None) => Some(c),
        _ => None,
    }
}

/// Fills in the app and the chart's own repository from the newest version's
/// metadata, unless overridden.
fn finish(name: &str, repo: Url, index: &ChartIndex, o: &Override) -> Resolved {
    let source = o
        .source
        .clone()
        .or_else(|| index.renovate_source_url().as_deref().and_then(github_repo));
    let app = o.app.clone().or_else(|| {
        index
            .versions
            .first()
            .and_then(|v| app_repo(v, source.as_deref()))
    });
    Resolved {
        name: name.to_string(),
        repo,
        app,
        source,
        note: o.note.clone(),
    }
}

/// The app's repository: the first GitHub repository among the chart's
/// `sources` and `home` that isn't the chart's own and doesn't look like a
/// chart repository.
fn app_repo(v: &ChartVersion, chart_source: Option<&str>) -> Option<String> {
    v.sources
        .iter()
        .chain(v.home.iter())
        .filter(|url| !looks_like_chart_repo(url))
        .filter_map(|url| github_repo(url))
        .find(|repo| Some(repo.as_str()) != chart_source)
}

/// `owner/repo` (lower case) of a GitHub URL, including the forms Renovate
/// rewrites links to in PR bodies (`redirect.github.com`, `togithub.com`).
pub fn github_repo(url: &str) -> Option<String> {
    let url = Url::parse(url.trim()).ok()?;
    if !matches!(url.scheme(), "https" | "http") {
        return None;
    }
    let host = url.host_str()?.to_ascii_lowercase();
    if !matches!(
        host.as_str(),
        "github.com" | "www.github.com" | "redirect.github.com" | "togithub.com"
    ) {
        return None;
    }
    let mut parts = url.path_segments()?.filter(|s| !s.is_empty());
    let owner = parts.next()?;
    let repo = parts.next()?;
    let repo = repo.strip_suffix(".git").unwrap_or(repo);
    let full = format!("{owner}/{repo}").to_ascii_lowercase();
    valid_repo(&full).then_some(full)
}

/// Where chart-releaser publishes `owner/repo`'s charts: its GitHub Pages
/// site. Built only from a validated `owner/repo`, so the host is always
/// `<owner>.github.io`.
fn pages_url(repo: &str) -> Option<Url> {
    let (owner, name) = repo.split_once('/')?;
    if !valid_repo(repo) || owner.contains('.') {
        return None;
    }
    Url::parse(&format!("https://{owner}.github.io/{name}/")).ok()
}

fn same_url(a: &Url, b: &Url) -> bool {
    a.as_str().trim_end_matches('/') == b.as_str().trim_end_matches('/')
}

/// Renovate's helm datasource rule for a chart repository: hosted on a known
/// forge, with "chart", "charts" or "helm" in the repository's name.
fn looks_like_chart_repo(url: &str) -> bool {
    let Ok(parsed) = Url::parse(url) else {
        return false;
    };
    let host = parsed.host_str().unwrap_or_default().to_ascii_lowercase();
    let forge = [
        "github",
        "gitlab",
        "bitbucket",
        "gitea",
        "forgejo",
        "codeberg",
    ]
    .iter()
    .any(|f| host.contains(f));
    let name = parsed
        .path_segments()
        .and_then(|mut s| s.rfind(|p| !p.is_empty()))
        .unwrap_or_default()
        .trim_end_matches(".git")
        .to_ascii_lowercase();
    forge && (name.contains("chart") || name.contains("helm"))
}

/// Renovate's `{{sourceUrl}}` for a chart, from its newest index entry: the
/// GitHub repository its package is released from, else `home` or a source
/// that looks like a chart repository, else the first source.
pub fn renovate_source_url(v: &ChartVersion) -> Option<String> {
    if let Some(url) = &v.url
        && let Some(rest) = url.strip_prefix("https://github.com/")
    {
        let mut parts = rest.splitn(3, '/');
        if let (Some(owner), Some(repo), Some(tail)) = (parts.next(), parts.next(), parts.next())
            && tail.starts_with("releases/")
        {
            return Some(format!("https://github.com/{owner}/{repo}"));
        }
    }
    if let Some(home) = &v.home
        && looks_like_chart_repo(home)
    {
        return Some(home.clone());
    }
    v.sources
        .iter()
        .find(|s| looks_like_chart_repo(s))
        .or_else(|| v.sources.first())
        .cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn version(url: Option<&str>, home: Option<&str>, sources: &[&str]) -> ChartVersion {
        ChartVersion {
            version: "1.0.0".into(),
            url: url.map(str::to_string),
            home: home.map(str::to_string),
            sources: sources.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        }
    }

    #[test]
    fn github_repos_in_every_form_renovate_writes() {
        for url in [
            "https://github.com/grafana/helm-charts",
            "https://github.com/Grafana/helm-charts.git",
            "https://redirect.github.com/grafana/helm-charts",
            "https://togithub.com/grafana/helm-charts/tree/main/charts/alloy",
            "https://www.github.com/grafana/helm-charts/",
        ] {
            assert_eq!(
                github_repo(url).as_deref(),
                Some("grafana/helm-charts"),
                "{url}"
            );
        }
        assert_eq!(github_repo("https://gitlab.com/a/b"), None);
        assert_eq!(github_repo("https://github.com/grafana"), None);
        assert_eq!(github_repo("not a url"), None);
    }

    #[test]
    fn source_url_as_renovate_computes_it() {
        // From the package's GitHub release download.
        let v = version(
            Some(
                "https://github.com/grafana/helm-charts/releases/download/alloy-1.4.0/alloy-1.4.0.tgz",
            ),
            Some("https://grafana.com/oss/alloy"),
            &["https://github.com/grafana/alloy"],
        );
        assert_eq!(
            renovate_source_url(&v).as_deref(),
            Some("https://github.com/grafana/helm-charts")
        );
        // Else home or a source that looks like a chart repo.
        let v = version(
            Some("https://charts.example.com/x-1.0.0.tgz"),
            None,
            &[
                "https://github.com/x/app",
                "https://github.com/x/helm-charts",
            ],
        );
        assert_eq!(
            renovate_source_url(&v).as_deref(),
            Some("https://github.com/x/helm-charts")
        );
        // Else the first source.
        let v = version(None, None, &["https://github.com/x/app"]);
        assert_eq!(
            renovate_source_url(&v).as_deref(),
            Some("https://github.com/x/app")
        );
    }

    #[test]
    fn app_is_the_source_that_isnt_the_chart() {
        let v = version(
            None,
            Some("https://grafana.com/oss/alloy"),
            &[
                "https://github.com/grafana/helm-charts",
                "https://github.com/grafana/alloy",
            ],
        );
        assert_eq!(
            app_repo(&v, Some("grafana/helm-charts")).as_deref(),
            Some("grafana/alloy")
        );
        let only_chart = version(None, None, &["https://github.com/x/charts"]);
        assert_eq!(app_repo(&only_chart, Some("x/charts")), None);
    }

    #[test]
    fn pages_urls_are_github_io_only() {
        assert_eq!(
            pages_url("grafana/helm-charts").unwrap().as_str(),
            "https://grafana.github.io/helm-charts/"
        );
        assert_eq!(pages_url("evil.example/x"), None);
        assert_eq!(pages_url("../x"), None);
    }
}
