/*
 * The devices a running `rmhost serve` trusts: listing them, taking a device's permissions
 * away, disconnecting and forgetting one, from lines on standard input.
 *
 * SPDX-License-Identifier: MIT
 * Copyright (c) 2026 RUXN LLC and contributors
 */
#ifndef DEVICECTL_H
#define DEVICECTL_H

#import <Foundation/Foundation.h>

#include "remoteman_core.h"

/* Starts with the host whose devices these are. */
void devicectl_start(rm_host *host);

/* Takes a line already split into words, if it is one of the device commands. Returns 1 when
 * it was, whether it worked or not, and 0 for any other line. */
int devicectl_command(NSArray<NSString *> *words);

/* Prints the device list when it has changed since it was last printed. Call often. */
void devicectl_tick(void);

/* A device was just accepted: clipboard and files start off for it, the rest as the core has
 * them. The person turns them on for a device they trust. */
void devicectl_new_device(NSString *fingerprint);

#endif
