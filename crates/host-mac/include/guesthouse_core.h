/* SPDX-License-Identifier: GPL-2.0-or-later */
/* The C interface of Guesthouse's Mac core, written by hand to match core/src/lib.rs. */

#ifndef GUESTHOUSE_CORE_H
#define GUESTHOUSE_CORE_H

#include <stdbool.h>
#include <stdint.h>

/* The core's version, such as "0.1.0". Static: do not free it. */
const char *guesthouse_core_version(void);

/* The default machines folder, ~/Library/Application Support/Guesthouse/Machines, or NULL when
 * HOME is not set. Free it with guesthouse_string_free. */
char *guesthouse_machines_folder(void);

/* Lists the machines in `folder`, or in ~/Library/Application Support/Guesthouse/Machines when
 * `folder` is NULL. Returns JSON, {"machines": [...], "skipped": [...]} or {"error": "..."}.
 * Free it with guesthouse_string_free. */
char *guesthouse_machine_list(const char *folder);

/* Finds the program `tool` ("qemu", "rmhost", "pvg-host" or "pvg-qemu") to run; the last two are
 * also checked for what they can do (--frames, pvg-fwd). `setting` is what Settings holds (NULL or
 * empty for nothing), `bundled` where the app bundle keeps its own copy (NULL for none). Returns
 * JSON, {"path", "source", "why"} (source: setting, environment, bundled or lab) or
 * {"error": "..."}. */
char *guesthouse_tool_locate(const char *tool, const char *setting, const char *bundled);

/* Adding a machine. `request` is JSON: name, cores, memory_mb, disk, firmware_code,
 * firmware_vars, option_rom (optional), host_memory_mb, host_cores. _check answers
 * {"problems": [...]} (empty when it can be made); _create makes it and answers {"id": "..."}
 * or {"problems": [...]}, and may take long when the files cannot be cloned. */
char *guesthouse_machine_check(const char *request);
char *guesthouse_machine_create(const char *request);

/* Changes a stopped machine's processors and memory: `request` is JSON, {"cores", "memory_mb",
 * "host_cores", "host_memory_mb"}. Answers {"ok": true} or {"problems": [...]}. */
char *guesthouse_machine_set_hardware(const char *id, const char *request);

/* Sets where machine `id`'s graphics device lives, "guarded" or "qemu". {"ok": true} or {"error"}. */
char *guesthouse_machine_set_graphics(const char *id, const char *graphics);

/* Makes a bug report and writes it in the Reports folder, without sending it. `request` is JSON
 * ({"machine", "kind", "description", "contact", "include_serial", "machine_running",
 * "app_version", "driver_version"}, each optional). Answers {"archive", "folder", "files",
 * "missing", "notes", "summary", "summary_header", "bytes", "warning"} or {"error"}. */
char *guesthouse_report_prepare(const char *request);

/* Sends a report made by guesthouse_report_prepare. Only for the Send button, after the person
 * has seen what is in it. Answers {"id", "issue_link"} or {"error"}. */
char *guesthouse_report_send(const char *archive, const char *summary_header, const char *summary_json);

/* Where new issues are opened: {"page": "..."}. */
char *guesthouse_report_issue_page(void);

/* Looks once for a newer Mac release (the shared update check): {"state": "newer", "version",
 * "where"}, {"state": "latest"} or {"state": "cant_tell"}. With `nightly` it reads the nightly
 * build's file, not the release's. May wait for the network, so not on the main thread.
 * Downloads nothing. */
char *guesthouse_update_check(const char *current, bool nightly);

/* Whether machine `id`'s graphics driver is the released one: {"state": "unknown"|"current"} or
 * {"state": "old", "reported", "released"} or {"state": "missing", "released"}. */
char *guesthouse_driver_state(const char *id);

/* Keeps the driver version machine `id` reported: {"ok": true} or {"error"}. */
char *guesthouse_driver_record(const char *id, const char *version);

/* Judges one check before a start or a new machine from facts the app read (memory against other
 * machines and what is free now, disk against what it can grow by, advice for a new machine).
 * `request` is JSON with "check" and its numbers; the answer is {"problem": null|{...}},
 * {"warning": ...} or {"error"}. */
char *guesthouse_start_check(const char *request);

/* Answers one rule of a machine's page: "rule" is next_step, is_alive, can_force_off,
 * can_open_screen, can_start, shows_install or start_check_order, with that rule's facts; the
 * answer is {"step"}, {"yes"}, {"order"} or {"error"}. States are the shared names. */
char *guesthouse_rule(const char *request);

/* Records the pixels and scaling (100 or 200) machine `id`'s screen should have to fill its
 * window, and for a running machine on the guarded path sends it to the graphics process as its
 * display mode, at most one a second, the last one winning. May wait a second or more: call it
 * off the main thread. {"ok": true, "applied": true, "mode": "WxH@60"} when the graphics process
 * took it, {"ok": true, "applied": false, "why": "..."} when nothing was sent, {"error": "..."}. */
