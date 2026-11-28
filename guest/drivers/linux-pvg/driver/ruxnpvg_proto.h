/* SPDX-License-Identifier: MIT */
/*
 * ruxnpvg_proto.h: the protocol of Apple's paravirtualized graphics device
 * (PCI 106b:eeee) and of its surface mapper, as the guest drivers speak it.
 *
 * Data only: numbers and the layouts of words, no types, no code and no
 * includes, so that a kernel driver of either system, a firmware driver and a
 * program can include it alike. It compiles as C99, as C++ and with
 * clang-cl's target.
 *
 * Nothing here is documented by Apple. Every number was seen from outside, on
 * macOS 27.2.1 build 26B131, protocol version 252.
 *
 * Words are 32 bits, little-endian. "A word index" is an offset divided by
 * four. Pages are the device's: 16 KiB, whatever the guest's own page size.
 *
 * Where a number is a choice the drivers made and not the device's, the
 * comment says "the drivers' choice". A driver may choose otherwise; the
 * device takes more than these.
 */
#ifndef RUXNPVG_PROTO_H
#define RUXNPVG_PROTO_H

/* ---- The PCI device ---------------------------------------------------- */

#define RUXNPVG_PCI_VENDOR		0x106bu
#define RUXNPVG_PCI_DEVICE		0xeeeeu

/* BAR 0: the graphics device's registers, 32 bits each. */
#define RUXNPVG_BAR_REGS		0
#define RUXNPVG_REGS_BYTES		0x4000u
/* BAR 2: the surface mapper's registers, 64 bits each. */
#define RUXNPVG_BAR_MAPPER		2

/*
 * Two MSI messages: 0 for the graphics device (every ring, every display
 * event, every fault), 1 for the mapper (once for each command it has done).
 */
#define RUXNPVG_MSI_MESSAGES		2
#define RUXNPVG_MSI_GRAPHICS		0
#define RUXNPVG_MSI_MAPPER		1

/* ---- Pages ------------------------------------------------------------- */

/* The device counts guest memory in pages of 16 KiB, by 32-bit numbers. */
#define RUXNPVG_PAGE_SHIFT		14
#define RUXNPVG_PAGE_BYTES		(1u << RUXNPVG_PAGE_SHIFT)
/*
 * How many bits of a page's number each place has room for, so the guest
 * memory it can name lies below 2^(bits + 14): a register or a command
 * names 32 bits, a task's table entry 31 (bit 31 is "writable"), the
 * mapper's page word 30 (shifted left by two).
 */
#define RUXNPVG_PAGE_NUMBER_BITS	32
#define RUXNPVG_TASK_PAGE_NUMBER_BITS	31
#define RUXNPVG_MAPPER_PAGE_NUMBER_BITS	30

/* ---- Registers of BAR 0 ------------------------------------------------ */

#define RUXNPVG_REG_RING_ON		0x1000u	/* 1 turns the first ring on at position 0, 0 off: see the ring */
#define RUXNPVG_REG_RING_BYTES		0x1004u	/* the first ring's length in bytes */
#define RUXNPVG_REG_RING_WRITTEN	0x1008u	/* how far the guest has written; writing it is the kick */
#define RUXNPVG_REG_RING_READ		0x100cu	/* how far the device has read */
#define RUXNPVG_REG_RING_COMMANDS	0x1010u	/* where in the ring's memory commands start */
#define RUXNPVG_REG_DISPLAY_EVENTS	0x1014u	/* a bit for each display; reads once */
#define RUXNPVG_REG_DONE		0x1018u	/* bit n: a stamp came into cell n; reads once */
#define RUXNPVG_REG_ROOT_PAGE		0x101cu	/* the device's root page; set before the ring is on */
#define RUXNPVG_REG_CHILD_WAKE		0x1020u	/* write a child ring's id: new commands there */
#define RUXNPVG_REG_RING_RESUME		0x1024u	/* write: the first ring goes on after a refused command */
#define RUXNPVG_REG_CHILD_RESUME	0x1028u	/* write a child ring's id: it goes on after a refusal */
#define RUXNPVG_REG_FAULT		0x102cu	/* reads 1 while any ring holds a refused command */
#define RUXNPVG_REG_RING_PAGE		0x1030u	/* the first page of the first ring's memory */
#define RUXNPVG_REG_VERSION		0x1034u	/* the version the guest speaks; reads what the device made of it */
#define RUXNPVG_REG_RING_REFUSED	0x103cu	/* where the first ring's refused command lies, or all ones */

