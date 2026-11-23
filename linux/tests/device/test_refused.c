// SPDX-License-Identifier: GPL-2.0-or-later
/* The list of what the device turned down, which a guest reads. It can't grow past its size. */
#include "refused.h"

#include <assert.h>
#include <stdio.h>
#include <string.h>

static RefusedList list;
static char text[NVFWD_REFUSED_MAX * NVFWD_REFUSED_LINE + 64];

static void test_lines_come_out_as_written(void)
{
    memset(&list, 0, sizeof(list));
    assert(refused_text(&list, text, sizeof(text)) == 0);
    refused_add(&list, "REFUSED", "ctrl", 0x20802209);
    refused_add(&list, "OVER", "limit", 0x2b);
    assert(refused_text(&list, text, sizeof(text)) == strlen("REFUSED ctrl 0x20802209\nOVER limit 0x2b\n"));
    assert(!strncmp(text, "REFUSED ctrl 0x20802209\nOVER limit 0x2b\n", strlen(text)));
}

static void test_it_stops_at_its_size(void)
{
    memset(&list, 0, sizeof(list));
    for (unsigned i = 0; i < NVFWD_REFUSED_MAX + 10; i++) {
        refused_add(&list, "REFUSED", "ctrl", i);
    }
    assert(list.count == NVFWD_REFUSED_MAX);
    refused_text(&list, text, sizeof(text));
    /* Room for one less than the size, and the last line says the list is full. */
    assert(strstr(text, "REFUSED ctrl 0x3e\n") && !strstr(text, "0x3f\n"));
    assert(!strcmp(list.line[NVFWD_REFUSED_MAX - 1], NVFWD_REFUSED_FULL));
    assert(strstr(text, NVFWD_REFUSED_FULL "\n") == text + strlen(text) - strlen(NVFWD_REFUSED_FULL "\n"));
}

static void test_a_long_line_is_cut_to_its_size(void)
{
    char what[200];

    memset(&list, 0, sizeof(list));
    memset(what, 'x', sizeof(what) - 1);
    what[sizeof(what) - 1] = 0;
    refused_add(&list, "REFUSED", what, 1);
    assert(strlen(list.line[0]) == NVFWD_REFUSED_LINE - 1);
    assert(refused_text(&list, text, sizeof(text)) == NVFWD_REFUSED_LINE);
}

static void test_only_whole_lines_fit_the_room(void)
{
    memset(&list, 0, sizeof(list));
    for (unsigned i = 0; i < NVFWD_REFUSED_MAX + 1; i++) {
        refused_add(&list, "REFUSED", "ctrl", 0x20800000u + i);
    }
    for (uint32_t room = 0; room < 200; room++) {
        uint32_t used;

        memset(text, '#', sizeof(text));
        used = refused_text(&list, text, room);
        assert(used <= room && (used == 0 || text[used - 1] == '\n'));
        for (uint32_t i = room; i < 220; i++) {
            assert(text[i] == '#'); /* nothing past the room it was given */
        }
    }
    assert(refused_text(&list, text, sizeof(text)) <= NVFWD_REFUSED_MAX * NVFWD_REFUSED_LINE);
}

int main(void)
{
    test_lines_come_out_as_written();
    test_it_stops_at_its_size();
    test_a_long_line_is_cut_to_its_size();
    test_only_whole_lines_fit_the_room();
    puts("ok");
    return 0;
}
