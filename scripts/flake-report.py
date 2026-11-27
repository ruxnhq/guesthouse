#!/usr/bin/env python3
"""The flaky tests of a night's flake hunt, and an issue for each.

    flake-report.py [--repo OWNER/REPO] [--jobs FILE] [--run-url URL] [--commit SHA] [--apply] LOGS

LOGS holds a folder for each run of the workspace's tests, named run-N or flake-run-N, with that
run's `cargo test` output in test.log. A test is flaky when it failed in some runs and passed in
others. A test binary that crashed, which cargo tells by its exit, counts the same way.
A test that failed in every run it ran in is a broken main, which the normal CI shows, so it is only
listed in the summary. A run that never got to the tests, as a failed build, isn't counted.

It prints what it found, and writes it to the job's summary in CI. Without --apply it prints the
issues it would write. With --apply each flaky test gets an issue labelled flaky-test, or a comment on
the open one with the same title. GitHub is asked through `gh`, which takes GH_TOKEN in CI. FILE has
the run's jobs, a JSON object a line with each job's name ("run N") and html_url, so the issue links
each failing run's log.
"""

import argparse
import json
import os
import re
import subprocess
import sys
from collections import defaultdict
from dataclasses import dataclass, field
from pathlib import Path

LABEL = "flaky-test"
LABEL_COLOUR = "d93f0b"
LABEL_TEXT = "Fails in some runs and passes in others. The nightly flake hunt files these."
# A night that finds more than this is a broken main or a broken hunt, not a few flaky tests.
MOST_ISSUES = 10
EXCERPT_LINES = 60
STOPPED = "(the test binary crashed)"

ANSI = re.compile(r"\x1b\[[0-9;]*[A-Za-z]")
STAMP = re.compile(r"^\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d(\.\d+)?Z ")
RUNNING = re.compile(r"^\s*Running (?:unittests )?(\S+) \((.+)\)$")
DOC_TESTS = re.compile(r"^\s*Doc-tests (\S+)$")
# The new build folder layout, build/PACKAGE/HASH/out/NAME-HASH, and the old one, deps/NAME-HASH.
NEW_LAYOUT = re.compile(r"build/([^/]+)/[0-9a-f]+/out/[^/]+$")
OLD_LAYOUT = re.compile(r"([^/]+)-[0-9a-f]+(\.exe)?$")
TEST = re.compile(r"^test (.+?) \.\.\. (ok|FAILED|ignored.*)$")
OUTPUT = re.compile(r"^---- (.+?) (stdout|stderr) ----$")
RUN_NUMBER = re.compile(r"(\d+)$")
# cargo's word on a test binary that failed. Exit status 101 is failed tests, and anything else is a crash.
EXITED = re.compile(r"process didn't exit successfully: `([^`\s]+)[^`]*` \((.+)\)")


@dataclass
class Run:
    """What one run of the tests said: each test's result, by binary and name, and what failed said."""

    number: int
    results: dict = field(default_factory=dict)
    output: dict = field(default_factory=dict)
    reached_tests: bool = False


def binary_label(source: str, path: str) -> str:
    """The test binary as a person finds it: its package and its source, as `app-core tests/flow.rs`."""
    found = NEW_LAYOUT.search(path)
    if found:
        return f"{found.group(1)} {source}"
    found = OLD_LAYOUT.search(path)
    return f"{found.group(1) if found else path} {source}"