/*
 * The early framebuffer's bank: a picture from one contiguous block of guest
 * memory, with no ring and no command. Until RUXNPVG_REG_EFB_ON reads 1 the
 * others read 0 and keep nothing; turning it off forgets them.
 */
#define RUXNPVG_REG_EFB_ON		0x1200u	/* 1 on, 0 off */
#define RUXNPVG_REG_EFB_MODE_COUNT	0x1204u	/* how many modes the display lists */
#define RUXNPVG_REG_EFB_MODE_INDEX	0x1208u	/* write: choose a mode to read at the next */
#define RUXNPVG_REG_EFB_MODE_SIZE	0x120cu	/* that mode, as a mode word, or all ones past the last */
#define RUXNPVG_REG_EFB_PAGE		0x1210u	/* the block's first page */
#define RUXNPVG_REG_EFB_BYTES		0x1214u	/* the block's length, whole pages */
#define RUXNPVG_REG_EFB_DEPTH		0x1218u	/* the bits of a pixel */
#define RUXNPVG_REG_EFB_MODE		0x121cu	/* a mode word; the picture shows after this write */
#define RUXNPVG_REG_EFB_ROW_ALIGN	0x1224u	/* reads 0x10 once on: a row's alignment, by the names */
#define RUXNPVG_EFB_DEPTH		32
#define RUXNPVG_EFB_MODE_NONE		0xffffffffu

/* A write of a display's port: the device looks at the cursor's words. */
#define RUXNPVG_REG_DISPLAY_LOOK	0x1220u

/* A mode word: the height above the width, 16 bits each. */
#define RUXNPVG_MODE_WORD(width, height)	(((height) << 16) | ((width) & 0xffffu))
#define RUXNPVG_MODE_WIDTH(word)	((word) & 0xffffu)
#define RUXNPVG_MODE_HEIGHT(word)	((word) >> 16)

/* ---- The version ------------------------------------------------------- */

/*
 * The newest version the device takes, and the one everything here was seen
 * at. Nothing was taken away between 10 and 252; ten commands came later.
 */
#define RUXNPVG_VERSION			252

/* ---- The device's root page ------------------------------------------- */

/*
 * The page RUXNPVG_REG_ROOT_PAGE names holds three tables, as byte offsets.
 * The device writes the first and the third. When the first ring is turned
 * off the device clears the display's word and each child ring's defined bit,
 * and leaves the tasks' words, although it has forgotten the tasks too: a
 * task's word is no sign the device knows it (seen on QEMU's device,
 * 2026-11-26).
 */
#define RUXNPVG_ROOT_TASK_ROOTS		0x000u	/* a word for each task id: its root page's number */
#define RUXNPVG_ROOT_CHILD_DESCRIPTORS	0x400u	/* child ring 1's descriptor, then one for each id */
#define RUXNPVG_ROOT_STATE_PAGES	0x900u	/* a word for each display port: its state page's number */

/* ---- Rings and commands ------------------------------------------------ */

