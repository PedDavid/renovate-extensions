//! Finding a chart's Helm repository and app, against fake upstreams that
//! record what was asked for.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use url::Url;

use chart_notes::cache::Cached;
use chart_notes::overrides;
use chart_notes::resolve::{Hints, Resolution, Resolver};
use chart_notes::sources::{
    Candidate, ChartIndex, ChartVersion, ReleaseNotes, Releases, Sources, UpstreamError,
};

#[derive(Default)]
struct Fake {
    /// Index URL (no trailing slash) → chart versions.
    indexes: HashMap<String, Vec<ChartVersion>>,
    /// Chart → Artifact Hub candidates.
    hub: HashMap<String, Vec<Candidate>>,
    hub_down: bool,
    asked: Mutex<Vec<String>>,
}

fn cached<T>(value: T) -> Cached<T> {
    Cached {
        value: Arc::new(value),
        fetched_at: jiff::Timestamp::UNIX_EPOCH,
        stale: None,
    }
}

#[async_trait]
impl Sources for Fake {
    async fn index(&self, repo: &Url, chart: &str) -> Result<Cached<ChartIndex>, UpstreamError> {
        let key = repo.as_str().trim_end_matches('/').to_string();
        self.asked.lock().unwrap().push(key.clone());
        match self.indexes.get(&key) {
            Some(v) if chart == "alloy" || chart == "nginx" => {
                Ok(cached(ChartIndex::new(v.clone())))
            }
            Some(_) => Ok(cached(ChartIndex::new(Vec::new()))),
            None => Err(UpstreamError::Status {
                url: key,
                status: 404,
            }),
        }
    }

    async fn search(&self, chart: &str) -> Result<Cached<Vec<Candidate>>, UpstreamError> {
        if self.hub_down {
            return Err(UpstreamError::Status {
                url: "artifacthub".into(),
                status: 503,
            });
        }
        Ok(cached(self.hub.get(chart).cloned().unwrap_or_default()))
    }

    async fn releases(&self, _: &str) -> Result<Cached<Releases>, UpstreamError> {
        Ok(cached(Releases::new(Vec::new())))
    }

    async fn notes(&self, _: &str, _: &str) -> Result<Cached<ReleaseNotes>, UpstreamError> {
        Ok(cached(ReleaseNotes::new(None, "https://github.com/")))
    }
}

/// A chart version released from `source` (a GitHub owner/repo), shipping
/// `app`.
fn released_from(source: &str, app: &str) -> Vec<ChartVersion> {
    vec![ChartVersion {
        version: "1.0.0".into(),
        app_version: Some("v1.0.0".into()),
        sources: vec![
            format!("https://github.com/{source}"),
            format!("https://github.com/{app}"),
        ],
        url: Some(format!(
            "https://github.com/{source}/releases/download/alloy-1.0.0/alloy-1.0.0.tgz"
        )),
        ..Default::default()
    }]
}

fn candidate(url: &str, official: bool) -> Candidate {
    Candidate {
        repo: url.parse().unwrap(),
        name: url.into(),
        display_name: url.into(),
        official,
        verified: false,
    }
}

fn resolver(fake: Fake, overrides_yaml: &str) -> (Resolver, Arc<Fake>) {
    let fake = Arc::new(fake);
    let r = Resolver::new(
        overrides::parse(overrides_yaml).unwrap(),
        fake.clone(),
        Duration::from_secs(60),
    );
    (r, fake)
}

fn source(url: &str) -> Hints {
    Hints {
        source: Some(url.into()),
        repo: None,
    }
}

fn found(r: &Resolution) -> &chart_notes::resolve::Resolved {
    match r {
        Resolution::Found(f) => f,
        other => panic!("not found: {other:?}"),
    }
}

#[tokio::test]
async fn source_on_github_pages_needs_no_artifact_hub() {
    let fake = Fake {
        indexes: HashMap::from([(
            "https://grafana.github.io/helm-charts".into(),
            released_from("grafana/helm-charts", "grafana/alloy"),
        )]),
        hub_down: true,
        ..Default::default()
    };
    let (r, _) = resolver(fake, "");
    let res = r
        .resolve("alloy", &source("https://github.com/grafana/helm-charts"))
        .await
        .unwrap();
    let f = found(&res);
    assert_eq!(f.repo.as_str(), "https://grafana.github.io/helm-charts/");
    assert_eq!(f.app.as_deref(), Some("grafana/alloy"));
    assert_eq!(f.source.as_deref(), Some("grafana/helm-charts"));
}

