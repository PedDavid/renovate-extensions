//! Page handlers.

use askama::Template;
use axum::extract::{Path, Query, State};
use axum::response::{Html, IntoResponse, Response};
use axum_extra::extract::CookieJar;
use serde::Deserialize;

use super::views::{ChartRow, ReleaseRow, VersionRow, fmt_date, relative, version_rows};
use super::{AppError, AppState, Back};
use crate::charts::Chart;
use crate::notes::{self, Direction};
use crate::sources::{ChartVersion, HelmIndex};

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
    fn new(state: &AppState, jar: &CookieJar, title: impl Into<String>) -> Layout {
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

fn chart_rows(state: &AppState) -> Vec<ChartRow> {
    state
        .inner
        .charts
        .iter()
        .map(|(name, chart)| ChartRow::new(name, chart))
        .collect()
}

#[derive(Template)]
#[template(path = "index.html")]
struct IndexPage {
    layout: Layout,
    charts: Vec<ChartRow>,
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
    format!(
        r#""packageRules": [
  {{
    "matchDatasources": ["helm"],
    // Renovate's default columns plus this one.
    "prBodyColumns": ["Package", "Type", "Update", "Change", "Pending", "App notes"],
    "prBodyDefinitions": {{
      "App notes": "[app notes]({base}/{{{{{{packageName}}}}}}?from={{{{{{currentVersion}}}}}}&to={{{{{{newVersion}}}}}})"
    }}
  }}
]"#
    )
}

