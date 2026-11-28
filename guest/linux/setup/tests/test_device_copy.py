"""The guest's modules and the device each have a copy of what they share. They have to stay the same."""
import os
import re
import unittest

LINUX = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
PATCH = os.path.join(os.path.dirname(os.path.dirname(LINUX)), "qemu", "qemu-gpupv.patch")


def file_in_patch(name):
    """The file as the whole series leaves it: the patch that adds it, then each later one that changes it."""
    with open(PATCH) as f:
        text = f.read()
    marker = f"+++ b/hw/misc/nvfwd/{name}\n"
    start = text.index(marker)
    body = text[text.index("@@", start):]
    lines = []
    for line in body.split("\n")[1:]:
        if not line.startswith("+"):
            break
        lines.append(line[1:])
    at = text.find(marker, start + 1)
    while at >= 0:
        lines = apply_hunks(lines, text[at + len(marker):].split("\ndiff --git")[0].split("\n-- \n")[0])
        at = text.find(marker, at + 1)
    return "\n".join(lines) + "\n"


def apply_hunks(lines, diff):
    """Applies one file's hunks, which have to match exactly where they say."""
    out, used = [], 0
    for hunk in re.finditer(r"^@@ -(\d+)(?:,\d+)? \+\d+(?:,\d+)? @@.*\n((?:[ +-].*\n?)*)", diff, flags=re.M):
        first = int(hunk.group(1)) - 1
        out += lines[used:first]
        used = first
        for line in hunk.group(2).split("\n"):
            if not line:
                continue
            if line[0] in " -":
                assert lines[used] == line[1:], (lines[used], line)
                used += 1
            if line[0] in " +":
                out.append(line[1:])
    return out + lines[used:]


class Copies(unittest.TestCase):
    def assert_same(self, name, compare=lambda text: text):
        with open(os.path.join(LINUX, "guest", name)) as f:
            self.assertEqual(compare(f.read()), compare(file_in_patch(name)))

    def test_wire_format(self):
        self.assert_same("nvfwd_proto.h", lambda text: re.sub(r"^/\* Wire format.*\n", "", text, flags=re.M))

    def test_small_tables(self):
        self.assert_same("fd_controls.h")
        self.assert_same("nv_tables_extra.h")


if __name__ == "__main__":
    unittest.main()
