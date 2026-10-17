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
