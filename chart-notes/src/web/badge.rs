//! `/<chart>/badge.svg?from=…&to=…&source=…`: a badge for a Renovate PR's
//! table. Loading it starts every lookup the notes page needs, so the page
//! is ready by the time someone clicks. GitHub fetches PR images through its
//! own proxy (which needs chart-notes to be public); Gitea, Forgejo and
//! self-managed GitLab let the viewer's browser fetch it (LAN is enough).
//!
//! The answer never waits on an upstream for long: proxies give up after a
//! few seconds, so the badge says "looking up" and a later view shows the
//! result.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use axum::extract::{Path, Query, State};
use axum::http::{HeaderValue, header};
use axum::response::{IntoResponse, Response};
use tokio::sync::watch;

use super::AppState;
use super::pages::Bump;
use crate::notes::Direction;
use crate::overrides::valid_name;
use crate::report::{self, Report, ReportError};

/// How long a badge waits for the lookup before answering "looking up".
const WAIT: Duration = Duration::from_millis(1500);
/// How long a finished lookup's summary is reused before looking again.
const REUSE: Duration = Duration::from_secs(60);

/// What a badge shows.
#[derive(Debug, Clone, PartialEq)]
pub struct Summary {
    pub text: String,
    /// "ready", "pending" or "error", for metrics and colour.
    pub result: &'static str,
}

type Slot = (Instant, watch::Receiver<Option<Summary>>);

/// Lookups badges have started, by chart and versions.
#[derive(Default)]
pub struct Warming(Mutex<HashMap<String, Slot>>);

impl Warming {
    /// The summary channel for `key`, starting `start` when there's no
    /// recent one.
    fn get_or_start(
        &self,
        key: &str,
        start: impl FnOnce(watch::Sender<Option<Summary>>),
    ) -> watch::Receiver<Option<Summary>> {
        let mut slots = self.0.lock().expect("warming lock poisoned");
        let now = Instant::now();
        if let Some((at, rx)) = slots.get(key)
            && (now.duration_since(*at) < REUSE || rx.borrow().is_none())
        {
            return rx.clone();
        }
        // Old entries go when new ones come, so the map stays small.
        slots.retain(|_, (at, _)| now.duration_since(*at) < REUSE * 10);
        let (tx, rx) = watch::channel(None);
        slots.insert(key.to_string(), (now, rx.clone()));
        start(tx);
        rx
    }
}

pub async fn badge(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Query(bump): Query<Bump>,
) -> Response {
    let (from, to) = (bump.from(), bump.to());
    if !valid_name(&name) || from.is_empty() || to.is_empty() || from.len() > 128 || to.len() > 128
    {
        state.inner.metrics.badge("error");
        return svg(&Summary {
            text: "unknown chart".into(),
            result: "error",
        });
    }
    let hints = bump.hints();
    let key = format!(
        "{name}|{from}|{to}|{}|{}",
        hints.source.as_deref().unwrap_or_default(),
        hints.repo.as_deref().unwrap_or_default()
    );
    let mut rx = state.inner.warming.get_or_start(&key, |tx| {
        let state = state.clone();
        let (name, from, to) = (name.clone(), from.clone(), to.clone());
        // Runs to the end even when the badge has long been answered: that
        // is the warming.
        tokio::spawn(async move {
            let report = report::build(
                &state.inner.resolver,
                state.inner.sources.as_ref(),
                &name,
                &hints,
                &from,
                &to,
            )
            .await;
            let _ = tx.send(Some(summarize(&report)));
        });
    });
    let summary = match tokio::time::timeout(WAIT, rx.wait_for(Option::is_some)).await {
        Ok(Ok(s)) => s.clone().expect("waited for Some"),
        _ => Summary {
            text: format!("{from} → {to} · looking up"),
            result: "pending",
        },
    };
    state.inner.metrics.badge(summary.result);
    svg(&summary)
}

/// One line for the badge: what changed in the app, or why it can't say.
pub fn summarize(report: &Result<Report, ReportError>) -> Summary {
    let r = match report {
        Ok(r) => r,
        Err(e) => {
            let text = match e {
                ReportError::UnknownChart(_) => "unknown chart",
                ReportError::Ambiguous { .. } => "pick a repository",
                ReportError::UnknownVersion(_) => "unknown version",
                ReportError::Upstream(_) => "upstream unavailable",
            };
            return Summary {
                text: text.into(),
                result: "error",
            };
        }
    };
    let plural =
        |n: usize, one: &str, many: &str| format!("{n} {}", if n == 1 { one } else { many });
    let chart = plural(r.chart_releases.len(), "chart release", "chart releases");
    let text = match (&r.from.app_version, &r.to.app_version, r.direction) {
        (Some(a), _, Direction::Same) => format!("app {a} unchanged · {chart}"),
        (Some(a), Some(b), dir) if r.chart.app.is_some() && r.releases_error.is_none() => {
            let releases = plural(r.app_releases.len(), "release", "releases");
            let arrow = if dir == Direction::Downgrade {
                "↓"
            } else {
                "→"
            };
            format!("app {a} {arrow} {b} · {releases}")
        }
        (Some(a), Some(b), _) => format!("app {a} → {b} · {chart}"),
        _ => chart,
    };
    Summary {
        text,
        result: "ready",
    }
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// A flat badge in the usual two-part style. Text widths are estimated
/// (no font metrics here): wide enough for Verdana at 11px.
fn svg(summary: &Summary) -> Response {
    const LABEL: &str = "release notes";
    let width = |s: &str| (s.chars().count() as f32 * 6.2).ceil() as u32 + 12;
    let (lw, vw) = (width(LABEL), width(&summary.text));
    // Each keeps 4.5:1 with white text.
    let colour = match summary.result {
        "ready" => "#1d4ed8",
        "pending" => "#4b5563",
        _ => "#b45309",
    };
    let text = xml_escape(&summary.text);
    let body = format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="20" role="img" aria-label="{LABEL}: {text}">
<title>{LABEL}: {text}</title>
<clipPath id="r"><rect width="{w}" height="20" rx="3" fill="#fff"/></clipPath>
<g clip-path="url(#r)"><rect width="{lw}" height="20" fill="#374151"/><rect x="{lw}" width="{vw}" height="20" fill="{colour}"/></g>
<g fill="#fff" text-anchor="middle" font-family="Verdana,Geneva,DejaVu Sans,sans-serif" font-size="11">
<text x="{lx}" y="14">{LABEL}</text><text x="{vx}" y="14">{text}</text>
</g>
</svg>"##,
        w = lw + vw,
        lx = lw / 2,
        vx = lw + vw / 2,
    );
    let mut res = (
        [(header::CONTENT_TYPE, "image/svg+xml; charset=utf-8")],
        body,
    )
        .into_response();
    // Image proxies cache by these: always ask again, so the next view
    // shows the finished lookup.
    res.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("no-cache, no-store, max-age=0, must-revalidate"),
    );
    res
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_text() {
        assert_eq!(xml_escape("a<b>&\"c\""), "a&lt;b&gt;&amp;&quot;c&quot;");
    }

    #[test]
    fn errors_summarize_without_detail() {
        let s = summarize(&Err(ReportError::Upstream("secret internal host".into())));
        assert_eq!(s.text, "upstream unavailable");
        assert_eq!(s.result, "error");
    }
}
