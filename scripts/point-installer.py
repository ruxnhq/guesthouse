#!/usr/bin/env python3
"""Points the installer at a release: step 7 of RELEASING.md for Linux, step 5 of the Windows host's, done from the release's own files.

    point-installer.py VERSION SHA256SUMS PAYLOAD_BYTES APPIMAGE_BYTES
    point-installer.py windows VERSION SETUP_SHA256 SETUP_BYTES

VERSION has no v, like 0.10.0. For Linux, SHA256SUMS is the file from the release, and the sizes are
the payload's and the AppImage's, in bytes, for the two sizes the site shows. It changes install.sh,
packaging/nix/package.nix, README.md and site/public/index.html.
For Windows, SETUP_SHA256 is the installer's .sha256 file and SETUP_BYTES the installer's size. It
changes only the Windows parts of site/public/index.html.
Run from the root of a checkout of main. It exits non-zero, without writing anything, if one of the
places it has to change isn't found exactly once.
"""

import base64
import re
import sys
from pathlib import Path

SEMVER = re.compile(r"\d+\.\d+\.\d+")


def hashes(sums: str, version: str) -> tuple[str, str]:
    """The payload's and the source's SHA-256 from the release's SHA256SUMS."""
    found = {}
    for line in sums.splitlines():
        parts = line.split()
        if len(parts) == 2 and re.fullmatch(r"[0-9a-f]{64}", parts[0]):
            found[parts[1].lstrip("*")] = parts[0]
    try:
        return found[f"guesthouse-{version}-x86_64.tar.xz"], found[f"guesthouse-{version}-source.tar.xz"]
    except KeyError as missing:
        sys.exit(f"SHA256SUMS has no line for {missing}.")


def sri(sha256_hex: str) -> str:
    """A SHA-256 as Nix writes it, which `nix hash convert --to sri` prints."""
    return "sha256-" + base64.b64encode(bytes.fromhex(sha256_hex)).decode()


def megabytes(size: int) -> str:
    """The size as the site writes it: binary megabytes, rounded."""
    return f"{round(size / 2**20)} MB"


def sub_once(text: str, pattern: str, repl: str, what: str) -> str:
    """One replacement, and a stop if the pattern isn't there exactly once."""
    text, count = re.subn(pattern, repl, text, flags=re.S | re.M)
    if count != 1:
        sys.exit(f"Expected {what} once, found it {count} times. Nothing was changed.")
    return text


def setup_megabytes(size: int) -> str:
    """The Windows installer's size as the site writes it: binary megabytes, one decimal."""
    return f"{size / 2**20:.1f} MB"


def point_windows(argv: list[str]) -> int:
    if len(argv) != 3 or not SEMVER.fullmatch(argv[0]):
        sys.exit(__doc__)
    version = argv[0]
    parts = Path(argv[1]).read_text().split()
    name = f"guesthouse-setup-{version}.exe"
    if len(parts) != 2 or not re.fullmatch(r"[0-9a-f]{64}", parts[0]) or parts[1].lstrip("*") != name:
        sys.exit(f"The .sha256 file isn't one line for {name}.")
    size = setup_megabytes(int(argv[2]))

    site = Path("site/public/index.html").read_text()
    site = sub_once(site, r'(<span class="eyebrow">Linux <b>\d+\.\d+\.\d+</b> · Windows <b>)\d+\.\d+\.\d+(</b>)', rf"\g<1>{version}\g<2>", "the Windows version in the site's top line")
    site = sub_once(site, r"(files\.ruxn\.com/p/guesthouse/guesthouse-setup-)\d+\.\d+\.\d+(\.exe\")", rf"\g<1>{version}\g<2>", "the installer's link")
    site = sub_once(site, r'(<span class="name">Guesthouse )\d+\.\d+\.\d+(</span><span class="meta">Installer · )\d+(?:\.\d+)? MB', rf"\g<1>{version}\g<2>{size}", "the installer's name and size")
    site = sub_once(site, r"(files\.ruxn\.com/p/guesthouse/guesthouse-setup-)\d+\.\d+\.\d+(\.exe\.sha256\")", rf"\g<1>{version}\g<2>", "the installer's hash link")
    Path("site/public/index.html").write_text(site)
    print(f"version={version}\nsha256={parts[0]}\nsize={size}")
    return 0


def main(argv: list[str]) -> int:
    if len(argv) > 1 and argv[1] == "windows":
        return point_windows(argv[2:])
    if len(argv) != 5 or not SEMVER.fullmatch(argv[1]):
        sys.exit(__doc__)
    version = argv[1]
    payload, source = hashes(Path(argv[2]).read_text(), version)
    payload_mb, appimage_mb = megabytes(int(argv[3])), megabytes(int(argv[4]))

    install = Path("install.sh").read_text()
    for name, value in (("RELEASE_VERSION", version), ("PAYLOAD_SHA256", payload), ("SOURCE_SHA256", source)):
        install = sub_once(install, rf'^{name}="[^"\n]*"$', f'{name}="{value}"', f"{name} in install.sh")

    nix = Path("packaging/nix/package.nix").read_text()
    nix = sub_once(nix, r'^(  version = ")[^"\n]*(";)$', rf"\g<1>{version}\g<2>", "version in package.nix")
    nix = sub_once(
        nix,
        r'(releases/download/v\$\{version\}/guesthouse-\$\{version\}-x86_64\.tar\.xz"[;\s\]]*hash = ")[^"\n]*(";)',
        rf"\g<1>{sri(payload)}\g<2>",
        "the payload's hash in package.nix",
    )

    readme = Path("README.md").read_text()
    readme = sub_once(readme, r"(\*\*Early days\.\*\* Linux )\d+\.\d+\.\d+( is out)", rf"\g<1>{version}\g<2>", "the status line in README.md")

    site = Path("site/public/index.html").read_text()
    site = sub_once(site, r'(<span class="eyebrow">Linux <b>)\d+\.\d+\.\d+(</b>)', rf"\g<1>{version}\g<2>", "the Linux version in the site's top line")
    # The installer's download, its name and size, the hash link, and the release's link, name and size.
    site = sub_once(site, r"(files\.ruxn\.com/p/guesthouse/guesthouse-installer-)\d+\.\d+\.\d+(\.AppImage\")", rf"\g<1>{version}\g<2>", "the AppImage link")
    site = sub_once(site, r'(<span class="name">Installer )\d+\.\d+\.\d+(</span><span class="meta">AppImage · )\d+ MB', rf"\g<1>{version}\g<2>{appimage_mb}", "the AppImage's name and size")
    site = sub_once(site, r"(files\.ruxn\.com/p/guesthouse/guesthouse-installer-)\d+\.\d+\.\d+(\.AppImage\.sha256\")", rf"\g<1>{version}\g<2>", "the AppImage's hash link")
    site = sub_once(site, r"(/releases/tag/v)\d+\.\d+\.\d+(\"><svg)", rf"\g<1>{version}\g<2>", "the release link")
    site = sub_once(site, r'(<span class="name">Release )\d+\.\d+\.\d+(</span><span class="meta">x86_64 · )\d+ MB', rf"\g<1>{version}\g<2>{payload_mb}", "the release's name and size")

    Path("install.sh").write_text(install)
    Path("packaging/nix/package.nix").write_text(nix)
    Path("README.md").write_text(readme)
    Path("site/public/index.html").write_text(site)
    print(f"version={version}\npayload={payload}\nsource={source}\nnix={sri(payload)}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))
