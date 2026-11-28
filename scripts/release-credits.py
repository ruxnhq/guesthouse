#!/usr/bin/env python3
"""The credits for a Linux release's notes: the outside people whose pull requests were merged into it.

    release-credits.py [--repo OWNER/REPO] [--apply] TAG

Prints a "Thanks" section for the release notes, one line per outside pull request merged since the
release before TAG and up to TAG's own, oldest first. With --apply it writes that section into the
body of TAG's release instead, in place of a section it wrote before, so running it again changes
nothing. It prints nothing and changes nothing when there were no outside pull requests.

Who counts is the rule of the Discord bot's credits command, which gives the same people. Pull
requests of the repo's owners, members and collaborators are theirs, and bots' aren't thanked, and
everyone else is, first-timers too. Nothing here knows about the CLA. GitHub is asked through `gh`,
which takes GH_TOKEN in CI, and the token only needs to read.
"""

import argparse
import json
import re
import subprocess
import sys
from datetime import datetime
from urllib.parse import quote_plus

# These write as the project. Their pull requests are not thanked as outside work.
OWN = frozenset({"OWNER", "MEMBER", "COLLABORATOR"})
PAGES, PAGE = 3, 100
MAX_TITLE = 200
START, END = "<!-- credits:start -->", "<!-- credits:end -->"
# A pull request that only signs the CLA isn't a contribution to thank. Credits are never tied to the CLA.
CLA_PATHS = ("CLA/", "CLA-SIGNERS.md")
LINUX_TAG = re.compile(r"v\d+\.\d+\.\d+")


def gh(*args: str) -> object:
    done = subprocess.run(["gh", *args], capture_output=True, text=True)
    if done.returncode != 0:
        sys.exit(f"gh {args[0]} {args[1]} failed: {done.stderr.strip()}")
    return json.loads(done.stdout) if done.stdout.strip() else None


def stamp(when: str) -> datetime:
    return datetime.fromisoformat(when.replace("Z", "+00:00"))


def window(releases: list[dict], tag: str) -> tuple[str | None, str]:
    """When the release before `tag` went out, or None for the first one, and when `tag` did."""
    this = next((r for r in releases if r.get("tag_name") == tag and r.get("published_at")), None)
    if this is None:
        sys.exit(f"I found no published release called {tag}.")
    before = [
        r
        for r in releases
        if LINUX_TAG.fullmatch(r.get("tag_name") or "")
        and not r.get("draft")
        and not r.get("prerelease")
        and r.get("published_at")
        and stamp(r["published_at"]) < stamp(this["published_at"])
    ]
    start = max(before, key=lambda r: stamp(r["published_at"]), default=None)
    return (start["published_at"] if start else None), this["published_at"]


def outside_pulls(items: list) -> list[tuple[int, str, str]]:
    """(number, title, login) of the outside pull requests, oldest first."""
    found = []
    for item in items:
        if not isinstance(item, dict) or "pull_request" not in item:
            continue
        user = item.get("user") or {}
        login, number, title = user.get("login"), item.get("number"), item.get("title")
        if not isinstance(login, str) or user.get("type") == "Bot" or item.get("author_association") in OWN:
            continue
        if isinstance(number, int) and isinstance(title, str):
            found.append((number, " ".join(title.split()), login))
    return sorted(found)


def only_signs_the_cla(files: list[str]) -> bool:
    """Whether a pull request's files are all CLA signatures."""
    return bool(files) and all(name.startswith(CLA_PATHS) for name in files)


def changed_files(repo: str, number: int) -> list[str]:
    answer = gh("api", f"repos/{repo}/pulls/{number}/files?per_page=100")
    return [f["filename"] for f in answer if isinstance(f, dict) and isinstance(f.get("filename"), str)]


def literal(title: str) -> str:
    """A title as plain text in the notes: in a code span, so none of it is read as Markdown, a link,
    a mention or HTML, with angle brackets shown as single quotes so a title can't hold the markers."""
    text = title.replace("<", "\u2039").replace(">", "\u203a")
    if len(text) > MAX_TITLE:
        text = text[: MAX_TITLE - 1].rstrip() + "\u2026"
    ticks = "`" * (max((len(run) for run in re.findall(r"`+", text)), default=0) + 1)
    return f"{ticks} {text} {ticks}"


def section(pulls: list[tuple[int, str, str]]) -> str:
    """The notes' section, between markers so a second run finds it again."""
    lines = [f"- {literal(title)} (#{number}, thanks @{login})" for number, title, login in pulls]
    return "\n".join([START, "**Thanks**", "", *lines, END])


def merged(body: str, new: str) -> str:
    """The release body with the section in it: in place of the old one, or else at the end."""
    old = re.compile(re.escape(START) + r".*?" + re.escape(END), re.S)
    if old.search(body):
        return old.sub(lambda _: new, body)
    return body.rstrip("\n") + "\n\n" + new + "\n"


def main() -> int:
    ask = argparse.ArgumentParser(description="Credits for a release's notes.")
    ask.add_argument("tag")
    ask.add_argument("--repo", default="ruxnlabs/guesthouse")
    ask.add_argument("--apply", action="store_true", help="write the section into the release's body")
    args = ask.parse_args()

    releases = gh("api", f"repos/{args.repo}/releases?per_page=100")
    since, until = window(releases, args.tag)
    span = f"{since}..{until}" if since else f"<={until}"
    query = quote_plus(f"repo:{args.repo} is:pr is:merged merged:{span}", safe=":/<>=.")
    items: list = []
    for page in range(1, PAGES + 1):
        answer = gh("api", f"search/issues?q={query}&sort=created&order=asc&per_page={PAGE}&page={page}")
        found = answer.get("items") if isinstance(answer, dict) else None
        if not isinstance(found, list):
            sys.exit("GitHub's search gave no list of pull requests.")
        items += found
        if len(found) < PAGE:
            break

    pulls = [p for p in outside_pulls(items) if not only_signs_the_cla(changed_files(args.repo, p[0]))]
    if not pulls:
        print(f"No outside pull requests in {args.tag}.", file=sys.stderr)
        return 0
    text = section(pulls)
    if not args.apply:
        print(text)
        return 0
    release = next(r for r in releases if r["tag_name"] == args.tag)
    body = merged(release.get("body") or "", text)
    if body != (release.get("body") or ""):
        gh("api", "-X", "PATCH", f"repos/{args.repo}/releases/{release['id']}", "-f", f"body={body}")
        print(f"Credits written into the notes of {args.tag}.", file=sys.stderr)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
