//! Lenient semver for app versions and release tags, which add a `v` or
//! leave out the patch (`v1.20.1`, `2.10`) as they please.

use semver::Version;

pub fn parse(s: &str) -> Option<Version> {
    let s = s.trim();
    let s = s.strip_prefix(['v', 'V']).unwrap_or(s);
    if let Ok(v) = Version::parse(s) {
        return Some(v);
    }
    // `1.2` or `1.2-rc.1`: add the missing patch before any suffix.
    let split = s.find(['-', '+']).unwrap_or(s.len());
    let (core, rest) = s.split_at(split);
    if core.split('.').count() == 2 {
        return Version::parse(&format!("{core}.0{rest}")).ok();
    }
    None
}

/// A stable version: no pre-release part.
pub fn is_stable(v: &Version) -> bool {
    v.pre.is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_what_charts_and_tags_use() {
        assert_eq!(parse("v1.20.1"), Some(Version::new(1, 20, 1)));
        assert_eq!(parse("1.20.1"), Some(Version::new(1, 20, 1)));
        assert_eq!(parse("2.10"), Some(Version::new(2, 10, 0)));
        assert_eq!(parse("v3.0-rc.1").map(|v| is_stable(&v)), Some(false));
        assert!(parse("v2.4.0-rc.1").is_some_and(|v| !is_stable(&v)));
        assert_eq!(parse("latest"), None);
        assert_eq!(parse("helm-loki-6.0.0"), None);
    }
}