char *guesthouse_machine_set_wanted_screen(const char *id, uint32_t width_px, uint32_t height_px,
                                           uint32_t scaling_percent);

/* Asks machine `id`'s graphics process for its status over its control socket: the numbers of
 * its line as JSON ({"published", "refused", ...}) or {"error": "..."}. */
char *guesthouse_graphics_status(const char *id);

/* Renames machine `id` (its folder's name). Answers {"ok": true} or {"error": "..."}. */
char *guesthouse_machine_rename(const char *id, const char *name);

/* Allows or forbids viewing machine `id`'s screen from other devices (its `allow_remote_view`),
 * choosing a port the first time. Answers {"port": N} or {"error": "..."}. Nothing listens
 * because of this alone: the agent shares the screen of a running machine. */
char *guesthouse_machine_set_remote_view(const char *id, bool on);

/* Frees a string the core returned. NULL is allowed. */
void guesthouse_string_free(char *text);

/* A machine's input, sent to its QMP socket as input-send-event. Opaque. Use a handle from one
 * thread at a time. No call waits on QEMU; input sent while QEMU is not there is dropped. */
typedef struct GuesthouseInput GuesthouseInput;

/* Starts sending input to the QMP socket at `path`, which need not exist yet: it is tried
 * every second. NULL when `path` is NULL or not UTF-8. Close it with guesthouse_input_close. */
GuesthouseInput *guesthouse_input_open(const char *path);

/* The pointer is at (x, y) in a view of view_width by view_height, from its top left, showing
 * a picture of picture_width by picture_height fitted inside it with bars. */
void guesthouse_input_pointer(GuesthouseInput *input, double x, double y,
                              double view_width, double view_height,
                              double picture_width, double picture_height);

/* Button `number` (0 left, 1 right, 2 middle) went down or up. */
void guesthouse_input_button(GuesthouseInput *input, uint32_t number, bool down);

/* A scroll, in points when `precise` (a trackpad) and in lines when not (a mouse wheel). */
void guesthouse_input_scroll(GuesthouseInput *input, double dx, double dy, bool precise);

/* The key with macOS key code `code` (NSEvent.keyCode) went down or up. */
void guesthouse_input_key(GuesthouseInput *input, uint16_t code, bool down);

/* The modifier key `code` changed (flagsChanged), and its kind of modifier is now on or off. */
void guesthouse_input_modifier(GuesthouseInput *input, uint16_t code, bool flag_on);

/* Lets go of every key and button the guest was told is held. */
void guesthouse_input_release_all(GuesthouseInput *input);

/* Lets go of everything held and closes the socket. NULL is allowed. */
void guesthouse_input_close(GuesthouseInput *input);

/* The agent and its socket. A request or an answer or an event is one line of JSON, as in
 * core/src/protocol.rs. Strings returned are freed with guesthouse_string_free. */

/* The agent's Unix socket, agent.sock in the support folder (GUESTHOUSE_SUPPORT_DIR moves it).
 * NULL when HOME is not set. */
char *guesthouse_agent_socket_path(void);

/* The agent itself, for the agent's own process. */
typedef struct GuesthouseAgent GuesthouseAgent;

/* Makes the agent for this user and finds the machines still running from before. Its settings
 * come from GUESTHOUSE_QEMU and the two GUESTHOUSE_*_TIMEOUT_MS variables. NULL without HOME. */
GuesthouseAgent *guesthouse_agent_new(void);

/* Answers one request line with one answer line. Does not wait on QEMU. */
char *guesthouse_agent_handle(const GuesthouseAgent *agent, const char *line);

/* True when nothing is running and nobody is listening. */
bool guesthouse_agent_is_idle(const GuesthouseAgent *agent);

/* Answers on the socket at `path` until the agent is idle. Returns 0 when it ended idle, 1 when
 * another agent already holds the socket, -1 on an error (printed to standard error). */
int32_t guesthouse_agent_serve(const GuesthouseAgent *agent, const char *path);

/* State changes for one listener (the XPC form of the agent uses this). */
typedef struct GuesthouseEvents GuesthouseEvents;
GuesthouseEvents *guesthouse_agent_events(const GuesthouseAgent *agent);
/* The next state change line, or NULL after `timeout_ms` with none. */
char *guesthouse_events_next(GuesthouseEvents *events, uint32_t timeout_ms);
void guesthouse_events_close(GuesthouseEvents *events);

/* The app's end of the socket. Receiving and sending may be done from different threads. */
typedef struct GuesthouseConnection GuesthouseConnection;

/* Connects to the agent's socket at `path`; NULL when there is no agent there. */
GuesthouseConnection *guesthouse_connection_open(const char *path);
/* Sends one request line; false when the agent is gone. */
bool guesthouse_connection_send(GuesthouseConnection *connection, const char *line);
/* The next line from the agent, or NULL after `timeout_ms` with none; `*closed` is set when
 * the agent hung up. */
char *guesthouse_connection_receive(GuesthouseConnection *connection, uint32_t timeout_ms, bool *closed);
void guesthouse_connection_close(GuesthouseConnection *connection);

#endif
