#!/usr/bin/env python3
"""
GET /<chart>/<from>/<to>: the app releases between two chart versions.

Renovate links here from a custom column in chart bump PRs, since it only
shows the chart's own release notes. Each chart's index.yaml says which app
version it ships (appVersion); GitHub has that app's releases.
"""
import html
import json
import os
import re
import time
import urllib.request
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

# Chart (Renovate's packageName) -> {"repo": <Helm repo URL>, "app": <GitHub
# owner/repo>, "note": <optional caveat shown on the page>}. charts.json next
# to this file is the default; CHARTS_FILE adds to it or overrides entries.
def load_charts() -> dict[str, dict[str, str]]:
    charts = json.loads((Path(__file__).parent / "charts.json").read_text())
    if extra := os.environ.get("CHARTS_FILE"):
        charts |= json.loads(Path(extra).read_text())
    return charts


CHARTS = load_charts()

CACHE_SECONDS = 600
_cache: dict[str, tuple[float, object]] = {}

GITHUB_TOKEN = os.environ.get("GITHUB_TOKEN", "").strip()


def fetch(url: str, headers: dict | None = None) -> bytes:
    hit = _cache.get(url)
    if hit and time.monotonic() - hit[0] < CACHE_SECONDS:
        return hit[1]
    req = urllib.request.Request(url, headers={"User-Agent": "chart-notes", **(headers or {})})
    with urllib.request.urlopen(req, timeout=30) as resp:
        body = resp.read()
    _cache[url] = (time.monotonic(), body)
    return body


def app_versions(repo_url: str, chart: str) -> dict[str, str]:
    """
    chart version -> appVersion, from the repo's index.yaml. Line-based rather
    than a YAML parser (stdlib only): an entry's own keys sit one indent below
    its `- `, deeper ones (dependencies, maintainers) are skipped.
    """
    lines = fetch(f"{repo_url}/index.yaml").decode().splitlines()
    versions: dict[str, str] = {}
    in_entries = in_chart = False
    chart_indent = key_indent = -1
    entry: dict[str, str] = {}

    def flush():
        if "version" in entry and "appVersion" in entry:
            versions[entry["version"]] = entry["appVersion"]
        entry.clear()

    for line in lines:
        stripped = line.lstrip(" ")
        indent = len(line) - len(stripped)
        if not in_entries:
            in_entries = line == "entries:"
            continue
        if not in_chart:
            if indent == 0 and stripped:
                break  # past entries without finding the chart
            if chart_indent < 0 and stripped:
                chart_indent = indent  # chart names sit at the first indent under entries
            if indent == chart_indent and stripped == f"{chart}:":
                in_chart = True
            continue
        if stripped and indent <= chart_indent and not stripped.startswith("- "):
            break  # next chart
        if stripped.startswith("- ") and (key_indent < 0 or indent + 2 == key_indent):
            flush()
            key_indent = indent + 2
            stripped, indent = stripped[2:], key_indent
        if indent == key_indent:
            m = re.match(r'(version|appVersion):\s*["\']?([^"\'\s]+)', stripped)
            if m:
                entry[m.group(1)] = m.group(2)
    flush()
    return versions


def parse(version: str) -> tuple[int, ...] | None:
    m = re.fullmatch(r"v?(\d+)\.(\d+)\.(\d+)", version)
    return tuple(int(p) for p in m.groups()) if m else None


def releases_between(app: str, old: str, new: str) -> list[dict]:
    """Stable releases with old < version <= new, newest first."""
    low, high = parse(old), parse(new)
    if not low or not high:
        return []
    headers = {"Accept": "application/vnd.github+json"}
    if GITHUB_TOKEN:
        headers["Authorization"] = f"Bearer {GITHUB_TOKEN}"
    found = []
    # Releases come newest-created first, with backports to older lines
    # mixed in; three pages reach well past any bump Renovate would offer.
    for page in range(1, 4):
        batch = json.loads(fetch(f"https://api.github.com/repos/{app}/releases?per_page=100&page={page}", headers))
        for release in batch:
            version = parse(release["tag_name"])
            if version and not release["prerelease"] and low < version <= high:
                found.append(release)
        if len(batch) < 100:
            break
    return sorted(found, key=lambda r: parse(r["tag_name"]), reverse=True)


def page(title: str, body: str) -> bytes:
    return f"""<!doctype html>
<html><head><meta charset="utf-8"><meta name="viewport" content="width=device-width">
<title>{html.escape(title)}</title>
<style>body{{font:16px/1.5 system-ui,sans-serif;max-width:48rem;margin:2rem auto;padding:0 1rem}}
code{{background:#8882;padding:0 .3em;border-radius:3px}}</style></head>
<body><h1>{html.escape(title)}</h1>{body}</body></html>""".encode()


def render(chart: str, old: str, new: str) -> tuple[int, bytes]:
    if chart not in CHARTS:
        return 404, page("Unknown chart", f"<p><code>{html.escape(chart)}</code> isn't in chart-notes' CHARTS map.</p>")
    repo_url, app = CHARTS[chart]["repo"], CHARTS[chart]["app"]
    versions = app_versions(repo_url, chart)
    old_app, new_app = versions.get(old), versions.get(new)
    title = f"{chart} {old} → {new}"
    if not old_app or not new_app:
        return 404, page(title, f"<p>No appVersion for one of these versions in <code>{html.escape(repo_url)}/index.yaml</code>.</p>")

    esc = html.escape
    summary = f"<p>App <a href=\"https://github.com/{esc(app)}\">{esc(app)}</a>: <code>{esc(old_app)}</code> → <code>{esc(new_app)}</code></p>"
    if note := CHARTS[chart].get("note"):
        summary += f"<p><em>{esc(note)}</em></p>"
    if old_app == new_app:
        return 200, page(title, summary + f"<p>Same {esc(app)} version.</p>")

    releases = releases_between(app, old_app, new_app)
    links = "".join(
        f"<li><a href=\"{esc(r['html_url'])}\">{esc(r['name'] or r['tag_name'])}</a></li>"
        for r in releases
    ) or f"<li><a href=\"https://github.com/{esc(app)}/releases\">All releases</a> (none matched {esc(new_app)})</li>"
    # appVersion and the tags don't always agree on the "v" (pyroscope: 2.4.0 vs v2.4.0).
    v = "v" if releases and releases[0]["tag_name"].startswith("v") else ""
    compare = f"https://github.com/{esc(app)}/compare/{esc(v + old_app.removeprefix('v'))}...{esc(v + new_app.removeprefix('v'))}"
    return 200, page(title, summary + f"<ul>{links}</ul><p><a href=\"{compare}\">Full diff</a></p>")


class Handler(BaseHTTPRequestHandler):
    def do_GET(self):
        parts = self.path.strip("/").split("/")
        if parts == ["healthz"]:
            status, body = 200, b"ok"
        elif len(parts) == 3:
            try:
                status, body = render(*parts)
            except Exception as e:
                print(f"error rendering {self.path}: {e}")
                status, body = 502, page("Upstream error", f"<p>{html.escape(str(e))}</p>")
        else:
            status, body = 404, page("Not found", "<p>Use /&lt;chart&gt;/&lt;from&gt;/&lt;to&gt;.</p>")
        self.send_response(status)
        self.send_header("Content-Type", "text/plain" if parts == ["healthz"] else "text/html; charset=utf-8")
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, fmt, *args):
        print(fmt % args)


if __name__ == "__main__":
    port = int(os.environ.get("PORT", "8080"))
    print(f"listening on :{port}")
    ThreadingHTTPServer(("", port), Handler).serve_forever()
