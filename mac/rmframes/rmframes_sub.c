/*
 * rmframes, the reading side. See rmframes.h for the protocol.
 *
 * SPDX-License-Identifier: MIT
 * Copyright (c) 2026 RUXN LLC and contributors
 *
 * The reader has two receive rights in one port set, and one dispatch source
 * on the set. `port` is the one the publisher sends to. `notify` is ours
 * alone: no send right to it is ever made, only the send-once rights the
 * kernel holds for our two requests (the publisher's port dying, and no
 * senders left on `port`), so only the kernel's notices come there. Each
 * message is taken by the port it came to: notices only on `notify`, the
 * publisher's messages only on `port`. A publisher, which may be a process
 * the guest can reach, so never makes us let go of a name of its choosing.
 */
#include "rmframes_sub.h"
#include "rmframes_peer.h"

#include <mach/mach.h>
#include <mach/notify.h>
#include <servers/bootstrap.h>
#include <stdatomic.h>
#include <stdbool.h>
#include <stddef.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

/* A release waits this long at most for room in the publisher's queue, which
 * is large; losing one would keep a slot busy until the next hello. */
#define RELEASE_SEND_TIMEOUT_MS 20
/* Where the kernel puts the sender's process id in an audit token: 0 for the
 * kernel itself (KERNEL_AUDIT_TOKEN_VALUE in mach/message.h). */
#define AUDIT_TOKEN_PID 5
/* The notices' sizes as received, without their trailers. */
#define DEAD_NAME_SIZE offsetof(mach_dead_name_notification_t, trailer)
#define NO_SENDERS_SIZE offsetof(mach_no_senders_notification_t, trailer)

struct rmf_sub {
    mach_port_t publisher;   /* send right to the publisher's service port */
    mach_port_t port;        /* our receive right, the publisher's to send to */
    mach_port_t notify;      /* our receive right, the kernel's notices only */
    mach_port_t set;         /* both of them, for the one source */
    dispatch_source_t source;
    rmf_sub_callbacks cb;
    int gone;
    /* The number the publisher gave us in SLOTS, for RELEASE and BYE; none
     * from a publisher built before there were several readers. Written on
     * the queue, read from any thread. */
    _Atomic uint32_t number;
    /* The publisher's process, once a message from it was found to be from
     * one of ours (rmframes_peer.c); 0 before. Its later messages are taken
     * by this alone, so the code's signature is checked once. */
    pid_t sender;
};

static void send_simple(rmf_sub *sub, mach_msg_header_t *header,
                        mach_msg_size_t size, mach_msg_timeout_t timeout)
{
    header->msgh_bits = MACH_MSGH_BITS_SET(MACH_MSG_TYPE_COPY_SEND, 0, 0, 0);
    header->msgh_size = size;
    header->msgh_remote_port = sub->publisher;
    header->msgh_local_port = MACH_PORT_NULL;
    mach_msg(header, MACH_SEND_MSG | MACH_SEND_TIMEOUT, size, 0,
             MACH_PORT_NULL, timeout, MACH_PORT_NULL);
}

static void handle_slots(rmf_sub *sub, rmf_slots_msg *msg)
{
    IOSurfaceRef surfaces[RMF_MAX_SLOTS] = { 0 };
    uint32_t count = msg->count;

    if (!(msg->header.msgh_bits & MACH_MSGH_BITS_COMPLEX) ||
        msg->header.msgh_size < RMF_SLOTS_SIZE_V1 ||
        msg->body.msgh_descriptor_count != RMF_MAX_SLOTS ||
        msg->version != RMF_VERSION || msg->format != RMF_FORMAT_BGRA ||
        count > RMF_MAX_SLOTS) {
        fprintf(stderr, "rmframes: a slots message this reader can't take\n");
        return;
    }
    /* Each descriptor a port, as the publisher sends them: the layout says
     * nothing about what the kernel actually put there. */
    for (uint32_t i = 0; i < RMF_MAX_SLOTS; i++) {
        if (msg->surface[i].type != MACH_MSG_PORT_DESCRIPTOR) {
            fprintf(stderr, "rmframes: slot %u is not a port\n", i);
            return;
        }
    }
    atomic_store(&sub->number, msg->header.msgh_size >= sizeof(*msg) ?
                               msg->reader : RMF_NO_READER);
    for (uint32_t i = 0; i < count; i++) {
        surfaces[i] = IOSurfaceLookupFromMachPort(msg->surface[i].name);
        if (!surfaces[i]) {
            fprintf(stderr, "rmframes: slot %u has no surface\n", i);
            count = i;
            break;
        }
    }
    sub->cb.on_slots(sub->cb.ctx, msg->generation, msg->width, msg->height,
                     count, surfaces);
    for (uint32_t i = 0; i < count; i++) {
        CFRelease(surfaces[i]);
    }
}