pub async fn index(State(state): State<AppState>, jar: CookieJar) -> Result<Response, AppError> {
    Ok(Html(
        IndexPage {
            layout: Layout::new(&state, &jar, "Charts"),
            charts: chart_rows(&state),
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
    rows: Vec<ChartRow>,
}

/// Results for the command palette: charts matching `q`, or the first few.
pub async fn search(
    State(state): State<AppState>,
    Query(query): Query<SearchQuery>,
) -> Result<Response, AppError> {
    let q = query.q.trim().to_string();
    let mut rows: Vec<ChartRow> = chart_rows(&state)
        .into_iter()
        .filter(|r| q.is_empty() || r.matches(&q))
        .collect();
    rows.truncate(8);
    Ok(Html(SearchResults { rows }.render()?).into_response())
}

#[derive(Deserialize, Default)]
pub struct Bump {
    #[serde(default)]
    from: Option<String>,
    #[serde(default)]
    to: Option<String>,
}

/// `/<chart>`: its versions. `/<chart>?from=<version>&to=<version>`: what
/// changed in the app between them.
pub async fn chart(
    State(state): State<AppState>,
    jar: CookieJar,
    Path(name): Path<String>,
    Query(bump): Query<Bump>,
) -> Result<Response, AppError> {
    let from = bump.from.map(|s| s.trim().to_string()).unwrap_or_default();
    let to = bump.to.map(|s| s.trim().to_string()).unwrap_or_default();
    let notes = !from.is_empty() && !to.is_empty();
    let result = match state.inner.charts.get(&name) {
        None => Err(AppError::not_found(format!(
            "chart-notes doesn't know a chart named {name:?}. Add it to the chart's `charts` value."
        ))),
        Some(chart) if notes => notes_page(&state, &jar, &name, chart, &from, &to).await,
        Some(chart) => versions_page(&state, &jar, &name, chart, &from, &to).await,
    };
    if notes {
        state.inner.metrics.page(match &result {
            Ok((_, degraded)) if *degraded => "upstream_error",
            Ok(_) => "ok",
            Err(AppError::NotFound { .. }) if state.inner.charts.get(&name).is_none() => {
                "unknown_chart"
            }
            Err(AppError::NotFound { .. }) => "unknown_version",
            Err(_) => "upstream_error",
        });
    }
    result.map(|(page, _)| page)
}

async fn load_index(
    state: &AppState,
    name: &str,
    chart: &Chart,
) -> Result<crate::cache::Cached<HelmIndex>, AppError> {
    state
        .inner
        .sources
        .index(chart)
        .await
        .map_err(|e| AppError::Upstream {
            message: e.to_string(),
            back: Back::chart(name),
        })
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
    chart: &Chart,
    from: &str,
    to: &str,
) -> Result<(Response, bool), AppError> {
    let index = load_index(state, name, chart).await?;
    let now = state.now();
    // The form offers the newest 200; the table shows the newest 30.
    let mut rows = version_rows(name, index.value.versions(name), now, 200);
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
        chart: ChartRow::new(name, chart),
        rows,
        options,
        from,
        to,
        fetched_rel: relative(index.fetched_at, now),
        stale: index.stale.clone().unwrap_or_default(),
    };
    Ok((Html(page.render()?).into_response(), false))
}

/// How many releases' notes a page loads. A bump spanning more lists the
/// rest with links only.
const MAX_NOTES: usize = 20;

#[derive(Template)]
#[template(path = "notes.html")]
struct NotesPage {
    layout: Layout,
    chart: ChartRow,
    from: String,
    to: String,
    from_date: String,
    to_date: String,
    old_app: String,
    new_app: String,
    /// "same", "upgrade", "downgrade" or "unknown".
    direction: &'static str,
    releases: Vec<ReleaseRow>,
    compare_url: String,
    releases_url: String,
    /// Why the releases couldn't be listed.
    releases_error: String,
    /// Why some releases' notes couldn't be loaded (the first error).
    notes_error: String,
    /// Why some of what's shown is an older copy.
    stale: Vec<String>,
    fetched_rel: String,
}

async fn notes_page(
    state: &AppState,
    jar: &CookieJar,
    name: &str,
    chart: &Chart,
    from: &str,
    to: &str,
) -> Result<(Response, bool), AppError> {
    let index = load_index(state, name, chart).await?;
    let missing = |version: &str| AppError::NotFound {
        message: format!(
            "{name} has no version {version:?} in {}.",
            chart.index_url()
        ),
        back: Back::chart(name),
    };
    let old = index.value.find(name, from).ok_or_else(|| missing(from))?;
    let new = index.value.find(name, to).ok_or_else(|| missing(to))?;
    let app_version = |v: &ChartVersion| {
        v.app_version.clone().ok_or_else(|| AppError::NotFound {
            message: format!("{name} {} declares no appVersion.", v.version),
            back: Back::chart(name),
        })
    };
    let (old_app, new_app) = (app_version(old)?, app_version(new)?);
    let direction = notes::direction(&old_app, &new_app);
    let now = state.now();

    let mut stale: Vec<String> = index.stale.iter().cloned().collect();
    let mut fetched_at = index.fetched_at;
    let mut releases = Vec::new();
    let mut releases_error = String::new();
    if direction != Direction::Same {
        match state.inner.sources.releases(&chart.app).await {
            Ok(r) => {
                stale.extend(r.stale.iter().cloned());
                fetched_at = fetched_at.min(r.fetched_at);
                releases = r.value.items.clone();
            }
            Err(e) => releases_error = e.to_string(),
        }
    }
    let between = notes::releases_between(&releases, &old_app, &new_app);
    let mut rows: Vec<ReleaseRow> = between.iter().map(|r| ReleaseRow::new(r, now)).collect();
    // Each release's notes are a request of their own (cached): load the
    // newest ones, in parallel, and link to the rest.
    let loaded = futures::future::join_all(
        between
            .iter()
            .take(MAX_NOTES)
            .map(|r| state.inner.sources.notes(&chart.app, &r.tag_name)),
    )
    .await;
    let mut notes_error = String::new();
    for (row, result) in rows.iter_mut().zip(loaded) {
        match result {
            Ok(n) => {
                stale.extend(n.stale.iter().cloned());
                match &n.value.html {
                    Some(html) => {
                        row.notes_html = html.clone();
                        row.notes = "shown";
                    }
                    None => row.notes = "empty",
                }
            }
            Err(e) => {
                row.notes = "failed";
                if notes_error.is_empty() {
                    notes_error = e.to_string();
                }
            }
        }
    }
    stale.dedup();
    let degraded = !releases_error.is_empty() || !notes_error.is_empty() || !stale.is_empty();
    let page = NotesPage {
        layout: Layout::new(
            state,
            jar,
            format!("{name} {} → {}", old.version, new.version),
        ),
        chart: ChartRow::new(name, chart),
        from: old.version.clone(),
        to: new.version.clone(),
        from_date: old.created.map(fmt_date).unwrap_or_default(),
        to_date: new.created.map(fmt_date).unwrap_or_default(),
        compare_url: notes::compare_url(&chart.app, &old_app, &new_app, &releases),
        releases_url: format!("{}/releases", chart.app_url()),
        old_app,
        new_app,
        direction: match direction {
            Direction::Same => "same",
            Direction::Upgrade => "upgrade",
            Direction::Downgrade => "downgrade",
            Direction::Unknown => "unknown",
        },
        releases: rows,
        releases_error,
        notes_error,
        stale,
        fetched_rel: relative(fetched_at, now),
    };
    Ok((Html(page.render()?).into_response(), degraded))
}
