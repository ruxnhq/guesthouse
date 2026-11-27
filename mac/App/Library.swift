// SPDX-License-Identifier: GPL-2.0-or-later
import AppKit
import Foundation
import Observation
import os

/// The machines the library window shows, as the core last listed them, and where the agent
/// says each stands.
@Observable
final class Library {
    private(set) var machines: [Machine] = []
    /// Why the machines could not be listed at all, if they could not.
    private(set) var problem: String?
    var showingNewMachine = false
    /// The message for something going wrong that is up, if one is.
    var attention: Problem?
    /// The bug report sheet that is up, if one is.
    var reporting: ReportStart?
    /// A page a message asked the machine's pane to open on.
    var tabRequest: MachineTab?
    /// Machines this run has asked to start and not yet seen come up or fail.
    @ObservationIgnored var startedHere = Set<String>()
    /// Installs whose missing password is being given back, so it is asked once for each stop.
    @ObservationIgnored var repairing = Set<String>()
    /// Set by "Quit and Shut Down Machines", so quitting does not ask.
    @ObservationIgnored var quitShutsDown = false
    /// The system whose install is being confirmed (the sheet), if any.
    var installing: InstallGuest?
    /// Installs whose "ready" screen the person has been through, so it is shown once.
    var installsSeen: Set<String> = Set(UserDefaults.standard.stringArray(forKey: SettingsKey.installsSeen) ?? [])
    /// The machine chosen in the sidebar.
    var selection: Machine.ID?
    /// What the agent last refused, shown to the user.
    var requestProblem: String?
    let agent = AgentModel()
    /// Machines this run has already asked the agent to share.
    @ObservationIgnored private var askedToShare = Set<String>()
    /// What each running protected machine's graphics process last said, read every few seconds
    /// while the machine runs (not only while a tab that shows it is open).
    private(set) var graphics: [String: GraphicsCounts] = [:]
    @ObservationIgnored private var graphicsTimer: Timer?
    /// The screens that are open, by machine folder, so the machine's pane can say how its
    /// screen is doing.
    var screens: [String: ScreenSession] = [:]

    /// The one library, so the agent connection belongs to the app and not to a window.
    static let shared = Library()

    private let log = Logger(subsystem: "com.ruxn.guesthouse", category: "library")

    init() {
        agent.onChange = { [weak self] in
            self?.syncSharing()
            self?.syncInstalls()
            self?.watchStarts()
            self?.repairInstalls()
        }
        graphicsTimer = Timer.scheduledTimer(withTimeInterval: Self.graphicsEvery, repeats: true) { [weak self] _ in
            MainActor.assumeIsolated { self?.readGraphics() }
        }
    }

    /// How often the graphics process is asked how it is doing: slowly, it is a glance.
    private static let graphicsEvery: TimeInterval = 5

    private func readGraphics() {
        let running = machines.filter { $0.graphics == "guarded" && agent.status(of: $0).state == "running" }
        graphics = graphics.filter { entry in running.contains { $0.agentID == entry.key } }
        for machine in running {
            let id = machine.agentID
            Task {
                let counts = await Task.detached { Core.graphicsStatus(machine: id) }.value
                if let counts { graphics[id] = counts }
            }
        }
    }

    func reload() {
        do {
            let listing = try Core.listMachines()
            machines = listing.machines
            problem = nil
            for skipped in listing.skipped {
                log.warning("Skipped \(skipped.folder, privacy: .public): \(skipped.reason, privacy: .public)")
            }
        } catch {
            machines = []
            problem = error.localizedDescription
        }
        syncSharing()
    }

    /// The machine's one screen session, which the screen window, the preview and the sidebar's
    /// small picture share, so they take one of the publisher's four reader places, not three. Nil when the machine has no screen.
    func screenSession(for machine: Machine) -> ScreenSession? {
        guard let service = machine.frames else { return nil }
        let key = machine.id + "|" + service
        if let known = sessions[key] { return known }
        let session = ScreenSession(machine: machine, service: service)
        sessions[key] = session
        return session
    }

