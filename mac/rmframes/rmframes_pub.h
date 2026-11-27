/*
 * rmframes, the publishing side. See rmframes.h for the protocol.
 *
 * SPDX-License-Identifier: MIT
 * Copyright (c) 2026 RUXN LLC and contributors
 *
 * The same files are built into QEMU's apple-gfx device (qemu-frames.diff),
 * guesthouse-pvg-host and testsource, so they all speak the protocol the
 * same way: rmframes_pub.c and rmframes_recv.c, with rmframes_pub_internal.h
 * between them.
 *
 * Use from one drawing thread:
 *   rmf_pub_set_slots()  when the picture gets a new size
 *   rmf_pub_acquire()    before drawing a frame: a slot, or -1 to draw elsewhere
 *   rmf_pub_publish()    once the slot's frame is drawn (from any thread)
 *   rmf_pub_cancel()     if the drawing failed
 * Messages from readers are taken on a queue of the publisher's own.
 */
#ifndef RMFRAMES_PUB_H
#define RMFRAMES_PUB_H

#include <stddef.h>
#include <stdint.h>
#include <IOSurface/IOSurfaceRef.h>

typedef struct rmf_pub rmf_pub;

/* Checks in `service` with the bootstrap server. NULL on failure, with the
 * reason in err. */
rmf_pub *rmf_pub_open(const char *service, char *err, size_t err_len);

/* The slots for the current picture size (count at most RMF_MAX_SLOTS). The
 * surfaces are retained; slots of the old size are no longer handed out.
 * Returns the new generation. */
uint32_t rmf_pub_set_slots(rmf_pub *pub, IOSurfaceRef const *surfaces,
                           uint32_t count, uint32_t width, uint32_t height);

/* A slot that no reader holds, now marked busy, and its generation; -1 when
 * there are no slots or readers hold them all. A slot is handed out with no
 * reader there too: the newest frame is then kept in it for a reader that
 * comes later, and slots are handed out in turn, so the newest one is the
 * last to be drawn over. */
int rmf_pub_acquire(rmf_pub *pub, uint32_t *generation);

/* The frame in `slot` is drawn: send it to every reader. host_time is
 * mach_absolute_time() at drawing. */
void rmf_pub_publish(rmf_pub *pub, uint32_t generation, int slot,
                     uint64_t host_time);

/* Nothing was drawn into `slot` after all. */
void rmf_pub_cancel(rmf_pub *pub, uint32_t generation, int slot);

/* Counters for a log line: frame messages sent to readers (one frame sent to
 * two readers counts two), and those that could not be (a reader's queue was
 * full or it went away). */
void rmf_pub_counts(rmf_pub *pub, uint64_t *sent, uint64_t *dropped);

/* How many readers there are now. */
uint32_t rmf_pub_readers(rmf_pub *pub);

/* Messages taken and not acted on: malformed, from no reader, or a notice
 * where only readers' messages may come. A few are usual (a late RELEASE);
 * many say something is sending what it should not. */
uint64_t rmf_pub_ignored(rmf_pub *pub);

void rmf_pub_close(rmf_pub *pub);

#endif