typedef union received {
    mach_msg_header_t header;
    rmf_slots_msg slots;
    rmf_frame_msg frame;
    rmf_refused_msg refused;
    mach_dead_name_notification_t dead;
    uint8_t room[sizeof(rmf_slots_msg) + MAX_TRAILER_SIZE + 64];
} received;

static void went(rmf_sub *sub)
{
    if (!sub->gone) {
        sub->gone = 1;
        sub->cb.on_gone(sub->cb.ctx);
    }
}

/* Whether a message on `port` came from the publisher: a process that is
 * one of ours, and the same one as before. */
static bool from_publisher(rmf_sub *sub, const mach_msg_audit_trailer_t *trailer)
{
    pid_t pid = (pid_t)trailer->msgh_audit.val[AUDIT_TOKEN_PID];

    if (trailer->msgh_trailer_size < sizeof(*trailer) || pid == 0) {
        return false;
    }
    if (sub->sender == 0 && rmf_peer_is_ours(&trailer->msgh_audit)) {
        sub->sender = pid;
    }
    return sub->sender == pid;
}

/* A message on `port`: the publisher's. */
static void take_message(rmf_sub *sub, received *buf,
                         const mach_msg_audit_trailer_t *trailer)
{
    mach_msg_header_t *h = &buf->header;
    bool complex = h->msgh_bits & MACH_MSGH_BITS_COMPLEX;

    if (!from_publisher(sub, trailer)) {
        return;
    }

    if (h->msgh_id == RMF_MSG_SLOTS) {
        handle_slots(sub, &buf->slots);
    } else if (complex) {
        /* Only SLOTS carries descriptors. */
    } else if (h->msgh_id == RMF_MSG_FRAME) {
        if (h->msgh_size >= sizeof(rmf_frame_msg) &&
            buf->frame.slot < RMF_MAX_SLOTS) {
            sub->cb.on_frame(sub->cb.ctx, &buf->frame);
        }
    } else if (h->msgh_id == RMF_MSG_REFUSED) {
        if (h->msgh_size >= sizeof(rmf_refused_msg) && !sub->gone) {
            buf->refused.reason[RMF_REASON_ROOM - 1] = 0;
            sub->gone = 1;
            if (sub->cb.on_refused) {
                sub->cb.on_refused(sub->cb.ctx, buf->refused.reason);
            } else {
                /* A reader built before refusals: it waits with no
                 * frames, as for a publisher with no picture yet. */
                fprintf(stderr, "rmframes: refused: %s\n", buf->refused.reason);
            }
        }
    }
    /* Anything else, the kernel's notices among them, is let go. */
}

static bool is_dead_name(mach_port_t name)
{
    mach_port_type_t type = 0;

    return mach_port_type(mach_task_self(), name, &type) == KERN_SUCCESS &&
           (type & MACH_PORT_TYPE_DEAD_NAME);
}

/* A message on `notify`, which only the kernel can send to; the audit token
 * says it is the kernel's all the same. */
static void take_notice(rmf_sub *sub, received *buf,
                        const mach_msg_audit_trailer_t *trailer)
{
    mach_msg_header_t *h = &buf->header;

    if (trailer->msgh_trailer_size < sizeof(*trailer) ||
        trailer->msgh_audit.val[AUDIT_TOKEN_PID] != 0 ||
        (h->msgh_bits & MACH_MSGH_BITS_COMPLEX) ||
        h->msgh_remote_port != MACH_PORT_NULL) {
        return;
    }
    if (h->msgh_id == MACH_NOTIFY_DEAD_NAME && h->msgh_size == DEAD_NAME_SIZE) {
        /* Only about the publisher's port, the one name we asked about. The
         * notice carries a reference of its own to the dead name. */
        if (buf->dead.not_port == sub->publisher &&
            is_dead_name(sub->publisher)) {
            mach_port_deallocate(mach_task_self(), sub->publisher);
            went(sub);
        }
    } else if (h->msgh_id == MACH_NOTIFY_NO_SENDERS &&
               h->msgh_size == NO_SENDERS_SIZE) {
        went(sub);
    }
    /* A send-once notice is a request another reader of this process took
     * over: nothing to do. */
}

