/*
 * rmframes, the publishing side. See rmframes.h for the protocol.
 *
 * SPDX-License-Identifier: MIT
 * Copyright (c) 2026 RUXN LLC and contributors
 *
 * Permission is hereby granted, free of charge, to any person obtaining a copy
 * of this software and associated documentation files (the "Software"), to
 * deal in the Software without restriction, including without limitation the
 * rights to use, copy, modify, merge, publish, distribute, sublicense, and/or
 * sell copies of the Software, and to permit persons to whom the Software is
 * furnished to do so, subject to the following conditions: the above
 * copyright notice and this permission notice shall be included in all copies
 * or substantial portions of the Software. THE SOFTWARE IS PROVIDED "AS IS",
 * WITHOUT WARRANTY OF ANY KIND.
 *
 * Threads: the drawing thread calls acquire and cancel, publish may come from
 * any thread; everything that touches a reader's port or the surfaces runs on
 * `queue`, one job at a time. What both sides need is kept in atomics: the
 * generation, and for each slot who holds it.
 *
 * A slot's holders are a bit per reader (its place in `readers`), and
 * DRAWING while the drawing thread has it. A slot is free when no bit is set.
 * The queue sets a reader's bit when a frame in the slot reaches the reader,
 * and clears it on that reader's RELEASE, or for every slot at once when the
 * reader goes or dies.
 *
 * What readers send, and the kernel's notices, are taken in rmframes_recv.c;
 * the ports are in rmframes_pub_internal.h.
 */
#include "rmframes_pub_internal.h"

#include <mach/notify.h>
#include <servers/bootstrap.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

/* SLOTS carries the surfaces and has to arrive; FRAME never waits. */
#define SLOTS_SEND_TIMEOUT_MS 50

typedef struct publish_job {
    rmf_pub *pub;
    uint32_t generation;
    int slot;
    uint64_t sequence;
    uint64_t host_time;
} publish_job;

typedef struct slots_job {
    rmf_pub *pub;
    uint32_t generation, count, width, height;
    IOSurfaceRef surfaces[RMF_MAX_SLOTS];
} slots_job;

bool rmf_slot_is_current(rmf_pub *pub, uint32_t generation, int slot)
{
    return slot >= 0 && slot < RMF_MAX_SLOTS &&
           generation == atomic_load(&pub->generation);
}

/* On queue. Asks the kernel to say on our own notify port, never the service
 * port, when the reader's port dies. A port already dead is not armed: the
 * first send to it fails and lets the reader go. */
void rmf_arm_dead_name(rmf_pub *pub, int index)
{
    rmf_reader *r = &pub->readers[index];
    mach_port_t previous = MACH_PORT_NULL;
    kern_return_t kr;

    kr = mach_port_request_notification(mach_task_self(), r->port,
                                        MACH_NOTIFY_DEAD_NAME, 0, pub->notify,
                                        MACH_MSG_TYPE_MAKE_SEND_ONCE, &previous);
    r->armed = kr == KERN_SUCCESS;
    if (previous != MACH_PORT_NULL) {
        mach_port_deallocate(mach_task_self(), previous);
    }
}

/* On queue. Takes back the reader's dead-name request. When there is none to
 * take back, though it was set, the kernel has already sent its notice: the
 * name is kept in `awaited` until the notice comes. */
static void disarm_dead_name(rmf_pub *pub, rmf_reader *r)
{
    mach_port_t previous = MACH_PORT_NULL;

    if (!r->armed) {
        return;
    }
    r->armed = false;
    mach_port_request_notification(mach_task_self(), r->port,
                                   MACH_NOTIFY_DEAD_NAME, 0, MACH_PORT_NULL,
                                   MACH_MSG_TYPE_MAKE_SEND_ONCE, &previous);
    if (previous != MACH_PORT_NULL) {
        mach_port_deallocate(mach_task_self(), previous);
        return;
    }
    for (int i = 0; i < RMF_AWAITED_ROOM; i++) {
        if (pub->awaited[i] == MACH_PORT_NULL) {
            pub->awaited[i] = r->port;
            return;
        }
    }
    /* No room: the notice is ignored when it comes, and its reference to a
     * dead name stays, which costs a name and nothing else. */
}

/* On queue. The reader is gone: everything it held is free. */
void rmf_drop_reader(rmf_pub *pub, int index)
{
    rmf_reader *r = &pub->readers[index];

    if (r->port == MACH_PORT_NULL) {
        return;
    }
    disarm_dead_name(pub, r);
    mach_port_deallocate(mach_task_self(), r->port);
    memset(r, 0, sizeof(*r));
    for (int slot = 0; slot < RMF_MAX_SLOTS; slot++) {
        atomic_fetch_and(&pub->holders[slot], ~rmf_reader_bit(index));
    }
    atomic_fetch_sub(&pub->reader_count, 1);
}