/*
 * A command: three words, its number, the length of the whole in bytes and
 * a stamp, then its payload. The device writes a command's stamp into the
 * ring's stamp cell when it has done the command, sets the cell's bit in
 * RUXNPVG_REG_DONE and raises message 0. A command that hands the GPU work
 * is stamped when the work is done.
 *
 * The ring's contract, for every driver: a command is done when the stamp
 * cell (or a ring's read position) says so. Read the cell or the done count,
 * recheck on a timer, and never count interrupts. Interrupts can merge (one
 * fewer than commands was seen on Windows) and can come late (one in 1,000
 * to 3,000 on Linux under QEMU's default for HVF, 86 to 2,122 in 70,000 on
 * Windows on an M4 Mac). Seen on both systems.
 */
#define RUXNPVG_COMMAND_HEADER_BYTES	12u
#define RUXNPVG_COMMAND_NUMBER		0	/* word indices of the header */
#define RUXNPVG_COMMAND_LENGTH		1
#define RUXNPVG_COMMAND_STAMP		2
#define RUXNPVG_COMMAND_PAYLOAD		3
/*
 * How often the drivers look at the cells with no interrupt: the drivers'
 * choice. The Linux driver's sleeping sender looks every millisecond (its
 * parameter recheck_us), its child rings' look with no waiter every 20 ms.
 */
#define RUXNPVG_RECHECK_MS		20

/*
 * Stamp cells: a word for each ring at the start of the first ring's
 * memory. Cell 0 is the first ring's; a child ring's is the index in its
 * descriptor. Commands start after them.
 */
#define RUXNPVG_STAMP_CELLS		64
#define RUXNPVG_RING_LEAST_COMMANDS_AT	(RUXNPVG_STAMP_CELLS * 4u)

/*
 * The first ring: one block of guest memory. Positions are 32-bit byte
 * counts that run on past the ring's end; the place is the count modulo the
 * room. The device takes a written position exactly one room ahead of the
 * read one.
 *
 * The room must divide 2^32, so that the place goes on without a jump where
 * the counts wrap; then no driver turns the ring off and on to rewind it.
 * Seen 2026-11-26 on QEMU's device and in the Guesthouse process with a room
 * of 2^16: both counts pass 2^32, a command lying across the wrap included,
 * with tasks, child rings, the display and the mapper live. The guard
 * refuses a position that wraps when the room does not divide 2^32.
 *
 * Turning the ring off (and on) puts both positions back to 0, and the
 * device forgets every task (the kernel task too), every child ring and the
 * display's state page, while a driver still thinks them there: every later
 * command 67 and swap refused, a child ring's jobs never stamped. It clears
 * words of the root page, see there. Seen on QEMU's device and in the
 * Guesthouse process, 2026-11-26; the guard forgets the same. Turn the ring
 * off only with nothing defined, or define everything anew after.
 */
#define RUXNPVG_RING_BYTES		0x14000u	/* the drivers' choice */
#define RUXNPVG_RING_COMMANDS_AT	0x4000u		/* the drivers' choice */
#define RUXNPVG_RING_ROOM		(RUXNPVG_RING_BYTES - RUXNPVG_RING_COMMANDS_AT)	/* 2^16 */
/*
 * The longest payload a child ring takes, and the Windows drivers' first
 * ring: the drivers' choice. The Linux driver's first ring takes a command
 * up to its room.
 */
#define RUXNPVG_RING_MAX_WORDS		2048
/* RUXNPVG_REG_RING_REFUSED and a descriptor's refused word with no refusal. */
#define RUXNPVG_NOT_REFUSED		0xffffffffu

/*
 * Child rings: a ring of its own for a task, 64 KiB in four pages that a
 * page of the guest's lists. Its positions lie in a descriptor of five words
 * in the root page, at RUXNPVG_ROOT_CHILD_DESCRIPTORS + 20 * (id - 1).
 */
