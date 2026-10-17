# Tailwind v4 standalone CLI (no Node.js needed):
# https://github.com/tailwindlabs/tailwindcss/releases/tag/v4.3.3
TAILWIND ?= tailwindcss
HELM ?= helm

.PHONY: css check demo helm-lint image visual visual-update visual-browsers

css: ## Rebuild chart-notes' assets/dist/app.css from assets/app.css and the templates
	$(TAILWIND) -i chart-notes/assets/app.css -o chart-notes/assets/dist/app.css --minify

check: ## What CI runs for the Rust code
	cargo fmt --check
	cargo clippy --all-targets --locked -- -D warnings
	cargo test --locked

visual: ## Browser tests: the ⌘K palette, and screenshots against chart-notes/tests/screenshots/
	cargo test --locked --package chart-notes --test palette --test visual -- --ignored

visual-update: ## Rewrite chart-notes/tests/screenshots/ (keep only images rendered on CI)
	UPDATE_SNAPSHOTS=1 cargo test --locked --package chart-notes --test visual -- --ignored

visual-browsers: ## Install the Chromium the Playwright driver expects
	cargo run --locked --package chart-notes --example install-browsers -- chromium

demo: ## Run chart-notes with sample data and no network
	cargo run --package chart-notes -- --demo

helm-lint: ## Lint and render the chart
	$(HELM) lint deploy/helm/chart-notes
	$(HELM) template chart-notes deploy/helm/chart-notes --set ingress.enabled=true \
	  --set serviceMonitor.enabled=true --set github.existingSecret=gh \
	  --set-json 'charts={"x":{"repo":"https://charts.example.com","app":"a/b"}}' >/dev/null
	deploy/helm/tests/render.sh

image: ## Build the chart-notes image
	docker build -f chart-notes/Dockerfile -t chart-notes .
