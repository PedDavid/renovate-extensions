#!/usr/bin/env bash
# Checks the rendered chart for contracts that `helm lint` does not see.
# Needs helm and yq (mikefarah, v4).
set -euo pipefail
cd "$(dirname "$0")/../../.."
chart=deploy/helm/chart-notes
render() {
  helm template chart-notes "$chart" "$@"
}
fail() { echo "FAIL: $*" >&2; exit 1; }
env_value() { yq "select(.kind == \"Deployment\") | .spec.template.spec.containers[0].env[] | select(.name == \"$1\") | .value"; }

# An exposed UI Service never carries the metrics port.
out=$(render --set service.type=LoadBalancer --set serviceMonitor.enabled=true)
ui=$(yq 'select(.kind == "Service" and .metadata.name == "chart-notes")' <<<"$out")
[[ $(yq '.spec.type' <<<"$ui") == LoadBalancer ]] || fail "UI Service type not applied"
[[ $(yq '[.spec.ports[].name] | join(",")' <<<"$ui") == http ]] || fail "UI Service exposes more than http"
metrics=$(yq 'select(.kind == "Service" and .metadata.name == "chart-notes-metrics")' <<<"$out")
[[ $(yq '.spec.type' <<<"$metrics") == ClusterIP ]] || fail "metrics Service is not ClusterIP"
sm=$(yq 'select(.kind == "ServiceMonitor")' <<<"$out")
[[ $(yq '.spec.selector.matchLabels."app.kubernetes.io/component"' <<<"$sm") == metrics ]] \
  || fail "ServiceMonitor does not select the metrics Service"

# The GitHub token is a mounted file, never an env var value.
out=$(render --set github.existingSecret=gh)
[[ $(env_value CHART_NOTES_GITHUB_TOKEN_FILE <<<"$out") == /etc/chart-notes/github/token ]] \
  || fail "token file path not set"
[[ $(yq 'select(.kind == "Deployment") | .spec.template.spec.volumes[] | select(.name == "github") | .secret.secretName' <<<"$out") == gh ]] \
  || fail "token Secret not mounted"

# Extra charts reach the app as a file, and changing them rolls the pod.
a=$(render --set-json 'charts={"x":{"repo":"https://charts.example.com","app":"a/b"}}')
b=$(render --set-json 'charts={"x":{"repo":"https://charts.example.com","app":"a/c"}}')
[[ $(env_value CHART_NOTES_CHARTS_FILE <<<"$a") == /etc/chart-notes/charts/charts.json ]] || fail "charts file not set"
[[ $(yq 'select(.kind == "ConfigMap") | .data."charts.json"' <<<"$a" | yq -p json '.x.app') == a/b ]] \
  || fail "charts ConfigMap does not hold the charts as JSON"
checksum='select(.kind == "Deployment") | .spec.template.metadata.annotations."checksum/charts"'
[[ $(yq "$checksum" <<<"$a") != $(yq "$checksum" <<<"$b") ]] || fail "charts change does not roll the pod"

# The public URL follows the Ingress unless set.
[[ $(render --set ingress.enabled=true | env_value CHART_NOTES_PUBLIC_URL) == https://chart-notes.example.com ]] \
  || fail "public URL not derived from the Ingress"
[[ $(render --set publicUrl=https://notes.example.org | env_value CHART_NOTES_PUBLIC_URL) == https://notes.example.org ]] \
  || fail "publicUrl not applied"

# The default image tag is the chart's appVersion, which the release
# workflow publishes (see .github/workflows/release.yml).
app=$(yq '.appVersion' "$chart/Chart.yaml")
image=$(render | yq 'select(.kind == "Deployment") | .spec.template.spec.containers[0].image')
[[ $image == *":$app" ]] || fail "default image $image is not tagged $app"
cargo_version=$(sed -n 's/^version = "\(.*\)"/\1/p' chart-notes/Cargo.toml | head -1)
[[ $app == "$cargo_version" ]] || fail "Chart appVersion $app differs from chart-notes/Cargo.toml version $cargo_version"

echo "chart render checks passed"
