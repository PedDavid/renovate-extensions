//! What changed in the app between two chart versions: the stable releases
//! in between and the diff link.

use std::cmp::Ordering;

use crate::sources::Release;
use crate::version;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Same,
    Upgrade,
    Downgrade,
    /// The app versions aren't semver, so their order is unknown.
    Unknown,
}

pub fn direction(old: &str, new: &str) -> Direction {
    if old == new {
        return Direction::Same;
    }
    match (version::parse(old), version::parse(new)) {
        (Some(a), Some(b)) => match a.cmp_precedence(&b) {
            Ordering::Less => Direction::Upgrade,
            Ordering::Greater => Direction::Downgrade,
            Ordering::Equal => Direction::Same,
        },
        _ => Direction::Unknown,
    }
}

/// Stable releases after the lower version up to and including the higher
/// one, newest first. A downgrade lists what it rolls back.
pub fn releases_between(releases: &[Release], old: &str, new: &str) -> Vec<Release> {
    let (Some(a), Some(b)) = (version::parse(old), version::parse(new)) else {
        return Vec::new();
    };
    let (low, high) = if a.cmp_precedence(&b).is_le() {
        (a, b)
    } else {
        (b, a)
    };
    let mut found: Vec<(semver::Version, &Release)> = releases
        .iter()
        .filter(|r| !r.prerelease && !r.draft)
        .filter_map(|r| version::parse(&r.tag_name).map(|v| (v, r)))
        .filter(|(v, _)| {
            version::is_stable(v)
                && v.cmp_precedence(&low).is_gt()
                && v.cmp_precedence(&high).is_le()
        })
        .collect();
    found.sort_by(|(a, _), (b, _)| b.cmp_precedence(a));
    // The same version tagged twice (`1.2.3` and `v1.2.3`) is listed once.
    found.dedup_by(|(a, _), (b, _)| a.cmp_precedence(b).is_eq());
    found.into_iter().map(|(_, r)| r.clone()).collect()
}

/// GitHub's compare view between the two app versions. appVersion and the
/// tags don't always agree on the "v" (pyroscope: `2.4.0` vs `v2.4.0`), so
/// the tags win when there are any.
pub fn compare_url(app: &str, old: &str, new: &str, releases: &[Release]) -> String {
    let tag = |version: &str| {
        let bare = version.strip_prefix('v').unwrap_or(version);
        releases
            .iter()
            .map(|r| r.tag_name.as_str())
            .find(|t| t.strip_prefix('v').unwrap_or(t) == bare)
            .map(str::to_string)
            .or_else(|| {
                let v = releases.iter().any(|r| r.tag_name.starts_with('v'));
                (v && !version.starts_with('v')).then(|| format!("v{version}"))
            })
            .unwrap_or_else(|| version.to_string())
    };
    format!(
        "https://github.com/{app}/compare/{}...{}",
        encode(&tag(old)),
        encode(&tag(new))
    )
}

/// One entry of a chart version's `artifacthub.io/changes` annotation.
#[derive(Debug, Clone, PartialEq)]
pub struct Change {
    /// added, changed, deprecated, removed, fixed or security; None when
    /// the chart gives plain strings.
    pub kind: Option<String>,
    pub description: String,
    /// (name, url), http(s) only.
    pub links: Vec<(String, String)>,
}

