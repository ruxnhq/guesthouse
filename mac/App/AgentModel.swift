// SPDX-License-Identifier: GPL-2.0-or-later
import Foundation
import Observation
import os

/// The agent's answer to one request.
nonisolated struct AgentAnswer {
    let ok: Bool
    let error: String?
    let statuses: [String: MachineStatus]?
    /// For a listing: the machines whose screen is being shared.
    var shares: [String: ShareStatus]? = nil
    /// For a listing or `install_status`: the installs. For `install`: the new machine's id.
    var installs: [String: InstallStatus]? = nil
    var machine: String? = nil
    /// The line the answer came in, for what reads it again (which build the agent is).
    var line = ""

    static func failure(_ text: String) -> AgentAnswer {
        AgentAnswer(ok: false, error: text, statuses: nil)
    }
}

/// An agent of another build that is still running machines.
nonisolated struct OlderAgent: Equatable {
    /// Its build, as the person reads it.
    let older: String
    /// The machines running in it, by the agent's name for them (the folder's name).
    let machines: [String]
    /// Those of them that are being installed.
    let installing: [String]
    /// Whether this app can work with it for now.
    let canContinue: Bool
}

/// The app's side of the agent: finds or starts it, sends what the user asks, and keeps the
/// states the agent reports. Everything the window shows about running machines comes from here.
@Observable
final class AgentModel {
    /// Machine id (the folder's name) to its state; absent means stopped.
    private(set) var statuses: [String: MachineStatus] = [:]
    /// Machine id to where its install stands; absent means none.
    private(set) var installs: [String: InstallStatus] = [:]
    /// Machine id to where its screen sharing stands; absent means off.
    private(set) var shares: [String: ShareStatus] = [:]
    private(set) var connected = false
    /// Told whenever a machine's state or share changes, so the library can act on it.
    @ObservationIgnored var onChange: () -> Void = {}
    /// How the agent is reached, for Settings: what is live and why not the other.
    private(set) var route = String(localized: "Not connected")
    /// An agent of another build that has machines running: what it holds, until the person
    /// has chosen. nil when the agent is this app's own or has been replaced.
    private(set) var olderAgent: OlderAgent?
    /// Told when an older agent with machines is found, so the library can say so in the window.
    @ObservationIgnored var onOlderAgent: (OlderAgent) -> Void = { _ in }

    @ObservationIgnored private var link: AgentLink?
    @ObservationIgnored private var nextID = 0
    @ObservationIgnored private var pending: [Int: CheckedContinuation<AgentAnswer, Never>] = [:]
    @ObservationIgnored private var maintaining = false
    /// An older agent this app cannot work with is connected, only to shut its machines down.
    @ObservationIgnored private var awaitingOlder = false
    @ObservationIgnored private var olderPID: Int32 = 0
    @ObservationIgnored private var askedAboutOlder = false
    @ObservationIgnored private let log = Logger(subsystem: "com.ruxn.guesthouse", category: "agent")

    func status(of machine: Machine) -> MachineStatus {
        statuses[machine.agentID] ?? .off
    }

    func share(of machine: Machine) -> ShareStatus {
        shares[machine.agentID] ?? .off
    }

    /// Whether any machine is on its way up or running.
    var anyAlive: Bool { statuses.values.contains { $0.isAlive } }

    var aliveMachineIDs: [String] { statuses.filter { $0.value.isAlive }.map(\.key) }

    /// Connects, and keeps trying: an agent that went away is found or started again.
    func maintain() {
        guard !maintaining else { return }
        maintaining = true
        Task {
            while true {
                if !connected && !awaitingOlder { await establish() }
                try? await Task.sleep(for: .seconds(2))
            }
        }
    }

    /// `scale` is what a Linux guest draws its desktop at: 2 for Sharp, 1 for Standard.
    func start(_ machine: Machine, qemu: String, rmhost: String?, pvgHost: String?, scale: Int) async -> String? {
        var request: [String: Any] = ["op": "start", "machine": machine.agentID, "scale": scale]
        if !qemu.isEmpty { request["qemu"] = qemu }
        if let rmhost { request["rmhost"] = rmhost }
        if let pvgHost { request["pvg_host"] = pvgHost }
        return await ask(request).error
    }

    /// Starts or stops sharing a running machine's screen, and with `pairing` opens pairing
    /// for a short time. A share that is up is not restarted for that: rmhost is told.
    /// Answers or ends a pairing on a shared machine: `pair_accept`, `pair_decline` or
    /// `pair_close`.
    func pair(_ op: String, machine: Machine) async -> String? {
        await ask(["op": op, "machine": machine.agentID]).error
    }

