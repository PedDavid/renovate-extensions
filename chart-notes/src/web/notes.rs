//! The notes page: a header that only needs the chart's index, then its
//! content loaded into it (htmx): one timeline of the chart's and the app's
//! releases, filtered by tab, or the default values' diff. `full=1` renders
//! it all in one response instead.

use askama::Template;
use axum::extract::{Path, Query, State};
use axum::response::{Html, IntoResponse, Response};
use axum_extra::extract::CookieJar;
use jiff::Timestamp;

use super::pages::{Bump, Layout, report_error};
use super::views::{ChartReleaseRow, ChartRow, ReleaseRow, fmt_date, relative};
use super::{AppError, AppState};
use crate::notes::Direction;
use crate::overrides::valid_name;
use crate::report::{self, Header, Notes, Report, ReportError, ValuesDiff};

/// Links for one bump: the page and its fragments, keeping the chart's hints.
struct Links {
    name: String,
    query: String,
}

impl Links {
    fn new(chart: &ChartRow, from: &str, to: &str) -> Links {
        let mut q = url::form_urlencoded::Serializer::new(String::new());
        q.append_pair("from", from).append_pair("to", to);
        Links {
            name: chart.name.clone(),
            query: format!("{}&{}", q.finish(), chart.query),
        }
    }

    fn page(&self, tab: &str) -> String {
        if tab == "all" {
            format!("/{}?{}", self.name, self.query)
        } else {
            format!("/{}?{}&tab={tab}", self.name, self.query)
        }
    }

    fn fragment(&self, tab: &str) -> String {
        if tab == "values" {
            format!("/{}/values?{}", self.name, self.query)
        } else {
            format!("/{}/timeline?{}&tab={tab}", self.name, self.query)
        }
    }
}

pub struct Tab {
    pub id: &'static str,
    pub label: &'static str,
    pub count: String,
    pub selected: bool,
    pub page_href: String,
    pub fragment_href: String,
}

fn tabs(links: &Links, selected: &str, counts: [String; 4]) -> Vec<Tab> {
    let ids = [
        ("all", "All"),
        ("app", "App"),
        ("chart", "Chart"),
        ("values", "Values diff"),
    ];
    ids.into_iter()
        .zip(counts)
        .map(|((id, label), count)| Tab {
            id,
            label,
            count,
            selected: id == selected,
            page_href: links.page(id),
            fragment_href: links.fragment(id),
        })
        .collect()
}

fn direction_str(d: Direction) -> &'static str {
    match d {
        Direction::Same => "same",
        Direction::Upgrade => "upgrade",
        Direction::Downgrade => "downgrade",
        Direction::Unknown => "unknown",
    }
}

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
    /// Where the content comes from, when it isn't inline.
    content_href: String,
    /// The content, rendered, with `full=1`.
    content: String,
    full_href: String,
}

/// `/<chart>?from=…&to=…`: the page.
pub async fn page(
    state: &AppState,
    jar: &CookieJar,
    name: &str,
    bump: &Bump,
    from: &str,
    to: &str,
) -> Result<Response, AppError> {
    let header = report::header(
        &state.inner.resolver,
        state.inner.sources.as_ref(),
        name,
        &bump.hints(),
        from,
        to,
    )
    .await;
    let header = match header {
        Ok(h) => h,
        Err(e) => {
            count_error(state, &e);
            return Err(report_error(state, jar, name, from, to, e));
        }
    };
    let chart = ChartRow::new(&header.chart);
    let links = Links::new(&chart, &header.from.version, &header.to.version);
    let tab = bump.tab();
    let content = if bump.full() {
        content(state, &header, &links, tab, true).await?
    } else {
        String::new()
    };
    let page = NotesPage {
        layout: Layout::new(
            state,
            jar,
            format!("{} {} → {}", name, header.from.version, header.to.version),
        ),
        content_href: format!("{}&first=1", links.fragment(tab)),
        full_href: format!("{}&full=1", links.page(tab)),
        content,
        from: header.from.version.clone(),
        to: header.to.version.clone(),
        from_date: header.from.created.map(fmt_date).unwrap_or_default(),
        to_date: header.to.created.map(fmt_date).unwrap_or_default(),
        old_app: header.from.app_version.clone().unwrap_or_default(),
        new_app: header.to.app_version.clone().unwrap_or_default(),
        direction: direction_str(header.direction),
        chart,
    };
    Ok(Html(page.render()?).into_response())
}

