/*
 * Input from a remoteman viewer into the guest, through QMP.
 *
 * SPDX-License-Identifier: MIT
 * Copyright (c) 2026 RUXN LLC and contributors
 *
 * QMP answers every command and sends events of its own; a thread reads and
 * drops all of it so the socket never fills. Commands are written whole and
 * never waited on, so a slow guest can't hold up the viewer's input thread.
 */
#include "qmp_input.h"

#include <errno.h>
#include <fcntl.h>
#include <libgen.h>
#include <math.h>
#include <pthread.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/un.h>
#include <unistd.h>

/* The usb-tablet's range on each axis (INPUT_EVENT_ABS_MAX in QEMU). */
#define ABS_MAX 32767
/* How many pixels of a viewer's scroll make one wheel step. A guess, to be
 * tuned against a real viewer. */
#define SCROLL_PIXELS_PER_STEP 40.0f

struct qmp_input {
    int fd;
    pthread_t reader;
    pthread_mutex_t lock;
    uint32_t width, height;
    float x, y;            /* where the pointer is, as fractions */
    float scroll_x, scroll_y;
    uint8_t keys_down[256 / 8];
    uint8_t buttons_down;  /* bit per button: 0 left, 1 right, 2 middle */
};

static const char *const button_names[3] = { "left", "right", "middle" };

static void *drain(void *opaque)
{
    qmp_input *q = opaque;
    char buf[4096];

    while (read(q->fd, buf, sizeof(buf)) > 0) {
    }
    return NULL;
}

static bool send_all(qmp_input *q, const char *text)
{
    size_t left = strlen(text);

    while (left > 0) {
        ssize_t n = write(q->fd, text, left);
        if (n < 0 && errno == EINTR) {
            continue;
        }
        if (n <= 0) {
            return false;
        }
        text += n;
        left -= (size_t)n;
    }
    return true;
}

/* One input-send-event with the given events (a JSON list's insides). */
static bool send_events(qmp_input *q, const char *events)
{
    char cmd[1024];

    snprintf(cmd, sizeof(cmd),
             "{\"execute\":\"input-send-event\",\"arguments\":{\"events\":[%s]}}\r\n",
             events);
    return send_all(q, cmd);
}

/* Reads one line (the greeting, or an answer) while nothing else reads. */
static bool read_line(int fd, char *out, size_t len)
{
    size_t n = 0;

    while (n + 1 < len) {
        char c;
        if (read(fd, &c, 1) != 1) {
            return false;
        }
        if (c == '\n') {
            break;
        }
        out[n++] = c;
    }
    out[n] = 0;
    return true;
}

/* A socket's path may be 103 bytes at most; a longer one is reached from its
 * folder, as other tools do. That changes the working folder for a moment, so
 * this is called before any other thread starts. */
static int connect_unix(const char *path)
{
    struct sockaddr_un addr = { .sun_family = AF_UNIX };
    char dir[1024], base[1024];
    int fd = socket(AF_UNIX, SOCK_STREAM, 0);
    int here = -1, r, saved;

    if (fd < 0) {
        return -1;
    }
    if (strlen(path) < sizeof(addr.sun_path)) {
        strcpy(addr.sun_path, path);
    } else {
        strlcpy(dir, path, sizeof(dir));
        strlcpy(base, path, sizeof(base));
        here = open(".", O_RDONLY | O_DIRECTORY);
        if (here < 0 || chdir(dirname(dir)) < 0 ||
            strlcpy(addr.sun_path, basename(base), sizeof(addr.sun_path)) >=
                sizeof(addr.sun_path)) {
            saved = errno;
            if (here >= 0) {
                fchdir(here);
                close(here);
            }
            close(fd);
            errno = saved;
            return -1;
        }
    }
    r = connect(fd, (struct sockaddr *)&addr, sizeof(addr));
    saved = errno;
    if (here >= 0) {
        fchdir(here);
        close(here);
    }
    if (r < 0) {
        close(fd);
        errno = saved;
        return -1;
    }
    return fd;
}

qmp_input *qmp_input_open(const char *path, char *err, size_t err_len)
{
    char line[4096];
    qmp_input *q;
    int fd = connect_unix(path);

    if (fd < 0) {
        snprintf(err, err_len, "QMP socket %s: %s", path, strerror(errno));
        return NULL;
    }
    q = calloc(1, sizeof(*q));
    q->fd = fd;
    q->x = q->y = 0.5f;
    pthread_mutex_init(&q->lock, NULL);
    if (!read_line(fd, line, sizeof(line)) || !strstr(line, "\"QMP\"") ||
        !send_all(q, "{\"execute\":\"qmp_capabilities\"}\r\n") ||
        !read_line(fd, line, sizeof(line)) || !strstr(line, "\"return\"")) {
        snprintf(err, err_len, "QMP socket %s: no greeting or capabilities refused", path);
        close(fd);
        free(q);
        return NULL;
    }
    pthread_create(&q->reader, NULL, drain, q);
    return q;
}

void qmp_input_set_size(qmp_input *q, uint32_t width, uint32_t height)
{
    pthread_mutex_lock(&q->lock);
    q->width = width;
    q->height = height;
    pthread_mutex_unlock(&q->lock);
}

