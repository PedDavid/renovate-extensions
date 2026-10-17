//! Page handlers.

use askama::Template;
use axum::extract::{Path, Query, State};
use axum::response::{Html, IntoResponse, Response};
use axum_extra::extract::CookieJar;
use serde::Deserialize;

use super::views::{ChartRow, VersionRow, relative, version_rows};
use super::{AppError, AppState, Back};
use crate::overrides::valid_name;
use crate::report::{self, ReportError};
use crate::resolve::Hints;
use crate::sources::Candidate;

/// Data shared by every full page.
pub struct Layout {
    pub title: String,
    pub asset_version: String,
    pub dark: bool,
    pub palette: String,
    pub demo: bool,
}

pub const PALETTES: [&str; 5] = ["neutral", "blue", "green", "orange", "violet"];

impl Layout {
    pub(super) fn new(state: &AppState, jar: &CookieJar, title: impl Into<String>) -> Layout {
        let palette = jar
            .get("chart_notes_palette")
            .map(|c| c.value().to_string())
            .filter(|p| PALETTES.contains(&p.as_str()))
            .unwrap_or_else(|| "neutral".into());
        Layout {
            title: title.into(),
            asset_version: state.inner.asset_version.clone(),
            dark: jar
                .get("chart_notes_theme")
                .is_some_and(|c| c.value() == "dark"),
            palette,
            demo: state.inner.demo,
        }
    }
}

/// Charts looked up so far (there's no fixed list: any chart can be asked
/// for).
fn chart_rows(state: &AppState) -> Vec<ChartRow> {
    state
        .inner
        .resolver
        .seen()
        .iter()
        .map(ChartRow::new)
        .collect()
}

#[derive(Template)]
#[template(path = "index.html")]
struct IndexPage {
    layout: Layout,
    charts: Vec<ChartRow>,
    overrides: Vec<String>,
    renovate_config: String,
}

/// The `packageRules` entry that adds the column to Renovate's Helm PRs.
fn renovate_config(state: &AppState) -> String {
    let base = state
        .inner
        .public_url
        .as_ref()
        .map(|u| u.as_str().trim_end_matches('/').to_string())
        .unwrap_or_else(|| "https://chart-notes.example.com".into());
    let query = "from={{{currentVersion}}}&to={{{newVersion}}}&source={{{sourceUrl}}}";
    let page = format!("{base}/{{{{{{packageName}}}}}}?{query}");
    let badge = format!("{base}/{{{{{{packageName}}}}}}/badge.svg?{query}");
    format!(
        r#""packageRules": [
  {{
    "matchDatasources": ["helm"],
    // Renovate's default columns plus this one.
    "prBodyColumns": ["Package", "Type", "Update", "Change", "Pending", "Notes"],
    "prBodyDefinitions": {{
      "Notes": "[![release notes]({badge})]({page})"
    }}
  }}
]"#
    )
}

#[derive(Deserialize, Default)]
pub struct Lookup {
    #[serde(default)]
    chart: Option<String>,
}

pub async fn index(
    State(state): State<AppState>,
    jar: CookieJar,
    Query(lookup): Query<Lookup>,
) -> Result<Response, AppError> {
    // The front page's "look up a chart" form.
    if let Some(name) = lookup.chart.as_deref().map(str::trim)
        && valid_name(name)
    {
        return Ok(axum::response::Redirect::to(&format!("/{name}")).into_response());
    }
    Ok(Html(
        IndexPage {
            layout: Layout::new(&state, &jar, "Charts"),
            charts: chart_rows(&state),
            overrides: state
                .inner
                .resolver
                .overrides()
                .iter()
                .map(|(name, _)| name.clone())
                .collect(),
            renovate_config: renovate_config(&state),
        }
        .render()?,
    )
    .into_response())
}

#[derive(Deserialize, Default)]
pub struct SearchQuery {
    #[serde(default)]
    q: String,
}

#[derive(Template)]
#[template(path = "partials/search_results.html")]
struct SearchResults {
    rows: Vec<SearchRow>,
}

struct SearchRow {
    name: String,
    href: String,
    detail: String,
}

/// Results for the command palette: charts looked up or overridden that
/// match `q`, and a way to look `q` up when it's a chart name.
pub async fn search(
    State(state): State<AppState>,
    Query(query): Query<SearchQuery>,
) -> Result<Response, AppError> {
    let q = query.q.trim().to_string();
    let mut rows: Vec<SearchRow> = chart_rows(&state)
        .into_iter()
        .filter(|r| q.is_empty() || r.matches(&q))
        .map(|r| SearchRow {
            href: r.href(),
            detail: if r.app.is_empty() { r.repo_host } else { r.app },
            name: r.name,
        })
        .collect();
    for (name, _) in state.inner.resolver.overrides().iter() {
        if !rows.iter().any(|r| &r.name == name)
            && (q.is_empty() || name.to_lowercase().contains(&q.to_lowercase()))
        {
            rows.push(SearchRow {
                href: format!("/{name}"),
                detail: "overridden".into(),
                name: name.clone(),
            });
        }
    }
    rows.truncate(8);
    if valid_name(&q) && !rows.iter().any(|r| r.name == q) {
        rows.push(SearchRow {
            href: format!("/{q}"),
            detail: "look up".into(),
            name: q,
        });
    }
    Ok(Html(SearchResults { rows }.render()?).into_response())
}