/* On queue. */
void rmf_send_slots(rmf_pub *pub, int index)
{
    rmf_reader *r = &pub->readers[index];
    rmf_slots_msg msg;
    kern_return_t kr;

    if (r->port == MACH_PORT_NULL || pub->count == 0) {
        return;
    }
    memset(&msg, 0, sizeof(msg));
    msg.header.msgh_bits = MACH_MSGH_BITS_SET(MACH_MSG_TYPE_COPY_SEND, 0, 0,
                                              MACH_MSGH_BITS_COMPLEX);
    msg.header.msgh_size = sizeof(msg);
    msg.header.msgh_remote_port = r->port;
    msg.header.msgh_id = RMF_MSG_SLOTS;
    msg.body.msgh_descriptor_count = RMF_MAX_SLOTS;
    for (uint32_t i = 0; i < RMF_MAX_SLOTS; i++) {
        msg.surface[i].type = MACH_MSG_PORT_DESCRIPTOR;
        if (i < pub->count) {
            /* A send right of our own, moved into the message. */
            msg.surface[i].name = IOSurfaceCreateMachPort(pub->surfaces[i]);
            msg.surface[i].disposition = MACH_MSG_TYPE_MOVE_SEND;
        } else {
            msg.surface[i].name = MACH_PORT_NULL;
            msg.surface[i].disposition = MACH_MSG_TYPE_COPY_SEND;
        }
    }
    msg.version = RMF_VERSION;
    msg.generation = pub->queued_generation;
    msg.count = pub->count;
    msg.width = pub->width;
    msg.height = pub->height;
    msg.format = RMF_FORMAT_BGRA;
    msg.reader = r->number;

    kr = mach_msg(&msg.header, MACH_SEND_MSG | MACH_SEND_TIMEOUT, sizeof(msg),
                  0, MACH_PORT_NULL, SLOTS_SEND_TIMEOUT_MS, MACH_PORT_NULL);
    if (kr == MACH_SEND_TIMED_OUT || kr == MACH_SEND_INTERRUPTED) {
        /* The kernel gave the rights back with the message. */
        mach_msg_destroy(&msg.header);
    } else if (kr != MACH_MSG_SUCCESS) {
        /* Refused before the rights were taken: they are still ours. */
        for (uint32_t i = 0; i < pub->count; i++) {
            mach_port_deallocate(mach_task_self(), msg.surface[i].name);
        }
    }
    if (kr != MACH_MSG_SUCCESS) {
        rmf_drop_reader(pub, index);
    }
}

/* On queue. Sends one reader a frame in a slot the caller has marked as the
 * reader's. On failure the mark is taken off again; a reader that has died
 * is let go. */
static void send_frame(rmf_pub *pub, int index, uint32_t generation, int slot,
                       uint64_t sequence, uint64_t host_time)
{
    rmf_reader *r = &pub->readers[index];
    rmf_frame_msg msg;
    kern_return_t kr;

    memset(&msg, 0, sizeof(msg));
    msg.header.msgh_bits = MACH_MSGH_BITS_SET(MACH_MSG_TYPE_COPY_SEND, 0, 0, 0);
    msg.header.msgh_size = sizeof(msg);
    msg.header.msgh_remote_port = r->port;
    msg.header.msgh_id = RMF_MSG_FRAME;
    msg.generation = generation;
    msg.slot = slot;
    msg.width = pub->width;
    msg.height = pub->height;
    msg.sequence = sequence;
    msg.host_time = host_time;

    kr = mach_msg(&msg.header, MACH_SEND_MSG | MACH_SEND_TIMEOUT, sizeof(msg),
                  0, MACH_PORT_NULL, 0, MACH_PORT_NULL);
    if (kr == MACH_MSG_SUCCESS) {
        atomic_fetch_add(&pub->sent, 1);
        return;
    }
    atomic_fetch_add(&pub->dropped, 1);
    atomic_fetch_and(&pub->holders[slot], ~rmf_reader_bit(index));
    if (kr == MACH_SEND_INVALID_DEST) {
        rmf_drop_reader(pub, index);
    }
}

/* On queue. A reader that has just come gets the newest frame, if its slot
 * still holds it: the slot is marked as the reader's first, so the drawing
 * thread can't take it meanwhile, and then its draw count says whether it
 * was drawn over before that. */
