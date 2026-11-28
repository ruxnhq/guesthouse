// SPDX-License-Identifier: GPL-2.0-or-later
import Foundation
import GuesthouseCore

// guesthouse-agent: owns the running machines so the app is UI only. The machines, their
// processes and every message live in the Rust core; this file only chooses how the app reaches
// it.
//
//   --xpc           answer on the mach service (what launchd starts it with)
//   --socket [PATH] answer on a Unix socket, by default agent.sock in the support folder (what
//                   the app starts when launchd's way is not available)
//   --print-state   print every machine's state as one line of JSON and leave

setvbuf(stdout, nil, _IOLBF, 0)
setvbuf(stderr, nil, _IOLBF, 0)

guard let agent = guesthouse_agent_new() else {
    fputs("guesthouse-agent: no home folder\n", stderr)
    exit(1)
}
// Which build this is, so the app can tell its own agent from one an older app left running.
let bundleInfo = Bundle.main.infoDictionary
guesthouse_agent_set_build(
    agent,
    bundleInfo?["CFBundleShortVersionString"] as? String ?? "0",
    bundleInfo?["CFBundleVersion"] as? String ?? "0"
)
let arguments = CommandLine.arguments

if arguments.contains("--print-state") {
    if let answer = guesthouse_agent_handle(agent, #"{"v":1,"id":1,"op":"list"}"#) {
        print(String(cString: answer))
        guesthouse_string_free(answer)
    }
    exit(0)
}

if arguments.contains("--xpc") {
    serveXPC(agent: agent)
}

var socketPath = ""
if let index = arguments.firstIndex(of: "--socket"), index + 1 < arguments.count, !arguments[index + 1].hasPrefix("--") {
    socketPath = arguments[index + 1]
} else if let path = guesthouse_agent_socket_path() {
    socketPath = String(cString: path)
    guesthouse_string_free(path)
}
guard !socketPath.isEmpty else {
    fputs("guesthouse-agent: no socket path\n", stderr)
    exit(1)
}
switch guesthouse_agent_serve(agent, socketPath) {
case 0:
    fputs("guesthouse-agent: idle, leaving\n", stderr)
    exit(0)
case 1:
    fputs("guesthouse-agent: another agent has \(socketPath)\n", stderr)
    exit(0)
default:
    exit(1)
}
