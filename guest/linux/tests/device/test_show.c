// SPDX-License-Identifier: GPL-2.0-or-later
/* The shape of a buffer offered to the viewer. */
#include "show_geometry.h"

#include <assert.h>
#include <stdio.h>

int main(void)
{
    /* What fwdcast offers: 1920x1080, pitch 7680, whole pages. */
    assert(show_geometry_ok(1920, 1080, 7680, 7680ull * 1080));
    assert(show_geometry_ok(1, 1, 4, 4096));
    assert(!show_geometry_ok(0, 1080, 7680, 1 << 24) && !show_geometry_ok(1920, 0, 7680, 1 << 24));
    assert(!show_geometry_ok(1920, 1080, 7679, 1 << 24));      /* four bytes a pixel past the pitch */
    assert(!show_geometry_ok(1920, 1080, 7680, 7680ull * 1080 - 4096));
    assert(!show_geometry_ok(1920, 1080, 7680, 7680ull * 1080 + 1));
    /* A width whose four bytes a pixel pass 32 bits is still wider than the pitch. */
    assert(!show_geometry_ok(1u << 30, 1, 4096, 4096));
    assert(!show_geometry_ok(0x40000001u, 1, 4, 4096));
    /* No side past the largest surface. */
    assert(show_geometry_ok(SHOW_LARGEST_SIDE, 1, SHOW_LARGEST_SIDE * 4, SHOW_LARGEST_SIDE * 4));
    assert(!show_geometry_ok(SHOW_LARGEST_SIDE + 1, 1, 0xfffff000u, 0xfffff000ull));
    assert(!show_geometry_ok(1, SHOW_LARGEST_SIDE + 1, 4096, 4096ull * (SHOW_LARGEST_SIDE + 1)));
    puts("ok");
    return 0;
}
