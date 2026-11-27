/*
 * attacker: checks that a publisher acts only on what it should. A second
 * process of the same user sends the service port forged dead-name notices
 * and readers' messages that are wrong in size, kind or index, while a real
 * reader reads, and a reader that holds every slot is then killed.
 *
 * SPDX-License-Identifier: MIT
 * Copyright (c) 2026 RUXN LLC and contributors
 *
 * Usage: attacker [-s TESTSOURCE]
 *   TESTSOURCE is the publisher to run (testsource beside this program). No
 *   GPU device or guest is involved. Exits 0 when every check passes.
 *
 * Three processes besides the publisher: this one forges, and two children
 * forked before it does anything else read through rmframes_sub.c: one gives
 * every frame back at once, one keeps every frame. Their counts are in a
 * page they share with this one.
 *
 *   A  the reader alone, then a round of forgeries: the reader keeps getting
 *      frames, and the publisher keeps running
 *   B  the holder comes and holds every slot, so the reader gets none; a
 *      second round of forgeries must not change that (a forged notice that
 *      let the holder go would free its slots)
 *   C  the holder is killed: the kernel's real notice lets it go, its slots
 *      are free, and the reader gets frames again
 * Every right a forged message carried must be let go by the publisher, and
 * at the end testsource says it still has its reader, and how many messages
 * it ignored.
 */
#include <mach/mach.h>
#include <mach/notify.h>
#include <servers/bootstrap.h>
#include <signal.h>
#include <spawn.h>
#include <stdatomic.h>
#include <stdbool.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/wait.h>
#include <unistd.h>

#include "rmframes.h"
#include "rmframes_sub.h"

extern char **environ;

enum { READER, HOLDER };

typedef struct shared {
    _Atomic uint64_t frames[2];
    _Atomic int gone[2];      /* 1 the publisher went, 2 refused */
    _Atomic int go[2];        /* the child may say hello */
} shared;

static shared *sh;
static dispatch_queue_t child_queue;
static rmf_sub *child_sub;
static int child_role;
static mach_port_t service;
static unsigned forged, unsent, failures, forged_in_all;

/* The children. */

static void c_slots(void *ctx, uint32_t generation, uint32_t width,
                    uint32_t height, uint32_t count, IOSurfaceRef const *s)
{
}

static void c_frame(void *ctx, const rmf_frame_msg *frame)
{
    atomic_fetch_add(&sh->frames[child_role], 1);
    if (child_role == READER) {
        rmf_sub_release(child_sub, frame->generation, frame->slot);
    }
}

static void c_gone(void *ctx)
{
    atomic_store(&sh->gone[child_role], 1);
}

static void c_refused(void *ctx, const char *reason)
{
    atomic_store(&sh->gone[child_role], 2);
}

static void c_open(void *name)
{
    rmf_sub_callbacks cb = { .on_slots = c_slots, .on_frame = c_frame,
                             .on_gone = c_gone, .on_refused = c_refused };
    char err[256];

    /* On the reader's own queue, so no frame comes before child_sub is set. */
    while (!(child_sub = rmf_sub_open(name, child_queue, cb, err,
                                      sizeof(err)))) {
        usleep(50000);
    }
}

static void child(const char *name, int role)
{
    child_queue = dispatch_queue_create("attacker.reader", NULL);
    child_role = role;
    while (!atomic_load(&sh->go[role])) {
        usleep(10000);
    }
    dispatch_sync_f(child_queue, (void *)name, c_open);
    for (;;) {
        pause();
    }
}

static pid_t fork_child(const char *name, int role)
{
    pid_t pid;

    fflush(stdout);
    pid = fork();
    if (pid == 0) {
        child(name, role);
        _exit(0);
    }
    return pid;
}

/* Forging. */

