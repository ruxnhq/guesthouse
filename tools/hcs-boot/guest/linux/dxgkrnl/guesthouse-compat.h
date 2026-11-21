/* SPDX-License-Identifier: GPL-2.0-or-later */
/*
 * The VMBus channel GUIDs of GPU-PV, which Microsoft's WSL kernel adds to linux/hyperv.h and a
 * distro's kernel lacks. dxgkrnl's ID table names them. The global channel's GUID is the one a WSL
 * guest's VMBus lists for it.
 */
#ifndef HV_GPUP_DXGK_GLOBAL_GUID
#define HV_GPUP_DXGK_GLOBAL_GUID \
	.guid = GUID_INIT(0xdde9cbc0, 0x5060, 0x4436, 0x94, 0x48, 0xea, 0x12, 0x54, 0xa5, 0xd1, 0x77)
#endif
#ifndef HV_GPUP_DXGK_VGPU_GUID
#define HV_GPUP_DXGK_VGPU_GUID \
	.guid = GUID_INIT(0x6e382d18, 0x3336, 0x4f4b, 0xac, 0xc4, 0x2b, 0x77, 0x03, 0xd4, 0xdf, 0x4a)
#endif