fn count_error(state: &AppState, e: &ReportError) {
    state.inner.metrics.page(match e {
        ReportError::UnknownChart(_) | ReportError::Ambiguous { .. } => "unknown_chart",
        ReportError::UnknownVersion(_) => "unknown_version",
        ReportError::Upstream(_) => "upstream_error",
    });
}

/// The rendered tab content. `count` says whether this is the page's own
/// load (counted in metrics) rather than a tab switch.
async fn content(
    state: &AppState,
    header: &Header,
    links: &Links,
    tab: &str,
    count: bool,
) -> Result<String, AppError> {
    let report = report::build(
        &state.inner.resolver,
        state.inner.sources.as_ref(),
        &header.chart.name,
        &crate::resolve::Hints {
            source: header
                .chart
                .source
                .as_ref()
                .map(|s| format!("https://github.com/{s}")),
            repo: Some(header.chart.repo.to_string()),
        },
        &header.from.version,
        &header.to.version,
    )
    .await;
    if count {
        match &report {
            Ok(r) => state
                .inner
                .metrics
                .page(if r.degraded() { "upstream_error" } else { "ok" }),
            Err(e) => count_error(state, e),
        }
    }
    let report = match report {
        Ok(r) => r,
        Err(e) => {
            return Ok(ErrorFragment {
                message: e.to_string(),
            }
            .render()?);
        }
    };
    let counts = [
        (report.chart_releases.len() + report.app_releases.len()).to_string(),
        report.app_releases.len().to_string(),
        report.chart_releases.len().to_string(),
        String::new(),
    ];
    if tab == "values" {
        let diff = report::values_diff(state.inner.sources.as_ref(), header).await;
        let mut counts = counts;
        counts[3] = values_count(&diff);
        return Ok(values_fragment(&report, diff, tabs(links, tab, counts))?);
    }
    Ok(timeline_fragment(
        state,
        &report,
        tab,
        tabs(links, tab, counts),
    )?)
}

fn values_count(diff: &ValuesDiff) -> String {
    let s = &diff.summary;
    if diff.error.is_some() || (s.added.is_empty() && s.removed.is_empty() && s.changed.is_empty())
    {
        return String::new();
    }
    format!("+{} −{}", s.added.len(), s.removed.len())
}

#[derive(Template)]
#[template(path = "partials/content_error.html")]
struct ErrorFragment {
    message: String,
}

/// One entry of the timeline: a chart release or an app release.
pub struct Entry {
    pub chart: Option<ChartReleaseRow>,
    pub app: Option<ReleaseRow>,
    /// Open by default (the newest few).
    pub open: bool,
}

#[derive(Template)]
#[template(path = "partials/timeline.html")]
struct TimelineFragment {
    tabs: Vec<Tab>,
    tab: String,
    chart: ChartRow,
    entries: Vec<Entry>,
    direction: &'static str,
    old_app: String,
    new_app: String,
    compare_url: String,
    releases_url: String,
    releases_error: String,
    notes_error: String,
    stale: Vec<String>,
    fetched_rel: String,
    app_count: usize,
}