/// Parses an `artifacthub.io/changes` annotation: a YAML list of strings,
/// or of `{kind, description, links: [{name, url}]}`. Anything else is no
/// changes rather than an error: it's free-form text in someone's chart.
pub fn parse_changes(raw: &str) -> Vec<Change> {
    let Ok(serde_yaml::Value::Sequence(items)) = serde_yaml::from_str::<serde_yaml::Value>(raw)
    else {
        return Vec::new();
    };
    fn text(v: Option<&serde_yaml::Value>) -> Option<&str> {
        v.and_then(|v| v.as_str()).map(str::trim)
    }
    items
        .iter()
        .filter_map(|item| match item {
            serde_yaml::Value::String(s) => Some(Change {
                kind: None,
                description: s.trim().to_string(),
                links: Vec::new(),
            }),
            serde_yaml::Value::Mapping(m) => {
                let description = text(m.get("description"))?.to_string();
                let kind = text(m.get("kind"))
                    .map(str::to_ascii_lowercase)
                    .filter(|k| {
                        matches!(
                            k.as_str(),
                            "added" | "changed" | "deprecated" | "removed" | "fixed" | "security"
                        )
                    });
                let links = m
                    .get("links")
                    .and_then(|l| l.as_sequence())
                    .map(|links| {
                        links
                            .iter()
                            .filter_map(|l| {
                                let url = text(l.get("url"))?;
                                let parsed = url::Url::parse(url).ok()?;
                                matches!(parsed.scheme(), "http" | "https").then(|| {
                                    let name = text(l.get("name")).unwrap_or(url);
                                    (name.to_string(), parsed.to_string())
                                })
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                Some(Change {
                    kind,
                    description,
                    links,
                })
            }
            _ => None,
        })
        .filter(|c| !c.description.is_empty())
        .collect()
}

/// Chart versions after the lower of `from` and `to` up to the higher one,
/// newest first, in the index's terms (all of them, pre-releases too: the
/// chart's own history is what was picked).
pub fn chart_versions_between<'a>(
    versions: &'a [crate::sources::ChartVersion],
    from: &str,
    to: &str,
) -> Vec<&'a crate::sources::ChartVersion> {
    let (Some(a), Some(b)) = (version::parse(from), version::parse(to)) else {
        return Vec::new();
    };
    let (low, high) = if a.cmp_precedence(&b).is_le() {
        (a, b)
    } else {
        (b, a)
    };
    let mut found: Vec<(semver::Version, &crate::sources::ChartVersion)> = versions
        .iter()
        .filter_map(|v| version::parse(&v.version).map(|p| (p, v)))
        .filter(|(p, _)| p.cmp_precedence(&low).is_gt() && p.cmp_precedence(&high).is_le())
        .collect();
    found.sort_by(|(a, _), (b, _)| b.cmp_precedence(a));
    found.into_iter().map(|(_, v)| v).collect()
}

/// GitHub's rendering of release notes, reduced to what is safe to embed:
/// no scripts, styles, forms or event handlers, only http(s) and mailto
/// links (opening in a new tab), relative links resolved against `base`
/// (the release's page). None when nothing is left to show.
pub fn sanitize(html: &str, base: &str) -> Option<String> {
    let base = url::Url::parse(base).ok()?;
    let clean = ammonia::Builder::default()
        .url_schemes(["http", "https", "mailto"].into())
        .url_relative(ammonia::UrlRelative::RewriteWithBase(base))
        .link_rel(Some("noopener noreferrer nofollow"))
        .set_tag_attribute_value("a", "target", "_blank")
        .clean(html)
        .to_string();
    let clean = clean.trim();
    (!clean.is_empty()).then(|| clean.to_string())
}

/// Tags can hold `/` and `+`; the compare URL needs them escaped.
fn encode(tag: &str) -> String {
    tag.chars()
        .map(|c| match c {
            'A'..='Z' | 'a'..='z' | '0'..='9' | '-' | '_' | '.' | '~' => c.to_string(),
            c => {
                let mut buf = [0; 4];
                c.encode_utf8(&mut buf)
                    .bytes()
                    .map(|b| format!("%{b:02X}"))
                    .collect()
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    pub fn release(tag: &str, prerelease: bool) -> Release {
        Release {
            tag_name: tag.into(),
            name: Some(tag.into()),
            prerelease,
            draft: false,
            html_url: format!("https://github.com/x/y/releases/tag/{tag}"),
            published_at: None,
        }
    }

    fn tags(releases: &[Release]) -> Vec<&str> {
        releases.iter().map(|r| r.tag_name.as_str()).collect()
    }

    fn sample() -> Vec<Release> {
        vec![
            release("v2.4.0", false),
            release("v2.4.0-rc.1", true),
            release("v2.5.0-rc.1", false), // a pre-release not marked as one
            release("v2.3.2", false),
            release("v2.3.1", false),
            release("v1.9.9", false),
            release("weekly-2026-10", false),
        ]
    }

    #[test]
    fn stable_releases_in_range_newest_first() {
        assert_eq!(
            tags(&releases_between(&sample(), "2.3.1", "2.4.0")),
            ["v2.4.0", "v2.3.2"]
        );
    }

    #[test]
    fn downgrade_lists_what_it_rolls_back() {
        assert_eq!(
            tags(&releases_between(&sample(), "v2.4.0", "v2.3.1")),
            ["v2.4.0", "v2.3.2"]
        );
        assert_eq!(direction("v2.4.0", "v2.3.1"), Direction::Downgrade);
    }

    #[test]
    fn directions() {
        assert_eq!(direction("1.0.0", "1.0.0"), Direction::Same);
        assert_eq!(direction("1.0.0", "v1.0.0"), Direction::Same);
        assert_eq!(direction("1.0.0", "1.0.1"), Direction::Upgrade);
        assert_eq!(direction("main-abc", "main-def"), Direction::Unknown);
    }

    #[test]
    fn unparseable_versions_list_nothing() {
        assert!(releases_between(&sample(), "latest", "2.4.0").is_empty());
    }

    #[test]
    fn duplicate_tags_are_listed_once() {
        let releases = vec![release("1.2.3", false), release("v1.2.3", false)];
        assert_eq!(releases_between(&releases, "1.2.2", "1.2.3").len(), 1);
    }

    #[test]
    fn changes_annotation_in_both_shapes() {
        let structured = parse_changes(
            "- kind: added\n  description: Add extraPorts.\n  links:\n    - name: \"PR #1\"\n      url: https://github.com/x/y/pull/1\n    - name: bad\n      url: javascript:alert(1)\n- kind: Weird\n  description: Something.\n- kind: fixed\n",
        );
        assert_eq!(structured.len(), 2);
        assert_eq!(structured[0].kind.as_deref(), Some("added"));
        assert_eq!(
            structured[0].links,
            [(
                "PR #1".to_string(),
                "https://github.com/x/y/pull/1".to_string()
            )]
        );
        assert_eq!(structured[1].kind, None);
        let plain = parse_changes("- Bump app to 1.2.3\n- Fix the PDB\n");
        assert_eq!(plain.len(), 2);
        assert_eq!(plain[1].description, "Fix the PDB");
        assert!(parse_changes("just text").is_empty());
        assert!(parse_changes(": : not yaml [").is_empty());
    }

    #[test]
    fn chart_versions_in_range_newest_first() {
        let v = |s: &str| crate::sources::ChartVersion {
            version: s.into(),
            ..Default::default()
        };
        let all = [v("1.4.0"), v("1.3.1"), v("1.3.0"), v("1.2.1")];
        let got: Vec<_> = chart_versions_between(&all, "1.3.0", "1.4.0")
            .iter()
            .map(|c| c.version.as_str())
            .collect();
        assert_eq!(got, ["1.4.0", "1.3.1"]);
        assert_eq!(chart_versions_between(&all, "1.4.0", "1.3.0").len(), 2);
    }

    #[test]
    fn notes_are_sanitized() {
        let base = "https://github.com/x/y/releases/tag/v1.0.0";
        let html = sanitize(
            r#"<h2>Fixes</h2><script>alert(1)</script><p onclick="x()">Fixed <a href="/x/y/pull/1">#1</a>
               and <a href="javascript:alert(1)">this</a>.</p><img src="https://github.com/a.png" onerror="x()">"#,
            base,
        )
        .unwrap();
        assert!(html.contains("<h2>Fixes</h2>"));
        assert!(!html.contains("script") && !html.contains("onclick") && !html.contains("onerror"));
        assert!(
            html.contains(r#"href="https://github.com/x/y/pull/1""#),
            "{html}"
        );
        assert!(html.contains(r#"target="_blank""#));
        assert!(!html.contains("javascript"));
        assert!(
            html.contains(r#"<img src="https://github.com/a.png">"#),
            "{html}"
        );
        assert_eq!(sanitize("  <script>x</script> ", base), None);
    }

    #[test]
    fn compare_follows_the_tags() {
        // appVersion has no "v", the tags do.
        assert_eq!(
            compare_url("x/y", "2.3.1", "2.4.0", &sample()),
            "https://github.com/x/y/compare/v2.3.1...v2.4.0"
        );
        // No releases to go by: the versions as given.
        assert_eq!(
            compare_url("x/y", "v1.0.0", "v1.1.0", &[]),
            "https://github.com/x/y/compare/v1.0.0...v1.1.0"
        );
        assert_eq!(
            compare_url("x/y", "a/b", "c+d", &[]),
            "https://github.com/x/y/compare/a%2Fb...c%2Bd"
        );
    }
}
