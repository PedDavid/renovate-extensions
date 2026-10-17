# Tailwind v4 standalone CLI (no Node.js needed):
# https://github.com/tailwindlabs/tailwindcss/releases/tag/v4.3.3
TAILWIND ?= tailwindcss
HELM ?= helm

.PHONY: css check demo helm-lint image

css: ## Rebuild chart-notes' assets/dist/app.css from assets/app.css and the templates
	$(TAILWIND) -i chart-notes/assets/app.css -o chart-notes/assets/dist/app.css --minify

check: ## What CI runs for the Rust code
	cargo fmt --check
	cargo clippy --all-targets --locked -- -D warnings
	cargo test --locked

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