    /// One session for each machine's screen, made when first asked for. A session reads
    /// frames only while somebody is presenting it.
    @ObservationIgnored private var sessions: [String: ScreenSession] = [:]

    func status(of machine: Machine) -> MachineStatus {
        agent.status(of: machine)
    }

    /// The QEMU binary from Settings, empty when none is chosen.
    static var qemuSetting: String {
        UserDefaults.standard.string(forKey: SettingsKey.qemuPath) ?? ""
    }

    /// The QEMU Start would run now, looked for afresh so a change in Settings counts at once.
    var qemu: ToolLocation { ToolLocation.find(.qemu, setting: Self.qemuSetting) }

    /// The checks Start makes first; a message can say to go on without one.
    enum StartCheck { case memory, disk, driver }

    func start(_ machine: Machine, ignoring: Set<StartCheck> = []) {
        let (plan, problem) = startPlan(for: machine)
        guard let plan else {
            requestProblem = problem
            return
        }
        if let lack = startProblem(for: machine, ignoring: ignoring) {
            self.attention = lack
            return
        }
        startedHere.insert(machine.agentID)
        let rmhost = self.rmhost.path
        let scale = Self.startScale(for: machine)
        run { await self.agent.start(machine, qemu: plan.qemu, rmhost: rmhost, pvgHost: plan.pvgHost, scale: scale) }
    }

    /// The scale a machine starts with: 2 when its screen is Sharp, 1 when Standard. As in the
    /// screen window, Sharp is Standard on a display that is not Retina.
    static func startScale(for machine: Machine) -> Int {
        let retina = (NSScreen.main?.backingScaleFactor ?? 2) >= 2
        return ScreenPrefs.sharp(machine.id) && retina ? 2 : 1
    }

    /// The program that shares screens with other devices, if it can be found.
    var rmhost: ToolLocation { ToolLocation.find(.rmhost) }

    func shutDown(_ machine: Machine) {
        run { await self.agent.shutDown(machine) }
    }

    func forceOff(_ machine: Machine) {
        run { await self.agent.forceOff(machine) }
    }

    func run(_ request: @escaping () async -> String?) {
        Task {
            if let problem = await request() {
                requestProblem = problem
            }
        }
    }

    /// Does what `--share`, `--start`, `--shut-down`, `--force-off` and `--pair` ask, once the
    /// agent is connected, in that order, so a machine started with sharing on is shared.
    func runLaunchActions() {
        let actions: [(String?, (Machine) -> Void)] = [
            (LaunchOptions.start, { self.start($0) }),
            (LaunchOptions.shutDown, shutDown),
            (LaunchOptions.forceOff, forceOff),
        ]
        guard actions.contains(where: { $0.0 != nil }) || LaunchOptions.share != nil
            || LaunchOptions.pair != nil || LaunchOptions.accept != nil || LaunchOptions.decline != nil
            || LaunchOptions.install != nil || LaunchOptions.installStatus != nil || LaunchOptions.installCancel != nil
        else { return }
        Task {
            while !agent.connected { try? await Task.sleep(for: .milliseconds(100)) }
            await runInstallActions()
            if let share = LaunchOptions.share {
                if let machine = machines.first(where: { $0.name == share.name }) {
                    setRemoteView(machine, allowed: share.on)
                } else {
                    print("guesthouse: no machine called \(share.name)")
                }
            }
            for (name, action) in actions {
                if let name, let machine = machines.first(where: { $0.name == name }) {
                    action(machine)
                } else if let name {
                    print("guesthouse: no machine called \(name)")
                }
            }
            if let name = LaunchOptions.pair { await openPairing(onMachineNamed: name) }
            if let name = LaunchOptions.accept { await answerDevice(onMachineNamed: name, accept: true) }
            if let name = LaunchOptions.decline { await answerDevice(onMachineNamed: name, accept: false) }
        }
    }