#[derive(Deserialize, Default, Clone)]
pub struct Bump {
    #[serde(default)]
    from: Option<String>,
    #[serde(default)]
    to: Option<String>,
    /// Renovate's `{{sourceUrl}}`.
    #[serde(default)]
    source: Option<String>,
    /// A repository picked among several.
    #[serde(default)]
    repo: Option<String>,
    /// all, app, chart or values.
    #[serde(default)]
    tab: Option<String>,
    /// Render everything in one response, without htmx.
    #[serde(default)]
    full: Option<String>,
    /// The page's own first request for its content (counted in metrics;
    /// switching tabs isn't).
    #[serde(default)]
    first: Option<String>,
}

impl Bump {
    pub fn from(&self) -> String {
        self.from.as_deref().unwrap_or_default().trim().to_string()
    }

    pub fn to(&self) -> String {
        self.to.as_deref().unwrap_or_default().trim().to_string()
    }

    pub fn tab(&self) -> &'static str {
        match self.tab.as_deref() {
            Some("app") => "app",
            Some("chart") => "chart",
            Some("values") => "values",
            _ => "all",
        }
    }

    pub fn full(&self) -> bool {
        self.full.is_some()
    }

    pub fn first(&self) -> bool {
        self.first.is_some()
    }

    pub fn hints(&self) -> Hints {
        let clean = |v: &Option<String>| {
            v.as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty() && s.len() <= 512)
                .map(str::to_string)
        };
        Hints {
            source: clean(&self.source),
            repo: clean(&self.repo),
        }
    }
}

/// `/<chart>`: its versions. `/<chart>?from=<version>&to=<version>`: what
/// changed in the chart and in the app between them.
pub async fn chart(
    State(state): State<AppState>,
    jar: CookieJar,
    Path(name): Path<String>,
    Query(bump): Query<Bump>,
) -> Result<Response, AppError> {
    if !valid_name(&name) {
        return Err(AppError::not_found(format!("{name:?} isn't a chart name.")));
    }
    let (from, to) = (bump.from(), bump.to());
    let hints = bump.hints();
    if from.is_empty() || to.is_empty() {
        return versions_page(&state, &jar, &name, &hints, &from, &to).await;
    }
    super::notes::page(&state, &jar, &name, &bump, &from, &to).await
}

#[derive(Template)]
#[template(path = "ambiguous.html")]
struct AmbiguousPage {
    layout: Layout,
    name: String,
    candidates: Vec<CandidateRow>,
}

struct CandidateRow {
    display_name: String,
    host: String,
    href: String,
    official: bool,
    verified: bool,
}

/// The error page for a failed report, or the page to pick a repository.
pub(super) fn report_error(
    state: &AppState,
    jar: &CookieJar,
    name: &str,
    from: &str,
    to: &str,
    e: ReportError,
) -> AppError {
    match e {
        ReportError::Ambiguous { candidates, .. } => {
            let page = AmbiguousPage {
                layout: Layout::new(state, jar, name),
                name: name.to_string(),
                candidates: candidate_rows(name, from, to, &candidates),
            };
            match page.render() {
                Ok(html) => AppError::Page(html),
                Err(e) => e.into(),
            }
        }
        ReportError::UnknownVersion(message) => AppError::NotFound {
            message,
            back: Back::chart(name),
        },
        ReportError::UnknownChart(_) => AppError::not_found(e.to_string()),
        ReportError::Upstream(message) => AppError::Upstream {
            message,
            back: Back::home(),
        },
    }
}

fn candidate_rows(name: &str, from: &str, to: &str, candidates: &[Candidate]) -> Vec<CandidateRow> {
    candidates
        .iter()
        .map(|c| {
            let mut q = url::form_urlencoded::Serializer::new(String::new());
            if !from.is_empty() && !to.is_empty() {
                q.append_pair("from", from).append_pair("to", to);
            }
            q.append_pair("repo", c.repo.as_str());
            CandidateRow {
                display_name: c.display_name.clone(),
                host: c.repo.host_str().unwrap_or_default().to_string(),
                href: format!("/{name}?{}", q.finish()),
                official: c.official,
                verified: c.verified,
            }
        })
        .collect()
}

#[derive(Template)]
#[template(path = "chart.html")]
struct VersionsPage {
    layout: Layout,
    chart: ChartRow,
    rows: Vec<VersionRow>,
    /// Every version, for the form.
    options: Vec<String>,
    from: String,
    to: String,
    fetched_rel: String,
    stale: String,
}

async fn versions_page(
    state: &AppState,
    jar: &CookieJar,
    name: &str,
    hints: &Hints,
    from: &str,
    to: &str,
) -> Result<Response, AppError> {
    let chart = report::locate(&state.inner.resolver, name, hints)
        .await
        .map_err(|e| report_error(state, jar, name, "", "", e))?;
    let index = state
        .inner
        .sources
        .index(&chart.repo, name)
        .await
        .map_err(|e| AppError::Upstream {
            message: e.to_string(),
            back: Back::home(),
        })?;
    let now = state.now();
    let row = ChartRow::new(&chart);
    // The form offers the newest 200; the table shows the newest 30.
    let mut rows = version_rows(&row, &index.value.versions, now, 200);
    let options: Vec<String> = rows.iter().map(|r| r.version.clone()).collect();
    rows.truncate(30);
    // Preselect the latest bump when nothing is picked yet.
    let to = if to.is_empty() {
        options.first().cloned().unwrap_or_default()
    } else {
        to.to_string()
    };
    let from = if from.is_empty() {
        options.get(1).cloned().unwrap_or_default()
    } else {
        from.to_string()
    };
    let page = VersionsPage {
        layout: Layout::new(state, jar, name),
        chart: row,
        rows,
        options,
        from,
        to,
        fetched_rel: relative(index.fetched_at, now),
        stale: index.stale.clone().unwrap_or_default(),
    };
    Ok(Html(page.render()?).into_response())
}
