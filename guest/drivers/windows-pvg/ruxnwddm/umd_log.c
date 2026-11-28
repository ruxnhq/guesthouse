// SPDX-License-Identifier: MIT
//
// A log for ruxnumd.dll, for the lab only: when the process that loads the
// driver has RUXNUMD_LOG set to a file name, each step of the adapter and
// device appends a line there. umdtest sets it. Otherwise, if the lab's
// folder C:\ruxnlog exists (wddm.ps1 makes it), each process writes
// umd-PID.txt there, the compositor included.

#include "umd.h"

#define LOG_PATH_CHARS	260

static HANDLE log_file = INVALID_HANDLE_VALUE;
static BOOL log_tried;

static HANDLE open_log(void)
{
	WCHAR path[LOG_PATH_CHARS];

	if (log_tried)
		return log_file;
	log_tried = TRUE;
	if (!GetEnvironmentVariableW(L"RUXNUMD_LOG", path, LOG_PATH_CHARS)) {
		static const WCHAR digits[] = L"0123456789";
		static const WCHAR prefix[] = L"C:\\ruxnlog\\umd-";
		DWORD pid = GetCurrentProcessId(), n = 0, i;
		WCHAR number[12];

		for (i = 0; prefix[i]; i++)
			path[n++] = prefix[i];
		i = 0;
		do {
			number[i++] = digits[pid % 10];
			pid /= 10;
		} while (pid);
		while (i)
			path[n++] = number[--i];
		path[n++] = L'.';
		path[n++] = L't';
		path[n++] = L'x';
		path[n++] = L't';
		path[n] = 0;
		if (GetFileAttributesW(L"C:\\ruxnlog") == INVALID_FILE_ATTRIBUTES)
			return INVALID_HANDLE_VALUE;
	}
	log_file = CreateFileW(path, FILE_APPEND_DATA, FILE_SHARE_READ | FILE_SHARE_WRITE, NULL,
			       OPEN_ALWAYS, FILE_ATTRIBUTE_NORMAL, NULL);
	return log_file;
}

// "what value" as text, the value in hexadecimal.
void umd_log(const char *what, ULONG64 value)
{
	static const char hex[] = "0123456789abcdef";
	char line[160];
	HANDLE f = open_log();
	DWORD n = 0, written;
	int shift;

	if (f == INVALID_HANDLE_VALUE)
		return;
	while (*what && n < sizeof(line) - 24)
		line[n++] = *what++;
	line[n++] = ' ';
	line[n++] = '0';
	line[n++] = 'x';
	for (shift = 60; shift >= 0; shift -= 4)
		line[n++] = hex[(value >> shift) & 0xf];
	line[n++] = '\r';
	line[n++] = '\n';
	WriteFile(f, line, n, &written, NULL);
}
