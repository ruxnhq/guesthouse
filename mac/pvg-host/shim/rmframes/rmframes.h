/*
 * rmframes: handing a display's frames to other processes as IOSurfaces.
 *
 * SPDX-License-Identifier: MIT
 * Copyright (c) 2026 RUXN LLC and contributors
 *
 * The publisher (QEMU's apple-gfx device, guesthouse-pvg-host, or testsource)
 * draws each frame into one of a few IOSurfaces it owns, which it calls slots.
 * A reader (rmhost, the Guesthouse app) gets the slots once, as Mach ports,
 * and then a small message per frame naming the slot. The pixels never pass
 * through either process's memory: the reader hands the IOSurface straight to
 * its encoder or its layer. A reader gives a slot back when it is done with
 * it, and the publisher draws only into slots that no reader holds, so a frame
 * is never overwritten while it is being read.
 *
 * Everything is Mach messages. The publisher checks in a bootstrap name (any
 * name the two sides agree on); a reader looks it up and says hello with a
 * port of its own, which the publisher sends to from then on.
 *
 *   reader -> publisher   HELLO    reply port = the reader's port
 *   publisher -> reader   SLOTS    the slots as IOSurface ports, their size,
 *                                  and the reader's number
 *   publisher -> reader   FRAME    slot n holds a new frame
 *   reader -> publisher   RELEASE  done with slot n
 *   reader -> publisher   BYE      going away (a dead reader is noticed too)
 *   publisher -> reader   REFUSED  there are as many readers as there may be
 *
 * Up to RMF_MAX_READERS readers at once. Each is sent every frame it has room
 * for, and a slot stays held while any reader holds it. A frame is skipped
 * only when no slot is free. A reader that comes while the picture stands
 * still is sent the newest frame straight after its slots.
 *
 * SLOTS comes after HELLO and again whenever the picture changes size; it
 * starts a new generation, and FRAME and RELEASE carry the generation they
 * belong to so that late ones are ignored. The publisher sends without
 * waiting: when a reader's queue is full that reader misses the frame and
 * holds nothing of it.
 *
 * Which reader a RELEASE or BYE is from: the number SLOTS gave it, sent back
 * at the end of the message, and the sending process as the kernel names it.
 * Readers built before there were several send no number; the process alone
 * names them, which is enough with one reader to a process. The numbers are
 * trailing fields, so old and new readers and publishers understand each
 * other: an old publisher sends a shorter SLOTS and reads the number as
 * nothing, an old reader ignores the number and never sees REFUSED (it waits,
 * with no frames, as it would for a publisher with no picture yet).
 */
#ifndef RMFRAMES_H
#define RMFRAMES_H

#include <stddef.h>
#include <stdint.h>
#include <mach/mach.h>

#define RMF_VERSION 1

#define RMF_MSG_HELLO   0x524d4601
#define RMF_MSG_SLOTS   0x524d4602
#define RMF_MSG_FRAME   0x524d4603
#define RMF_MSG_RELEASE 0x524d4604
#define RMF_MSG_BYE     0x524d4605
#define RMF_MSG_REFUSED 0x524d4606

/* The most slots a publisher may have. SLOTS always carries this many port
 * descriptors, the unused ones null, so the message has one fixed layout. */
#define RMF_MAX_SLOTS 8

/* The most readers a publisher serves at once. */
#define RMF_MAX_READERS 4

/* The only pixel format so far: 'BGRA', 8 bits each, what the device draws. */
#define RMF_FORMAT_BGRA 0x42475241

/* No reader number yet: SLOTS has not come, or came from a publisher built
 * before there were several readers. A RELEASE or BYE that carries it is
 * ignored; the reader's port dying says it has gone. */
#define RMF_NO_READER 0

typedef struct rmf_hello_msg {
    mach_msg_header_t header;
    uint32_t version;
    uint32_t pid;
} rmf_hello_msg;

typedef struct rmf_slots_msg {
    mach_msg_header_t header;
    mach_msg_body_t body;
    mach_msg_port_descriptor_t surface[RMF_MAX_SLOTS];
    uint32_t version;
    uint32_t generation;
    uint32_t count;
    uint32_t width;
    uint32_t height;
    uint32_t format;
    uint32_t reader;     /* this reader's number, for RELEASE and BYE */
} rmf_slots_msg;

typedef struct rmf_frame_msg {
    mach_msg_header_t header;
    uint32_t generation;
    uint32_t slot;
    uint32_t width;
    uint32_t height;
    uint64_t sequence;   /* counts every frame published, from 1 */
    uint64_t host_time;  /* mach_absolute_time() when the frame was drawn */
} rmf_frame_msg;

typedef struct rmf_release_msg {
    mach_msg_header_t header;
    uint32_t generation;
    uint32_t slot;
    uint32_t reader;     /* from SLOTS; not sent by an old reader */
} rmf_release_msg;

typedef struct rmf_bye_msg {
    mach_msg_header_t header;
    uint32_t reader;     /* as in RELEASE */
} rmf_bye_msg;

/* Sent to a reader the publisher has no room for, in place of SLOTS. */
#define RMF_REASON_ROOM 128
typedef struct rmf_refused_msg {
    mach_msg_header_t header;
    uint32_t version;
    char reason[RMF_REASON_ROOM];   /* ends in a zero */
} rmf_refused_msg;

/* The messages as readers and publishers sent them before the reader's
 * number was added: what a message must be at least to be read. */
#define RMF_SLOTS_SIZE_V1   offsetof(rmf_slots_msg, reader)
#define RMF_RELEASE_SIZE_V1 offsetof(rmf_release_msg, reader)
#define RMF_BYE_SIZE_V1     offsetof(rmf_bye_msg, reader)

#endif
