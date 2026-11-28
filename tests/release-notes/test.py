#!/usr/bin/env python3
"""scripts/release-notes.py on a made-up repo: both kinds of merges, a merge that isn't a pull
request, a pull request found twice, and pull requests for each host.

    python3 tests/release-notes/test.py
"""

import importlib.util
import os
import subprocess
import tempfile
import unittest
from pathlib import Path

SCRIPT = Path(__file__).resolve().parents[2] / "scripts" / "release-notes.py"
spec = importlib.util.spec_from_file_location("release_notes", SCRIPT)
assert spec and spec.loader
notes = importlib.util.module_from_spec(spec)
spec.loader.exec_module(notes)

ENV = {
    "GIT_AUTHOR_NAME": "Someone",
    "GIT_AUTHOR_EMAIL": "someone@example.com",
    "GIT_COMMITTER_NAME": "Someone",
    "GIT_COMMITTER_EMAIL": "someone@example.com",
    "GIT_CONFIG_GLOBAL": os.devnull,
    "GIT_CONFIG_NOSYSTEM": "1",
}


class Repo:
    def __init__(self, path: Path):
        self.path = path
        self.git("init", "-q", "-b", "main")

    def git(self, *args: str) -> str:
        done = subprocess.run(
            ["git", *args], cwd=self.path, capture_output=True, text=True, env={**os.environ, **ENV}, check=True
        )
        return done.stdout

    def touch(self, name: str) -> None:
        file = self.path / name
        file.parent.mkdir(parents=True, exist_ok=True)
        with file.open("a") as out:
            out.write("a line\n")
        self.git("add", name)

    def squash(self, subject: str, *files: str, email: str = "") -> None:
        for name in files or ("README.md",):
            self.touch(name)
        self.git("commit", "-q", "-m", subject, *(["--author", f"Bot <{email}>"] if email else []))

    def merge(self, branch: str, message: str, *files: str) -> None:
        """A branch with one commit per file, merged into main with a merge commit."""
        made = bool(self.git("branch", "--list", branch).strip())
        self.git("checkout", "-q", *([] if made else ["-b"]), branch)
        for name in files:
            self.touch(name)
            self.git("commit", "-q", "-m", f"Change {name}")
        self.git("checkout", "-q", "main")
        self.git("merge", "-q", "--no-ff", "-m", message, branch)


