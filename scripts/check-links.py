#!/usr/bin/env python3
"""Checks the links in the docs.

A link to a file in the repo has to find the file, and a #anchor has to find a heading (or an id
in an HTML page) in it. That is an error, and the exit status is 1. With --external it checks the
links to other sites instead, says GitHub warnings for the ones that fail, and always exits 0.
Run from the repo's root. Usage: check-links.py [--external] PATH [PATH ...]
"""
import concurrent.futures
import html
import os
import re
import sys
import urllib.error
import urllib.parse
import urllib.request

ROOT = os.getcwd()
MD_LINK = re.compile(r"!?\[(?:[^\]\\]|\\.)*\]\(\s*(<[^>]*>|[^)\s]*)[^)]*\)")
MD_AUTOLINK = re.compile(r"<(https?://[^>\s]+)>")
MD_REFDEF = re.compile(r"^ {0,3}\[[^\]]+\]:\s*(\S+)", re.M)
HTML_ATTR = re.compile(r"""\b(?:href|src)\s*=\s*(?:"([^"]*)"|'([^']*)')""", re.I)
HTML_ID = re.compile(r"""\b(?:id|name)\s*=\s*(?:"([^"]*)"|'([^']*)')""", re.I)
HEADING = re.compile(r"^ {0,3}#{1,6}[ \t]+(.+?)[ \t#]*$", re.M)
FENCE = re.compile(r"^ {0,3}(```|~~~).*?^ {0,3}\1[ \t]*$", re.M | re.S)
INLINE_CODE = re.compile(r"(`+).+?\1", re.S)


def files_under(paths):
    for path in paths:
        if os.path.isdir(path):
            for base, dirs, names in os.walk(path):
                dirs[:] = sorted(d for d in dirs if d != ".git")
                for name in sorted(names):
                    if name.endswith((".md", ".html")):
                        yield os.path.join(base, name)
        elif path.endswith((".md", ".html")):
            yield path


def strip_code(text):
    return INLINE_CODE.sub("", FENCE.sub("", text))


def slug(heading):
    """GitHub's anchor for a heading."""
    heading = re.sub(r"!?\[([^\]]*)\]\([^)]*\)", r"\1", heading)
    heading = re.sub(r"<[^>]+>", "", heading)
    heading = html.unescape(heading).replace("`", "").replace("*", "").lower()
    heading = re.sub(r"[^\w\- ]", "", heading)
    return heading.replace(" ", "-")


_anchors = {}


def anchors(path):
    if path not in _anchors:
        text = open(path, encoding="utf-8", errors="replace").read()
        found = set()
        if path.endswith(".md"):
            seen = {}
            for match in HEADING.finditer(FENCE.sub("", text)):
                base = slug(match.group(1))
                count = seen.get(base, 0)
                seen[base] = count + 1
                found.add(base if count == 0 else f"{base}-{count}")
        for match in HTML_ID.finditer(text):
            found.add(match.group(1) or match.group(2))
        _anchors[path] = found
    return _anchors[path]


def links_of(path):
    text = open(path, encoding="utf-8", errors="replace").read()
    if path.endswith(".md"):
        text = strip_code(text)
        found = [m.group(1).strip("<>") for m in MD_LINK.finditer(text)]
        found += MD_AUTOLINK.findall(text) + MD_REFDEF.findall(text)
        found += [a or b for a, b in HTML_ATTR.findall(text)]
    else:
        found = [a or b for a, b in HTML_ATTR.findall(text)]
    for target in found:
        if target:
            yield html.unescape(target)


def is_external(target):
    return re.match(r"https?://", target, re.I) is not None


def internal_problem(path, target):
    """Why an internal link is broken, or None."""
    if re.match(r"[a-z][a-z0-9+.-]*:", target, re.I) or target.startswith("//"):
        return None  # mailto:, tel:, and the like
    target = target.split("?", 1)[0]
    file_part, _, anchor = target.partition("#")
    anchor = urllib.parse.unquote(anchor)
    if file_part == "":
        dest = path
    else:
        file_part = urllib.parse.unquote(file_part)
        if file_part.startswith("/"):
            # Rooted at the repo on GitHub, and at the site's folder in a page of the site.
            base = os.path.join(ROOT, "site", "public") if path.startswith("site" + os.sep) else ROOT
            dest = os.path.join(base, file_part.lstrip("/"))
        else:
            dest = os.path.join(os.path.dirname(path), file_part)
        dest = os.path.normpath(dest)
        if not os.path.exists(dest):
            return "no such file"
        if os.path.isdir(dest):
            index = os.path.join(dest, "index.html")
            if anchor and os.path.exists(index):
                dest = index
            else:
                return None if not anchor else "anchor in a folder"
    if anchor and dest.endswith((".md", ".html")):
        if anchor not in anchors(dest) and anchor.lower() not in {a.lower() for a in anchors(dest)}:
            return f"no heading or id '{anchor}' in {os.path.relpath(dest, ROOT)}"
    return None


def external_problem(url):
    url = url.split("#", 1)[0]
    headers = {"User-Agent": "guesthouse-docs-link-check"}
    for method in ("HEAD", "GET"):
        request = urllib.request.Request(url, method=method, headers=headers)
        try:
            with urllib.request.urlopen(request, timeout=20) as reply:
                return None if reply.status < 400 else f"status {reply.status}"
        except urllib.error.HTTPError as error:
            if error.code in (401, 403, 429) or (method == "GET" and error.code < 500):
                return None if error.code in (401, 403, 429) else f"status {error.code}"
            problem = f"status {error.code}"
        except Exception as error:  # a site that is down, slow, or with a bad certificate
            problem = type(error).__name__
    return problem


def main():
    args = sys.argv[1:]
    external = "--external" in args
    paths = [a for a in args if a != "--external"]
    bad = 0
    pages = {}
    for path in files_under(paths):
        path = os.path.relpath(path)
        for target in links_of(path):
            if is_external(target):
                pages.setdefault(target, []).append(path)
            elif not external:
                problem = internal_problem(path, target)
                if problem:
                    bad += 1
                    print(f"::error file={path}::{target} ({problem})" if os.environ.get("GITHUB_ACTIONS")
                          else f"{path}: {target} ({problem})")
    if external:
        with concurrent.futures.ThreadPoolExecutor(8) as pool:
            urls = sorted(pages)
            for url, problem in zip(urls, pool.map(external_problem, urls)):
                if problem:
                    where = pages[url][0]
                    print(f"::warning file={where}::{url} ({problem})")
        print(f"checked {len(pages)} external links")
        return 0
    print(f"{bad} broken internal links")
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
