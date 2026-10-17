//! The chart's default `values.yaml` between two versions: read from each
//! version's package, compared line by line for the diff and key by key for
//! the summary.

use std::collections::BTreeMap;
use std::io::Read;

use similar::{ChangeTag, TextDiff};

/// Packages bigger than this aren't read.
pub const MAX_PACKAGE: usize = 20 * 1024 * 1024;
/// A `values.yaml` bigger than this isn't read.
const MAX_VALUES: u64 = 2 * 1024 * 1024;

/// The top-level `values.yaml` of a chart package (`<chart>/values.yaml`
/// in the gzipped tar), or None when it has none.
pub fn extract(package: &[u8]) -> std::io::Result<Option<String>> {
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(package));
    for entry in archive.entries()? {
        let entry = entry?;
        let path = entry.path()?.into_owned();
        let parts: Vec<_> = path.components().collect();
        if parts.len() == 2 && parts[1].as_os_str() == "values.yaml" {
            let mut text = String::new();
            entry.take(MAX_VALUES).read_to_string(&mut text)?;
            return Ok(Some(text));
        }
    }
    Ok(None)
}

/// What changed in the defaults, by key path (`controller.podDisruptionBudget.enabled`).
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Summary {
    pub added: Vec<String>,
    pub removed: Vec<String>,
    pub changed: Vec<String>,
}

/// Compares the two files' leaves. Unparseable YAML gives an empty summary
/// (the line diff still shows).
pub fn summarize(old: &str, new: &str) -> Summary {
    let (Ok(a), Ok(b)) = (
        serde_yaml::from_str::<serde_yaml::Value>(old),
        serde_yaml::from_str::<serde_yaml::Value>(new),
    ) else {
        return Summary::default();
    };
    let (mut a_leaves, mut b_leaves) = (BTreeMap::new(), BTreeMap::new());
    flatten(&a, String::new(), &mut a_leaves);
    flatten(&b, String::new(), &mut b_leaves);
    let mut s = Summary::default();
    for (k, v) in &b_leaves {
        match a_leaves.get(k) {
            None => s.added.push(k.clone()),
            Some(old) if old != v => s.changed.push(k.clone()),
            _ => {}
        }
    }
    // A key that went from a leaf (`{}`, a value) to nested keys was
    // reshaped, not removed: its new keys are listed as added.
    s.removed = a_leaves
        .keys()
        .filter(|k| !b_leaves.contains_key(*k))
        .filter(|k| {
            let prefix = format!("{k}.");
            !b_leaves.keys().any(|n| n.starts_with(&prefix))
        })
        .cloned()
        .collect();
    s
}

