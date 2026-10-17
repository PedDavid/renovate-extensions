//! WCAG contrast of every palette's primary colour against its foreground,
//! in light and dark mode, computed from the CSS the page ships.

use std::collections::BTreeMap;

const BASE: &str = include_str!("../assets/vendor/basecoat/base/base.css");
const THEMES: &str = include_str!("../assets/themes.css");
const PALETTES: [&str; 5] = ["neutral", "blue", "green", "orange", "violet"];
/// WCAG 2.2 AA for normal text; Basecoat puts button labels in these pairs.
const MIN_CONTRAST: f64 = 4.5;

type Vars = BTreeMap<String, String>;

/// `--name: value;` declarations of every top-level block whose selector is
/// exactly `selector`.
fn block(css: &str, selector: &str) -> Vars {
    let css = strip_comments(css);
    let mut vars = Vars::new();
    let mut rest = css.as_str();
    while let Some(open) = rest.find('{') {
        let head = rest[..open].rsplit(['}', ';']).next().unwrap_or("").trim();
        let close = open + rest[open..].find('}').expect("unclosed block");
        if head == selector {
            for decl in rest[open + 1..close].split(';') {
                if let Some((k, v)) = decl.trim().split_once(':')
                    && k.trim().starts_with("--")
                {
                    vars.insert(k.trim().to_string(), v.trim().to_string());
                }
            }
        }
        rest = &rest[close + 1..];
    }
    vars
}

fn strip_comments(css: &str) -> String {
    let mut out = String::new();
    let mut rest = css;
    while let Some(start) = rest.find("/*") {
        out.push_str(&rest[..start]);
        let end = rest[start..].find("*/").expect("unclosed comment");
        rest = &rest[start + end + 2..];
    }
    out.push_str(rest);
    out
}

/// The custom properties in effect on `<html>` for a palette and mode,
/// following the cascade: Basecoat's `:root`/`.dark`, then the palette's
/// rules, which are more specific.
fn effective(palette: &str, dark: bool) -> Vars {
    let mut vars = block(BASE, ":root");
    if dark {
        vars.extend(block(BASE, ".dark"));
    }
    if palette != "neutral" {
        vars.extend(block(THEMES, &format!("html[data-palette='{palette}']")));
        if dark {
            vars.extend(block(
                THEMES,
                &format!("html[data-palette='{palette}'].dark"),
            ));
        }
    }
    vars
}

fn parse_oklch(value: &str) -> [f64; 3] {
    let inner = value
        .strip_prefix("oklch(")
        .and_then(|v| v.strip_suffix(')'))
        .unwrap_or_else(|| panic!("not an oklch() colour: {value}"));
    let parts: Vec<f64> = inner
        .split_whitespace()
        .map(|p| p.parse().unwrap())
        .collect();
    [parts[0], parts[1], parts.get(2).copied().unwrap_or(0.0)]
}

/// OKLCH to linear sRGB (unclamped).
fn linear_srgb([l, c, h]: [f64; 3]) -> [f64; 3] {
    let (a, b) = (c * h.to_radians().cos(), c * h.to_radians().sin());
    let l_ = (l + 0.396_337_777_4 * a + 0.215_803_757_3 * b).powi(3);
    let m_ = (l - 0.105_561_345_8 * a - 0.063_854_172_8 * b).powi(3);
    let s_ = (l - 0.089_484_177_5 * a - 1.291_485_548 * b).powi(3);
    [
        4.076_741_662_1 * l_ - 3.307_711_591_3 * m_ + 0.230_969_929_2 * s_,
        -1.268_438_004_6 * l_ + 2.609_757_401_1 * m_ - 0.341_319_396_5 * s_,
        -0.004_196_086_3 * l_ - 0.703_418_614_7 * m_ + 1.707_614_701 * s_,
    ]
}

fn luminance([r, g, b]: [f64; 3]) -> f64 {
    let clamp = |x: f64| x.clamp(0.0, 1.0);
    0.2126 * clamp(r) + 0.7152 * clamp(g) + 0.0722 * clamp(b)
}

/// Relative luminance after bringing the colour into sRGB the two ways a
/// browser might: clipping, or lowering chroma (CSS Color 4 gamut mapping).
/// Returns both, so a pair must pass either way.
fn luminances(color: [f64; 3]) -> [f64; 2] {
    let clipped = luminance(linear_srgb(color));
    let in_gamut = |c: [f64; 3]| {
        linear_srgb(c)
            .iter()
            .all(|x| (-1e-6..=1.0 + 1e-6).contains(x))
    };
    let (mut lo, mut hi) = (0.0, color[1]);
    if !in_gamut(color) {
        for _ in 0..40 {
            let mid = (lo + hi) / 2.0;
            if in_gamut([color[0], mid, color[2]]) {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        hi = lo;
    }
    [clipped, luminance(linear_srgb([color[0], hi, color[2]]))]
}

fn contrast(a: [f64; 3], b: [f64; 3]) -> f64 {
    let (la, lb) = (luminances(a), luminances(b));
    let mut worst = f64::INFINITY;
    for x in la {
        for y in lb {
            let (hi, lo) = if x > y { (x, y) } else { (y, x) };
            worst = worst.min((hi + 0.05) / (lo + 0.05));
        }
    }
    worst
}

#[test]
fn every_palette_has_readable_primary_controls() {
    let mut failures = Vec::new();
    for palette in PALETTES {
        for dark in [false, true] {
            let vars = effective(palette, dark);
            let ratio = contrast(
                parse_oklch(&vars["--primary"]),
                parse_oklch(&vars["--primary-foreground"]),
            );
            let mode = if dark { "dark" } else { "light" };
            if ratio < MIN_CONTRAST {
                failures.push(format!("{palette} {mode}: {ratio:.2}:1"));
            }
        }
    }
    assert!(
        failures.is_empty(),
        "primary/primary-foreground below {MIN_CONTRAST}:1: {failures:?}"
    );
}

#[test]
fn contrast_matches_known_values() {
    // Black on white is 21:1, and the old light-mode green was 2.96:1.
    let (black, white) = ([0.0, 0.0, 0.0], [1.0, 0.0, 0.0]);
    assert!((contrast(black, white) - 21.0).abs() < 0.01);
    let old_green = contrast([0.648, 0.2, 131.684], [0.986, 0.031, 120.757]);
    assert!((old_green - 2.96).abs() < 0.05, "{old_green}");
}

#[test]
fn css_blocks_are_found() {
    for palette in PALETTES.iter().filter(|p| **p != "neutral") {
        let light = block(THEMES, &format!("html[data-palette='{palette}']"));
        let dark = block(THEMES, &format!("html[data-palette='{palette}'].dark"));
        assert!(light.contains_key("--primary-foreground"), "{palette}");
        assert!(dark.contains_key("--primary"), "{palette}");
    }
    assert!(block(BASE, ":root").contains_key("--primary"));
    assert!(block(BASE, ".dark").contains_key("--primary"));
}
