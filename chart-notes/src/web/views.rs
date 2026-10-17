//! View models: charts, versions and releases turned into pre-formatted
//! rows for the templates.

use jiff::{SignedDuration, Timestamp};

use crate::notes::Change;
use crate::report::{ChartNotes, ChartRelease};
use crate::resolve::Resolved;
use crate::sources::{ChartVersion, Release};
use crate::version;

pub fn fmt_date(t: Timestamp) -> String {
    t.strftime("%Y-%m-%d").to_string()
}

pub fn humanize(d: SignedDuration) -> String {
    let secs = d.as_secs().abs();
    let (value, unit) = if secs >= 86_400 {
        (secs / 86_400, "d")
    } else if secs >= 3600 {
        (secs / 3600, "h")
    } else if secs >= 60 {
        (secs / 60, "m")
    } else {
        (secs, "s")
    };
    format!("{value}{unit}")
}

/// "in 12d", "3d ago", "now".
pub fn relative(t: Timestamp, now: Timestamp) -> String {
    let d = t.duration_since(now);
    if d.abs() < SignedDuration::from_secs(60) {
        "now".into()
    } else if d.is_positive() {
        format!("in {}", humanize(d))
    } else {
        format!("{} ago", humanize(d))
    }
}

/// Only http(s) URLs are rendered as links (never `javascript:` and co).
pub fn safe_link(url: &str) -> Option<String> {
    let parsed = url::Url::parse(url).ok()?;
    matches!(parsed.scheme(), "http" | "https").then(|| parsed.to_string())
}

#[derive(Clone, Debug)]
pub struct ChartRow {
    pub name: String,
    /// GitHub `owner/repo`, empty when unknown.
    pub app: String,
    pub app_url: String,
    pub repo: String,
    pub repo_host: String,
    /// The chart's own GitHub `owner/repo`, empty when unknown.
    pub source: String,
    pub note: String,
    /// `source=…&repo=…`: carried on links so the next page finds the
    /// chart the same way.
    pub query: String,
}

impl ChartRow {
    pub fn new(chart: &Resolved) -> ChartRow {
        let mut query = url::form_urlencoded::Serializer::new(String::new());
        if let Some(s) = &chart.source {
            query.append_pair("source", &format!("https://github.com/{s}"));
        }
        query.append_pair("repo", chart.repo.as_str());
        ChartRow {
            name: chart.name.clone(),
            app: chart.app.clone().unwrap_or_default(),
            app_url: chart.app_url().unwrap_or_default(),
            repo: chart.repo.to_string(),
            repo_host: chart.repo.host_str().unwrap_or_default().into(),
            source: chart.source.clone().unwrap_or_default(),
            note: chart.note.clone().unwrap_or_default(),
            query: query.finish(),
        }
    }

    pub fn href(&self) -> String {
        format!("/{}?{}", self.name, self.query)
    }

    pub fn matches(&self, q: &str) -> bool {
        let q = q.to_lowercase();
        [&self.name, &self.app]
            .into_iter()
            .any(|f| f.to_lowercase().contains(&q))
    }
}

pub struct VersionRow {
    pub version: String,
    pub app_version: String,
    pub created_date: String,
    pub created_rel: String,
    /// Notes for the bump from the previous version to this one.
    pub notes_href: Option<String>,
    pub app_changed: bool,
}