#[tokio::test]
async fn source_picks_among_artifact_hub_candidates() {
    let fake = Fake {
        indexes: HashMap::from([
            (
                "https://forks.example/charts".into(),
                released_from("someone/forked-charts", "someone/alloy"),
            ),
            (
                "https://charts.example.com".into(),
                released_from("example/helm-charts", "grafana/alloy"),
            ),
        ]),
        hub: HashMap::from([(
            "alloy".into(),
            vec![
                candidate("https://forks.example/charts", true),
                candidate("https://charts.example.com", false),
            ],
        )]),
        ..Default::default()
    };
    let (r, _) = resolver(fake, "");
    let res = r
        .resolve("alloy", &source("https://github.com/example/helm-charts"))
        .await
        .unwrap();
    assert_eq!(found(&res).repo.as_str(), "https://charts.example.com/");
}

#[tokio::test]
async fn without_a_source_one_official_candidate_wins() {
    let fake = Fake {
        indexes: HashMap::from([(
            "https://charts.example.com".into(),
            released_from("example/helm-charts", "grafana/alloy"),
        )]),
        hub: HashMap::from([(
            "alloy".into(),
            vec![
                candidate("https://charts.example.com", true),
                candidate("https://forks.example/charts", false),
            ],
        )]),
        ..Default::default()
    };
    let (r, _) = resolver(fake, "");
    let res = r.resolve("alloy", &Hints::default()).await.unwrap();
    assert_eq!(found(&res).repo.as_str(), "https://charts.example.com/");
}

#[tokio::test]
async fn without_a_source_several_candidates_are_ambiguous_until_picked() {
    let fake = Fake {
        indexes: HashMap::from([(
            "https://b.example".into(),
            released_from("b/charts", "nginx/nginx"),
        )]),
        hub: HashMap::from([(
            "nginx".into(),
            vec![
                candidate("https://a.example", false),
                candidate("https://b.example", false),
            ],
        )]),
        ..Default::default()
    };
    let (r, _) = resolver(fake, "");
    let res = r.resolve("nginx", &Hints::default()).await.unwrap();
    assert!(matches!(res.as_ref(), Resolution::Ambiguous(c) if c.len() == 2));
    let picked = Hints {
        source: None,
        repo: Some("https://b.example/".into()),
    };
    let res = r.resolve("nginx", &picked).await.unwrap();
    assert_eq!(found(&res).repo.as_str(), "https://b.example/");
}

#[tokio::test]
async fn urls_from_requests_are_never_fetched() {
    let fake = Fake {
        hub: HashMap::from([(
            "alloy".into(),
            vec![candidate("https://charts.example.com", true)],
        )]),
        indexes: HashMap::from([(
            "https://charts.example.com".into(),
            released_from("example/helm-charts", "grafana/alloy"),
        )]),
        ..Default::default()
    };
    let (r, fake) = resolver(fake, "");
    let hints = Hints {
        // Not GitHub: no Pages guess. Not a candidate: ignored.
        source: Some("http://10.0.0.1/admin".into()),
        repo: Some("http://169.254.169.254/latest".into()),
    };
    r.resolve("alloy", &hints).await.unwrap();
    let asked = fake.asked.lock().unwrap().clone();
    assert!(
        asked.iter().all(|u| u == "https://charts.example.com"),
        "{asked:?}"
    );
}

#[tokio::test]
async fn overrides_win() {
    let fake = Fake {
        indexes: HashMap::from([(
            "https://charts.internal.example".into(),
            released_from("me/charts", "me/not-the-app"),
        )]),
        hub_down: true,
        ..Default::default()
    };
    let (r, fake) = resolver(
        fake,
        "alloy: {repo: 'https://charts.internal.example', app: me/app, note: Hi}",
    );
    let res = r.resolve("alloy", &Hints::default()).await.unwrap();
    let f = found(&res);
    assert_eq!(f.app.as_deref(), Some("me/app"));
    assert_eq!(f.note.as_deref(), Some("Hi"));
    assert_eq!(fake.asked.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn unknown_and_hub_down() {
    let (r, _) = resolver(Fake::default(), "");
    let res = r.resolve("nope", &Hints::default()).await.unwrap();
    assert_eq!(*res, Resolution::NotFound);
    let (r, _) = resolver(
        Fake {
            hub_down: true,
            ..Default::default()
        },
        "",
    );
    assert!(r.resolve("nope", &Hints::default()).await.is_err());
}