/// Leaves by dotted path. Lists and empty maps are leaves (compared whole).
fn flatten(v: &serde_yaml::Value, path: String, out: &mut BTreeMap<String, String>) {
    match v {
        serde_yaml::Value::Mapping(m) if !m.is_empty() => {
            for (k, child) in m {
                let key = match k {
                    serde_yaml::Value::String(s) => s.clone(),
                    other => serde_yaml::to_string(other)
                        .unwrap_or_default()
                        .trim()
                        .to_string(),
                };
                let p = if path.is_empty() {
                    key
                } else {
                    format!("{path}.{key}")
                };
                flatten(child, p, out);
            }
        }
        serde_yaml::Value::Tagged(t) => flatten(&t.value, path, out),
        leaf => {
            if !path.is_empty() {
                out.insert(path, serde_yaml::to_string(leaf).unwrap_or_default());
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Line {
    /// "add", "del" or "ctx".
    pub kind: &'static str,
    pub old: Option<usize>,
    pub new: Option<usize>,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Hunk {
    /// The top-level key the hunk is under, when there is one.
    pub section: String,
    pub lines: Vec<Line>,
}

/// The line diff, in hunks with three lines of context.
pub fn diff(old: &str, new: &str) -> Vec<Hunk> {
    let diff = TextDiff::from_lines(old, new);
    let new_lines: Vec<&str> = new.lines().collect();
    let old_lines: Vec<&str> = old.lines().collect();
    diff.grouped_ops(3)
        .iter()
        .map(|group| {
            let mut lines = Vec::new();
            for op in group {
                for change in diff.iter_changes(op) {
                    lines.push(Line {
                        kind: match change.tag() {
                            ChangeTag::Insert => "add",
                            ChangeTag::Delete => "del",
                            ChangeTag::Equal => "ctx",
                        },
                        old: change.old_index().map(|i| i + 1),
                        new: change.new_index().map(|i| i + 1),
                        text: change.value().trim_end_matches(['\n', '\r']).to_string(),
                    });
                }
            }
            // The section is the last top-level key at or above the hunk's
            // first line, in whichever file that line is from.
            let section = lines
                .first()
                .map(|l| match (l.new, l.old) {
                    (Some(n), _) => section_at(&new_lines, n),
                    (None, Some(o)) => section_at(&old_lines, o),
                    _ => String::new(),
                })
                .unwrap_or_default();
            Hunk { section, lines }
        })
        .collect()
}

/// The top-level key (`image:` at column 0) at or above 1-based `line`.
fn section_at(lines: &[&str], line: usize) -> String {
    lines[..line.min(lines.len())]
        .iter()
        .rev()
        .find_map(|l| {
            let first = l.chars().next()?;
            if first.is_whitespace() || first == '#' || first == '-' {
                return None;
            }
            let (key, _) = l.split_once(':')?;
            Some(key.trim().to_string())
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    const OLD: &str = "image:\n  repository: grafana/alloy\n  tag: v1.10.2\ncontroller:\n  type: daemonset\n  podDisruptionBudget: {}\nlegacy: true\n";
    const NEW: &str = "image:\n  repository: grafana/alloy\n  tag: v1.11.2\ncontroller:\n  type: daemonset\n  podDisruptionBudget:\n    enabled: false\nextraPorts: []\n";

    #[test]
    fn summary_by_key() {
        let s = summarize(OLD, NEW);
        assert_eq!(
            s.added,
            ["controller.podDisruptionBudget.enabled", "extraPorts"]
        );
        // podDisruptionBudget went from {} to nested keys: not removed.
        assert_eq!(s.removed, ["legacy"]);
        assert_eq!(s.changed, ["image.tag"]);
        assert_eq!(summarize("{", NEW), Summary::default());
    }

    #[test]
    fn hunks_name_their_section() {
        let hunks = diff(OLD, NEW);
        assert!(!hunks.is_empty());
        let all: Vec<_> = hunks.iter().flat_map(|h| h.lines.iter()).collect();
        assert!(
            all.iter()
                .any(|l| l.kind == "del" && l.text == "  tag: v1.10.2" && l.old == Some(3))
        );
        assert!(
            all.iter()
                .any(|l| l.kind == "add" && l.text == "  tag: v1.11.2" && l.new == Some(3))
        );
        assert!(
            hunks
                .iter()
                .any(|h| h.section == "image" || h.section == "controller")
        );
        assert!(diff(OLD, OLD).is_empty());
    }

    #[test]
    fn reads_values_from_a_package() {
        let mut tgz = Vec::new();
        {
            let gz = flate2::write::GzEncoder::new(&mut tgz, flate2::Compression::fast());
            let mut tar = tar::Builder::new(gz);
            for (path, body) in [
                ("alloy/Chart.yaml", "name: alloy\n"),
                ("alloy/charts/crds/values.yaml", "nested: true\n"),
                ("alloy/values.yaml", OLD),
            ] {
                let mut header = tar::Header::new_gnu();
                header.set_size(body.len() as u64);
                header.set_mode(0o644);
                header.set_cksum();
                tar.append_data(&mut header, path, body.as_bytes()).unwrap();
            }
            tar.into_inner().unwrap().finish().unwrap();
        }
        assert_eq!(extract(&tgz).unwrap().as_deref(), Some(OLD));
        assert!(extract(b"not gzip").is_err());
    }
}
