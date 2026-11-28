// SPDX-License-Identifier: GPL-2.0
//
// dispcfg: Windows' display topology, as the Display settings page sees it.
//
//   dispcfg list   every path Windows knows: its adapter (by device path),
//                  source and target, whether it is active, where it lies
//                  on the desktop and how large it is
//   dispcfg solo   makes the output of ruxnwddm's adapter the only active
//                  one, as "Show only on 2" would, so that the compositor
//                  composes the desktop for our output alone and not for
//                  the basic display adapter Windows keeps beside it (run
//                  rn1: its output had the desktop, ours none)
//   dispcfg save   makes our output the only active one, at 0,0, and saves
//                  that in Windows' display database, so every later boot
//                  starts with it: `DisplaySwitch /internal` was not kept
//                  over a boot (run rn15), and the basic display adapter
//                  cannot be disabled ("critical system device"), so this
//                  is how a boot comes up on our output alone
//
// Plain display settings through SetDisplayConfig; no driver is touched.
// Exit status 0 when all went well, 1 when not, 2 for a usage error.

#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <stdio.h>
#include <string.h>
#include <wchar.h>

#define MOST_PATHS	64
#define MOST_MODES	128
#define OUR_IDS		L"VEN_106B&DEV_EEEE"

static DISPLAYCONFIG_PATH_INFO paths[MOST_PATHS];
static DISPLAYCONFIG_MODE_INFO modes[MOST_MODES];

static BOOL adapter_name(LUID luid, WCHAR *out, size_t chars)
{
	DISPLAYCONFIG_ADAPTER_NAME name;

	memset(&name, 0, sizeof(name));
	name.header.type = DISPLAYCONFIG_DEVICE_INFO_GET_ADAPTER_NAME;
	name.header.size = sizeof(name);
	name.header.adapterId = luid;
	if (DisplayConfigGetDeviceInfo(&name.header) != ERROR_SUCCESS)
		return FALSE;
	wcsncpy(out, name.adapterDevicePath, chars - 1);
	out[chars - 1] = 0;
	return TRUE;
}

static BOOL is_ours(LUID luid)
{
	WCHAR name[256];

	return adapter_name(luid, name, 256) && wcsstr(_wcsupr(name), OUR_IDS) != NULL;
}

static LONG query(UINT32 flags, UINT32 *path_count, UINT32 *mode_count)
{
	LONG status = GetDisplayConfigBufferSizes(flags, path_count, mode_count);

	if (status != ERROR_SUCCESS)
		return status;
	if (*path_count > MOST_PATHS || *mode_count > MOST_MODES)
		return ERROR_INSUFFICIENT_BUFFER;
	return QueryDisplayConfig(flags, path_count, paths, mode_count, modes, NULL);
}

static int list(void)
{
	UINT32 path_count, mode_count, i;
	LONG status = query(QDC_ALL_PATHS, &path_count, &mode_count);

	printf("QueryDisplayConfig: %ld, %u paths, %u modes\n", status, path_count, mode_count);
	if (status != ERROR_SUCCESS)
		return 1;
	for (i = 0; i < path_count; i++) {
		const DISPLAYCONFIG_PATH_INFO *p = &paths[i];
		WCHAR name[256] = L"?";
		UINT32 m = p->sourceInfo.modeInfoIdx;

		adapter_name(p->sourceInfo.adapterId, name, 256);
		printf("path %u: %s source %u target %u available %d%s\n  adapter %ls\n", i,
		       (p->flags & DISPLAYCONFIG_PATH_ACTIVE) ? "ACTIVE" : "inactive", p->sourceInfo.id,
		       p->targetInfo.id, p->targetInfo.targetAvailable,
		       is_ours(p->sourceInfo.adapterId) ? " (ours)" : "", name);
		if ((p->flags & DISPLAYCONFIG_PATH_ACTIVE) && m < mode_count &&
		    modes[m].infoType == DISPLAYCONFIG_MODE_INFO_TYPE_SOURCE)
			printf("  at %ld,%ld, %ux%u\n", modes[m].sourceMode.position.x,
			       modes[m].sourceMode.position.y, modes[m].sourceMode.width,
			       modes[m].sourceMode.height);
	}
	return 0;
}

