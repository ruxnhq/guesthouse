/*
 * Input from a remoteman viewer, put into the guest through QEMU's QMP
 * socket as input-send-event commands: the usb-tablet takes the pointer as
 * absolute positions, the usb-kbd the keys.
 *
 * SPDX-License-Identifier: MIT
 * Copyright (c) 2026 RUXN LLC and contributors
 */
#ifndef RMHOST_QMP_INPUT_H
#define RMHOST_QMP_INPUT_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

typedef struct qmp_input qmp_input;

/* Connects to the QMP socket at `path` and turns command mode on. NULL on
 * failure, with the reason in err. Call it before starting other threads: a
 * long path is reached by changing folder for a moment. */
qmp_input *qmp_input_open(const char *path, char *err, size_t err_len);

/* The guest's picture size, for turning pixel moves into positions. */
void qmp_input_set_size(qmp_input *q, uint32_t width, uint32_t height);

/* One event. `kind` and the fields are those of remoteman's rm_input
 * (docs/PROTOCOL.md, Input): 1 pointer to (x, y fractions), 2 pointer by
 * (x, y pixels), 3 button, 4 scroll (x, y pixels), 5 key (usage), 6 touch.
 * Text, pen and game controllers are not taken. Returns false for an event
 * that was dropped. */
bool qmp_input_event(qmp_input *q, uint8_t kind, uint8_t button, uint8_t down,
                     uint8_t phase, uint16_t usage, float x, float y);

/* Lets go of every key and button still down (the viewer left). */
void qmp_input_release_all(qmp_input *q);

void qmp_input_close(qmp_input *q);

/* The QEMU key name (QKeyCode) for a USB HID usage on page 7, or NULL. */
const char *hid_usage_to_qcode(uint16_t usage);

#endif
