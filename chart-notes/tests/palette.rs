//! The ⌘K palette in Chromium (see `ui/mod.rs`): its ARIA combobox state
//! while moving through results, and reopening it with a search still in
//! flight. Like the screenshots, these need Playwright's Chromium, so plain
//! `cargo test` skips them and `make visual` runs them.

mod common;
mod ui;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use axum::extract::Request;
use axum::middleware::{self, Next};
use playwright_rs::protocol::{AriaRole, GetByRoleOptions, Locator, Page};
use playwright_rs::{expect, expect_page};
use tokio::sync::watch;
use ui::{ORIGIN, open, open_app};

fn named(name: &str) -> Option<GetByRoleOptions> {
    let mut options = GetByRoleOptions::default();
    options.name = Some(name.into());
    Some(options)
}

fn input(page: &Page) -> Locator {
    page.get_by_role(AriaRole::Combobox, named("Search charts"))
}

/// The same input by CSS: playwright-rs checks focus with `querySelectorAll`,
/// which does not understand role selectors.
fn input_css(page: &Page) -> Locator {
    page.locator("#command-palette input[role=combobox]")
}

/// Options of the palette's listbox (the page has other options, in selects).
fn results(page: &Page) -> Locator {
    page.get_by_role(AriaRole::Listbox, named("Charts"))
        .get_by_role(AriaRole::Option, None)
}

async fn open_palette(page: &Page) {
    page.keyboard().press("Control+k", None).await.unwrap();
    expect(page.locator("#command-palette"))
        .to_have_attribute("open", "")
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "needs a Playwright browser: make visual"]
async fn palette_is_a_combobox_that_tracks_the_selected_option() {
    let ui = open("/", &[]).await;
    let page = &ui.page;
    open_palette(page).await;
    let input = input(page);
    expect(input_css(page)).to_be_focused().await.unwrap();
    expect(input.clone())
        .to_have_attribute("aria-controls", "command-results")
        .await
        .unwrap();
    expect(page.get_by_role(AriaRole::Listbox, named("Charts")))
        .to_be_visible()
        .await
        .unwrap();
    let options = results(page);
    expect(options.first()).to_be_visible().await.unwrap();
    expect(input.clone())
        .to_have_attribute("aria-expanded", "true")
        .await
        .unwrap();

    let first = options.nth(0).get_attribute("id").await.unwrap().unwrap();
    let second = options.nth(1).get_attribute("id").await.unwrap().unwrap();
    expect(input.clone())
        .to_have_attribute("aria-activedescendant", &first)
        .await
        .unwrap();
    page.keyboard().press("ArrowDown", None).await.unwrap();
    expect(input.clone())
        .to_have_attribute("aria-activedescendant", &second)
        .await
        .unwrap();
    expect(options.nth(1))
        .to_have_attribute("aria-selected", "true")
        .await
        .unwrap();
    expect(options.nth(0))
        .to_have_attribute("aria-selected", "false")
        .await
        .unwrap();
    // Focus never leaves the input.
    expect(input_css(page)).to_be_focused().await.unwrap();
}

#[tokio::test]
#[ignore = "needs a Playwright browser: make visual"]
async fn reopening_the_palette_does_not_follow_a_stale_result() {
    // Once `hold` is set, /search waits until `release` opens the gate, so
    // the old results would still be showing if the palette kept them.
    let hold = Arc::new(AtomicBool::new(false));
    let (release, gate) = watch::channel(false);
    let app = common::demo_app().layer(middleware::from_fn({
        let hold = hold.clone();
        move |req: Request, next: Next| {
            let hold = hold.clone();
            let mut gate = gate.clone();
            async move {
                if req.uri().path() == "/search" && hold.load(Ordering::SeqCst) {
                    let _ = gate.wait_for(|open| *open).await;
                }
                next.run(req).await
            }
        }
    }));
    let ui = open_app("/", &[], app).await;
    let page = &ui.page;
    open_palette(page).await;
    let input = input(page);
    input.fill("traefik", None).await.unwrap();
    expect(results(page)).to_have_count(1).await.unwrap();
    // Close by clicking the backdrop, which leaves the field and results as
    // they were (Escape would clear the field and search again).
    page.mouse().click(5.0, 5.0, None).await.unwrap();
    expect(page.locator("#command-palette"))
        .not()
        .to_have_attribute("open", "")
        .await
        .unwrap();

    hold.store(true, Ordering::SeqCst);
    open_palette(page).await;
    expect(input.clone()).to_have_value("").await.unwrap();
    expect(results(page)).to_have_count(0).await.unwrap();
    expect(input.clone())
        .to_have_attribute("aria-expanded", "false")
        .await
        .unwrap();
    expect(input)
        .not()
        .to_have_attribute_regex("aria-activedescendant", ".+")
        .await
        .unwrap();
    page.keyboard().press("Enter", None).await.unwrap();
    expect_page(page)
        .to_have_url(&format!("{ORIGIN}/"))
        .await
        .unwrap();

    release.send(true).unwrap();
    expect(results(page).first()).to_be_visible().await.unwrap();
}
