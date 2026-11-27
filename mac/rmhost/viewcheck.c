/*
 * viewcheck: a remoteman viewer with no screen, for checking rmhost serve on
 * this Mac. It connects through remoteman's core (the client half of the same
 * library), saves the stream, counts frames and sends a little input.
 *
 * SPDX-License-Identifier: MIT
 * Copyright (c) 2026 RUXN LLC and contributors
 *
 * Usage: viewcheck [-o OUT.hevc] [-H] HOST:PORT STATE_DIR SECONDS
 *   Pairs with whatever host answers. OUT.hevc is the bare stream (Annex-B),
 *   which ffprobe and ffplay take. Halfway through it moves the pointer to the
 *   middle, clicks, scrolls down three steps and presses and releases "a".
 *   With -H it then presses left Shift and leaves with it still down, to see
 *   that the host lets go of it.
 */
#include <getopt.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

#include "remoteman_core.h"

#define HID_A 0x04
#define HID_LEFT_SHIFT 0xe1

typedef struct seen {
    FILE *out;
    unsigned frames, keyframes;
    uint32_t width, height;
    int64_t age_sum_us;
    unsigned ages;
} seen;

static void on_stream(void *ctx, uint16_t epoch, uint8_t codec, uint16_t chroma, uint8_t depth,
                      uint32_t width, uint32_t height, uint32_t refresh, uint8_t hdr)
{
    seen *s = ctx;
    s->width = width;
    s->height = height;
    printf("viewcheck: stream %u: %s %u-bit %u, %ux%u at %u Hz\n", epoch,
           codec == 0 ? "HEVC" : "AV1", depth, chroma, width, height, refresh);
    fflush(stdout);
}

static void on_frame(void *ctx, const uint8_t *data, size_t len, uint8_t key, uint32_t number,
                     uint64_t capture_us, int64_t age_us)
{
    seen *s = ctx;
    s->frames++;
    s->keyframes += key & 1;
    if (age_us >= 0) {
        s->age_sum_us += age_us;
        s->ages++;
    }
    if (s->out) {
        fwrite(data, 1, len, s->out);
    }
}

static void on_closed(void *ctx, const char *reason)
{
    printf("viewcheck: closed: %s\n", reason ? reason : "");
    fflush(stdout);
}

int main(int argc, char **argv)
{
    seen s = { 0 };
    rm_callbacks cb = { .ctx = &s, .on_stream = on_stream, .on_frame = on_frame,
                        .on_closed = on_closed };
    char err[512];
    rm_client *c;
    double seconds;
    const char *out = NULL;
    int hold_shift = 0, opt;

    while ((opt = getopt(argc, argv, "o:H")) != -1) {
        switch (opt) {
        case 'o': out = optarg; break;
        case 'H': hold_shift = 1; break;
        default: goto usage;
        }
    }
    if (argc - optind != 3) {
        goto usage;
    }
    argv += optind;
    seconds = atof(argv[2]);
    if (out && !(s.out = fopen(out, "wb"))) {
        perror(out);
        return 1;
    }
    c = rm_client_connect(argv[0], argv[1], "viewcheck", 1, 1920, 1080, 1.0f, 60, "hevc", "",
                          cb, err, sizeof(err));
    if (!c) {
        fprintf(stderr, "viewcheck: %s\n", err);
        return 1;
    }
    usleep((useconds_t)(seconds * 5e5));
    rm_client_pointer_to(c, 0.5f, 0.5f);
    rm_client_button(c, 0, 1);
    rm_client_button(c, 0, 0);
    rm_client_scroll(c, 0, 120);
    rm_client_key(c, HID_A, 1);
    rm_client_key(c, HID_A, 0);
    if (hold_shift) {
        rm_client_key(c, HID_LEFT_SHIFT, 1);
    }
    usleep((useconds_t)(seconds * 5e5));

    uint32_t lost = 0, asked = 0, rtt = 0;
    rm_client_stats(c, &lost, &asked, &rtt);
    rm_client_close(c);
    if (s.out) {
        fclose(s.out);
    }
    printf("viewcheck: %u frames (%u keyframes) in %.1f s, %u lost, %u keyframes asked for, "
           "mean age %.1f ms, round trip %.1f ms\n",
           s.frames, s.keyframes, seconds, lost, asked,
           s.ages ? s.age_sum_us / 1000.0 / s.ages : -1.0, rtt / 1000.0);
    return 0;

usage:
    fprintf(stderr, "usage: viewcheck [-o OUT.hevc] [-H] HOST:PORT STATE_DIR SECONDS\n");
    return 2;
}
