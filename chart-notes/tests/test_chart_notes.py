import json
import sys
import unittest
from pathlib import Path
from unittest import mock

sys.path.insert(0, str(Path(__file__).parent.parent))
import chart_notes  # noqa: E402

FIXTURES = Path(__file__).parent / "fixtures"
REPO = "https://charts.example.com"


def fake_fetch(releases: list[dict]):
    """index.yaml from the fixture, GitHub releases from `releases`."""

    def fetch(url, headers=None):
        if url == f"{REPO}/index.yaml":
            return (FIXTURES / "index.yaml").read_bytes()
        if "api.github.com" in url:
            return json.dumps(releases if "page=1" in url else []).encode()
        raise AssertionError(f"unexpected fetch {url}")

    return fetch


def release(tag, prerelease=False):
    return {"tag_name": tag, "name": tag, "prerelease": prerelease, "html_url": f"https://github.com/x/y/releases/tag/{tag}"}


class AppVersions(unittest.TestCase):
    def setUp(self):
        patcher = mock.patch.object(chart_notes, "fetch", fake_fetch([]))
        patcher.start()
        self.addCleanup(patcher.stop)

    def test_reads_entry_keys_not_nested_ones(self):
        # alloy's dependency also has a `version:`, one indent deeper.
        self.assertEqual(chart_notes.app_versions(REPO, "alloy"), {"1.13.1": "v1.20.1", "1.13.0": "v1.20.0"})

    def test_stops_at_the_next_chart(self):
        self.assertEqual(chart_notes.app_versions(REPO, "loki"), {"18.17.0": "3.7.8"})

    def test_indented_list_and_quoted_values(self):
        self.assertEqual(chart_notes.app_versions(REPO, "pyroscope"), {"2.4.0": "2.4.0", "2.3.1": "2.3.1"})

    def test_unknown_chart(self):
        self.assertEqual(chart_notes.app_versions(REPO, "tempo"), {})


class Render(unittest.TestCase):
    def render(self, chart, old, new, releases=()):
        charts = {chart: {"repo": REPO, "app": "x/y"}}
        with mock.patch.object(chart_notes, "fetch", fake_fetch(list(releases))), \
             mock.patch.dict(chart_notes.CHARTS, charts, clear=True):
            status, body = chart_notes.render(chart, old, new)
        return status, body.decode()

    def test_lists_stable_releases_in_range(self):
        status, body = self.render("pyroscope", "2.3.1", "2.4.0",
                                   [release("v2.4.0"), release("v2.4.0-rc.1", prerelease=True),
                                    release("v2.3.2"), release("v2.3.1"), release("v1.9.9")])
        self.assertEqual(status, 200)
        self.assertIn("releases/tag/v2.4.0\"", body)
        self.assertIn("releases/tag/v2.3.2\"", body)
        self.assertNotIn("v2.3.1\"", body)
        self.assertNotIn("rc.1", body)
        # appVersion has no "v", the tags do: the diff link follows the tags.
        self.assertIn("compare/v2.3.1...v2.4.0", body)

    def test_same_app_version(self):
        status, body = self.render("alloy", "1.13.0", "1.13.0")
        self.assertEqual(status, 200)
        self.assertIn("Same x/y version", body)

    def test_unknown_version(self):
        status, _ = self.render("alloy", "1.13.0", "9.9.9")
        self.assertEqual(status, 404)

    def test_unknown_chart(self):
        with mock.patch.dict(chart_notes.CHARTS, {}, clear=True):
            status, _ = chart_notes.render("nope", "1", "2")
        self.assertEqual(status, 404)


class Charts(unittest.TestCase):
    def test_bundled_charts_are_well_formed(self):
        for name, chart in chart_notes.load_charts().items():
            self.assertTrue(chart["repo"].startswith("https://"), name)
            self.assertRegex(chart["app"], r"^[\w.-]+/[\w.-]+$", name)

    def test_charts_file_overrides_and_extends(self):
        extra = FIXTURES.parent / "extra-charts.json"
        extra.write_text(json.dumps({"loki": {"repo": REPO, "app": "x/y"}, "new": {"repo": REPO, "app": "a/b"}}))
        self.addCleanup(extra.unlink)
        with mock.patch.dict("os.environ", {"CHARTS_FILE": str(extra)}):
            charts = chart_notes.load_charts()
        self.assertEqual(charts["loki"]["app"], "x/y")
        self.assertEqual(charts["new"]["app"], "a/b")
        self.assertIn("alloy", charts)


if __name__ == "__main__":
    unittest.main()