    /// Allows or forbids `what` (`input` is control) for a trusted device.
    func deviceAllow(machine: Machine, fingerprint: String, what: String, on: Bool) async -> String? {
        await ask(["op": "device_allow", "machine": machine.agentID, "fingerprint": fingerprint, "what": what, "on": on]).error
    }

    /// Stops trusting a device (`device_revoke`) or disconnects the viewer (`device_drop`).
    func deviceRemove(machine: Machine, fingerprint: String) async -> String? {
        await ask(["op": "device_revoke", "machine": machine.agentID, "fingerprint": fingerprint]).error
    }

    func deviceDisconnect(machine: Machine) async -> String? {
        await ask(["op": "device_drop", "machine": machine.agentID]).error
    }

    /// Starts installing `guest` ("windows" or "ubuntu") in a new machine called `name`. Returns
    /// the machine's id, or why it did not start.
    func install(
        name: String, guest: String, file: String?, qemu: String?, pvgHost: String?,
        sizes: InstallSizes? = nil, password: String? = nil, desktop: String? = nil
    ) async -> (id: String?, error: String?) {
        var request: [String: Any] = ["op": "install", "name": name, "guest": guest]
        if let desktop { request["desktop"] = desktop }
        if let password { request["password"] = password }
        if let sizes {
            request["cores"] = sizes.cores
            request["memory_mb"] = sizes.memoryGB * 1024
            request["disk_gb"] = sizes.diskGB
        }
        if let file { request["file"] = file }
        if let qemu { request["qemu"] = qemu }
        if let pvgHost { request["pvg_host"] = pvgHost }
        let answer = await ask(request, timeout: .seconds(15))
        return (answer.machine, answer.error)
    }

    /// What the person chose at a failure: `try_again`, `choose_installer` or `open_without_driver`.
    func installChoose(machine: String, choice: String, path: String? = nil, password: String? = nil) async -> String? {
        var request: [String: Any] = ["op": "install_choose", "machine": machine, "choice": choice]
        if let password { request["password"] = password }
        if let path { request["path"] = path }
        return await ask(request).error
    }

    func installCancel(machine: String) async -> String? {
        await ask(["op": "install_cancel", "machine": machine]).error
    }

    /// Where one install stands, asked of the agent.
    func installStatus(machine: String) async -> InstallStatus? {
        await ask(["op": "install_status", "machine": machine]).installs?[machine]
    }

    func shareScreen(of machine: Machine, on: Bool, pairing: Bool = false, rmhost: String?) async -> String? {
        var request: [String: Any] = ["op": "share", "machine": machine.agentID, "on": on, "pairing": pairing]
        if let rmhost { request["rmhost"] = rmhost }
        return await ask(request).error
    }

    func shutDown(_ machine: Machine) async -> String? {
        await ask(["op": "shutdown", "machine": machine.agentID]).error
    }

    func forceOff(_ machine: Machine) async -> String? {
        await ask(["op": "force_off", "machine": machine.agentID]).error
    }