/// The newest `limit` versions, newest first, each linked to the bump from
/// the one before it.
pub fn version_rows(
    chart: &ChartRow,
    versions: &[ChartVersion],
    now: Timestamp,
    limit: usize,
) -> Vec<VersionRow> {
    let mut sorted: Vec<&ChartVersion> = versions.iter().collect();
    // Newest version first; any that don't parse last, in the index's order.
    sorted.sort_by_cached_key(|v| std::cmp::Reverse(version::parse(&v.version).map(Precedence)));
    sorted
        .iter()
        .enumerate()
        .take(limit)
        .map(|(i, v)| {
            let previous = sorted.get(i + 1);
            VersionRow {
                version: v.version.clone(),
                app_version: v.app_version.clone().unwrap_or_default(),
                created_date: v.created.map(fmt_date).unwrap_or_default(),
                created_rel: v.created.map(|t| relative(t, now)).unwrap_or_default(),
                notes_href: previous.map(|p| notes_href(chart, &p.version, &v.version)),
                app_changed: previous.is_some_and(|p| p.app_version != v.app_version),
            }
        })
        .collect()
}

/// Orders versions by semver precedence, ignoring build metadata.
struct Precedence(semver::Version);

impl PartialEq for Precedence {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other).is_eq()
    }
}

impl Eq for Precedence {}

impl PartialOrd for Precedence {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Precedence {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.0.cmp_precedence(&other.0)
    }
}

pub fn notes_href(chart: &ChartRow, from: &str, to: &str) -> String {
    format!(
        "/{}?from={}&to={}&{}",
        chart.name,
        query_value(from),
        query_value(to),
        chart.query
    )
}

/// Versions are mostly safe in a query, but `+` (build metadata) would read
/// back as a space.
fn query_value(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}

pub struct ReleaseRow {
    pub title: String,
    pub tag: String,
    pub url: Option<String>,
    pub published_date: String,
    pub published_rel: String,
    /// Sanitized HTML (see `notes::sanitize`), empty when there's none.
    pub notes_html: String,
    /// "shown", "empty" (published without notes), "failed" or "skipped"
    /// (past the number of releases whose notes are loaded).
    pub notes: &'static str,
}

impl ReleaseRow {
    pub fn new(r: &Release, now: Timestamp) -> ReleaseRow {
        let title = r
            .name
            .as_deref()
            .map(str::trim)
            .filter(|n| !n.is_empty())
            .unwrap_or(&r.tag_name);
        ReleaseRow {
            title: title.into(),
            tag: r.tag_name.clone(),
            url: safe_link(&r.html_url),
            published_date: r.published_at.map(fmt_date).unwrap_or_default(),
            published_rel: r.published_at.map(|t| relative(t, now)).unwrap_or_default(),
            notes_html: String::new(),
            notes: "skipped",
        }
    }

    /// Whether the title says more than the tag.
    pub fn titled(&self) -> bool {
        self.title != self.tag
    }
}

/// One of the chart's own versions in a bump, with its changes.
pub struct ChartReleaseRow {
    pub version: String,
    pub app_version: String,
    pub created_date: String,
    pub created_rel: String,
    pub url: Option<String>,
    /// "changes", "release" or "none".
    pub kind: &'static str,
    pub changes: Vec<ChangeRow>,
    /// Sanitized HTML of the chart's GitHub release.
    pub html: String,
}

pub struct ChangeRow {
    /// added, changed, … or empty.
    pub kind: String,
    /// Escaped HTML: the description with `code` spans.
    pub description_html: String,
    pub links: Vec<(String, String)>,
}

impl ChangeRow {
    fn new(c: &Change) -> ChangeRow {
        ChangeRow {
            kind: c.kind.clone().unwrap_or_default(),
            description_html: code_spans(&c.description),
            links: c.links.clone(),
        }
    }
}

