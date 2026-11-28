#!/usr/bin/env python3
"""A first draft of a release's notes, from the pull requests merged since the release before.

    release-notes.py [--previous TAG] [--intro TEXT] TAG
    release-notes.py --print-previous TAG
    release-notes.py --list-previous TAG

TAG is vX.Y.Z for the Linux host or windows-vX.Y.Z for the Windows host. The release before is the
newest tag of the same kind below TAG that TAG's commit has in it, unless --previous names one. The
--print-previous only prints that tag, or nothing if there's none, and --list-previous prints every
tag it could be, newest first, for a workflow to pick the newest that was really published.

The pull requests are the ones merged into main between the two, in the order they were merged:
squash merges, found by the (#N) at the end of their subjects, and merge commits, "Merge pull
request #N from ...", whose title is the first line of their message. Other merges, like "Merge
branch 'main' into ...", aren't pull requests. A pull request found twice is listed once. It reads
only git and needs no token. Each goes under the area its title starts with, like "App windows:
..." under App windows. An area with one pull request goes under Other with its title whole, and
so does a title with no area.

Each host's notes are only that host's news. A title that names a host ("Mac host: ...", "Windows
app: ...", "for the Mac host") is that host's, unless it also says every host or both. A title
that names none is every host's, unless every file it touched is one host's: mac/ and the Mac
crates are the Mac host's, windows/ and the Windows host's crates are the Windows host's, and the
Linux app, daemon, GPU backends, their cores and packaging are the Linux host's.

Left out, and listed with why in a comment at the end so nothing goes missing unseen: another
host's pull requests, the release and version pull requests, the ones pointing the installer or
the site at a release, contributor licence signatures, and bots'.
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
MERGE = re.compile(r"^Merge pull request #(?P<number>\d+) from (?P<branch>\S+)")
AREA = re.compile(r"^(?P<area>[^:]{2,40}):\s+(?P<rest>\S.*)$")
LEFT_OUT = [
    re.compile(r"^Point the (installer|site) at "),
    re.compile(r"^Version \d+\.\d+\.\d+$"),
    re.compile(r"^Windows \d+\.\d+\.\d+$"),
    re.compile(r"^Release (Windows )?\d+\.\d+\.\d+$"),
]
CLA = re.compile(r"^Sign(ing)? (the )?(contributor licen[cs]e agreement|CLA)\b", re.I)
# The hosts, by the words a title names them with. "Windows" alone is often the guest, so it isn't one.
HOSTS = {
    "mac": re.compile(r"\b(Mac|macOS)\b"),
    "windows": re.compile(r"\bWindows (hosts?|app|service|installer)\b|^Windows:"),
    "linux": re.compile(r"\bLinux (hosts?|app)\b|^Remote view on Linux:"),
}
EVERY_HOST = re.compile(r"\b(every|each|both|all) hosts?\b", re.I)
# The files only one host builds. Cargo.lock, Cargo.toml and CLA-SIGNERS.md say nothing either way, and any
# other file is every host's.
HOST_FILES = {
    # The old crates/ paths are kept for pull requests from before the folders moved (2026-11-27).
    "mac": re.compile(r"^(mac/|crates/(host-mac|pvg-host|pvg-sys)/)"),
    "windows": re.compile(
        r"^(windows/|crates/((app|host|service|installer)-windows|hcs|control)/|shared/control/|tools/hcs-boot/"
        r"|scripts/windows-)"
    ),
    "linux": re.compile(
        r"^((crates|linux)/(app|app-core|core|daemon|backend|backend-[a-z]+|sensors|replay|frame-socket|transport)/"
        r"|packaging/|install\.sh$|uninstall\.sh$|tests/(install|proxmox)/|tools/card-check/)"
    ),
}
NEUTRAL_FILES = {"Cargo.lock", "Cargo.toml", "CLA-SIGNERS.md"}
NAMES = {"mac": "the Mac host", "windows": "the Windows host", "linux": "the Linux host"}

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


def pulls(since: str | None, tag: str) -> list[tuple[int, str, str, list[str]]]:
    """(number, title, author's email, files touched) of the pull requests merged into main in
    since..tag, oldest first: squash merges and merge commits of pull requests, each once."""
    span = f"{since}..{tag}" if since else tag
    found: dict[int, tuple[int, str, str, list[str]]] = {}
    log = git(
        "log", "--first-parent", "--reverse", "--diff-merges=first-parent", "--name-only",
        "--format=%x00%ae%x09%s%x09%b%x00", span,
    )
    # Each commit is "\0email\tsubject\tbody\0" and then the files it touched, one a line.
    parts = log.split("\0")[1:]
    for head, files in zip(parts[0::2], parts[1::2]):
        email, _, rest = head.partition("\t")
        subject, _, body = rest.partition("\t")
        touched = [name for name in files.splitlines() if name.strip()]
        if match := MERGE.match(subject.strip()):
            title = next((line for line in body.splitlines() if line.strip()), match["branch"])
        elif match := PULL.match(subject.strip()):
            title = match["title"]
        else:
            continue
        number = int(match["number"])
        if number in found:
            found[number][3].extend(touched)
            continue
        found[number] = (number, " ".join(title.split()), email, touched)
    return list(found.values())


def hosts_of(title: str, files: list[str]) -> tuple[set[str], str]:
    """The hosts a pull request is news for, and how that was told: by its title or its files."""
    named = {host for host, words in HOSTS.items() if words.search(title)}
    if named and not EVERY_HOST.search(title):
        return named, "by its title"
    files = [name for name in files if name not in NEUTRAL_FILES]
    if named or not files:
        return set(HOSTS), ""
    hosts: set[str] = set()
    for name in files:
        mine = {host for host, where in HOST_FILES.items() if where.match(name)}
        hosts |= mine or set(HOSTS)
    return hosts, "by the files it touched"


def left_out(item: tuple[int, str, str, list[str]], kind: str) -> str | None:
    """Why a pull request is left out of `kind`'s notes, or None if it goes in."""
    _, title, email, files = item
    if "[bot]" in email:
        return "a bot's"
    if CLA.search(title):
        return "a contributor licence signature"
    if any(rule.search(title) for rule in LEFT_OUT):
        return "a release chore"
    hosts, how = hosts_of(title, files)
    if kind not in hosts:
        return f"{' and '.join(NAMES[host] for host in sorted(hosts))} only, {how}"
    return None


def area_of(title: str) -> tuple[str, str]:
    """The area a title starts with, "Windows hosts" and "Windows host" as one, and the rest of it."""
    match = AREA.match(title)
    if not match:
        return "Other", title
    area = match["area"].strip()
    return (area[:-1] if area.endswith("s") and len(area) > 3 else area), match["rest"]


def grouped(items: list[tuple[int, str, str, list[str]]]) -> dict[str, list[str]]:
    areas = [area_of(item[1])[0] for item in items]
    groups: dict[str, list[str]] = {}
    for (number, title, _, _), area in zip(items, areas):
        line = area_of(title)[1]
        if areas.count(area) < 2:
            area, line = "Other", title
        line = line[0].upper() + line[1:]
        groups.setdefault(area, []).append(f"- {line.rstrip('.')} (#{number})")
    first = sorted((a for a in groups if a not in LAST), key=lambda a: -len(groups[a]))
    return {area: groups[area] for area in first + [a for a in LAST if a in groups]}


def notes(tag: str, since: str | None, intro: str, items: list[tuple[int, str, str, list[str]]]) -> str:
    kind = kind_of(tag)
    kept = [item for item in items if not left_out(item, kind)]
    dropped = [(item, left_out(item, kind)) for item in items if left_out(item, kind)]
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
        out += [f"  {title} (#{number}): {why}" for (number, title, _, _), why in dropped]
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