void rmf_send_newest(rmf_pub *pub, int index)
{
    int slot = pub->newest_slot;
    uint32_t bit = rmf_reader_bit(index);
    uint32_t old;

    if (slot < 0) {
        return;
    }
    old = atomic_load(&pub->holders[slot]);
    do {
        if (old & RMF_DRAWING) {
            return;   /* a newer frame is on its way to everyone */
        }
    } while (!atomic_compare_exchange_weak(&pub->holders[slot], &old, old | bit));
    if (atomic_load(&pub->drawn[slot]) != pub->newest_drawn) {
        atomic_fetch_and(&pub->holders[slot], ~bit);
        return;
    }
    send_frame(pub, index, pub->queued_generation, slot, pub->newest_sequence,
               pub->newest_time);
}

static void do_set_slots(void *opaque)
{
    slots_job *job = opaque;
    rmf_pub *pub = job->pub;

    for (uint32_t i = 0; i < pub->count; i++) {
        CFRelease(pub->surfaces[i]);
    }
    memcpy(pub->surfaces, job->surfaces, sizeof(pub->surfaces));
    pub->count = job->count;
    pub->width = job->width;
    pub->height = job->height;
    pub->queued_generation = job->generation;
    pub->newest_slot = -1;
    for (int i = 0; i < RMF_MAX_READERS; i++) {
        rmf_send_slots(pub, i);
    }
    free(job);
}

static void do_publish(void *opaque)
{
    publish_job *job = opaque;
    rmf_pub *pub = job->pub;
    int slot = job->slot;

    if (job->generation != pub->queued_generation || slot < 0 ||
        (uint32_t)slot >= pub->count) {
        if (rmf_slot_is_current(pub, job->generation, slot)) {
            atomic_fetch_and(&pub->holders[slot], ~RMF_DRAWING);
        }
        free(job);
        return;
    }
    for (int i = 0; i < RMF_MAX_READERS; i++) {
        if (pub->readers[i].port != MACH_PORT_NULL) {
            /* Marked first: a RELEASE can only come after the send. */
            atomic_fetch_or(&pub->holders[slot], rmf_reader_bit(i));
            send_frame(pub, i, job->generation, slot, job->sequence,
                       job->host_time);
        }
    }
    pub->newest_slot = slot;
    pub->newest_drawn = atomic_load(&pub->drawn[slot]);
    pub->newest_sequence = job->sequence;
    pub->newest_time = job->host_time;
    atomic_fetch_and(&pub->holders[slot], ~RMF_DRAWING);
    free(job);
}

static void do_cancel_source(void *opaque)
{
    rmf_pub *pub = opaque;

    for (int i = 0; i < RMF_MAX_READERS; i++) {
        rmf_drop_reader(pub, i);
    }
    /* Notices still queued go with the notify port; their names are dead and
     * let go here. */
    for (int i = 0; i < RMF_AWAITED_ROOM; i++) {
        if (pub->awaited[i] != MACH_PORT_NULL) {
            mach_port_deallocate(mach_task_self(), pub->awaited[i]);
        }
    }
    mach_port_mod_refs(mach_task_self(), pub->service,
                       MACH_PORT_RIGHT_RECEIVE, -1);
    mach_port_mod_refs(mach_task_self(), pub->notify,
                       MACH_PORT_RIGHT_RECEIVE, -1);
    mach_port_mod_refs(mach_task_self(), pub->set,
                       MACH_PORT_RIGHT_PORT_SET, -1);
}

/* The notify port and the set. On failure nothing of them is left. */
static kern_return_t make_ports(rmf_pub *pub)
{
    mach_port_t task = mach_task_self();
    kern_return_t kr;

    kr = mach_port_allocate(task, MACH_PORT_RIGHT_RECEIVE, &pub->notify);
    if (kr != KERN_SUCCESS) {
        return kr;
    }
    kr = mach_port_allocate(task, MACH_PORT_RIGHT_PORT_SET, &pub->set);
    if (kr != KERN_SUCCESS) {
        mach_port_mod_refs(task, pub->notify, MACH_PORT_RIGHT_RECEIVE, -1);
        return kr;
    }
    kr = mach_port_move_member(task, pub->service, pub->set);
    if (kr == KERN_SUCCESS) {
        kr = mach_port_move_member(task, pub->notify, pub->set);
    }
    if (kr != KERN_SUCCESS) {
        mach_port_mod_refs(task, pub->notify, MACH_PORT_RIGHT_RECEIVE, -1);
        mach_port_mod_refs(task, pub->set, MACH_PORT_RIGHT_PORT_SET, -1);
    }
    return kr;
}

