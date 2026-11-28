// SPDX-License-Identifier: GPL-2.0-only OR MIT
//
// A log kept in the kernel for processes that cannot write one: Edge's GPU
// process runs in a sandbox that may not open C:\ruxnlog, so its winemetal
// lines (and its refused command buffers) were lost (run rn71, where it was
// most likely the process whose command buffers faulted the GPU). Any
// process may still send escapes to our adapter, so such a process sends
// each line here (RUXNWDDM_ESCAPE_LOG), and wddmtool reads them back
// (RUXNWDDM_ESCAPE_LOG_READ, administrators only).
//
// Off unless Parameters\SandboxLog is 1 when the first line comes, as a lab
// switch: no snapshot has it.
//
// The driver adds its own lines too (ruxnwddm_klog_event): each ring command
// and each mapper command, with a sequence number, the process and the
// device's answer, so that what was unmapped, freed or deleted before a
// GPU fault is on the disk in order (wddmtool klogfollow). The last KLOG_BYTES bytes are kept, each line
// with its process id in front; a reader names the offset it has read to
// and gets what came after it, or the oldest kept if it fell behind.

#include "pvg.h"

#define POOL_TAG_KLOG	'lkur'
#define KLOG_BYTES	(4u << 20)

enum { KLOG_UNKNOWN, KLOG_DECIDING, KLOG_ON, KLOG_OFF };

static LONG state = KLOG_UNKNOWN;
static char *ring;
static ULONG64 total;	// bytes ever written; ring[total % KLOG_BYTES] is next
// A mutex, not a spin lock: the escape's data may be paged.
static FAST_MUTEX lock;

// STATUS_SUCCESS when on, STATUS_NOT_SUPPORTED when off, STATUS_RETRY
// while the first caller decides (that line is dropped, the sender goes on).
static NTSTATUS klog_on(void)
{
	LONG now = InterlockedCompareExchange(&state, KLOG_DECIDING, KLOG_UNKNOWN);

	if (now == KLOG_ON)
		return STATUS_SUCCESS;
	if (now == KLOG_OFF)
		return STATUS_NOT_SUPPORTED;
	if (now == KLOG_DECIDING)
		return STATUS_RETRY;
	ExInitializeFastMutex(&lock);
	if (ruxnwddm_record_read(L"SandboxLog") == 1)
		ring = ExAllocatePool2(POOL_FLAG_NON_PAGED, KLOG_BYTES, POOL_TAG_KLOG);
	InterlockedExchange(&state, ring ? KLOG_ON : KLOG_OFF);
	return ring ? STATUS_SUCCESS : STATUS_NOT_SUPPORTED;
}

static void put(const char *bytes, ULONG length)
{
	ULONG i;

	for (i = 0; i < length; i++)
		ring[(total + i) % KLOG_BYTES] = bytes[i];
	total += length;
}

// STATUS_NOT_SUPPORTED when the log is off, so that the sender stops.
NTSTATUS ruxnwddm_klog_line(struct ruxnwddm_escape_log *line)
{
	char head[16];
	ULONG pid = (ULONG)(ULONG_PTR)PsGetCurrentProcessId(), length = line->length, n = 0, i;
	NTSTATUS status = klog_on();

	if (!NT_SUCCESS(status) || status == STATUS_RETRY)
		return status;
	if (length > sizeof(line->text))
		length = sizeof(line->text);
	// "[pid] ", the digits written backwards first.
	head[n++] = '[';
	{
		char digits[10];
		ULONG d = 0;

		do {
			digits[d++] = (char)('0' + pid % 10);
			pid /= 10;
		} while (pid && d < sizeof(digits));
		while (d)
			head[n++] = digits[--d];
	}
	head[n++] = ']';
	head[n++] = ' ';
	for (i = 0; i < length; i++) {
		if (line->text[i] == '\n' || line->text[i] == '\r' || !line->text[i])
			break;
	}
	ExAcquireFastMutex(&lock);
	put(head, n);
	put(line->text, i);
	put("\n", 1);
	ExReleaseFastMutex(&lock);
	return STATUS_SUCCESS;
}

NTSTATUS ruxnwddm_klog_read(struct ruxnwddm_escape_log_read *read)
{
	ULONG64 from = read->from, oldest;
	ULONG length, i;
	NTSTATUS status = klog_on();

	if (!NT_SUCCESS(status) || status == STATUS_RETRY)
		return status;
	ExAcquireFastMutex(&lock);
	oldest = total > KLOG_BYTES ? total - KLOG_BYTES : 0;
	if (from < oldest || from > total)
		from = oldest;
	length = (ULONG)min(total - from, (ULONG64)sizeof(read->text));
	for (i = 0; i < length; i++)
		read->text[i] = ring[(from + i) % KLOG_BYTES];
	read->from = from;
	read->length = length;
	read->total = total;
	ExReleaseFastMutex(&lock);
	return STATUS_SUCCESS;
}

static void put_hex(char *line, ULONG *n, ULONG64 value)
{
	static const char digits[] = "0123456789abcdef";
	char backwards[16];
	ULONG d = 0;

	do {
		backwards[d++] = digits[value & 15];
		value >>= 4;
	} while (value && d < sizeof(backwards));
	while (d)
		line[(*n)++] = backwards[--d];
	line[(*n)++] = ' ';
}

// One line of the driver's own: "[pid] #seq what a b c status", numbers in
// hex. Only at PASSIVE or APC level, and only while the log is on.
void ruxnwddm_klog_event(const char *what, ULONG64 a, ULONG64 b, ULONG64 c, LONG status)
{
	static LONG64 sequence;
	struct ruxnwddm_escape_log *line;
	ULONG n = 0;

	KIRQL irql = KeGetCurrentIrql();

	if (irql > APC_LEVEL || state == KLOG_OFF || state == KLOG_DECIDING)
		return;
	// The registry is read at PASSIVE level only.
	if (state == KLOG_UNKNOWN && (irql != PASSIVE_LEVEL || klog_on() != STATUS_SUCCESS))
		return;
	line = ExAllocatePool2(POOL_FLAG_NON_PAGED, sizeof(*line), POOL_TAG_KLOG);
	if (!line)
		return;
	line->text[n++] = '#';
	put_hex(line->text, &n, (ULONG64)InterlockedIncrement64(&sequence));
	while (*what && n < 64)
		line->text[n++] = *what++;
	line->text[n++] = ' ';
	put_hex(line->text, &n, a);
	put_hex(line->text, &n, b);
	put_hex(line->text, &n, c);
	put_hex(line->text, &n, (ULONG)status);
	line->length = n;
	ruxnwddm_klog_line(line);
	ExFreePoolWithTag(line, POOL_TAG_KLOG);
}
