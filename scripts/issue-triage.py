#!/usr/bin/env python3
"""Says what to do with a bug report: which card labels to add, and whether to ask for a report ID.

Reads one issue as JSON (`gh issue view N --json number,body,labels,comments`), and the names of
the labels the repository has, and prints one action per line for the workflow to carry out.
    label NAME      add this label
    comment         post the comment in ISSUE_COMMENT, once
It never removes a label, and it never decides anything from the reporter's own words but the
card they named. Nothing here touches GitHub.
Usage: issue-triage.py ISSUE.json [LABEL ...]
       issue-triage.py --comment      prints the comment's text
"""
import json
import re
import sys

# Hidden in the comment, so that a second run can tell it was already made.
MARKER = "<!-- triage:report-id -->"

COMMENT = (
    MARKER
    + "\nThanks for the report. Please send one from the app too, so the logs can be found. "
    "Choose Show Log in the menu, then Send Report. It gives you a report ID. "
    "Add a line to your first post with the words Report ID and the ID. "
    "If the app won't open, guesthouse-report does the same from a terminal. "
    "docs/reporting.md has the steps.\n"
)

# The words of a card's name, per label. CPUs are taken out first, so "AMD Ryzen 7 with an RTX
# 4070" is an NVIDIA card.
CPU = re.compile(
    r"\b(amd\s+)?ryzen\s*(\d\s*)?(\w*\d\w*)?|\b(intel\s+)?core\s+(ultra\s*)?(i\d|\d)[\s-]*(\w*\d\w*)?|\bxeon\b|\bthreadripper\b|\bepyc\b",
    re.I,
)
CARDS = {
    "nvidia": re.compile(r"nvidia|geforce|\b[rg]tx\b|\bgtx\b|quadro|\btesla\b|\btitan\b", re.I),
    "amd": re.compile(r"\bamd\b|radeon|\brx\s?\d|\bvega\b|\bnavi\b|\brdna", re.I),
    "intel": re.compile(r"\bintel\b|\barc\b|\bb[5-9]\d0\b|\ba[3-7]\d0\b|\biris\b|\buhd\b", re.I),
}

REPORT_ID = re.compile(r"report\s*id\s*:|report\s*id\W{0,4}[0-9a-f]{8,}", re.I)


def section(body, heading):
    """What the form put under a heading, or None when it has no such heading."""
    found = re.search(r"^### " + re.escape(heading) + r"[ \t]*\r?\n(.*?)(?=^### |\Z)", body, re.S | re.M)
    return found.group(1).strip() if found else None


def triage(issue, repo_labels):
    body = issue.get("body") or ""
    actions = []
    card = section(body, "Your graphics card")
    # Only the bug form has both of these. The feature form's card line has another name.
    if card is None or section(body, "What happened") is None:
        return actions
    card = CPU.sub(" ", card)
    have = {label["name"] for label in issue.get("labels", [])}
    for name, pattern in CARDS.items():
        if pattern.search(card) and name in repo_labels and name not in have:
            actions.append("label " + name)
    asked = any(MARKER in (c.get("body") or "") for c in issue.get("comments", []))
    if not REPORT_ID.search(body) and not asked:
        actions.append("comment")
    return actions


def main():
    if sys.argv[1] == "--comment":
        sys.stdout.write(COMMENT)
        return
    issue = json.load(open(sys.argv[1]))
    for action in triage(issue, set(sys.argv[2:])):
        print(action)


if __name__ == "__main__":
    main()