static float clamp01(float v)
{
    return v < 0 ? 0 : v > 1 ? 1 : v;
}

static bool move_to(qmp_input *q, float x, float y)
{
    char ev[256];

    q->x = clamp01(x);
    q->y = clamp01(y);
    snprintf(ev, sizeof(ev),
             "{\"type\":\"abs\",\"data\":{\"axis\":\"x\",\"value\":%d}},"
             "{\"type\":\"abs\",\"data\":{\"axis\":\"y\",\"value\":%d}}",
             (int)lrintf(q->x * ABS_MAX), (int)lrintf(q->y * ABS_MAX));
    return send_events(q, ev);
}

static bool button(qmp_input *q, uint8_t which, bool down)
{
    char ev[128];

    if (which > 2) {
        return false;
    }
    if (down) {
        q->buttons_down |= 1u << which;
    } else {
        q->buttons_down &= ~(1u << which);
    }
    snprintf(ev, sizeof(ev), "{\"type\":\"btn\",\"data\":{\"down\":%s,\"button\":\"%s\"}}",
             down ? "true" : "false", button_names[which]);
    return send_events(q, ev);
}

/* Whole wheel steps out of the pixels gathered so far. The tablet counts a
 * step on each press of a wheel button, so each is a press and a release. */
static bool scroll(qmp_input *q, float dx, float dy)
{
    char ev[1024] = "";
    size_t used = 0;

    q->scroll_x += dx;
    q->scroll_y += dy;
    while (fabsf(q->scroll_y) >= SCROLL_PIXELS_PER_STEP ||
           fabsf(q->scroll_x) >= SCROLL_PIXELS_PER_STEP) {
        const char *name;
        if (fabsf(q->scroll_y) >= SCROLL_PIXELS_PER_STEP) {
            /* Positive dy is towards the end of the document: the wheel turned down. */
            name = q->scroll_y > 0 ? "wheel-down" : "wheel-up";
            q->scroll_y -= copysignf(SCROLL_PIXELS_PER_STEP, q->scroll_y);
        } else {
            name = q->scroll_x > 0 ? "wheel-right" : "wheel-left";
            q->scroll_x -= copysignf(SCROLL_PIXELS_PER_STEP, q->scroll_x);
        }
        if (used + 160 > sizeof(ev)) {
            break;
        }
        used += (size_t)snprintf(ev + used, sizeof(ev) - used,
                                 "%s{\"type\":\"btn\",\"data\":{\"down\":true,\"button\":\"%s\"}},"
                                 "{\"type\":\"btn\",\"data\":{\"down\":false,\"button\":\"%s\"}}",
                                 used ? "," : "", name, name);
    }
    return used == 0 || send_events(q, ev);
}

static bool key(qmp_input *q, uint16_t usage, bool down)
{
    const char *name = hid_usage_to_qcode(usage);
    char ev[160];

    if (!name) {
        return false;
    }
    if (down) {
        q->keys_down[usage / 8] |= 1u << (usage % 8);
    } else {
        q->keys_down[usage / 8] &= ~(1u << (usage % 8));
    }
    snprintf(ev, sizeof(ev),
             "{\"type\":\"key\",\"data\":{\"down\":%s,\"key\":{\"type\":\"qcode\",\"data\":\"%s\"}}}",
             down ? "true" : "false", name);
    return send_events(q, ev);
}

bool qmp_input_event(qmp_input *q, uint8_t kind, uint8_t which, uint8_t down,
                     uint8_t phase, uint16_t usage, float x, float y)
{
    bool ok = false;

    pthread_mutex_lock(&q->lock);
    switch (kind) {
    case 1:  /* pointer to */
        ok = move_to(q, x, y);
        break;
    case 2:  /* pointer by, in pixels */
        if (q->width && q->height) {
            ok = move_to(q, q->x + x / q->width, q->y + y / q->height);
        }
        break;
    case 3:
        ok = button(q, which, down != 0);
        break;
    case 4:
        ok = scroll(q, x, y);
        break;
    case 5:
        ok = key(q, usage, down != 0);
        break;
    case 6:  /* touch: one finger works the left button */
        ok = move_to(q, x, y);
        if (phase == 0 || phase == 2) {
            ok = button(q, 0, phase == 0) && ok;
        }
        break;
    default:
        break;
    }
    pthread_mutex_unlock(&q->lock);
    return ok;
}

void qmp_input_release_all(qmp_input *q)
{
    pthread_mutex_lock(&q->lock);
    for (uint16_t usage = 0; usage < 256; usage++) {
        if (q->keys_down[usage / 8] & (1u << (usage % 8))) {
            key(q, usage, false);
        }
    }
    for (uint8_t b = 0; b < 3; b++) {
        if (q->buttons_down & (1u << b)) {
            button(q, b, false);
        }
    }
    q->scroll_x = q->scroll_y = 0;
    pthread_mutex_unlock(&q->lock);
}

void qmp_input_close(qmp_input *q)
{
    shutdown(q->fd, SHUT_RDWR);
    pthread_join(q->reader, NULL);
    close(q->fd);
    pthread_mutex_destroy(&q->lock);
    free(q);
}