#define RUXNPVG_CHILD_FIRST_ID		1
#define RUXNPVG_CHILD_LAST_ID		63	/* 31 up to version 60 */
#define RUXNPVG_CHILD_BYTES		0x10000u
#define RUXNPVG_CHILD_PAGES		4
#define RUXNPVG_CHILD_DESCRIPTOR_BYTES	20u
#define RUXNPVG_CHILD_WRITTEN		0	/* word indices: the guest's */
#define RUXNPVG_CHILD_READ		1	/* the device's; 0 when defined */
#define RUXNPVG_CHILD_REFUSED		2	/* the device's: where a refused command lies */
#define RUXNPVG_CHILD_CELL		3	/* the stamp cell's index, below 64 */
#define RUXNPVG_CHILD_PAGE_LIST		4	/* the page that lists the ring's pages */
#define RUXNPVG_CHILD_DEFINED		(1u << 16)	/* in the cell word, set by the device */

/*
 * Command numbers of the first ring, and the payload in bytes each driver
 * sends. Where a driver sends a payload of its own length, the macro takes
 * the counts.
 */
#define RUXNPVG_CMD_NOP_WORD		0	/* nothing seen; wants 4 bytes or more */
#define RUXNPVG_CMD_NOP			30	/* nothing, with any payload */

/* A display's: the port first. */
#define RUXNPVG_CMD_DISPLAY_STATE_PAGE	1	/* port, page number or 0 to take it back */
#define RUXNPVG_CMD_DISPLAY_STATE_PAGE_BYTES	8u
#define RUXNPVG_CMD_DISPLAY_ACK		2	/* port, the mode list's sequence number */
#define RUXNPVG_CMD_DISPLAY_ACK_BYTES	8u
#define RUXNPVG_CMD_CURSOR_GLYPH	4	/* see the glyph's words below */
#define RUXNPVG_CMD_CURSOR_GLYPH_BYTES	44u
#define RUXNPVG_CMD_CURSOR_SHOW		5	/* port, 1 or 0 */
#define RUXNPVG_CMD_CURSOR_SHOW_BYTES	8u
#define RUXNPVG_CMD_DISPLAY_SWAP	8	/* port, task 0, mapping id in 64 bits, six zero words */
#define RUXNPVG_CMD_DISPLAY_SWAP_BYTES	40u

/* A task's. */
#define RUXNPVG_CMD_TASK_DELETE		32	/* the id, not shifted */
#define RUXNPVG_CMD_TASK_DELETE_BYTES	4u
#define RUXNPVG_CMD_TASK_UNMAP		34	/* task, address and length in it, 64 bits each */
#define RUXNPVG_CMD_TASK_UNMAP_BYTES	20u
#define RUXNPVG_CMD_DELETE_RESOURCE	37	/* task, object id */
#define RUXNPVG_CMD_DELETE_RESOURCE_BYTES	8u
#define RUXNPVG_CMD_DELETE_OBJECT	40	/* task, delete operation, 12, object id */
#define RUXNPVG_CMD_DELETE_OBJECT_BYTES	16u
#define RUXNPVG_CMD_CHILD_DEFINE	48	/* the ring's id */
#define RUXNPVG_CMD_CHILD_DEFINE_BYTES	4u
#define RUXNPVG_CMD_CHILD_DELETE	49	/* the ring's id */
#define RUXNPVG_CMD_CHILD_DELETE_BYTES	4u
#define RUXNPVG_CMD_SYNC		53	/* task, count, an object id for each */
#define RUXNPVG_CMD_SYNC_BYTES(count)	(8u + 4u * (count))
#define RUXNPVG_CMD_EXEC_OLDER		55	/* as 67, with resources of 24 bytes */
#define RUXNPVG_CMD_EXEC_OLDER_BYTES(resources, buffers) \
	(12u + 24u * (resources) + 16u * (buffers))
#define RUXNPVG_CMD_TASK_DEFINE		56	/* id << 1, length in 64 bits, root page */
#define RUXNPVG_CMD_TASK_DEFINE_BYTES	16u
#define RUXNPVG_CMD_DEVICE_INFO		58	/* key limit, pairs, page: no driver sends it */
#define RUXNPVG_CMD_COMPUTE_INFO	59	/* no driver sends it */
#define RUXNPVG_CMD_EXEC		67	/* task, resource count, buffer count, then each */
#define RUXNPVG_CMD_EXEC_BYTES(resources, buffers) \
	(12u + 8u * (resources) + 16u * (buffers))