// Our adapter's first path with a target, alone and active; Windows picks
// its mode.
static int solo(void)
{
	UINT32 path_count, mode_count, i;
	DISPLAYCONFIG_PATH_INFO only;
	LONG status = query(QDC_ALL_PATHS, &path_count, &mode_count);

	if (status != ERROR_SUCCESS) {
		printf("QueryDisplayConfig: %ld\n", status);
		return 1;
	}
	for (i = 0; i < path_count; i++)
		if (is_ours(paths[i].sourceInfo.adapterId) && paths[i].targetInfo.targetAvailable)
			break;
	if (i == path_count) {
		printf("no path of our adapter with a target\n");
		return 1;
	}
	only = paths[i];
	only.flags |= DISPLAYCONFIG_PATH_ACTIVE;
	only.sourceInfo.modeInfoIdx = DISPLAYCONFIG_PATH_MODE_IDX_INVALID;
	only.targetInfo.modeInfoIdx = DISPLAYCONFIG_PATH_MODE_IDX_INVALID;
	status = SetDisplayConfig(1, &only, 0, NULL,
				  SDC_APPLY | SDC_TOPOLOGY_SUPPLIED | SDC_ALLOW_PATH_ORDER_CHANGES);
	printf("SetDisplayConfig (our path %u alone): %ld\n", i, status);
	return status == ERROR_SUCCESS ? 0 : 1;
}

// Our active path alone, its desktop at 0,0, with its own source and
// target modes, applied and saved in Windows' display database, so that
// the next boot starts with it.
static int save(void)
{
	DISPLAYCONFIG_MODE_INFO only_modes[2];
	DISPLAYCONFIG_PATH_INFO only;
	UINT32 path_count, mode_count, i;
	LONG status = query(QDC_ONLY_ACTIVE_PATHS, &path_count, &mode_count);

	if (status != ERROR_SUCCESS) {
		printf("QueryDisplayConfig: %ld\n", status);
		return 1;
	}
	for (i = 0; i < path_count; i++)
		if (is_ours(paths[i].sourceInfo.adapterId))
			break;
	if (i == path_count || paths[i].sourceInfo.modeInfoIdx >= mode_count ||
	    paths[i].targetInfo.modeInfoIdx >= mode_count) {
		printf("no active path of our adapter with its modes\n");
		return 1;
	}
	only = paths[i];
	only_modes[0] = modes[only.sourceInfo.modeInfoIdx];
	only_modes[1] = modes[only.targetInfo.modeInfoIdx];
	only_modes[0].sourceMode.position.x = 0;
	only_modes[0].sourceMode.position.y = 0;
	only.sourceInfo.modeInfoIdx = 0;
	only.targetInfo.modeInfoIdx = 1;
	status = SetDisplayConfig(1, &only, 2, only_modes,
				  SDC_APPLY | SDC_USE_SUPPLIED_DISPLAY_CONFIG | SDC_SAVE_TO_DATABASE |
					  SDC_ALLOW_CHANGES);
	printf("SetDisplayConfig (our path %u alone at 0,0, saved): %ld\n", i, status);
	return status == ERROR_SUCCESS ? 0 : 1;
}

int main(int argc, char **argv)
{
	if (argc == 2 && !strcmp(argv[1], "list"))
		return list();
	if (argc == 2 && !strcmp(argv[1], "solo")) {
		int status = solo();

		Sleep(3000);
		list();
		return status;
	}
	if (argc == 2 && !strcmp(argv[1], "save")) {
		int status = save();

		list();
		return status;
	}
	printf("usage: dispcfg list | solo | save\n");
	return 2;
}