    /// What `--install`, `--install-status` and `--install-cancel` ask. The status and the changes
    /// are printed by `--print-states`.
    private func runInstallActions() async {
        if let install = LaunchOptions.install {
            if let error = await startInstall(
                guest: InstallGuest(rawValue: install.guest) ?? .windows, name: install.name,
                file: LaunchOptions.installer, sizes: nil)
            {
                print("guesthouse: the install did not start: \(error)")
            }
            reload()
        }
        for (name, action) in [(LaunchOptions.installStatus, 0), (LaunchOptions.installCancel, 1)] {
            guard let name else { continue }
            reload()
            guard let machine = machines.first(where: { $0.name == name }) else {
                print("guesthouse: no machine called \(name)")
                continue
            }
            if action == 0 {
                if let status = await agent.installStatus(machine: machine.agentID) {
                    print(status.printLine())
                } else {
                    print("guesthouse: \(name) has no install")
                }
            } else if let error = await agent.installCancel(machine: machine.agentID) {
                print("guesthouse: \(error)")
            }
        }
    }

    /// Waits (a minute at most) for the machine's screen to be shared, then opens pairing.
    private func openPairing(onMachineNamed name: String) async {
        guard let machine = machines.first(where: { $0.name == name }) else {
            print("guesthouse: no machine called \(name)")
            return
        }
        for _ in 0..<600 {
            let share = agent.share(of: machine)
            if share.state == .listening {
                pairNewDevice(machine)
                return
            }
            try? await Task.sleep(for: .milliseconds(100))
        }
        print("guesthouse: \(name) is not being shared, so pairing was not opened")
    }

    /// Waits (ten minutes at most) for a device to wait on the machine, then answers it. The
    /// device's name and code are on the `share:` line that `--print-states` prints.
    private func answerDevice(onMachineNamed name: String, accept: Bool) async {
        guard let machine = machines.first(where: { $0.name == name }) else {
            print("guesthouse: no machine called \(name)")
            return
        }
        for _ in 0..<6000 {
            if agent.share(of: machine).pending != nil {
                if accept { acceptDevice(machine) } else { declineDevice(machine) }
                return
            }
            try? await Task.sleep(for: .milliseconds(100))
        }
        print("guesthouse: no device waited on \(name), so nothing was answered")
    }

    /// The rule: a running machine whose switch is on is shared, however it was started. The
    /// agent starts it for a machine it started or found itself; this asks for a machine whose
    /// share is not there and not failed, once for each time it runs, so a hand-edited file or a
    /// machine found by an agent without rmhost is covered too.
    func syncSharing() {
        guard agent.connected else { return }
        let running = Set(machines.filter { agent.status(of: $0).state == "running" }.map(\.agentID))
        askedToShare.formIntersection(running)
        guard let rmhost = self.rmhost.path else { return }
        for machine in machines where machine.remoteView && machine.startable && running.contains(machine.agentID) {
            guard agent.shares[machine.agentID] == nil, !askedToShare.contains(machine.agentID) else { continue }
            askedToShare.insert(machine.agentID)
            run { await self.agent.shareScreen(of: machine, on: true, rmhost: rmhost) }
        }
    }
}

/// Keys of the app's settings in UserDefaults.
enum SettingsKey {
    static let qemuPath = "qemuPath"
    /// What quitting does when machines are running: "ask", "keep" or "shutdown".
    static let quitBehavior = "quitBehavior"
    /// Whether the screen window shows frame numbers over the picture (View menu).
    static let showScreenStatistics = "showScreenStatistics"
    /// Whether the menu bar item shows while a machine runs.
    static let menuBarItem = "menuBarItem"
    /// "system", "dark" or "light".
    static let colours = "colours"
    /// "blue" (Guesthouse's own) or "system".
    static let accent = "accent"
    /// Whether the app looks for a newer version by itself (once a day, when it opens).
    static let autoCheckUpdates = "autoCheckUpdates"
    /// Whether the update check reads the nightly build's file (off unless the person turns it on).
    static let nightlyChannel = "nightlyChannel"
    /// When it last looked, as seconds since 1970, and what it found (the core's answer).
    static let updateCheckedAt = "updateCheckedAt"
    static let updateFound = "updateFound"
    /// The ids of installs whose "ready" screen has been shown.
    static let installsSeen = "installsSeen"
}
