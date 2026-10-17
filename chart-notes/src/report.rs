//! Everything about one chart bump: the chart's own changes and the app's
//! releases in between, with their notes. The notes page renders it and the
//! badge summarises it; computing it warms every cache the page uses.

use jiff::Timestamp;

use crate::notes::{self, Change, Direction};
use crate::resolve::{Hints, Resolution, Resolved, Resolver};
use crate::sources::{Candidate, ChartVersion, Release, Sources};

/// How many releases' notes a report loads. A bump spanning more lists the
/// rest with links only.
pub const MAX_NOTES: usize = 20;
/// How many chart versions' GitHub releases are fetched when the index has
/// no `artifacthub.io/changes` for them.
pub const MAX_CHART_RELEASES: usize = 10;

#[derive(Debug, Clone)]
pub enum Notes {
    /// Sanitized HTML.
    Shown(String),
    /// Published without notes.
    Empty,
    Failed,
    /// Past `MAX_NOTES`: linked, not loaded.
    Skipped,
}

#[derive(Debug, Clone)]
pub struct AppRelease {
    pub release: Release,
    pub notes: Notes,
}

#[derive(Debug, Clone)]
pub enum ChartNotes {
    /// From the index's `artifacthub.io/changes`.
    Changes(Vec<Change>),
    /// The chart repository's GitHub release for this version.
    Release(String),
    None,
}

#[derive(Debug, Clone)]
pub struct ChartRelease {
    pub version: ChartVersion,
    pub notes: ChartNotes,
    /// The GitHub release page, when the chart's repository is known.
    pub url: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Report {
    pub chart: Resolved,
    pub from: ChartVersion,
    pub to: ChartVersion,
    pub direction: Direction,
    pub chart_releases: Vec<ChartRelease>,
    pub app_releases: Vec<AppRelease>,
    /// The app's compare view; None without both app versions and the repo.
    pub compare_url: Option<String>,
    /// Why the app's releases couldn't be listed.
    pub releases_error: Option<String>,
    /// Why some notes couldn't be loaded (the first error).
    pub notes_error: Option<String>,
    /// Why some of what's shown is an older copy.
    pub stale: Vec<String>,
    /// The oldest of the upstream copies used.
    pub fetched_at: Timestamp,
}

impl Report {
    /// Whether an upstream failed: the report is missing or has older data.
    pub fn degraded(&self) -> bool {
        self.releases_error.is_some() || self.notes_error.is_some() || !self.stale.is_empty()
    }
}

#[derive(Debug, Clone, thiserror::Error)]
pub enum ReportError {
    #[error(
        "No Helm repository with a chart named {0:?} was found. Add it under `overrides` in the Helm chart's values."
    )]
    UnknownChart(String),
    #[error("Several Helm repositories have a chart named {name:?}.")]
    Ambiguous {
        name: String,
        candidates: Vec<Candidate>,
    },
    #[error("{0}")]
    UnknownVersion(String),
    #[error("{0}")]
    Upstream(String),
}

/// Finds the chart, or says why not.
pub async fn locate(
    resolver: &Resolver,
    name: &str,
    hints: &Hints,
) -> Result<Resolved, ReportError> {
    match resolver
        .resolve(name, hints)
        .await
        .map_err(|e| ReportError::Upstream(e.to_string()))?
        .as_ref()
    {
        Resolution::Found(r) => Ok(r.clone()),
        Resolution::Ambiguous(candidates) => Err(ReportError::Ambiguous {
            name: name.to_string(),
            candidates: candidates.clone(),
        }),
        Resolution::NotFound => Err(ReportError::UnknownChart(name.to_string())),
    }
}