#define RUXNPVG_CMD_SET_LISTS		68	/* no driver sends it */

/*
 * Commands 8, 10 and 12, refused, are stamped with no fault. Every other
 * refused command faults its ring: RUXNPVG_REG_FAULT reads 1, the ring
 * reads on and holds later stamps back until it is resumed, and the resume
 * skips the refused command.
 */

/* ---- Tasks ------------------------------------------------------------- */

/*
 * A task is an address space of the guest's for the device to read through:
 * a root page, a page table, and a length. Task 0 is the kernel task, which
 * display commands name and which the device keeps in the host's process.
 * Every other task is a user task, kept in a helper process.
 */
#define RUXNPVG_KERNEL_TASK		0
#define RUXNPVG_TASK_FIRST_ID		1
#define RUXNPVG_TASK_LAST_ID		255
/* A task of this length or more is refused. */
#define RUXNPVG_TASK_TOO_LONG		(64ull << 30)

/* Words of a task's root page. 64-bit values are two words, low first. */
#define RUXNPVG_TASK_ROOT_TABLE		0	/* the table's top page */
#define RUXNPVG_TASK_ROOT_DEPTH		1	/* 1, 2 or 3 */
#define RUXNPVG_TASK_ROOT_OBJECTS	2	/* the object list's address in the task, 64 bits, on a page */
#define RUXNPVG_TASK_ROOT_OBJECTS_BYTES	4
#define RUXNPVG_TASK_ROOT_LENGTH	5	/* the task's length, 64 bits: the device writes it */
#define RUXNPVG_TASK_ROOT_PLACEMENTS	7	/* the placement list's address, 64 bits */
#define RUXNPVG_TASK_ROOT_PLACEMENTS_BYTES	9

/*
 * The page table: a page holds 4096 entries, so each level takes 12 bits of
 * the address above the 14 of a page. With depth 1 a task is at most
 * 64 MiB. An entry is a page's number; bit 31 says the host may write the
 * page. There is no valid bit: an entry of 0 fails the mapping.
 */
#define RUXNPVG_TABLE_SHIFT		12
#define RUXNPVG_TABLE_ENTRIES		(1u << RUXNPVG_TABLE_SHIFT)
#define RUXNPVG_ENTRY_WRITABLE		(1u << 31)

/*
 * The object list: 12 bytes for each object id, at 12 * id. The first word
 * is the kind in its low byte and the description's length above it; then
 * the description's address in the task, 64 bits. Id 0 is never an object.
 * The placement list has 16 bytes for each id, and the device takes the pair
 * only if both have room for the same count. The host writes it.
 */
#define RUXNPVG_OBJECT_ENTRY_BYTES	12u
#define RUXNPVG_OBJECT_ENTRY_WORD(kind, bytes)	((kind) | ((bytes) << 8))
#define RUXNPVG_PLACEMENT_ENTRY_BYTES	16u

/*
 * Object kinds, the device's own numbers. A resource can be named in a
 * submit's resource list; a state object (kind 7) must not be.
 *
 * Kind 11 is a texture over a mapping of the surface mapper. It may be
 * named as a resource that is read, and sampled in a shader: one task draws
 * into a surface, another samples it. Seen on Windows on an M4 Mac and on
 * Linux on an M5 Mac, 2026-11-26.
 *
 * Kinds 13 (a heap over the guest's pages) and 14 are not sent: a try of
 * 13 reached the Mac's kernel GPU driver.
 */