rmf_pub *rmf_pub_open(const char *service, char *err, size_t err_len)
{
    rmf_pub *pub = calloc(1, sizeof(*pub));
    mach_port_limits_t limits = { .mpl_qlimit = MACH_PORT_QLIMIT_LARGE };
    kern_return_t kr;

    kr = bootstrap_check_in(bootstrap_port, service, &pub->service);
    if (kr != KERN_SUCCESS) {
        snprintf(err, err_len, "bootstrap_check_in(%s): %s", service,
                 bootstrap_strerror(kr));
        free(pub);
        return NULL;
    }
    kr = make_ports(pub);
    if (kr != KERN_SUCCESS) {
        snprintf(err, err_len, "the ports of %s: %s", service,
                 mach_error_string(kr));
        mach_port_mod_refs(mach_task_self(), pub->service,
                           MACH_PORT_RIGHT_RECEIVE, -1);
        free(pub);
        return NULL;
    }
    snprintf(pub->name, sizeof(pub->name), "%s", service);
    pub->newest_slot = -1;
    /* Releases must never be lost, or slots would stay busy for good. The
     * kernel's notices are send-once messages, which no queue limit stops. */
    mach_port_set_attributes(mach_task_self(), pub->service,
                             MACH_PORT_LIMITS_INFO, (mach_port_info_t)&limits,
                             MACH_PORT_LIMITS_INFO_COUNT);
    pub->queue = dispatch_queue_create("rmframes.publisher", NULL);
    pub->source = dispatch_source_create(DISPATCH_SOURCE_TYPE_MACH_RECV,
                                         pub->set, 0, pub->queue);
    dispatch_set_context(pub->source, pub);
    dispatch_source_set_event_handler_f(pub->source, rmf_pub_receive);
    dispatch_source_set_cancel_handler_f(pub->source, do_cancel_source);
    dispatch_resume(pub->source);
    return pub;
}

uint32_t rmf_pub_set_slots(rmf_pub *pub, IOSurfaceRef const *surfaces,
                           uint32_t count, uint32_t width, uint32_t height)
{
    slots_job *job = calloc(1, sizeof(*job));

    if (count > RMF_MAX_SLOTS) {
        count = RMF_MAX_SLOTS;
    }
    job->pub = pub;
    job->generation = atomic_fetch_add(&pub->generation, 1) + 1;
    job->count = count;
    job->width = width;
    job->height = height;
    for (uint32_t i = 0; i < count; i++) {
        job->surfaces[i] = (IOSurfaceRef)CFRetain(surfaces[i]);
    }
    atomic_store(&pub->slot_count, count);
    for (int slot = 0; slot < RMF_MAX_SLOTS; slot++) {
        atomic_store(&pub->holders[slot], 0);
    }
    dispatch_async_f(pub->queue, job, do_set_slots);
    return job->generation;
}

int rmf_pub_acquire(rmf_pub *pub, uint32_t *generation)
{
    uint32_t count = atomic_load(&pub->slot_count);
    uint32_t start = atomic_load(&pub->next);

    for (uint32_t k = 0; k < count; k++) {
        uint32_t slot = (start + k) % count;
        uint32_t old = atomic_load(&pub->holders[slot]);

        /* The queue may mark the slot meanwhile; then it is not free. */
        while (old == 0) {
            if (atomic_compare_exchange_weak(&pub->holders[slot], &old,
                                             RMF_DRAWING)) {
                atomic_fetch_add(&pub->drawn[slot], 1);
                atomic_store(&pub->next, slot + 1);
                *generation = atomic_load(&pub->generation);
                return (int)slot;
            }
        }
    }
    return -1;
}

void rmf_pub_publish(rmf_pub *pub, uint32_t generation, int slot,
                     uint64_t host_time)
{
    publish_job *job = malloc(sizeof(*job));

    job->pub = pub;
    job->generation = generation;
    job->slot = slot;
    job->sequence = atomic_fetch_add(&pub->sequence, 1) + 1;
    job->host_time = host_time;
    dispatch_async_f(pub->queue, job, do_publish);
}

void rmf_pub_cancel(rmf_pub *pub, uint32_t generation, int slot)
{
    if (rmf_slot_is_current(pub, generation, slot)) {
        atomic_fetch_and(&pub->holders[slot], ~RMF_DRAWING);
    }
}

void rmf_pub_counts(rmf_pub *pub, uint64_t *sent, uint64_t *dropped)
{
    *sent = atomic_load(&pub->sent);
    *dropped = atomic_load(&pub->dropped);
}

uint32_t rmf_pub_readers(rmf_pub *pub)
{
    return atomic_load(&pub->reader_count);
}

uint64_t rmf_pub_ignored(rmf_pub *pub)
{
    return atomic_load(&pub->ignored);
}

static void do_free(void *opaque)
{
    rmf_pub *pub = opaque;

    for (uint32_t i = 0; i < pub->count; i++) {
        CFRelease(pub->surfaces[i]);
    }
    free(pub);
}

void rmf_pub_close(rmf_pub *pub)
{
    dispatch_source_cancel(pub->source);
    dispatch_release(pub->source);
    dispatch_async_f(pub->queue, pub, do_free);
    dispatch_release(pub->queue);
}
