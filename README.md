# renovate-extensions

Small services that fill gaps in what [Renovate](https://docs.renovatebot.com/)
shows in its PRs. Renovate has no plugin API, so each one is a web service a
custom PR table column links to.

## chart-notes

Renovate's PRs for Helm chart bumps only carry the chart's own release notes,
which rarely say what changed in the app the chart packages. Renovate also
drops the chart's `appVersion`, so no PR template can link to the app's notes
directly.

`GET /<chart>/<from>/<to>` reads `appVersion` for both chart versions from the
chart repository's `index.yaml`, then lists the app's stable GitHub releases
in between, with a link to the full diff. When the app version didn't change,
it says so.

### Renovate config

```json5
"packageRules": [
  {
    "matchDatasources": ["helm"],
    // Renovate's default columns plus this one.
    "prBodyColumns": ["Package", "Type", "Update", "Change", "Pending", "App notes"],
    "prBodyDefinitions": {
      "App notes": "[app notes](https://chart-notes.example.com/{{{packageName}}}/{{{currentVersion}}}/{{{newVersion}}})"
    }
  }
]
```

Only charts from Helm repositories (Renovate's `helm` datasource) are
covered, not OCI-hosted ones.

### Charts

A chart name alone doesn't say which repository it comes from (`loki` is in
grafana-community's, `alloy` in grafana's), so each chart is listed in
[`chart-notes/charts.json`](chart-notes/charts.json) with its Helm repository
and the GitHub repository of its app. The Helm chart's `charts` value adds to
that list or overrides entries without a new image:

```yaml
charts:
  my-chart:
    repo: https://charts.example.com   # serves index.yaml
    app: owner/repo                    # the app's GitHub releases
    note: Optional caveat shown on the page.
```

### Deploying

The chart is at [`deploy/helm/chart-notes`](deploy/helm/chart-notes). Every
push to `main` publishes `ghcr.io/peddavid/renovate-extensions/chart-notes`
tagged `sha-<commit>` and `main`, plus the chart's `appVersion` the first time
that version is seen.

Unauthenticated, GitHub allows 60 API requests an hour. Set
`github.existingSecret` to a Secret holding any token to raise that.

### Developing

Python 3.14, standard library only.

```bash
python -m unittest discover -s chart-notes/tests
python chart-notes/chart_notes.py   # serves on :8080, PORT to change
```
