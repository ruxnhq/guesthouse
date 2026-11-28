// SPDX-License-Identifier: GPL-2.0-or-later
import Foundation
import ServiceManagement
import os

/// Getting an agent to talk to: registered with launchd, or started by the app itself.
nonisolated enum AgentLauncher {
    /// The switch between the two ways. On, the app registers the agent with launchd
    /// (`SMAppService`) and reaches it over XPC, and falls back to the second way if that does
    /// not work. Off, it only ever starts the agent itself. `GUESTHOUSE_AGENT=socket` in the
    /// environment turns it off for one run.
    static let preferLaunchd = ProcessInfo.processInfo.environment["GUESTHOUSE_AGENT"] != "socket"

    private static let plistName = "com.ruxn.guesthouse.agent.plist"
    private static let log = Logger(subsystem: "com.ruxn.guesthouse", category: "agent")

    enum Registration {
        case ready
        case failed(String)
    }

    /// Registers the launchd agent, if it is not yet.
    static func registerWithLaunchd() -> Registration {
        let service = SMAppService.agent(plistName: plistName)
        if service.status == .notRegistered || service.status == .notFound {
            do {
                try service.register()
            } catch {
                return .failed("launchd would not register the agent: \(error.localizedDescription)")
            }
        }
        switch service.status {
        case .enabled:
            return .ready
        case .requiresApproval:
            return .failed("the agent waits for approval in System Settings, Login Items")
        default:
            return .failed("launchd does not know the agent (status \(service.status.rawValue))")
        }
    }

    /// The launchd label of the agent, which is also its mach service.
    private static let label = "com.ruxn.guesthouse.agent"

    /// Ends an agent that has nothing running, so this app's own can take its place: it is taken
    /// out of launchd (this bundle's registration, then whatever job holds the label, which may
    /// be another bundle's) and, if it was not launchd's, sent SIGTERM. Waits for it to be gone,
    /// and says so if it is not. It never sends SIGKILL.
    static func retire(pid: Int32) async -> Bool {
        let service = SMAppService.agent(plistName: plistName)
        try? await service.unregister()
        if await run("/bin/launchctl", ["print", "gui/\(getuid())/\(label)"]) {
            _ = await run("/bin/launchctl", ["bootout", "gui/\(getuid())/\(label)"])
        }
        if await gone(pid, within: 3) { return true }
        if pid > 1 { kill(pid, SIGTERM) }
        return await gone(pid, within: 5)
    }

    private static func gone(_ pid: Int32, within seconds: Int) async -> Bool {
        for _ in 0..<(seconds * 10) {
            if pid <= 1 || (kill(pid, 0) != 0 && errno == ESRCH) { return true }
            try? await Task.sleep(for: .milliseconds(100))
        }
        return pid <= 1 || (kill(pid, 0) != 0 && errno == ESRCH)
    }

    /// Runs a program and says whether it ended well.
    private static func run(_ program: String, _ arguments: [String]) async -> Bool {
        await withCheckedContinuation { continuation in
            let process = Process()
            process.executableURL = URL(filePath: program)
            process.arguments = arguments
            process.standardOutput = FileHandle.nullDevice
            process.standardError = FileHandle.nullDevice
            process.terminationHandler = { continuation.resume(returning: $0.terminationStatus == 0) }
            do { try process.run() } catch { continuation.resume(returning: false) }
        }
    }

    /// Takes the agent out of launchd again: for `--unregister-agent`.
    static func unregisterFromLaunchd() -> String {
        let service = SMAppService.agent(plistName: plistName)
        do {
            try service.unregister()
            return "unregistered"
        } catch {
            return "not unregistered: \(error.localizedDescription)"
        }
    }

    /// Starts the agent as a process of its own that goes on when the app quits: its own
    /// session, standard output and error in `agent.log` beside the socket. Returns why it
    /// could not, if it could not.
    static func startDetached(supportFolder: URL) -> String? {
        let agent = Bundle.main.bundleURL.appending(path: "Contents/MacOS/guesthouse-agent").path(percentEncoded: false)
        try? FileManager.default.createDirectory(at: supportFolder, withIntermediateDirectories: true)
        let logPath = supportFolder.appending(path: "agent.log").path(percentEncoded: false)

        var actions: posix_spawn_file_actions_t?
        posix_spawn_file_actions_init(&actions)
        defer { posix_spawn_file_actions_destroy(&actions) }
        posix_spawn_file_actions_addopen(&actions, 0, "/dev/null", O_RDONLY, 0)
        posix_spawn_file_actions_addopen(&actions, 1, logPath, O_WRONLY | O_CREAT | O_APPEND, 0o644)
        posix_spawn_file_actions_adddup2(&actions, 1, 2)

        var attributes: posix_spawnattr_t?
        posix_spawnattr_init(&attributes)
        defer { posix_spawnattr_destroy(&attributes) }
        posix_spawnattr_setflags(&attributes, Int16(POSIX_SPAWN_SETSID))

        var pid: pid_t = 0
        let arguments = [agent, "--socket"]
        let argv: [UnsafeMutablePointer<CChar>?] = arguments.map { strdup($0) } + [nil]
        defer { argv.forEach { free($0) } }
        let result = posix_spawn(&pid, agent, &actions, &attributes, argv, environ)
        if result != 0 {
            return "cannot start \(agent): \(String(cString: strerror(result)))"
        }
        log.info("Started the agent as pid \(pid)")
        return nil
    }
}