#define RUXNPVG_OBJECT_BUFFER		1	/* 16 bytes: length 64 bits, page in the task, flags */
#define RUXNPVG_OBJECT_TEXTURE		2
#define RUXNPVG_OBJECT_FUNCTION		6
#define RUXNPVG_OBJECT_STATE		7	/* a sampler, depth and stencil state, a pipeline */
#define RUXNPVG_OBJECT_TEXTURE_VIEW	8
#define RUXNPVG_OBJECT_SURFACE_TEXTURE	11
#define RUXNPVG_OBJECT_SURFACE_BUFFER	15	/* a buffer over a mapping: no driver makes it */
/* A buffer's flag: its memory is the host's, and the guest's stays as it was. */
#define RUXNPVG_BUFFER_PRIVATE		(1u << 1)

/*
 * Commands 67 and 55 name their resources. In 67 a resource is two words,
 * its id and its flags; in 55 six, its id, a word whose bit 8 says the same
 * as the flag below, and four zero words. Then a buffer is its address in
 * the task and its length, 64 bits each.
 */
#define RUXNPVG_RESOURCE_WORDS		2
#define RUXNPVG_RESOURCE_OLDER_WORDS	6
#define RUXNPVG_RESOURCE_WRITTEN	(1u << 1)	/* the buffers write it */
#define RUXNPVG_RESOURCE_OLDER_WRITTEN	(1u << 8)
#define RUXNPVG_BUFFER_WORDS		4

/*
 * The serializer's operations, as a description's tail holds them, and the
 * delete operations command 40 carries (each 12 bytes: number, 12, id).
 * Segments of a command buffer: render 0, compute 1, blit 2; segment kind 5
 * is not sent.
 */
#define RUXNPVG_OP_SAMPLER		3
#define RUXNPVG_OP_DEPTH_STENCIL	4
#define RUXNPVG_OP_TEXTURE_VIEW		7
#define RUXNPVG_OP_TEXTURE_VIEW_RANGE	8
#define RUXNPVG_OP_COMPUTE_PIPELINE	11
#define RUXNPVG_OP_SURFACE_TEXTURE	12	/* 48 bytes; width at byte 16, height at 20 */
#define RUXNPVG_OP_SURFACE_TEXTURE_BYTES	48u
#define RUXNPVG_OP_SURFACE_TEXTURE_WIDTH	16
#define RUXNPVG_OP_SURFACE_TEXTURE_HEIGHT	20
#define RUXNPVG_OP_RENDER_PIPELINE	14
#define RUXNPVG_OP_FUNCTION		15
#define RUXNPVG_OP_TEXTURE_VIEW_SWIZZLE	27
#define RUXNPVG_OP_VIEW_BASE		12	/* byte of a view's operation naming its texture */
#define RUXNPVG_OP_DELETE_BUFFER	1000
#define RUXNPVG_OP_DELETE_TEXTURE	1001
#define RUXNPVG_OP_DELETE_DEPTH_STENCIL	1002
#define RUXNPVG_OP_DELETE_SAMPLER	1003
#define RUXNPVG_OP_DELETE_FUNCTION	1005
#define RUXNPVG_OP_DELETE_COMPUTE_PIPELINE	1006
#define RUXNPVG_OP_DELETE_RENDER_PIPELINE	1007
#define RUXNPVG_OP_DELETE_BYTES		12u
#define RUXNPVG_SEGMENT_RENDER		0
#define RUXNPVG_SEGMENT_COMPUTE		1
#define RUXNPVG_SEGMENT_BLIT		2

/*
 * The layout of a user task both drivers use: the drivers' choice. A task
 * of 1 GiB with a table of two levels; the object list in page 0, the
 * placement list in page 1, 16 pages of descriptions from page 2 in pieces
 * of 64 bytes, buffer objects from page 128 (2 MiB) on.
 */
