/*
 * rmframes, the publishing side: what readers send, and the kernel's notices
 * that a reader died. See rmframes.h for the protocol.
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
 * Every message is taken by the port it came to. Any process of the user can
 * send to the service port, so only HELLO, RELEASE and BYE are read there,
 * HELLO only from a process that is one of ours (rmframes_peer.c), and
 * RELEASE and BYE act only for a reader of the sending process, as the
 * kernel names it. Only the kernel can send to the notify port, so only
 * notices are read there, and a dead-name notice acts only on a name that is
 * a reader's port we asked about, and is dead. Anything else is counted in
 * `ignored`, and the rights it carried are let go.
 */
#include "rmframes_pub_internal.h"
#include "rmframes_peer.h"

#include <mach/notify.h>
#include <stddef.h>
#include <stdio.h>
#include <string.h>

/* A refusal is a courtesy: it is not worth waiting long for. */
#define REFUSED_SEND_TIMEOUT_MS 50
/* Where the kernel puts the sender's process id in an audit token. It is 0
 * for the kernel itself, which sends with KERNEL_AUDIT_TOKEN_VALUE, all
 * zeros (mach/message.h); no process has id 0. */
#define AUDIT_TOKEN_PID 5
/* The size of a dead-name notice as received, without its trailer. */
#define DEAD_NAME_SIZE offsetof(mach_dead_name_notification_t, trailer)

typedef union received {
    mach_msg_header_t header;
    rmf_hello_msg hello;
    rmf_release_msg release;
    rmf_bye_msg bye;
    mach_dead_name_notification_t dead;
    uint8_t room[256];
} received;

static void ignore(rmf_pub *pub)
{
    atomic_fetch_add(&pub->ignored, 1);
}

/* The reader a RELEASE or BYE came from: by the number it was given, from
 * the process that was given it; a reader built before numbers (a message
 * without the field) is known by its process alone. A new reader that has no
 * number yet (it closed before its SLOTS came, or was refused) is no reader
 * here: its port's death says it has gone. -1 if none. */
static int find_reader(rmf_pub *pub, bool has_field, uint32_t number, pid_t pid)
{
    if (has_field && number == RMF_NO_READER) {
        return -1;
    }
    for (int i = 0; i < RMF_MAX_READERS; i++) {
        rmf_reader *r = &pub->readers[i];

        if (r->port != MACH_PORT_NULL && r->pid == pid &&
            (!has_field || r->number == number)) {
            return i;
        }
    }
    return -1;
}

/* Tells a reader there is no room for it, and lets its port go. */
static void refuse(rmf_pub *pub, mach_port_t port)
{
    rmf_refused_msg msg;

    memset(&msg, 0, sizeof(msg));
    msg.header.msgh_bits = MACH_MSGH_BITS_SET(MACH_MSG_TYPE_COPY_SEND, 0, 0, 0);
    msg.header.msgh_size = sizeof(msg);
    msg.header.msgh_remote_port = port;
    msg.header.msgh_id = RMF_MSG_REFUSED;
    msg.version = RMF_VERSION;
    snprintf(msg.reason, sizeof(msg.reason),
             "%s already has %d readers, as many as it serves", pub->name,
             RMF_MAX_READERS);
    mach_msg(&msg.header, MACH_SEND_MSG | MACH_SEND_TIMEOUT, sizeof(msg), 0,
             MACH_PORT_NULL, REFUSED_SEND_TIMEOUT_MS, MACH_PORT_NULL);
    mach_port_deallocate(mach_task_self(), port);
}

/* A reader said hello with `port`, a send right it gave us: give it a place,
 * its slots and the newest frame, or refuse it. False if the port is not
 * taken (it is a reader's already, or one of our own): the caller lets go of
 * it then. */
static bool welcome(rmf_pub *pub, mach_port_t port, pid_t pid)
{
    if (port == pub->service || port == pub->notify) {
        return false;
    }
    for (int i = 0; i < RMF_MAX_READERS; i++) {
        if (pub->readers[i].port == port) {
            return false;
        }
    }
    for (int i = 0; i < RMF_MAX_READERS; i++) {
        rmf_reader *r = &pub->readers[i];

        if (r->port != MACH_PORT_NULL) {
            continue;
        }
        /* A place that was let go holds nothing: rmf_drop_reader saw to it. */
        if (++pub->last_number == RMF_NO_READER) {
            ++pub->last_number;
        }
        r->port = port;
        r->pid = pid;
        r->number = pub->last_number;
        rmf_arm_dead_name(pub, i);
        atomic_fetch_add(&pub->reader_count, 1);
        rmf_send_slots(pub, i);
        if (pub->readers[i].port != MACH_PORT_NULL) {
            rmf_send_newest(pub, i);
        }
        return true;
    }
    refuse(pub, port);
    return true;
}

/* A message on the service port: a reader's, or ignored. A reader is welcome
 * only from a process that is one of ours (rmframes_peer.c). */