def parse(number: int, text: str) -> Run:
    run = Run(number)
    binary = None
    labels, crashed = {}, set()
    capture, captured = None, []

    def end_capture():
        nonlocal capture, captured
        if capture is not None:
            run.output[capture] = "\n".join(captured).strip("\n")
        capture, captured = None, []

    for raw in text.splitlines():
        line = STAMP.sub("", ANSI.sub("", raw)).rstrip()
        if found := RUNNING.match(line):
            end_capture()
            binary = binary_label(found.group(1), found.group(2))
            labels[found.group(2)] = binary
            continue
        if found := EXITED.search(line):
            path = found.group(1)
            label = next((labels[key] for key in labels if path.endswith(key)), None)
            if label and found.group(2) != "exit status: 101":
                crashed.add(label)
            continue
        if found := DOC_TESTS.match(line):
            end_capture()
            binary = f"{found.group(1)} doc-tests"
            continue
        if binary is None:
            continue
        if re.match(r"^running \d+ tests?$", line):
            run.reached_tests = True
            continue
        if line.startswith("test result: "):
            end_capture()
            continue
        if found := OUTPUT.match(line):
            end_capture()
            capture = (binary, found.group(1))
            continue
        if capture is not None:
            if line == "failures:":
                end_capture()
            else:
                captured.append(line)
            continue
        if found := TEST.match(line):
            if not found.group(2).startswith("ignored"):
                run.results[(binary, found.group(1))] = found.group(2) == "ok"
    end_capture()
    for label in labels.values():
        run.results[(label, STOPPED)] = label not in crashed
    return run


def read_runs(folder: Path) -> list[Run]:
    runs = []
    for log in sorted(folder.glob("*/test.log")):
        found = RUN_NUMBER.search(log.parent.name)
        number = int(found.group(1)) if found else len(runs) + 1
        runs.append(parse(number, log.read_text(errors="replace")))
    return sorted(runs, key=lambda run: run.number)


@dataclass
class Finding:
    binary: str
    test: str
    failed: list
    passed: int

    @property
    def title(self) -> str:
        if self.test == STOPPED:
            return f"Flaky crash: the tests of {self.binary}"
        return f"Flaky test: {self.test} in {self.binary}"

    @property
    def subject(self) -> str:
        if self.test == STOPPED:
            return f"The test binary of `{self.binary}` crashed: it"
        return f"`{self.test}` in `{self.binary}`"


def findings(runs: list[Run]) -> tuple[list[Finding], list[Finding]]:
    """The flaky tests, and those that failed every time they ran."""
    failed, passed = defaultdict(list), defaultdict(int)
    for run in runs:
        for key, ok in run.results.items():
            if ok:
                passed[key] += 1
            else:
                failed[key].append(run)
    flaky, always = [], []
    for key in sorted(failed):
        found = Finding(key[0], key[1], failed[key], passed[key])
        (flaky if found.passed else always).append(found)
    return flaky, always


def fenced(text: str) -> str:
    fence = "```"
    while fence in text:
        fence += "`"
    return f"{fence}text\n{text}\n{fence}"


def excerpt(found: Finding) -> str:
    for run in found.failed:
        said = run.output.get((found.binary, found.test))
        if said:
            lines = said.splitlines()
            if len(lines) > EXCERPT_LINES:
                lines = lines[:EXCERPT_LINES] + [f"... and {len(lines) - EXCERPT_LINES} more lines in the log"]
            return f"What it said in run {run.number}:\n\n{fenced(chr(10).join(lines))}\n"
    return ""


def run_links(found: Finding, jobs: dict) -> str:
    lines = []
    for run in found.failed:
        url = jobs.get(run.number)
        lines.append(f"- [run {run.number}]({url})" if url else f"- run {run.number}")
    return "\n".join(lines)


def counts(found: Finding, total: int) -> str:
    return f"failed in {len(found.failed)} of {total} runs and passed in {found.passed}"


def issue_body(found: Finding, total: int, jobs: dict, run_url: str, commit: str) -> str:
    at = f" at {commit[:12]}" if commit else ""
    return "\n".join(
        [
            f"{found.subject} {counts(found, total)}, in the nightly flake hunt on main{at}.",
            "",
            "Failing runs:",
            run_links(found, jobs),
            "",
            excerpt(found),
            f"The hunt runs the workspace's tests {total} times, each run on its own runner under extra "
            f"CPU load. A test that fails every time isn't filed here. The night's hunt: {run_url}",
        ]
    )


def comment_body(found: Finding, total: int, jobs: dict, run_url: str, commit: str) -> str:
    at = f" at {commit[:12]}" if commit else ""
    return "\n".join(
        [
            f"Again on main{at}: it {counts(found, total)}. The hunt: {run_url}",
            "",
            run_links(found, jobs),
            "",
            excerpt(found),
        ]
    ).rstrip() + "\n"


