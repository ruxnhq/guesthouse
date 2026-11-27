/*
 * Opening and closing pairing on a running `rmhost serve`, from lines on its standard input.
 *
 * SPDX-License-Identifier: MIT
 * Copyright (c) 2026 RUXN LLC and contributors
 */
#ifndef PAIRCTL_H
#define PAIRCTL_H

#include "remoteman_core.h"

/* Starts reading commands from standard input and watching pairing. `already_open` says the
 * host was started with -p, which stays open with no time limit until a device pairs. The
 * control lives until the process ends. */
void pairctl_start(rm_host *host, int already_open);

#endif