static void do_receive(void *opaque)
{
    rmf_sub *sub = opaque;
    received buf;
    const mach_msg_option_t options =
        MACH_RCV_MSG | MACH_RCV_TIMEOUT |
        MACH_RCV_TRAILER_TYPE(MACH_MSG_TRAILER_FORMAT_0) |
        MACH_RCV_TRAILER_ELEMENTS(MACH_RCV_TRAILER_AUDIT);

    for (;;) {
        kern_return_t kr = mach_msg(&buf.header, options, 0, sizeof(buf),
                                    sub->set, 0, MACH_PORT_NULL);
        if (kr == MACH_RCV_TOO_LARGE) {
            continue;   /* the kernel has destroyed it, rights and all */
        }
        if (kr != MACH_MSG_SUCCESS) {
            return;
        }
        if (buf.header.msgh_local_port == sub->notify) {
            take_notice(sub, &buf, (const mach_msg_audit_trailer_t *)
                        ((uint8_t *)&buf + round_msg(buf.header.msgh_size)));
        } else if (buf.header.msgh_local_port == sub->port) {
            take_message(sub, &buf, (const mach_msg_audit_trailer_t *)
                         ((uint8_t *)&buf + round_msg(buf.header.msgh_size)));
        }
        /* The surface ports were looked up already; this frees them. */
        mach_msg_destroy(&buf.header);
    }
}

static void do_cancel(void *opaque)
{
    rmf_sub *sub = opaque;
    mach_port_t task = mach_task_self();

    mach_port_mod_refs(task, sub->port, MACH_PORT_RIGHT_RECEIVE, -1);
    mach_port_mod_refs(task, sub->notify, MACH_PORT_RIGHT_RECEIVE, -1);
    mach_port_mod_refs(task, sub->set, MACH_PORT_RIGHT_PORT_SET, -1);
    mach_port_deallocate(task, sub->publisher);
    free(sub);
}

/* Our two receive rights and the set. On failure nothing of them is left. */
static kern_return_t make_ports(rmf_sub *sub)
{
    mach_port_t task = mach_task_self();
    kern_return_t kr;

    kr = mach_port_allocate(task, MACH_PORT_RIGHT_PORT_SET, &sub->set);
    if (kr != KERN_SUCCESS) {
        return kr;
    }
    kr = mach_port_allocate(task, MACH_PORT_RIGHT_RECEIVE, &sub->port);
    if (kr == KERN_SUCCESS) {
        kr = mach_port_allocate(task, MACH_PORT_RIGHT_RECEIVE, &sub->notify);
        if (kr != KERN_SUCCESS) {
            mach_port_mod_refs(task, sub->port, MACH_PORT_RIGHT_RECEIVE, -1);
        }
    }
    if (kr == KERN_SUCCESS) {
        kr = mach_port_move_member(task, sub->port, sub->set);
        if (kr == KERN_SUCCESS) {
            kr = mach_port_move_member(task, sub->notify, sub->set);
        }
        if (kr != KERN_SUCCESS) {
            mach_port_mod_refs(task, sub->port, MACH_PORT_RIGHT_RECEIVE, -1);
            mach_port_mod_refs(task, sub->notify, MACH_PORT_RIGHT_RECEIVE, -1);
        }
    }
    if (kr != KERN_SUCCESS) {
        mach_port_mod_refs(task, sub->set, MACH_PORT_RIGHT_PORT_SET, -1);
    }
    return kr;
}

