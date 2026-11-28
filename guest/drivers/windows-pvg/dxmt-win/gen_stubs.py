#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
#
# Writes stubs.c: every function of DXMT's winemetal.h that winemetal over
# ruxnpvg (winemetal_pvg.c and the wm_*.c beside it) does not define, as a function that logs its name the first time and
# returns zero (no object, false, or nothing), so that a run shows what DXMT
# calls beyond the slice.
#
# Usage: gen_stubs.py WINEMETAL_H OUT_C SOURCE...

import re
import sys


def declarations(header):
    text = open(header).read()
    for m in re.finditer(r'^WINEMETAL_API\s+(.*?);', text, re.S | re.M):
        decl = ' '.join(m.group(1).split())
        name = re.search(r'(\w+)\s*\(', decl).group(1)
        yield name, decl


def defined(sources):
    have = set()
    for source in sources:
        text = open(source).read()
        have |= set(re.findall(r'^[A-Za-z_][\w \*]*?\b(\w+)\s*\(', text, re.M))
    return have


def main():
    header, out = sys.argv[1:3]
    have = defined(sys.argv[3:])
    lines = [
        '/* Made by gen_stubs.py from winemetal.h: the functions winemetal_pvg.c */',
        '/* does not answer. Each logs its name once and fails. */',
        '#include <windows.h>',
        '#define WINEMETAL_API __declspec(dllexport)',
        '#include "winemetal.h"',
        '#include "winemetal_pvg.h"',
        '',
    ]
    count = 0
    for name, decl in declarations(header):
        if name in have:
            continue
        count += 1
        ret = decl[:decl.index(name)].strip()
        lines.append(decl)
        lines.append('{')
        lines.append('\tstatic LONG once;')
        lines.append('\tif (!InterlockedExchange(&once, 1))')
        lines.append('\t\tpvg_log("STUB: %s (not answered: fails)", "' + name + '");')
        if ret == 'void':
            pass
        elif ret.startswith('struct'):
            lines.append('\t%s r = {0};' % ret)
            lines.append('\treturn r;')
        elif ret.startswith('enum'):
            lines.append('\treturn (%s)0;' % ret)
        else:
            lines.append('\treturn 0;')
        lines.append('}')
        lines.append('')
    open(out, 'w').write('\n'.join(lines))
    print('gen_stubs: %d stubs' % count)


if __name__ == '__main__':
    main()