fn timeline_fragment(
    state: &AppState,
    r: &Report,
    tab: &str,
    tabs: Vec<Tab>,
) -> askama::Result<String> {
    let now = state.now();
    let chart = ChartRow::new(&r.chart);
    let mut dated: Vec<(Option<Timestamp>, Entry)> = Vec::new();
    if tab != "app" {
        for c in &r.chart_releases {
            dated.push((
                c.version.created,
                Entry {
                    chart: Some(ChartReleaseRow::new(c, now)),
                    app: None,
                    open: false,
                },
            ));
        }
    }
    if tab != "chart" {
        for a in &r.app_releases {
            let mut row = ReleaseRow::new(&a.release, now);
            match &a.notes {
                Notes::Shown(html) => {
                    row.notes_html = html.clone();
                    row.notes = "shown";
                }
                Notes::Empty => row.notes = "empty",
                Notes::Failed => row.notes = "failed",
                Notes::Skipped => row.notes = "skipped",
            }
            dated.push((
                a.release.published_at,
                Entry {
                    chart: None,
                    app: Some(row),
                    open: false,
                },
            ));
        }
    }
    // Newest first; undated ones last, in their own order.
    dated.sort_by_key(|(t, _)| std::cmp::Reverse(*t));
    let mut entries: Vec<Entry> = dated.into_iter().map(|(_, e)| e).collect();
    for e in entries.iter_mut().take(5) {
        e.open = true;
    }
    TimelineFragment {
        tabs,
        tab: tab.to_string(),
        releases_url: if chart.app_url.is_empty() {
            String::new()
        } else {
            format!("{}/releases", chart.app_url)
        },
        chart,
        entries,
        direction: direction_str(r.direction),
        old_app: r.from.app_version.clone().unwrap_or_default(),
        new_app: r.to.app_version.clone().unwrap_or_default(),
        compare_url: r.compare_url.clone().unwrap_or_default(),
        releases_error: r.releases_error.clone().unwrap_or_default(),
        notes_error: r.notes_error.clone().unwrap_or_default(),
        stale: r.stale.clone(),
        fetched_rel: relative(r.fetched_at, now),
        app_count: r.app_releases.len(),
    }
    .render()
}

#[derive(Template)]
#[template(path = "partials/values.html")]
struct ValuesFragment {
    tabs: Vec<Tab>,
    chart: ChartRow,
    from: String,
    to: String,
    diff: ValuesDiff,
}

fn values_fragment(r: &Report, diff: ValuesDiff, tabs: Vec<Tab>) -> askama::Result<String> {
    ValuesFragment {
        tabs,
        chart: ChartRow::new(&r.chart),
        from: r.from.version.clone(),
        to: r.to.version.clone(),
        diff,
    }
    .render()
}

async fn fragment(
    state: AppState,
    name: String,
    bump: Bump,
    values: bool,
) -> Result<Response, AppError> {
    let (from, to) = (bump.from(), bump.to());
    if !valid_name(&name) || from.is_empty() || to.is_empty() {
        return Err(AppError::not_found("no such chart or versions"));
    }
    let header = match report::header(
        &state.inner.resolver,
        state.inner.sources.as_ref(),
        &name,
        &bump.hints(),
        &from,
        &to,
    )
    .await
    {
        Ok(h) => h,
        Err(e) => {
            if bump.first() {
                count_error(&state, &e);
            }
            return Ok(Html(
                ErrorFragment {
                    message: e.to_string(),
                }
                .render()?,
            )
            .into_response());
        }
    };
    let chart = ChartRow::new(&header.chart);
    let links = Links::new(&chart, &header.from.version, &header.to.version);
    let tab = if values { "values" } else { bump.tab() };
    let html = content(&state, &header, &links, tab, bump.first()).await?;
    Ok(Html(html).into_response())
}

/// `/<chart>/timeline?…&tab=all|app|chart`.
pub async fn timeline(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Query(bump): Query<Bump>,
) -> Result<Response, AppError> {
    fragment(state, name, bump, false).await
}

/// `/<chart>/values?…`.
pub async fn values(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Query(bump): Query<Bump>,
) -> Result<Response, AppError> {
    fragment(state, name, bump, true).await
}