static void take_message(rmf_pub *pub, received *buf,
                         const mach_msg_audit_trailer_t *trailer)
{
    mach_msg_header_t *h = &buf->header;
    pid_t pid = (pid_t)trailer->msgh_audit.val[AUDIT_TOKEN_PID];
    mach_msg_size_t size = h->msgh_size;
    int index;

    /* None of a reader's messages carries descriptors. */
    if (h->msgh_bits & MACH_MSGH_BITS_COMPLEX) {
        ignore(pub);
        return;
    }
    switch (h->msgh_id) {
    case RMF_MSG_HELLO:
        /* The reply port must be a send right, as the reader made it. */
        if (size >= sizeof(rmf_hello_msg) &&
            buf->hello.version == RMF_VERSION &&
            MACH_MSGH_BITS_REMOTE(h->msgh_bits) == MACH_MSG_TYPE_PORT_SEND &&
            MACH_PORT_VALID(h->msgh_remote_port) &&
            rmf_peer_is_ours(&trailer->msgh_audit) &&
            welcome(pub, h->msgh_remote_port, pid)) {
            h->msgh_remote_port = MACH_PORT_NULL;   /* ours now */
            return;
        }
        break;
    case RMF_MSG_RELEASE:
        if (size >= RMF_RELEASE_SIZE_V1) {
            bool has_field = size >= sizeof(rmf_release_msg);

            index = find_reader(pub, has_field, buf->release.reader, pid);
            if (index >= 0 &&
                rmf_slot_is_current(pub, buf->release.generation,
                                    (int)buf->release.slot)) {
                atomic_fetch_and(&pub->holders[buf->release.slot],
                                 ~rmf_reader_bit(index));
                return;
            }
        }
        break;
    case RMF_MSG_BYE:
        if (size >= RMF_BYE_SIZE_V1) {
            bool has_field = size >= sizeof(rmf_bye_msg);

            index = find_reader(pub, has_field, buf->bye.reader, pid);
            if (index >= 0) {
                rmf_drop_reader(pub, index);
                return;
            }
        }
        break;
    default:
        /* The kernel's notices among them: they never come here. */
        break;
    }
    ignore(pub);
}

static bool is_dead_name(mach_port_t name)
{
    mach_port_type_t type = 0;

    return mach_port_type(mach_task_self(), name, &type) == KERN_SUCCESS &&
           (type & MACH_PORT_TYPE_DEAD_NAME);
}

/* A dead-name notice from the kernel: a reader died. The notice carries a
 * reference of its own to the dead name, let go here, but only for a name we
 * asked about: a live reader's port, or one let go while its notice was on
 * the way (`awaited`). */
static void take_dead_name(rmf_pub *pub, mach_port_t name)
{
    if (!MACH_PORT_VALID(name) || !is_dead_name(name)) {
        ignore(pub);
        return;
    }
    for (int i = 0; i < RMF_MAX_READERS; i++) {
        rmf_reader *r = &pub->readers[i];

        if (r->port == name && r->armed) {
            r->armed = false;   /* the kernel took the request when it fired */
            rmf_drop_reader(pub, i);
            mach_port_deallocate(mach_task_self(), name);
            return;
        }
    }
    for (int i = 0; i < RMF_AWAITED_ROOM; i++) {
        if (pub->awaited[i] == name) {
            pub->awaited[i] = MACH_PORT_NULL;
            mach_port_deallocate(mach_task_self(), name);
            return;
        }
    }
    ignore(pub);
}

/* A message on the notify port, which only the kernel can send to; the
 * audit token says it is the kernel's all the same. */
static void take_notice(rmf_pub *pub, received *buf,
                        const mach_msg_audit_trailer_t *trailer)
{
    mach_msg_header_t *h = &buf->header;
    bool kernel = trailer->msgh_trailer_size >= sizeof(*trailer) &&
                  trailer->msgh_audit.val[AUDIT_TOKEN_PID] == 0;

    if (!kernel || (h->msgh_bits & MACH_MSGH_BITS_COMPLEX) ||
        h->msgh_remote_port != MACH_PORT_NULL) {
        ignore(pub);
        return;
    }
    if (h->msgh_id == MACH_NOTIFY_DEAD_NAME && h->msgh_size == DEAD_NAME_SIZE) {
        take_dead_name(pub, buf->dead.not_port);
    } else if (h->msgh_id == MACH_NOTIFY_SEND_ONCE &&
               h->msgh_size == sizeof(mach_msg_header_t)) {
        /* A request taken back: its send-once right came home. */
    } else {
        ignore(pub);
    }
}

void rmf_pub_receive(void *opaque)
{
    rmf_pub *pub = opaque;
    received buf;
    const mach_msg_option_t options =
        MACH_RCV_MSG | MACH_RCV_TIMEOUT |
        MACH_RCV_TRAILER_TYPE(MACH_MSG_TRAILER_FORMAT_0) |
        MACH_RCV_TRAILER_ELEMENTS(MACH_RCV_TRAILER_AUDIT);

    for (;;) {
        kern_return_t kr = mach_msg(&buf.header, options, 0, sizeof(buf),
                                    pub->set, 0, MACH_PORT_NULL);
        const mach_msg_audit_trailer_t *trailer;

        if (kr == MACH_RCV_TOO_LARGE) {
            /* Bigger than any message here: the kernel has destroyed it,
             * rights and all. */
            ignore(pub);
            continue;
        }
        if (kr != MACH_MSG_SUCCESS) {
            return;
        }
        /* The kernel puts the trailer after the message, in the buffer. */
        trailer = (const mach_msg_audit_trailer_t *)
                  ((uint8_t *)&buf + round_msg(buf.header.msgh_size));
        if (buf.header.msgh_local_port == pub->notify) {
            take_notice(pub, &buf, trailer);
        } else if (buf.header.msgh_local_port == pub->service) {
            take_message(pub, &buf, trailer);
        } else {
            ignore(pub);
        }
        mach_msg_destroy(&buf.header);
    }
}