#define RUXNPVG_TASK_BYTES		(1ull << 30)
#define RUXNPVG_TASK_DEPTH		2
#define RUXNPVG_TASK_OBJECTS_PAGE	0
#define RUXNPVG_TASK_PLACEMENTS_PAGE	1
#define RUXNPVG_TASK_DESCRIPTIONS_PAGE	2
#define RUXNPVG_TASK_DESCRIPTION_PAGES	16
#define RUXNPVG_TASK_FIRST_BO_PAGE	128
#define RUXNPVG_DESCRIPTION_SLOT	64
#define RUXNPVG_MAX_OBJECTS		1024
#define RUXNPVG_MAX_DESCRIPTION		0x4000
#define RUXNPVG_MAX_HEAD		16
#define RUXNPVG_MAX_RESOURCES		256
#define RUXNPVG_MAX_BUFFERS		16
/* The kernel task: 1 MiB, one level, the cursor's picture at address 0. */
#define RUXNPVG_KERNEL_TASK_BYTES	0x100000u
#define RUXNPVG_KERNEL_TASK_DEPTH	1
#define RUXNPVG_GLYPH_ADDRESS		0

/* ---- A display --------------------------------------------------------- */

/* Both drivers use port 0, the only one the guard passes. */
#define RUXNPVG_DISPLAY_PORT		0

/*
 * The state page command 1 hands a display, as byte offsets. The device
 * writes the modes and the events, the guest the mask and the cursor. The
 * guest hands the page over zeroed.
 */
#define RUXNPVG_STATE_EVENTS		0x100u	/* set by the device, cleared by the guest */
#define RUXNPVG_STATE_EVENT_MASK	0x104u	/* the events that raise an interrupt */
#define RUXNPVG_STATE_SEQUENCE		0x200u	/* the mode list's number, for the acknowledgement */
#define RUXNPVG_STATE_MODE_COUNT	0x208u	/* 16 bits */
#define RUXNPVG_STATE_MODES		0x210u	/* 16 bytes each: a mode word, then 16.16 hertz */
#define RUXNPVG_STATE_MODE_BYTES	16u
#define RUXNPVG_STATE_CURSOR_PLACE	0xe00u	/* x below y, 16 bits each */
#define RUXNPVG_STATE_CURSOR_SHOWN	0xe04u
#define RUXNPVG_STATE_LIST_STATE	0xfc0u
#define RUXNPVG_LIST_WAITING		1	/* a new list waits for its acknowledgement */
#define RUXNPVG_LIST_ACKNOWLEDGED	2
/* A mode count above this ends the host's process. */
#define RUXNPVG_STATE_MOST_MODES	128

/* Event bits. */
#define RUXNPVG_EVENT_SWAP_DONE		(1u << 1)
#define RUXNPVG_EVENT_PAGE_WRITTEN	(1u << 2)	/* at hand-over and with every new list */
#define RUXNPVG_EVENT_MODES		(1u << 3)

/*
 * Command 4's payload, as word indices: port, task 0, the glyph's address
 * and the bytes it takes, the stride (64 bits each), the size as a mode
 * word, the hot spot as one (y above x), and the sum of the glyph's words.
 */
#define RUXNPVG_GLYPH_WORD_PORT		0
#define RUXNPVG_GLYPH_WORD_TASK		1
#define RUXNPVG_GLYPH_WORD_AT		2
#define RUXNPVG_GLYPH_WORD_BYTES	4
#define RUXNPVG_GLYPH_WORD_STRIDE	6
#define RUXNPVG_GLYPH_WORD_SIZE		8
#define RUXNPVG_GLYPH_WORD_HOT		9
#define RUXNPVG_GLYPH_WORD_SUM		10
#define RUXNPVG_CURSOR_MAX_SIDE		256	/* a wider or higher picture is refused */
#define RUXNPVG_GLYPH_MOST_BYTES	(RUXNPVG_CURSOR_MAX_SIDE * RUXNPVG_CURSOR_MAX_SIDE * 4u)

/* ---- The surface mapper (BAR 2) ---------------------------------------- */

