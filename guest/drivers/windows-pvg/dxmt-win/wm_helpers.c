// SPDX-License-Identifier: MIT
//
// DXMT's helper functions, those this device has: DXMT asks its own helper
// library (dxmt_command.metal, which this build leaves out) for them by
// name. The ones written here as AIR (helpers/*.ll), each made into a
// library of one function by our own air2metallib at build time and
// carried in this DLL (helper_libraries.h, generated), are made into real
// functions; every other name stays a placeholder that fails when used.
//
// Notepad's tab row and every ClearView with rectangles draw with
// vs_clear_rt and fs_clear_rt_* (run rn16: its row stayed empty).

#include "wm_internal.h"
#include "helper_libraries.h"

// The function made from the helper library of this name, or 0 if this
// build has none. Under winemetal's lock.
uint32_t wm_helper_function(const char *name)
{
	static UCHAR description[PVG_WIRE_MOST_DESCRIPTION];
	static uint32_t made[HELPER_COUNT];
	struct memory m;
	uint32_t number;
	size_t length;
	unsigned i;

	for (i = 0; i < HELPER_COUNT; i++)
		if (!strcmp(helper_names[i], name))
			break;
	if (i == HELPER_COUNT)
		return 0;
	if (made[i])
		return made[i];
	if (!allocate(helper_sizes[i], WM_USE_HELPER, &m))
		return 0;
	memcpy(address(&m), helper_bytes[i], helper_sizes[i]);
	number = wm_take_number();
	length = pvg_wire_function(description, sizeof(description), number, 0, (uint32_t)helper_sizes[i]);
	if (!number || !length ||
	    !create_object(number, RUXNPVG_OBJECT_FUNCTION, &m, 0, description + PVG_WIRE_HEAD_FUNCTION,
			   (ULONG)(length - PVG_WIRE_HEAD_FUNCTION)))
		return 0;
	made[i] = number;
	pvg_log("helper %s: our own library of %u bytes, function %u", name, (unsigned)helper_sizes[i], number);
	return number;
}