class Notes(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.dir = tempfile.TemporaryDirectory()
        repo = Repo(Path(cls.dir.name))
        cls.repo = repo
        repo.squash("Start")
        repo.git("tag", "v0.1.0")
        repo.git("tag", "windows-v0.1.0")
        repo.squash("Windows app: a gear in the title bar (#10)", "windows/app/src/main.rs")
        repo.merge(
            "remote-view",
            "Merge pull request #11 from ruxnlabs/remote-view\n\nRemote view: a pairing step always ends\n\nMore words.",
            "shared/remote-view/src/lib.rs",
        )
        repo.squash("Mac host: the app icon (#12)", "mac/Icon/icon.png")
        repo.merge(
            "mac-graphics",
            "Merge pull request #13 from ruxnlabs/mac-graphics\n\nThe graphics process",
            "mac/App/a.swift",
            "mac/pvg-host/src/lib.rs",
        )
        repo.merge(
            "only-windows",
            "Merge pull request #14 from ruxnlabs/only-windows\n\nKeep up with a chatty guest",
            "windows/setup.ps1",
            "windows/host/src/lib.rs",
        )
        # A branch that took main in, then was merged: the merge of main isn't a pull request.
        repo.git("checkout", "-q", "-b", "catch-up")
        repo.touch("linux/core/src/lib.rs")
        repo.git("commit", "-q", "-m", "Catch up")
        repo.git("checkout", "-q", "main")
        repo.squash("Linux app: a new button (#15)", "linux/app/src/main.rs")
        repo.git("checkout", "-q", "catch-up")
        repo.git("merge", "-q", "--no-ff", "-m", "Merge branch 'main' into catch-up", "main")
        repo.git("checkout", "-q", "main")
        repo.git("merge", "-q", "--no-ff", "-m", "Merge remote-tracking branch 'origin/catch-up'", "catch-up")
        # The same pull request twice: a merge commit, then a squash with its number.
        # Its branch has a commit with the number too, as one squashed into it would.
        repo.git("checkout", "-q", "-b", "twice")
        repo.touch("shared/idle/src/inner.rs")
        repo.git("commit", "-q", "-m", "Move the idle rules into a crate every host can share (#16)")
        repo.git("checkout", "-q", "main")
        repo.merge(
            "twice",
            "Merge pull request #16 from ruxnlabs/twice\n\nMove the idle rules into a crate every host can share",
            "shared/idle/src/lib.rs",
        )
        repo.squash("Move the idle rules into a crate every host can share (#16)", "shared/idle/src/more.rs")
        repo.squash("Docs and site: the Mac nightly (#17)", "site/mac.html")
        repo.squash("Remote view: a host that times pairing itself (#18)", "shared/remote-view/src/time.rs")
        repo.squash(
            "Machine states: one list for every host, the Linux host's, and the Windows host's (#19)",
            "shared/state/src/lib.rs",
        )
        repo.squash("Release Windows 0.2.0 (#20)", "windows/app/Cargo.toml")
        repo.squash("Point the site at Windows 0.1.0 (#21)", "site/index.html")
        repo.squash("Bump a thing (#22)", "Cargo.lock", email="41898282+github-actions[bot]@users.noreply.github.com")
        repo.squash("AMD: the fence block goes to the card (#23)", "linux/backend-amd/src/lib.rs", "Cargo.lock")
        repo.squash("Sign the CLA (#24)", "CLA-SIGNERS.md")
        repo.git("tag", "v0.2.0")
        repo.git("tag", "windows-v0.2.0")

    @classmethod
    def tearDownClass(cls):
        cls.dir.cleanup()

    def setUp(self):
        self.cwd = os.getcwd()
        os.chdir(self.repo.path)

    def tearDown(self):
        os.chdir(self.cwd)

    def draft(self, tag: str) -> str:
        since = notes.previous_tag(tag, notes.kind_of(tag))
        return notes.notes(tag, since, "Intro.", notes.pulls(since, tag))

    def listed(self, text: str) -> list[int]:
        top = text.split("<!-- Left out")[0]
        return [int(line.rsplit("(#", 1)[1].rstrip(")")) for line in top.splitlines() if line.startswith("- ")]

    def left_out(self, text: str) -> str:
        return text.split("<!-- Left out of the draft:")[1]

    def test_pull_requests_of_both_kinds_in_order(self):
        found = [number for number, _, _, _ in notes.pulls("v0.1.0", "v0.2.0")]
        self.assertEqual(found, [10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24])

    def test_inner_commit_is_in_the_merge(self):
        inner = self.repo.git("log", "--format=%s", "v0.1.0..v0.2.0")
        self.assertEqual(inner.count("crate every host can share (#16)"), 2)

    def test_merge_title_is_the_first_line_of_its_message(self):
        titles = {number: title for number, title, _, _ in notes.pulls("v0.1.0", "v0.2.0")}
        self.assertEqual(titles[11], "Remote view: a pairing step always ends")
        self.assertEqual(titles[16], "Move the idle rules into a crate every host can share")

    def test_windows(self):
        text = self.draft("windows-v0.2.0")
        self.assertEqual(sorted(self.listed(text)), [10, 11, 14, 16, 18, 19])
        self.assertEqual(text.count("(#16)"), 1)
        self.assertNotIn("Merge branch", text)
        self.assertNotIn("remote-tracking", text)
        out = self.left_out(text)
        self.assertIn("Mac host: the app icon (#12): the Mac host only, by its title", out)
        self.assertIn("The graphics process (#13): the Mac host only, by the files it touched", out)
        self.assertIn("Linux app: a new button (#15): the Linux host only, by its title", out)
        self.assertIn("Docs and site: the Mac nightly (#17): the Mac host only, by its title", out)
        self.assertIn("Release Windows 0.2.0 (#20): a release chore", out)
        self.assertIn("Point the site at Windows 0.1.0 (#21): a release chore", out)
        self.assertIn("Bump a thing (#22): a bot's", out)
        self.assertIn("AMD: the fence block goes to the card (#23): the Linux host only, by the files it touched", out)
        self.assertIn("Sign the CLA (#24): a contributor licence signature", out)

    def test_linux(self):
        text = self.draft("v0.2.0")
        self.assertEqual(sorted(self.listed(text)), [11, 15, 16, 18, 19, 23])
        out = self.left_out(text)
        self.assertIn("Windows app: a gear in the title bar (#10): the Windows host only, by its title", out)
        self.assertIn("Keep up with a chatty guest (#14): the Windows host only, by the files it touched", out)

    def test_every_left_out_one_is_listed(self):
        for tag in ("v0.2.0", "windows-v0.2.0"):
            text = self.draft(tag)
            for number in range(10, 25):
                self.assertEqual(text.count(f"(#{number})"), 1, f"#{number} in {tag}")


if __name__ == "__main__":
    unittest.main()