/* Registers, 64 bits each. Addresses are guest-physical, in bytes. */
#define RUXNPVG_MAP_REG_RING		0x1000u	/* the ring's address */
#define RUXNPVG_MAP_REG_SLOTS		0x1008u	/* its length in commands, a multiple of 1024 */
#define RUXNPVG_MAP_REG_TABLE		0x1010u	/* the table of description pages */
#define RUXNPVG_MAP_REG_WRITTEN		0x1018u	/* commands written; writing it is the kick */
#define RUXNPVG_MAP_REG_DONE		0x1020u	/* commands done */
#define RUXNPVG_MAP_REG_LAST		0x1028u	/* reads 1 */
#define RUXNPVG_MAP_REGS_BYTES		0x10000u

/*
 * A command: 16 bytes, its number, a mapping id, and a word the device
 * writes the outcome to. Any ring length other than a multiple of 1024
 * commands ends the host's process, so both drivers use one page.
 */
#define RUXNPVG_MAP_COMMAND_BYTES	16u
#define RUXNPVG_MAP_COMMAND_NUMBER	0	/* word indices */
#define RUXNPVG_MAP_COMMAND_ID		1
#define RUXNPVG_MAP_COMMAND_OUTCOME	2
#define RUXNPVG_MAP_SLOTS		(RUXNPVG_PAGE_BYTES / RUXNPVG_MAP_COMMAND_BYTES)
#define RUXNPVG_MAP_CMD_MAP		1
#define RUXNPVG_MAP_CMD_UNMAP		2
/* Outcomes: 0, or an error of the host's. */
#define RUXNPVG_MAP_OUTCOME_DONE	0u
#define RUXNPVG_MAP_OUTCOME_BUSY	0xe00002d5u	/* unmap of a mapping a display still shows */

/* Mapping ids run from 1 to 8191; the table names a page of 32 descriptions. */
#define RUXNPVG_MAP_FIRST_ID		1
#define RUXNPVG_MAP_LAST_ID		8191
#define RUXNPVG_MAP_IDS_PER_PAGE	32
#define RUXNPVG_MAP_DESCRIPTION_BYTES	0x200u

/* How the mapper names a page: its number shifted left by two, bit 0 for valid. */
#define RUXNPVG_MAP_PAGE_WORD(number)	(((number) << 2) | 1u)

/*
 * A surface's description, as word indices. A plain picture sets these and
 * leaves every other word zero. Width and height: an element's side (1) in
 * the low byte, the side in pixels above it.
 */
#define RUXNPVG_DESC_PAGE_LIST		0	/* the page listing the surface's pages, a page word */
#define RUXNPVG_DESC_FORMAT		1
#define RUXNPVG_DESC_BYTES		4	/* the surface's length, whole pages */
#define RUXNPVG_DESC_WIDTH		5
#define RUXNPVG_DESC_HEIGHT		6
#define RUXNPVG_DESC_PITCH		7	/* the bytes of a row */
#define RUXNPVG_DESC_PIXEL_BYTES	8
#define RUXNPVG_DESC_SIDE(pixels)	(((pixels) << 8) | 1u)
#define RUXNPVG_FORMAT_BGRA		0x42475241u	/* "BGRA" */
#define RUXNPVG_PIXEL_BYTES		4
/* A surface's pages are listed in one page. */
#define RUXNPVG_SURFACE_MAX_PAGES	(RUXNPVG_PAGE_BYTES / 4u)
/* The widest and highest surface both drivers make: the drivers' choice. */
#define RUXNPVG_MAX_SIDE		8192

/* ---- Waits both drivers use: the drivers' choice ----------------------- */

#define RUXNPVG_RING_TIMEOUT_MS		2000
#define RUXNPVG_MAP_TIMEOUT_MS		2000
#define RUXNPVG_TASK_TIMEOUT_MS		10000

#endif /* RUXNPVG_PROTO_H */
