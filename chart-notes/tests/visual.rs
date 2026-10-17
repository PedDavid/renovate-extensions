//! Pixel screenshots of the `--demo` UI in Chromium (see `ui/mod.rs`),
//! compared with the PNGs in `tests/screenshots/`.
//!
//! They need Playwright's Chromium, so plain `cargo test` skips them:
//! `make visual` runs them and `make visual-update` rewrites the baselines.
//! Baselines are rendered on CI's Linux runner (fonts and antialiasing differ
//! between machines): take them from the `screenshots` artifact of the CI
//! workflow, run by hand with "update screenshots" ticked.

mod common;
mod ui;

use std::path::PathBuf;

use playwright_rs::protocol::Page;
use playwright_rs::{Animations, ScreenshotAssertionOptions, expect, expect_page};
use ui::open;

/// Compares the viewport with `tests/screenshots/{name}.png`. A missing
/// baseline fails rather than being written, unless `UPDATE_SNAPSHOTS` is set;
/// the capture is kept next to it as `{name}-actual.png` either way.
async fn assert_screenshot(page: &Page, name: &str) {
    let dir: PathBuf = [env!("CARGO_MANIFEST_DIR"), "tests", "screenshots"]
        .iter()
        .collect();
    // Failure captures are <name>-actual.png and <name>-diff.png, which
    // .gitignore drops: a baseline named like one would never be committed.
    assert!(
        !name.ends_with("-actual") && !name.ends_with("-diff"),
        "{name}: baselines can't end in -actual or -diff"
    );
    let baseline = dir.join(format!("{name}.png"));
    let update = std::env::var_os("UPDATE_SNAPSHOTS").is_some();
    let options = |update| {
        ScreenshotAssertionOptions::builder()
            .animations(Animations::Disabled)
            .update_snapshots(update)
            .build()
    };
    if !update && !baseline.exists() {
        let actual = dir.join(format!("{name}-actual.png"));
        let _ = expect_page(page)
            .to_have_screenshot(&actual, Some(options(true)))
            .await;
        panic!(
            "no baseline at {}; the capture is in {}",
            baseline.display(),
            actual.display()
        );
    }
    if let Err(e) = expect_page(page)
        .to_have_screenshot(&baseline, Some(options(update)))
        .await
    {
        panic!("{name}: {e}");
    }
}

/// Opens a notes page and waits for htmx to load its content.
async fn open_notes(path: &str, cookies: &[(&str, &str)]) -> ui::Ui {
    let ui = open(path, cookies).await;
    expect(ui.page.locator("#notes [role=tablist]"))
        .to_be_visible()
        .await
        .unwrap();
    ui
}

#[tokio::test]
#[ignore = "needs a Playwright browser: make visual"]
async fn charts() {
    let ui = open("/", &[]).await;
    assert_screenshot(&ui.page, "charts").await;
}

#[tokio::test]
#[ignore = "needs a Playwright browser: make visual"]
async fn charts_dark_violet() {
    let ui = open(
        "/",
        &[
            ("chart_notes_theme", "dark"),
            ("chart_notes_palette", "violet"),
        ],
    )
    .await;
    assert_screenshot(&ui.page, "charts-dark-violet").await;
}

#[tokio::test]
#[ignore = "needs a Playwright browser: make visual"]
async fn versions() {
    let ui = open("/alloy", &[]).await;
    assert_screenshot(&ui.page, "versions").await;
}

#[tokio::test]
#[ignore = "needs a Playwright browser: make visual"]
async fn notes_upgrade() {
    let ui = open_notes("/alloy?from=1.3.0&to=1.4.0", &[]).await;
    assert_screenshot(&ui.page, "notes-upgrade").await;
}

#[tokio::test]
#[ignore = "needs a Playwright browser: make visual"]
async fn notes_with_note_dark() {
    let ui = open_notes(
        "/kube-prometheus-stack?from=78.5.0&to=79.1.0",
        &[("chart_notes_theme", "dark")],
    )
    .await;
    assert_screenshot(&ui.page, "notes-note-dark").await;
}

#[tokio::test]
#[ignore = "needs a Playwright browser: make visual"]
async fn notes_downgrade() {
    let ui = open_notes("/alloy?from=1.4.0&to=1.3.1", &[]).await;
    assert_screenshot(&ui.page, "notes-downgrade").await;
}

#[tokio::test]
#[ignore = "needs a Playwright browser: make visual"]
async fn notes_same_app_version() {
    let ui = open_notes("/alloy?from=1.2.0&to=1.2.1", &[]).await;
    assert_screenshot(&ui.page, "notes-same").await;
}

#[tokio::test]
#[ignore = "needs a Playwright browser: make visual"]
async fn notes_github_down() {
    let ui = open_notes("/traefik?from=37.1.2&to=37.2.0", &[]).await;
    assert_screenshot(&ui.page, "notes-github-down").await;
}

#[tokio::test]
#[ignore = "needs a Playwright browser: make visual"]
async fn unknown_version() {
    let ui = open("/alloy?from=1.3.0&to=9.9.9", &[]).await;
    assert_screenshot(&ui.page, "unknown-version").await;
}

#[tokio::test]
#[ignore = "needs a Playwright browser: make visual"]
async fn command_palette() {
    let ui = open("/", &[]).await;
    ui.page.keyboard().press("Control+k", None).await.unwrap();
    ui.page.keyboard().type_text("graf", None).await.unwrap();
    expect(ui.page.locator("#command-results [role=option]").first())
        .to_be_visible()
        .await
        .unwrap();
    expect(ui.page.locator("#command-results [role=option]"))
        .to_have_count(1)
        .await
        .unwrap();
    assert_screenshot(&ui.page, "command-palette").await;
}

#[tokio::test]
#[ignore = "needs a Playwright browser: make visual"]
async fn values_diff_tab() {
    let ui = open_notes("/alloy?from=1.3.0&to=1.4.0", &[]).await;
    ui.page
        .locator("[role=tab][data-tab=values]")
        .click(None)
        .await
        .unwrap();
    expect(
        ui.page
            .locator("[role=tab][data-tab=values][aria-selected=true]"),
    )
    .to_be_visible()
    .await
    .unwrap();
    // The tab has its own URL, so the back button and links work.
    expect_page(&ui.page)
        .to_have_url_regex(r"/alloy\?from=1\.3\.0&to=1\.4\.0&.*tab=values$")
        .await
        .unwrap();
    assert_screenshot(&ui.page, "notes-values").await;
}

#[tokio::test]
#[ignore = "needs a Playwright browser: make visual"]
async fn chart_tab_dark() {
    let ui = open_notes(
        "/alloy?from=1.3.0&to=1.4.0&tab=chart",
        &[("chart_notes_theme", "dark")],
    )
    .await;
    assert_screenshot(&ui.page, "notes-chart-tab-dark").await;
}