static void send_raw(mach_msg_header_t *h, mach_msg_size_t size)
{
    kern_return_t kr;

    h->msgh_remote_port = service;
    h->msgh_size = size;
    kr = mach_msg(h, MACH_SEND_MSG | MACH_SEND_TIMEOUT, size, 0,
                  MACH_PORT_NULL, 1000, MACH_PORT_NULL);
    if (kr == MACH_MSG_SUCCESS) {
        forged++;
    } else {
        unsent++;
    }
}

static void send_plain(uint32_t id, const void *body, mach_msg_size_t body_size)
{
    union {
        mach_msg_header_t h;
        uint8_t room[4096];
    } m;

    memset(&m, 0, sizeof(m));
    m.h.msgh_bits = MACH_MSGH_BITS_SET(MACH_MSG_TYPE_COPY_SEND, 0, 0, 0);
    m.h.msgh_id = id;
    if (body_size > sizeof(m) - sizeof(m.h)) {
        body_size = sizeof(m) - sizeof(m.h);
    }
    if (body) {
        memcpy(m.room + sizeof(m.h), body, body_size);
    }
    send_raw(&m.h, sizeof(m.h) + body_size);
}

/* A dead-name notice as the kernel lays it out, naming `name`. */
static void send_dead_name(mach_port_name_t name)
{
    mach_dead_name_notification_t n;

    memset(&n, 0, sizeof(n));
    n.not_header.msgh_bits = MACH_MSGH_BITS_SET(MACH_MSG_TYPE_COPY_SEND, 0, 0, 0);
    n.not_header.msgh_id = MACH_NOTIFY_DEAD_NAME;
    n.NDR = NDR_record;
    n.not_port = name;
    send_raw(&n.not_header, offsetof(mach_dead_name_notification_t, trailer));
}

/* Ports whose rights go into forged messages: the publisher must let every
 * one go. Each is a receive right of ours with no send right here. */
#define TRACKED_ROOM 32
static mach_port_t tracked[TRACKED_ROOM];
static int tracked_count;

static mach_port_t tracked_port(void)
{
    mach_port_t p = MACH_PORT_NULL;

    mach_port_allocate(mach_task_self(), MACH_PORT_RIGHT_RECEIVE, &p);
    if (tracked_count < TRACKED_ROOM) {
        tracked[tracked_count++] = p;
    }
    return p;
}

/* A message with a reply right of the given kind and a body. */
static void send_with_reply(uint32_t id, mach_msg_type_name_t kind,
                            const void *body, mach_msg_size_t body_size)
{
    union {
        mach_msg_header_t h;
        uint8_t room[256];
    } m;

    memset(&m, 0, sizeof(m));
    m.h.msgh_bits = MACH_MSGH_BITS_SET(MACH_MSG_TYPE_COPY_SEND, kind, 0, 0);
    m.h.msgh_local_port = tracked_port();
    m.h.msgh_id = id;
    if (body) {
        memcpy(m.room + sizeof(m.h), body, body_size);
    }
    send_raw(&m.h, sizeof(m.h) + body_size);
}

/* A message carrying one port descriptor, a send right made from a tracked
 * port, and then `tail` as its body. */
typedef struct with_descriptor {
    mach_msg_header_t h;
    mach_msg_body_t body;
    mach_msg_port_descriptor_t port;
    uint8_t tail[64];
} with_descriptor;

static void send_with_descriptor(uint32_t id, const void *tail,
                                 mach_msg_size_t tail_size)
{
    with_descriptor m;

    memset(&m, 0, sizeof(m));
    m.h.msgh_bits = MACH_MSGH_BITS_SET(MACH_MSG_TYPE_COPY_SEND, 0, 0,
                                       MACH_MSGH_BITS_COMPLEX);
    m.h.msgh_id = id;
    m.body.msgh_descriptor_count = 1;
    m.port.type = MACH_MSG_PORT_DESCRIPTOR;
    m.port.name = tracked_port();
    m.port.disposition = MACH_MSG_TYPE_MAKE_SEND;
    if (tail_size > sizeof(m.tail)) {
        tail_size = sizeof(m.tail);
    }
    memcpy(m.tail, tail, tail_size);
    send_raw(&m.h, offsetof(with_descriptor, tail) + tail_size);
}

