/*
 * rmframes: whether the process at the other end is one of ours.
 *
 * SPDX-License-Identifier: MIT
 * Copyright (c) 2026 RUXN LLC and contributors
 */
#ifndef RMFRAMES_PEER_H
#define RMFRAMES_PEER_H

#include <mach/mach.h>
#include <stdbool.h>

/* True if the process `token` names may publish to us or read from us: any
 * process, for a program signed ad hoc or not at all, as a build of one's
 * own is; otherwise only a program Apple's certificate for this program's
 * own team signed. */
bool rmf_peer_is_ours(const audit_token_t *token);

#endif