pub async fn build(
    resolver: &Resolver,
    sources: &dyn Sources,
    name: &str,
    hints: &Hints,
    from: &str,
    to: &str,
) -> Result<Report, ReportError> {
    let chart = locate(resolver, name, hints).await?;
    let index = sources
        .index(&chart.repo, name)
        .await
        .map_err(|e| ReportError::Upstream(e.to_string()))?;
    let missing = |version: &str| {
        ReportError::UnknownVersion(format!(
            "{name} has no version {version:?} in {}.",
            crate::sources::index_url(&chart.repo)
        ))
    };
    let old = index.value.find(from).ok_or_else(|| missing(from))?.clone();
    let new = index.value.find(to).ok_or_else(|| missing(to))?.clone();

    let mut stale: Vec<String> = index.stale.iter().cloned().collect();
    let mut fetched_at = index.fetched_at;
    let mut notes_error = None;

    // The chart's own changes, and the app's releases, at the same time.
    let chart_side = chart_releases(sources, &chart, &index.value.versions, &old, &new);
    let app_side = app_releases(sources, &chart, &old, &new);
    let (chart_side, app_side) = futures::join!(chart_side, app_side);

    let (chart_releases, chart_error) = chart_side;
    if let Some(e) = chart_error {
        notes_error.get_or_insert(e);
    }
    let side = app_side;
    stale.extend(side.stale);
    if let Some(t) = side.fetched_at {
        fetched_at = fetched_at.min(t);
    }
    if let Some(e) = side.notes_error {
        notes_error.get_or_insert(e);
    }
    stale.sort();
    stale.dedup();

    let direction = match (&old.app_version, &new.app_version) {
        (Some(a), Some(b)) => notes::direction(a, b),
        _ => Direction::Unknown,
    };
    let compare_url = match (&chart.app, &old.app_version, &new.app_version) {
        (Some(app), Some(a), Some(b)) if direction != Direction::Same => {
            Some(notes::compare_url(app, a, b, &side.releases_for_compare))
        }
        _ => None,
    };
    Ok(Report {
        chart,
        from: old,
        to: new,
        direction,
        chart_releases,
        app_releases: side.releases,
        compare_url,
        releases_error: side.releases_error,
        notes_error,
        stale,
        fetched_at,
    })
}

struct AppSide {
    releases: Vec<AppRelease>,
    /// All the app's releases, for the compare URL's tag style.
    releases_for_compare: Vec<Release>,
    releases_error: Option<String>,
    notes_error: Option<String>,
    stale: Vec<String>,
    fetched_at: Option<Timestamp>,
}

async fn app_releases(
    sources: &dyn Sources,
    chart: &Resolved,
    old: &ChartVersion,
    new: &ChartVersion,
) -> AppSide {
    let mut side = AppSide {
        releases: Vec::new(),
        releases_for_compare: Vec::new(),
        releases_error: None,
        notes_error: None,
        stale: Vec::new(),
        fetched_at: None,
    };
    let (Some(app), Some(old_app), Some(new_app)) =
        (&chart.app, &old.app_version, &new.app_version)
    else {
        return side;
    };
    if notes::direction(old_app, new_app) == Direction::Same {
        return side;
    }
    let all = match sources.releases(app).await {
        Ok(r) => {
            side.stale.extend(r.stale.iter().cloned());
            side.fetched_at = Some(r.fetched_at);
            r.value.items.clone()
        }
        Err(e) => {
            side.releases_error = Some(e.to_string());
            return side;
        }
    };
    let between = notes::releases_between(&all, old_app, new_app);
    let loaded = futures::future::join_all(
        between
            .iter()
            .take(MAX_NOTES)
            .map(|r| sources.notes(app, &r.tag_name)),
    )
    .await;
    let mut loaded = loaded.into_iter();
    for release in between {
        let notes = match loaded.next() {
            None => Notes::Skipped,
            Some(Ok(n)) => {
                side.stale.extend(n.stale.iter().cloned());
                match &n.value.html {
                    Some(html) => Notes::Shown(html.clone()),
                    None => Notes::Empty,
                }
            }
            Some(Err(e)) => {
                side.notes_error.get_or_insert(e.to_string());
                Notes::Failed
            }
        };
        side.releases.push(AppRelease { release, notes });
    }
    side.releases_for_compare = all;
    side
}

