// SPDX-License-Identifier: MIT
//
// ruxnpvg's task controls over ruxnwddm's escapes (pvg_kmt.c).

#ifndef PVG_KMT_H
#define PVG_KMT_H

#include <stdbool.h>

struct ruxnwddm_escape_pvg_info;

typedef void (*pvg_kmt_log_fn)(const char *line);

// Finds our adapter and makes the kernel device whose task this process
// uses. Once per process; true if already done.
bool pvg_kmt_open(pvg_kmt_log_fn log);
// One task control: in_size bytes in, up to out_size bytes out. *status is
// the driver's NTSTATUS for it; false unless it is 0.
bool pvg_kmt_control(DWORD code, const void *in, DWORD in_size, void *out, DWORD out_size,
		     LONG *status);
// A log line for the driver to keep (klog.c), once the adapter is found.
// False when the driver keeps none: the caller then stops sending.
bool pvg_kmt_log(const char *text, unsigned length);
// The driver's counters of tasks, surfaces, display and flips.
bool pvg_kmt_info(struct ruxnwddm_escape_pvg_info *info);

#endif