def summary(runs: list[Run], counted: list[Run], flaky: list[Finding], always: list[Finding]) -> str:
    lines = [f"## Flake hunt: {len(counted)} of {len(runs)} runs got to the tests", ""]
    missing = [str(run.number) for run in runs if run not in counted]
    if missing:
        lines += [f"Runs that never got to the tests, as a failed build: {', '.join(missing)}.", ""]
    if not flaky and not always:
        lines.append("Every test that ran passed in every run.")
    for heading, found_list in (("Flaky", flaky), ("Failed in every run", always)):
        if found_list:
            lines += [f"### {heading}", "", "| test | binary | failed | passed |", "|---|---|---|---|"]
            for found in found_list:
                test = found.test.replace("|", "\\|")
                lines.append(f"| `{test}` | {found.binary} | {len(found.failed)} | {found.passed} |")
            lines.append("")
    return "\n".join(lines) + "\n"


def gh(*args: str, stdin: str | None = None) -> str:
    done = subprocess.run(["gh", *args], input=stdin, capture_output=True, text=True)
    if done.returncode != 0:
        sys.exit(f"gh {' '.join(args[:2])} failed: {done.stderr.strip()}")
    return done.stdout


def read_jobs(path: Path | None) -> dict:
    jobs = {}
    if path is None:
        return jobs
    for line in path.read_text().splitlines():
        if not line.strip():
            continue
        job = json.loads(line)
        found = re.fullmatch(r"run (\d+)", job.get("name") or "")
        if found and job.get("html_url"):
            jobs[int(found.group(1))] = job["html_url"]
    return jobs


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("logs", type=Path)
    parser.add_argument("--repo", default=os.environ.get("GITHUB_REPOSITORY", ""))
    parser.add_argument("--jobs", type=Path)
    parser.add_argument("--run-url", default="")
    parser.add_argument("--commit", default="")
    parser.add_argument("--apply", action="store_true")
    arguments = parser.parse_args()

    runs = read_runs(arguments.logs)
    counted = [run for run in runs if run.reached_tests]
    flaky, always = findings(counted)
    text = summary(runs, counted, flaky, always)
    print(text)
    if os.environ.get("GITHUB_STEP_SUMMARY"):
        with open(os.environ["GITHUB_STEP_SUMMARY"], "a") as written:
            written.write(text)
    if len(counted) < 2:
        print("Fewer than two runs got to the tests, so nothing can be called flaky.")
        return 0
    if len(flaky) > MOST_ISSUES:
        print(f"{len(flaky)} flaky tests is more than {MOST_ISSUES}: no issues, as that is a broken hunt or main.")
        return 1
    if not flaky:
        return 0

    jobs = read_jobs(arguments.jobs)
    total = len(counted)
    if not arguments.apply:
        for found in flaky:
            print(f"--- would file or comment: {found.title}\n")
            print(issue_body(found, total, jobs, arguments.run_url, arguments.commit))
        return 0
    if not arguments.repo:
        sys.exit("--repo is needed with --apply")
    repo = arguments.repo
    gh("label", "create", LABEL, "--repo", repo, "--color", LABEL_COLOUR, "--description", LABEL_TEXT, "--force")
    listed = json.loads(
        gh("issue", "list", "--repo", repo, "--label", LABEL, "--state", "open", "--limit", "500", "--json", "number,title")
    )
    open_issues = {issue["title"]: issue["number"] for issue in listed}
    for found in flaky:
        number = open_issues.get(found.title)
        if number is not None:
            body = comment_body(found, total, jobs, arguments.run_url, arguments.commit)
            gh("issue", "comment", str(number), "--repo", repo, "--body-file", "-", stdin=body)
            print(f"commented on #{number}: {found.title}")
        else:
            body = issue_body(found, total, jobs, arguments.run_url, arguments.commit)
            made = gh(
                "issue", "create", "--repo", repo, "--title", found.title, "--label", LABEL, "--body-file", "-", stdin=body
            )
            print(f"filed {made.strip()}: {found.title}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
