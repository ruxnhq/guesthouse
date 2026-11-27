#!/usr/bin/env python3
"""A first draft of a release's notes, from the pull requests merged since the release before.

    release-notes.py [--previous TAG] [--intro TEXT] TAG
    release-notes.py --print-previous TAG
    release-notes.py --list-previous TAG

TAG is vX.Y.Z for the Linux host or windows-vX.Y.Z for the Windows host. The release before is the
newest tag of the same kind below TAG that TAG's commit has in it, unless --previous names one. The
--print-previous only prints that tag, or nothing if there's none, and --list-previous prints every
tag it could be, newest first, for a workflow to pick the newest that was really published. The
pull requests are the squash merges on main between the two, found by the (#N) at the end of their
subjects, so it reads only git and needs no token. Each goes under the area its title starts with,
like "App windows: ..." under App windows. An area with one pull request goes under Other with
its title whole, and so does a title with no area.

Left out, and listed in a comment at the end so nothing goes missing unseen: the version pull
requests, the ones pointing the installer at a release, contributor licence signatures, and bots'.
There's no Thanks section: release-credits.yml writes that once the release is published.
"""

import argparse
import re
import subprocess
import sys

KINDS = {
    "linux": re.compile(r"v(\d+)\.(\d+)\.(\d+)"),
    "windows": re.compile(r"windows-v(\d+)\.(\d+)\.(\d+)"),
}
PULL = re.compile(r"^(?P<title>.*\S)\s+\(#(?P<number>\d+)\)$")
AREA = re.compile(r"^(?P<area>[^:]{2,40}):\s+(?P<rest>\S.*)$")
LEFT_OUT = [
    re.compile(r"^Point the installer at "),
    re.compile(r"^Version \d+\.\d+\.\d+$"),
    re.compile(r"^Windows \d+\.\d+\.\d+$"),
    re.compile(r"^Sign(ing)? (the )?(contributor licen[cs]e agreement|CLA)\b", re.I),
]
# The areas that go last, in this order, after the others by size.
LAST = ["Docs", "Other"]

LINUX_INTRO = """\
TODO: one or two sentences on what this release is about.

**To update,** use the button in the app, or run `~/.local/lib/guesthouse/current/install.sh --update`. \
Your machines, disks and settings stay as they are. To install, use the line in the README, or download \
the AppImage below and open it."""


def git(*args: str) -> str:
    done = subprocess.run(["git", *args], capture_output=True, text=True)
    if done.returncode != 0:
        sys.exit(f"git {' '.join(args)} failed: {done.stderr.strip()}")
    return done.stdout


def kind_of(tag: str) -> str:
    for kind, shape in KINDS.items():
        if shape.fullmatch(tag):
            return kind
    sys.exit(f"{tag} is neither vX.Y.Z nor windows-vX.Y.Z.")


def version(tag: str, kind: str) -> tuple[int, ...]:
    found = KINDS[kind].fullmatch(tag)
    assert found
    return tuple(int(part) for part in found.groups())


def previous_tags(tag: str, kind: str) -> list[str]:
    """The tags of the same kind below `tag` that `tag` has in it, newest first."""
    mine = version(tag, kind)
    older = [
        other
        for other in git("tag", "--list").split()
        if KINDS[kind].fullmatch(other) and version(other, kind) < mine
    ]
    return [
        other
        for other in sorted(older, key=lambda t: version(t, kind), reverse=True)
        if subprocess.run(["git", "merge-base", "--is-ancestor", other, tag], capture_output=True).returncode == 0
    ]


def previous_tag(tag: str, kind: str) -> str | None:
    """The newest tag of the same kind below `tag` that `tag` has in it."""
    return next(iter(previous_tags(tag, kind)), None)


def pulls(since: str | None, tag: str) -> list[tuple[int, str, str]]:
    """(number, title, author's email) of the squash merges on main in since..tag, oldest first."""
    span = f"{since}..{tag}" if since else tag
    found = []
    for line in git("log", "--first-parent", "--reverse", "--format=%ae%x09%s", span).splitlines():
        email, _, subject = line.partition("\t")
        match = PULL.match(subject.strip())
        if match:
            found.append((int(match["number"]), " ".join(match["title"].split()), email))
    return found


def left_out(title: str, email: str) -> bool:
    return "[bot]" in email or any(rule.search(title) for rule in LEFT_OUT)


def area_of(title: str) -> tuple[str, str]:
    """The area a title starts with, "Windows hosts" and "Windows host" as one, and the rest of it."""
    match = AREA.match(title)
    if not match:
        return "Other", title
    area = match["area"].strip()
    return (area[:-1] if area.endswith("s") and len(area) > 3 else area), match["rest"]


def grouped(items: list[tuple[int, str, str]]) -> dict[str, list[str]]:
    areas = [area_of(title)[0] for _, title, _ in items]
    groups: dict[str, list[str]] = {}
    for (number, title, _), area in zip(items, areas):
        line = area_of(title)[1]
        if areas.count(area) < 2:
            area, line = "Other", title
        line = line[0].upper() + line[1:]
        groups.setdefault(area, []).append(f"- {line.rstrip('.')} (#{number})")
    first = sorted((a for a in groups if a not in LAST), key=lambda a: -len(groups[a]))
    return {area: groups[area] for area in first + [a for a in LAST if a in groups]}


def notes(tag: str, since: str | None, intro: str, items: list[tuple[int, str, str]]) -> str:
    kept = [item for item in items if not left_out(item[1], item[2])]
    dropped = [item for item in items if left_out(item[1], item[2])]
    out = [
        f"<!-- Drafted from the pull requests merged {f'since {since}' if since else 'so far'}. "
        "Edit it before publishing. The Thanks section is added once it's published. -->",
        "",
        intro.strip(),
    ]
    for area, lines in grouped(kept).items():
        out += ["", f"**{area}**", "", *lines]
    if not kept:
        out += ["", "No pull requests were merged since the release before."]
    if dropped:
        out += ["", "<!-- Left out of the draft:"]
        out += [f"  {title} (#{number})" for number, title, _ in dropped]
        out += ["-->"]
    return "\n".join(out) + "\n"


def main() -> int:
    ask = argparse.ArgumentParser(description="A draft of a release's notes.")
    ask.add_argument("tag")
    ask.add_argument("--previous", help="the release before, if not the newest tag of the same kind")
    ask.add_argument("--intro", help="what goes at the top, in place of the Linux host's usual opening")
    ask.add_argument("--print-previous", action="store_true", help="only print the release before")
    ask.add_argument("--list-previous", action="store_true", help="only print what could be the release before")
    args = ask.parse_args()

    kind = kind_of(args.tag)
    if args.list_previous:
        for other in previous_tags(args.tag, kind):
            print(other)
        return 0
    since = args.previous or previous_tag(args.tag, kind)
    if args.print_previous:
        print(since or "")
        return 0
    if since:
        print(f"Pull requests since {since}.", file=sys.stderr)
    intro = args.intro if args.intro is not None else LINUX_INTRO
    sys.stdout.write(notes(args.tag, since, intro, pulls(since, args.tag)))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
