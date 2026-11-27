/*
 * rmframes, the publishing side: what rmframes_pub.c and rmframes_recv.c
 * share. Not for users of the publisher; they include rmframes_pub.h.
 *
 * SPDX-License-Identifier: MIT
 * Copyright (c) 2026 RUXN LLC and contributors
 *
 * The publisher has two receive rights in one port set, and one dispatch
 * source on the set:
 *   service  checked in with bootstrap: any process of the user can send to
 *            it, so only readers' messages are taken there
 *   notify   the publisher's own: no send right to it is ever made, only the
 *            send-once rights the kernel holds for the dead-name requests on
 *            readers' ports, so only the kernel's notices come there
 */
#ifndef RMFRAMES_PUB_INTERNAL_H
#define RMFRAMES_PUB_INTERNAL_H

#include "rmframes_pub.h"
#include "rmframes.h"

#include <dispatch/dispatch.h>
#include <mach/mach.h>
#include <stdatomic.h>
#include <stdbool.h>
#include <sys/types.h>

/* The holders bit of the drawing thread, apart from the readers' bits. */
#define RMF_DRAWING (1u << 31)
#define RMF_SERVICE_ROOM 128
/* Dead-name notices on their way for readers already let go; see awaited. */
#define RMF_AWAITED_ROOM (2 * RMF_MAX_READERS)

typedef struct rmf_reader {
    mach_port_t port;   /* MACH_PORT_NULL: this place is free */
    pid_t pid;          /* the process that said hello, as the kernel says */
    uint32_t number;    /* given in SLOTS, sent back in RELEASE and BYE */
    bool armed;         /* a dead-name request on port, to notify, is set */
} rmf_reader;

struct rmf_pub {
    mach_port_t service;   /* receive right, checked in with bootstrap */
    mach_port_t notify;    /* receive right, the kernel's notices only */
    mach_port_t set;       /* both of them, for the one source */
    dispatch_queue_t queue;
    dispatch_source_t source;
    char name[RMF_SERVICE_ROOM];

    /* Only on `queue`. */
    rmf_reader readers[RMF_MAX_READERS];
    uint32_t last_number;
    /* Names of readers let go while the kernel's dead-name notice for them
     * was already queued: the notice holds a reference to the name, which is
     * let go when it comes. MACH_PORT_NULL: free. */
    mach_port_t awaited[RMF_AWAITED_ROOM];
    IOSurfaceRef surfaces[RMF_MAX_SLOTS];
    uint32_t count, width, height;
    uint32_t queued_generation;
    /* The newest frame published in this generation, for a reader that
     * comes later: its slot (-1 for none yet), and the slot's draw count
     * then, which says whether it has been drawn over since. */
    int newest_slot;
    uint32_t newest_drawn;
    uint64_t newest_sequence, newest_time;

    /* Any thread. */
    _Atomic uint32_t generation;
    _Atomic uint32_t slot_count;
    _Atomic uint32_t holders[RMF_MAX_SLOTS];
    _Atomic uint32_t drawn[RMF_MAX_SLOTS];   /* times handed out for drawing */
    _Atomic uint32_t next;      /* where acquire starts looking, so slots take turns */
    _Atomic uint64_t sequence;
    _Atomic uint32_t reader_count;
    _Atomic uint64_t sent, dropped;
    _Atomic uint64_t ignored;   /* messages taken and not acted on */
};

static inline uint32_t rmf_reader_bit(int index)
{
    return 1u << index;
}

/* In rmframes_pub.c, on queue. */
bool rmf_slot_is_current(rmf_pub *pub, uint32_t generation, int slot);
void rmf_arm_dead_name(rmf_pub *pub, int index);
void rmf_drop_reader(rmf_pub *pub, int index);
void rmf_send_slots(rmf_pub *pub, int index);
void rmf_send_newest(rmf_pub *pub, int index);

/* In rmframes_recv.c: the source's event handler, on queue. */
void rmf_pub_receive(void *opaque);

#endif