rmf_sub *rmf_sub_open(const char *service, dispatch_queue_t queue,
                      rmf_sub_callbacks callbacks, char *err, size_t err_len)
{
    rmf_sub *sub = calloc(1, sizeof(*sub));
    mach_port_limits_t limits = { .mpl_qlimit = MACH_PORT_QLIMIT_LARGE };
    mach_port_t previous = MACH_PORT_NULL;
    rmf_hello_msg hello;
    kern_return_t kr;

    sub->cb = callbacks;
    kr = bootstrap_look_up(bootstrap_port, service, &sub->publisher);
    if (kr != KERN_SUCCESS) {
        snprintf(err, err_len, "no publisher named %s (%s)", service,
                 bootstrap_strerror(kr));
        free(sub);
        return NULL;
    }
    kr = make_ports(sub);
    if (kr != KERN_SUCCESS) {
        snprintf(err, err_len, "the ports for %s: %s", service,
                 mach_error_string(kr));
        mach_port_deallocate(mach_task_self(), sub->publisher);
        free(sub);
        return NULL;
    }
    mach_port_set_attributes(mach_task_self(), sub->port, MACH_PORT_LIMITS_INFO,
                             (mach_port_info_t)&limits,
                             MACH_PORT_LIMITS_INFO_COUNT);
    /* Told on our notify port when the publisher dies. A process has one
     * name for the publisher whatever number of readers it opens, and the
     * name one dead-name request: a later reader's takes the place of an
     * earlier one's, which the no-senders request below makes up for. */
    mach_port_request_notification(mach_task_self(), sub->publisher,
                                   MACH_NOTIFY_DEAD_NAME, 0, sub->notify,
                                   MACH_MSG_TYPE_MAKE_SEND_ONCE, &previous);
    if (previous != MACH_PORT_NULL) {
        mach_port_deallocate(mach_task_self(), previous);
    }

    sub->source = dispatch_source_create(DISPATCH_SOURCE_TYPE_MACH_RECV,
                                         sub->set, 0, queue);
    dispatch_set_context(sub->source, sub);
    dispatch_source_set_event_handler_f(sub->source, do_receive);
    dispatch_source_set_cancel_handler_f(sub->source, do_cancel);
    dispatch_resume(sub->source);

    memset(&hello, 0, sizeof(hello));
    hello.header.msgh_bits = MACH_MSGH_BITS_SET(MACH_MSG_TYPE_COPY_SEND,
                                                MACH_MSG_TYPE_MAKE_SEND, 0, 0);
    hello.header.msgh_size = sizeof(hello);
    hello.header.msgh_remote_port = sub->publisher;
    hello.header.msgh_local_port = sub->port;
    hello.header.msgh_id = RMF_MSG_HELLO;
    hello.version = RMF_VERSION;
    hello.pid = (uint32_t)getpid();
    kr = mach_msg(&hello.header, MACH_SEND_MSG | MACH_SEND_TIMEOUT,
                  sizeof(hello), 0, MACH_PORT_NULL, 1000, MACH_PORT_NULL);
    if (kr != MACH_MSG_SUCCESS) {
        snprintf(err, err_len, "hello to %s: %s", service,
                 mach_error_string(kr));
        dispatch_source_cancel(sub->source);
        dispatch_release(sub->source);
        return NULL;
    }
    /* The hello gave the publisher the one send right to our port there is.
     * When the publisher lets it go, because it died or let this reader go,
     * we hear it: for each reader of a process apart. */
    mach_port_request_notification(mach_task_self(), sub->port,
                                   MACH_NOTIFY_NO_SENDERS, 1, sub->notify,
                                   MACH_MSG_TYPE_MAKE_SEND_ONCE, &previous);
    return sub;
}

void rmf_sub_release(rmf_sub *sub, uint32_t generation, uint32_t slot)
{
    rmf_release_msg msg;

    memset(&msg, 0, sizeof(msg));
    msg.header.msgh_id = RMF_MSG_RELEASE;
    msg.generation = generation;
    msg.slot = slot;
    msg.reader = atomic_load(&sub->number);
    send_simple(sub, &msg.header, sizeof(msg), RELEASE_SEND_TIMEOUT_MS);
}

void rmf_sub_close(rmf_sub *sub)
{
    rmf_bye_msg msg;

    memset(&msg, 0, sizeof(msg));
    msg.header.msgh_id = RMF_MSG_BYE;
    msg.reader = atomic_load(&sub->number);
    send_simple(sub, &msg.header, sizeof(msg), 0);
    dispatch_source_cancel(sub->source);
    dispatch_release(sub->source);
}