/* Forged dead-name notices for every name a small task is likely to have:
 * index 1 to 2047, every generation. The publisher's readers' ports, its
 * surfaces and its service port are among them. */
static void forge_dead_names(void)
{
    for (uint32_t index = 1; index < 0x800; index++) {
        for (uint32_t gen = 3; gen < 0x100; gen += 4) {
            send_dead_name((index << 8) | gen);
        }
    }
}

static void forge_readers_messages(void)
{
    struct { uint32_t generation, slot, reader; } release;
    uint32_t number;
    uint32_t slots[] = { 0, 1, 7, 8, 255, 0x7fffffff, 0xffffffff };

    /* Releases for any generation, slot and reader number: none is from a
     * reader of this process, so none may clear a hold. */
    for (uint32_t g = 0; g < 40; g++) {
        for (unsigned s = 0; s < sizeof(slots) / sizeof(slots[0]); s++) {
            release.generation = g;
            release.slot = slots[s];
            send_plain(RMF_MSG_RELEASE, &release, 8);   /* as an old reader */
            for (uint32_t r = 0; r < 12; r++) {
                release.reader = r;
                send_plain(RMF_MSG_RELEASE, &release, sizeof(release));
            }
        }
    }
    send_plain(RMF_MSG_RELEASE, NULL, 0);               /* too short */
    send_plain(RMF_MSG_RELEASE, &release, 4);           /* too short */
    /* Byes for any reader number, and as an old reader. */
    send_plain(RMF_MSG_BYE, NULL, 0);
    for (number = 0; number < 64; number++) {
        send_plain(RMF_MSG_BYE, &number, sizeof(number));
    }
}

static void forge_round(void)
{
    uint32_t hello[2] = { RMF_VERSION, (uint32_t)getpid() };
    uint32_t bad_hello[2] = { RMF_VERSION + 7, (uint32_t)getpid() };
    mach_port_name_t name = 0x103;
    static uint8_t ndr_name[4096];   /* the notice's body, then zeros */

    memcpy(ndr_name, &NDR_record, sizeof(NDR_record_t));
    memcpy(ndr_name + sizeof(NDR_record_t), &name, sizeof(name));

    forge_dead_names();
    /* Notices wrong in size, or carrying rights. */
    send_plain(MACH_NOTIFY_DEAD_NAME, NULL, 0);                 /* short */
    send_plain(MACH_NOTIFY_DEAD_NAME, ndr_name, 4);             /* short */
    send_plain(MACH_NOTIFY_DEAD_NAME, ndr_name, 200);           /* long */
    send_plain(MACH_NOTIFY_DEAD_NAME, ndr_name, 4000);          /* too big */
    send_with_descriptor(MACH_NOTIFY_DEAD_NAME, ndr_name, 12);
    send_with_reply(MACH_NOTIFY_DEAD_NAME, MACH_MSG_TYPE_MAKE_SEND, ndr_name, 12);
    /* The kernel's other notices. */
    send_plain(MACH_NOTIFY_PORT_DELETED, ndr_name, 12);
    send_plain(MACH_NOTIFY_PORT_DESTROYED, ndr_name, 12);
    send_plain(MACH_NOTIFY_NO_SENDERS, ndr_name, 12);
    send_plain(MACH_NOTIFY_SEND_ONCE, NULL, 0);
    /* Hellos that must not make a reader; their rights must be let go. */
    send_plain(RMF_MSG_HELLO, hello, sizeof(hello));            /* no port */
    send_with_reply(RMF_MSG_HELLO, MACH_MSG_TYPE_MAKE_SEND_ONCE, hello,
                    sizeof(hello));
    send_with_reply(RMF_MSG_HELLO, MACH_MSG_TYPE_MAKE_SEND, bad_hello,
                    sizeof(bad_hello));
    send_with_reply(RMF_MSG_HELLO, MACH_MSG_TYPE_MAKE_SEND, hello, 4);
    send_with_descriptor(RMF_MSG_HELLO, hello, sizeof(hello));
    {
        /* The service port as its own reader. */
        union { mach_msg_header_t h; uint8_t room[64]; } m;

        memset(&m, 0, sizeof(m));
        m.h.msgh_bits = MACH_MSGH_BITS_SET(MACH_MSG_TYPE_COPY_SEND,
                                           MACH_MSG_TYPE_COPY_SEND, 0, 0);
        m.h.msgh_local_port = service;
        m.h.msgh_id = RMF_MSG_HELLO;
        memcpy(m.room + sizeof(m.h), hello, sizeof(hello));
        send_raw(&m.h, sizeof(m.h) + sizeof(hello));
    }
    /* Readers' messages with rights they never carry. */
    send_with_descriptor(RMF_MSG_RELEASE, hello, 12);
    send_with_reply(RMF_MSG_BYE, MACH_MSG_TYPE_MAKE_SEND, NULL, 0);
    send_with_reply(0x12345678, MACH_MSG_TYPE_MAKE_SEND, NULL, 0);
    forge_readers_messages();
}