    /// A shutdown request nobody waits for, for quitting.
    func sendShutDownWithoutAnswer(_ machine: Machine) {
        nextID += 1
        link?.send(#"{"v":1,"id":\#(nextID),"op":"shutdown","machine":"\#(machine.agentID)"}"#)
    }

    // MARK: Connecting

    private enum Adoption { case taken, none, replaced }

    /// Connects to an agent that is this app's own. An older one with nothing running is
    /// replaced, which needs a second look at what is there. When launchd starts another copy's
    /// agent again after it was replaced (two copies of Guesthouse on the Mac with one bundle
    /// identifier, such as one in Downloads: launchd finds the agent's program by that
    /// identifier and may pick the other copy), launchd is given up for this run and the app
    /// starts its own agent from its own bundle, over the socket.
    private func establish() async {
        var launchd = AgentLauncher.preferLaunchd
        var patience = LaunchdPatience()
        for _ in 0..<5 {
            guard await establishOnce(launchd: launchd) == .replaced else { return }
            if launchd && patience.replacedOne() { launchd = false }
        }
    }

    private func establishOnce(launchd: Bool) async -> Adoption {
        var why: String? = launchd || !AgentLauncher.preferLaunchd
            ? nil : String(localized: "launchd starts the agent of another copy of Guesthouse")
        if launchd {
            switch AgentLauncher.registerWithLaunchd() {
            case .ready:
                switch await adopt(XPCLink(onLine: lineHandler, onClose: closeHandler), route: String(localized: "launchd, over XPC")) {
                case .taken: return .taken
                case .replaced: return .replaced
                case .none: why = String(localized: "launchd's agent did not answer")
                }
            case .failed(let reason):
                why = reason
            }
            log.notice("Not using launchd: \(why ?? "", privacy: .public)")
        }
        let socket = Core.agentSocketPath
        if let socket {
            let found = await adopt(SocketLink(path: socket, onLine: lineHandler, onClose: closeHandler), route: socketRoute(why))
            if found != .none { return found }
        }
        if let support = Core.supportFolder, let problem = AgentLauncher.startDetached(supportFolder: support) {
            route = problem
            return .none
        }
        // The agent needs a moment to open its socket.
        for _ in 0..<30 {
            try? await Task.sleep(for: .milliseconds(100))
            if let socket {
                let found = await adopt(SocketLink(path: socket, onLine: lineHandler, onClose: closeHandler), route: socketRoute(why))
                if found != .none { return found }
            }
        }
        route = String(localized: "The agent did not start")
        return .none
    }

    private func socketRoute(_ why: String?) -> String {
        let base = String(localized: "started by the app, over a Unix socket")
        return why.map { "\(base) (\($0))" } ?? base
    }

    /// Takes `link` if the agent answers a listing through it and is this app's own. Another
    /// build with nothing running is ended (`.replaced`); one with machines running is told to
    /// the person (`olderAgent`) and is kept while the app can work with it.
    private func adopt(_ candidate: (any AgentLink)?, route: String) async -> Adoption {
        guard let candidate else { return .none }
        link = candidate
        let answer = await ask(["op": "list"], timeout: .seconds(3))
        guard answer.ok, let listed = answer.statuses else {
            candidate.close()
            link = nil
            return .none
        }
        var route = route
        switch AgentBuild.verdict(ofListing: answer.line) {
        case .own:
            break
        case .replace(let older):
            log.notice("Replacing the agent of \(older, privacy: .public) with this app's own")
            if LaunchOptions.printStates { print("older agent: \(older), nothing running, replacing it") }
            if await AgentLauncher.retire(pid: candidate.peerPID) {
                candidate.close()
                link = nil
                return .replaced
            }
            log.notice("The agent of \(older, privacy: .public) would not leave; using it")
            route += " " + String(localized: "(an older build that would not leave)")
        case .busy(let older, let running, let installing, let canContinue):
            olderPID = candidate.peerPID
            let found = OlderAgent(older: older, machines: running, installing: installing, canContinue: canContinue)
            olderAgent = found
            if LaunchOptions.printStates {
                print("older agent: \(older) running \(running.joined(separator: ", ")); can continue: \(canContinue)")
            }
            if !askedAboutOlder {
                askedAboutOlder = true
                onOlderAgent(found)
            }
            route = String(localized: "an older Guesthouse's agent (\(older)) that is still running machines")
            if !canContinue {
                // Only to shut its machines down; nothing else is shown from it.
                awaitingOlder = true
                self.route = route
                return .taken
            }
        }
        statuses = listed.filter { $0.value != .off }
        shares = answer.shares ?? [:]
        installs = answer.installs ?? [:]
        if LaunchOptions.printStates {
            for (machine, install) in installs.sorted(by: { $0.key < $1.key }) { print(install.printLine()); _ = machine }
            print("connected: \(route)")
            for (machine, share) in shares.sorted(by: { $0.key < $1.key }) { print(share.printLine(machine: machine)) }
            for (machine, status) in statuses.sorted(by: { $0.key < $1.key }) {
                print("state: \(machine) \(status.state)\(status.reason.map { ": \($0)" } ?? "")")
            }
        }
        connected = true
        self.route = route
        onChange()
        return .taken
    }

    private var lineHandler: @Sendable (String) -> Void {
        { [weak self] line in Task { @MainActor in self?.receive(line) } }
    }

    private var closeHandler: @Sendable () -> Void {
        { [weak self] in Task { @MainActor in self?.lost() } }
    }

    private func lost() {
        if awaitingOlder {
            awaitingOlder = false
            olderAgent = nil
            link?.close()
            link = nil
            return
        }
        guard connected else { return }
        connected = false
        route = String(localized: "Not connected")
        link?.close()
        link = nil
    }

    // MARK: An older agent

    /// Not Now: the older agent is kept as it is while the app can work with it. When it cannot,
    /// the app stays without machines and says why.
    func keepOlderAgent() {
        if awaitingOlder, let older = olderAgent {
            route = String(localized: "An older Guesthouse is running \(ListFormatter.localizedString(byJoining: older.machines)); shut it down to use this version")
        }
    }

    /// Shut Them Down and Update: asks each machine the older agent holds to shut down the
    /// ordinary way, waits for them, and when they are all off ends the older agent and starts
    /// this app's own. Nothing is forced.
    func shutDownAndUpdate() {
        guard let older = olderAgent, older.installing.isEmpty else { return }
        Task {
            for machine in older.machines { _ = await ask(["op": "shutdown", "machine": machine]) }
            for _ in 0..<90 {
                try? await Task.sleep(for: .seconds(1))
                let answer = await ask(["op": "list"], timeout: .seconds(3))
                guard answer.ok else { break }
                if case .replace = AgentBuild.verdict(ofListing: answer.line) {
                    log.notice("The older agent has nothing running; replacing it")
                    let pid = olderPID
                    link?.close()
                    link = nil
                    awaitingOlder = false
                    olderAgent = nil
                    connected = false
                    _ = await AgentLauncher.retire(pid: pid)
                    await establish()
                    return
                }
            }
            // Some machine did not shut down in time: the person is told again, and may try again.
            askedAboutOlder = false
            route = String(localized: "A machine in the older Guesthouse did not shut down")
        }
    }

    // MARK: Lines

    private func ask(_ fields: [String: Any], timeout: Duration = .seconds(5)) async -> AgentAnswer {
        guard let link else { return .failure(String(localized: "The agent is not running.")) }
        nextID += 1
        let id = nextID
        var request = fields
        request["v"] = 1
        request["id"] = id
        guard let data = try? JSONSerialization.data(withJSONObject: request),
              let line = String(data: data, encoding: .utf8)
        else { return .failure("Cannot write the request.") }
        return await withCheckedContinuation { continuation in
            pending[id] = continuation
            link.send(line)
            Task {
                try? await Task.sleep(for: timeout)
                if let late = pending.removeValue(forKey: id) {
                    late.resume(returning: .failure(String(localized: "The agent did not answer.")))
                }
            }
        }
    }

    private func receive(_ line: String) {
        guard let object = try? JSONSerialization.jsonObject(with: Data(line.utf8)) as? [String: Any] else { return }
        if object["event"] as? String == "install" {
            if let install = InstallStatus(json: object) {
                installs[install.machine] = install
                if LaunchOptions.printStates { print(install.printLine()) }
                onChange()
            }
            return
        }
        if object["event"] as? String == "share" {
            if let machine = object["machine"] as? String, let share = ShareStatus(json: object) {
                shares[machine] = share.state == .off ? nil : share
                if LaunchOptions.printStates { print(share.printLine(machine: machine)) }
                onChange()
            }
            return
        }
        if object["event"] as? String == "state" {
            if let machine = object["machine"] as? String, let status = Self.status(from: object) {
                statuses[machine] = status == .off ? nil : status
                if LaunchOptions.printStates {
                    print("state: \(machine) \(status.state)\(status.reason.map { ": \($0)" } ?? "")")
                }
                onChange()
            }
            return
        }
        guard let id = object["id"] as? Int, let continuation = pending.removeValue(forKey: id) else { return }
        var listed: [String: MachineStatus]?
        if let machines = object["machines"] as? [[String: Any]] {
            listed = [:]
            for machine in machines {
                if let name = machine["machine"] as? String, let status = Self.status(from: machine) {
                    listed?[name] = status
                }
            }
        }
        var shared: [String: ShareStatus]?
        if let entries = object["shares"] as? [[String: Any]] {
            shared = [:]
            for entry in entries {
                if let name = entry["machine"] as? String, let share = ShareStatus(json: entry) {
                    shared?[name] = share
                }
            }
        }
        var installed: [String: InstallStatus]?
        if let entries = object["installs"] as? [[String: Any]] {
            installed = [:]
            for entry in entries {
                if let install = InstallStatus(json: entry) { installed?[install.machine] = install }
            }
        }
        continuation.resume(returning: AgentAnswer(
            ok: object["ok"] as? Bool ?? false,
            error: object["error"] as? String,
            statuses: listed,
            shares: shared,
            installs: installed,
            machine: object["machine"] as? String,
            line: line
        ))
    }

    private static func status(from object: [String: Any]) -> MachineStatus? {
        guard let state = object["state"] as? String else { return nil }
        return MachineStatus(
            state: state,
            reason: object["reason"] as? String
        )
    }
}