/// The chart versions in the bump, with their changes: the index's
/// `artifacthub.io/changes` when any version has them, else the chart
/// repository's GitHub releases (chart-releaser tags them `<chart>-<version>`).
async fn chart_releases(
    sources: &dyn Sources,
    chart: &Resolved,
    versions: &[ChartVersion],
    old: &ChartVersion,
    new: &ChartVersion,
) -> (Vec<ChartRelease>, Option<String>) {
    let between = notes::chart_versions_between(versions, &old.version, &new.version);
    let tag = |v: &ChartVersion| format!("{}-{}", chart.name, v.version);
    let url = |v: &ChartVersion| {
        chart
            .source
            .as_ref()
            .map(|s| format!("https://github.com/{s}/releases/tag/{}", tag(v)))
    };
    let annotated = between.iter().any(|v| v.changes.is_some());
    if annotated || chart.source.is_none() {
        let releases = between
            .into_iter()
            .map(|v| ChartRelease {
                notes: match v.changes.as_deref().map(notes::parse_changes) {
                    Some(c) if !c.is_empty() => ChartNotes::Changes(c),
                    _ => ChartNotes::None,
                },
                url: url(v),
                version: v.clone(),
            })
            .collect();
        return (releases, None);
    }
    let source = chart.source.as_deref().unwrap_or_default();
    let tags: Vec<String> = between
        .iter()
        .take(MAX_CHART_RELEASES)
        .map(|v| tag(v))
        .collect();
    let loaded = futures::future::join_all(tags.iter().map(|t| sources.notes(source, t))).await;
    let mut loaded = loaded.into_iter();
    let mut error = None;
    let releases = between
        .into_iter()
        .map(|v| {
            let notes = match loaded.next() {
                Some(Ok(n)) => match &n.value.html {
                    Some(html) => ChartNotes::Release(html.clone()),
                    None => ChartNotes::None,
                },
                // No such release: the chart isn't released with notes.
                Some(Err(crate::sources::UpstreamError::Status { status: 404, .. })) | None => {
                    ChartNotes::None
                }
                Some(Err(e)) => {
                    error.get_or_insert(e.to_string());
                    ChartNotes::None
                }
            };
            ChartRelease {
                notes,
                url: url(v),
                version: v.clone(),
            }
        })
        .collect();
    (releases, error)
}

/// What the page header needs: quick once the chart's index is cached.
#[derive(Debug, Clone)]
pub struct Header {
    pub chart: Resolved,
    pub from: ChartVersion,
    pub to: ChartVersion,
    pub direction: Direction,
}

pub async fn header(
    resolver: &Resolver,
    sources: &dyn Sources,
    name: &str,
    hints: &Hints,
    from: &str,
    to: &str,
) -> Result<Header, ReportError> {
    let chart = locate(resolver, name, hints).await?;
    let index = sources
        .index(&chart.repo, name)
        .await
        .map_err(|e| ReportError::Upstream(e.to_string()))?;
    let missing = |version: &str| {
        ReportError::UnknownVersion(format!(
            "{name} has no version {version:?} in {}.",
            crate::sources::index_url(&chart.repo)
        ))
    };
    let old = index.value.find(from).ok_or_else(|| missing(from))?.clone();
    let new = index.value.find(to).ok_or_else(|| missing(to))?.clone();
    let direction = match (&old.app_version, &new.app_version) {
        (Some(a), Some(b)) => notes::direction(a, b),
        _ => Direction::Unknown,
    };
    Ok(Header {
        chart,
        from: old,
        to: new,
        direction,
    })
}

/// The default values' change between the two versions.
#[derive(Debug, Clone)]
pub struct ValuesDiff {
    pub summary: crate::values::Summary,
    pub hunks: Vec<crate::values::Hunk>,
    /// Why a side couldn't be read, if one couldn't.
    pub error: Option<String>,
}

async fn read_values(
    sources: &dyn Sources,
    chart: &Resolved,
    v: &ChartVersion,
) -> Result<String, String> {
    let Some(package) = &v.url else {
        return Err(format!("{} {} lists no package", chart.name, v.version));
    };
    match sources
        .values(&chart.repo, &chart.name, &v.version, package)
        .await
    {
        Ok(c) => Ok(c.value.text.clone().unwrap_or_default()),
        Err(e) => Err(e.to_string()),
    }
}

pub async fn values_diff(sources: &dyn Sources, header: &Header) -> ValuesDiff {
    let (old, new) = futures::join!(
        read_values(sources, &header.chart, &header.from),
        read_values(sources, &header.chart, &header.to)
    );
    match (old, new) {
        (Ok(a), Ok(b)) => ValuesDiff {
            summary: crate::values::summarize(&a, &b),
            hunks: crate::values::diff(&a, &b),
            error: None,
        },
        (Err(e), _) | (_, Err(e)) => ValuesDiff {
            summary: Default::default(),
            hunks: Vec::new(),
            error: Some(e),
        },
    }
}