/* Checking. */

static void check(bool ok, const char *what, unsigned long long a,
                  unsigned long long b)
{
    printf("attacker: %s %s (%llu, %llu)\n", ok ? "PASS" : "FAIL", what, a, b);
    fflush(stdout);
    if (!ok) {
        failures++;
    }
}

static uint64_t frames(int role)
{
    return atomic_load(&sh->frames[role]);
}

static bool alive(pid_t pid)
{
    return waitpid(pid, NULL, WNOHANG) == 0;
}

/* Every tracked right let go by the publisher: no send or send-once right
 * to the port is left anywhere. A send-once right let go comes back as a
 * notice in the port's queue, and counts until it is taken from there. */
static unsigned rights_kept(void)
{
    unsigned kept = 0;

    for (int i = 0; i < tracked_count; i++) {
        mach_port_status_t st;
        mach_msg_type_number_t n = MACH_PORT_RECEIVE_STATUS_COUNT;
        union { mach_msg_header_t h; uint8_t room[256]; } m;

        while (mach_msg(&m.h, MACH_RCV_MSG | MACH_RCV_TIMEOUT, 0, sizeof(m),
                        tracked[i], 0, MACH_PORT_NULL) == MACH_MSG_SUCCESS) {
            mach_msg_destroy(&m.h);
        }

        mach_port_get_attributes(mach_task_self(), tracked[i],
                                 MACH_PORT_RECEIVE_STATUS,
                                 (mach_port_info_t)&st, &n);
        if (st.mps_srights || st.mps_sorights) {
            kept++;
        }
    }
    return kept;
}

static pid_t spawn_source(const char *path, const char *name, int out)
{
    posix_spawn_file_actions_t fa;
    char *argv[] = { (char *)path, "-n", (char *)name, "-t", "24", NULL };
    pid_t pid;

    posix_spawn_file_actions_init(&fa);
    posix_spawn_file_actions_adddup2(&fa, out, 1);
    if (posix_spawn(&pid, path, &fa, NULL, argv, environ) != 0) {
        perror(path);
        exit(2);
    }
    posix_spawn_file_actions_destroy(&fa);
    return pid;
}