fn escape_html(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

/// Escapes `text`, turning `backticked` spans into `<code>`, the one bit of
/// Markdown change descriptions commonly use.
pub fn code_spans(text: &str) -> String {
    let parts: Vec<&str> = text.split('`').collect();
    // An odd count of backticks leaves the last one literal.
    let closed = if parts.len().is_multiple_of(2) {
        parts.len() - 1
    } else {
        parts.len()
    };
    let mut out = String::new();
    for (i, part) in parts.iter().enumerate() {
        if i >= closed {
            out.push('`');
            out.push_str(&escape_html(part));
        } else if i % 2 == 1 {
            out.push_str("<code>");
            out.push_str(&escape_html(part));
            out.push_str("</code>");
        } else {
            out.push_str(&escape_html(part));
        }
    }
    out
}

impl ChartReleaseRow {
    pub fn new(r: &ChartRelease, now: Timestamp) -> ChartReleaseRow {
        let (kind, changes, html) = match &r.notes {
            ChartNotes::Changes(c) => (
                "changes",
                c.iter().map(ChangeRow::new).collect(),
                String::new(),
            ),
            ChartNotes::Release(h) => ("release", Vec::new(), h.clone()),
            ChartNotes::None => ("none", Vec::new(), String::new()),
        };
        ChartReleaseRow {
            version: r.version.version.clone(),
            app_version: r.version.app_version.clone().unwrap_or_default(),
            created_date: r.version.created.map(fmt_date).unwrap_or_default(),
            created_rel: r
                .version
                .created
                .map(|t| relative(t, now))
                .unwrap_or_default(),
            url: r.url.as_deref().and_then(safe_link),
            kind,
            changes,
            html,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_http_links_are_rendered() {
        assert_eq!(
            safe_link("https://github.com/x/y").as_deref(),
            Some("https://github.com/x/y")
        );
        assert_eq!(safe_link("javascript:alert(1)"), None);
        assert_eq!(safe_link("not a url"), None);
    }

    #[test]
    fn code_spans_are_escaped() {
        assert_eq!(
            code_spans("Add `a<b>` & <i>x</i>"),
            "Add <code>a&lt;b&gt;</code> &amp; &lt;i&gt;x&lt;/i&gt;"
        );
        assert_eq!(code_spans("odd ` tick"), "odd ` tick");
        assert_eq!(code_spans("`a` and `b"), "<code>a</code> and `b");
    }

    #[test]
    fn formats() {
        let now: Timestamp = "2026-09-25T00:00:00Z".parse().unwrap();
        let later: Timestamp = "2026-10-07T00:00:00Z".parse().unwrap();
        assert_eq!(relative(later, now), "in 12d");
        assert_eq!(relative(now, later), "12d ago");
        let chart = ChartRow::new(&Resolved {
            name: "x".into(),
            repo: "https://charts.example.com".parse().unwrap(),
            app: None,
            source: Some("x/charts".into()),
            note: None,
        });
        assert_eq!(
            notes_href(&chart, "1.0.0+up1", "1.1.0"),
            "/x?from=1.0.0%2Bup1&to=1.1.0&source=https%3A%2F%2Fgithub.com%2Fx%2Fcharts&repo=https%3A%2F%2Fcharts.example.com%2F"
        );
    }

    #[test]
    fn version_rows_link_each_bump() {
        let v = |version: &str, app: &str| ChartVersion {
            version: version.into(),
            app_version: Some(app.into()),
            ..Default::default()
        };
        let now = Timestamp::UNIX_EPOCH;
        let chart = ChartRow::new(&Resolved {
            name: "x".into(),
            repo: "https://c.example".parse().unwrap(),
            app: None,
            source: None,
            note: None,
        });
        let rows = version_rows(
            &chart,
            &[v("1.0.0", "a"), v("1.1.0", "b"), v("1.0.1", "a")],
            now,
            10,
        );
        let got: Vec<_> = rows
            .iter()
            .map(|r| (r.version.as_str(), r.notes_href.as_deref(), r.app_changed))
            .collect();
        assert_eq!(
            got,
            [
                (
                    "1.1.0",
                    Some("/x?from=1.0.1&to=1.1.0&repo=https%3A%2F%2Fc.example%2F"),
                    true
                ),
                (
                    "1.0.1",
                    Some("/x?from=1.0.0&to=1.0.1&repo=https%3A%2F%2Fc.example%2F"),
                    false
                ),
                ("1.0.0", None, false),
            ]
        );
    }
}
