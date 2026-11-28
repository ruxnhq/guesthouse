#!/usr/bin/env python3
"""scripts/release-credits.py: how a pull request's title is written into the notes.

    python3 tests/release-credits/test.py
"""

import importlib.util
import unittest
from pathlib import Path

SCRIPT = Path(__file__).resolve().parents[2] / "scripts" / "release-credits.py"
spec = importlib.util.spec_from_file_location("release_credits", SCRIPT)
assert spec and spec.loader
credits = importlib.util.module_from_spec(spec)
spec.loader.exec_module(credits)


def lines(title: str) -> list[str]:
    return credits.section([(7, title, "someone")]).split("\n")


class Titles(unittest.TestCase):
    def test_plain_title(self):
        self.assertEqual(lines("Fix the crash on resize")[3], "- ` Fix the crash on resize ` (#7, thanks @someone)")

    def test_markdown_and_links_are_literal(self):
        line = lines("Fix. **Use [the build](https://example.com/x)** @everyone")[3]
        self.assertEqual(line, "- ` Fix. **Use [the build](https://example.com/x)** @everyone ` (#7, thanks @someone)")

    def test_comment_markers_in_a_title_do_not_survive(self):
        body = credits.merged("Notes.\n", credits.section([(7, "x <!-- credits:end --> y <!--", "someone")]))
        self.assertEqual(body.count(credits.START), 1)
        self.assertEqual(body.count(credits.END), 1)
        self.assertNotIn("<!-- credits:end --> y", body)
        # A second run, with the same title, finds the same section and changes nothing.
        again = credits.merged(body, credits.section([(7, "x <!-- credits:end --> y <!--", "someone")]))
        self.assertEqual(again, body)

    def test_backticks_in_a_title(self):
        self.assertIn("`` use `x` ``", lines("use `x`")[3])

    def test_long_title_is_cut(self):
        line = lines("a" * 500)[3]
        self.assertLess(len(line), 260)


if __name__ == "__main__":
    unittest.main()