int main(int argc, char **argv)
{
    char source[1024], name[64], said[4096] = "";
    int out[2], opt, status;
    uint64_t a, b;
    pid_t src, reader, holder;
    ssize_t len;

    snprintf(source, sizeof(source), "%s", argv[0]);
    strcpy(strrchr(source, '/') ? strrchr(source, '/') + 1 : source,
           "testsource");
    while ((opt = getopt(argc, argv, "s:")) != -1) {
        if (opt != 's') {
            fprintf(stderr, "usage: attacker [-s TESTSOURCE]\n");
            return 2;
        }
        snprintf(source, sizeof(source), "%s", optarg);
    }
    sh = mmap(NULL, sizeof(*sh), PROT_READ | PROT_WRITE,
              MAP_SHARED | MAP_ANON, -1, 0);
    snprintf(name, sizeof(name), "rmf.attack.%d", getpid());

    /* The children first, before this process uses dispatch or IPC. */
    reader = fork_child(name, READER);
    holder = fork_child(name, HOLDER);
    pipe(out);
    src = spawn_source(source, name, out[1]);
    close(out[1]);
    for (int i = 0; i < 100 && bootstrap_look_up(bootstrap_port, name,
                                                 &service) != KERN_SUCCESS; i++) {
        usleep(50000);
    }
    if (service == MACH_PORT_NULL) {
        fprintf(stderr, "attacker: no publisher %s\n", name);
        return 2;
    }

    /* A: the reader alone, then forgeries. */
    atomic_store(&sh->go[READER], 1);
    sleep(2);
    a = frames(READER);
    check(a >= 60, "the reader gets frames", a, 0);
    forge_round();
    printf("attacker: round 1: %u messages forged, %u not sent\n", forged, unsent);
    sleep(1);
    a = frames(READER);
    usleep(1500000);
    b = frames(READER);
    check(alive(src), "the publisher runs after the forgeries", 0, 0);
    check(b - a >= 60 && !atomic_load(&sh->gone[READER]),
          "the reader still gets frames after the forgeries", b - a,
          atomic_load(&sh->gone[READER]));

    /* B: the holder takes every slot; forgeries must not let it go. */
    atomic_store(&sh->go[HOLDER], 1);
    sleep(2);
    a = frames(READER);
    sleep(1);
    b = frames(READER);
    check(frames(HOLDER) >= RMF_MAX_SLOTS && b - a <= 2,
          "the holder holds every slot, the reader gets none",
          frames(HOLDER), b - a);
    forged_in_all = forged;
    forged = unsent = 0;
    forge_round();
    forged_in_all += forged;
    printf("attacker: round 2: %u messages forged, %u not sent\n", forged, unsent);
    a = frames(READER);
    usleep(1500000);
    b = frames(READER);
    check(alive(src) && b - a <= 2 && !atomic_load(&sh->gone[HOLDER]),
          "forgeries let neither the holder nor its slots go", b - a,
          atomic_load(&sh->gone[HOLDER]));

    /* C: the holder dies: the kernel's notice frees its slots. */
    kill(holder, SIGKILL);
    waitpid(holder, NULL, 0);
    usleep(500000);
    a = frames(READER);
    usleep(1500000);
    b = frames(READER);
    check(b - a >= 60 && !atomic_load(&sh->gone[READER]),
          "the holder killed, its slots are free and the reader gets frames",
          b - a, atomic_load(&sh->gone[READER]));
    {
        unsigned kept = rights_kept();

        check(kept == 0, "every right in a forged message was let go", kept,
              tracked_count);
    }

    /* The publisher's own account at its end. */
    waitpid(src, &status, 0);
    for (size_t got = 0; got < sizeof(said) - 1; got += (size_t)len) {
        len = read(out[0], said + got, sizeof(said) - 1 - got);
        if (len <= 0) {
            break;
        }
    }
    printf("%s", said);
    {
        const char *p = strstr(said, " readers, ");
        unsigned readers = 99;
        unsigned long long ignored = 0;

        if (p) {
            while (p > said && p[-1] >= '0' && p[-1] <= '9') {
                p--;
            }
            sscanf(p, "%u readers, %llu messages ignored", &readers, &ignored);
        }
        check(WIFEXITED(status) && WEXITSTATUS(status) == 0 && readers == 1,
              "testsource ended well with its one reader", readers, status);
        check(ignored >= forged_in_all, "testsource ignored every forgery",
              ignored, forged_in_all);
    }
    kill(reader, SIGKILL);
    waitpid(reader, NULL, 0);
    printf("attacker: %s\n", failures ? "FAILED" : "all passed");
    return failures ? 1 : 0;
}
