// SPDX-License-Identifier: GPL-2.0-or-later
import Foundation

/// The messages for things going wrong, in the words of the design: what happened, what it means
/// for the person's machines and what they can do. Only data to show: the judging (is there enough
/// memory, enough disk, is the driver old) is the core's (`start_checks.rs`, `driver_version.rs`),
/// and the words for memory and disk come from it with the numbers the buttons need.
nonisolated enum Problem: Identifiable, Equatable {
    /// This Mac has no way to draw a machine with its GPU.
    case noGPU(has: String)
    /// Starting `machine` would need more memory than this Mac has to spare (the core's `title` and
    /// `message`). `running` is the one machine holding the memory, if it is one. `startAnyway` when
    /// the shortage is only what is free at the moment, which the Mac can make room for.
    case notEnoughMemory(machine: String, title: String, message: String, running: String?, startAnyway: Bool)
    /// The machine's disk can grow to more than this Mac has room for, or this Mac is nearly full
    /// (the core's words). `startAnyway` for the first: a disk takes only what the guest writes.
    case notEnoughDisk(machine: String, title: String, message: String, startAnyway: Bool)
    /// The graphics driver in the machine is older than the released one (`reported` is its
    /// version) or is not in it at all (`reported` is nil). The version it should have is `released`.
    case driverOld(machine: String, machineID: String, released: String, reported: String?)
    /// `machine` stopped before its desktop came up.
    case didNotStart(machine: String, machineID: String)
    /// An agent of another build, which an older Guesthouse started, is still running `machines`
    /// (`older` names its build). `installing` are those that are being installed. `canContinue`
    /// when this app can work against it for now.
    case olderAgent(older: String, machines: [String], installing: [String], canContinue: Bool)

    var id: String {
        switch self {
        case .noGPU: "no-gpu"
        case .notEnoughMemory(let machine, _, _, _, _): "memory-" + machine
        case .notEnoughDisk(let machine, _, _, _): "disk-" + machine
        case .driverOld(_, let machineID, _, _): "driver-" + machineID
        case .didNotStart(_, let machineID): "start-" + machineID
        case .olderAgent: "older-agent"
        }
    }

    var title: String {
        switch self {
        case .noGPU:
            String(localized: "This Mac can't draw a machine with its GPU")
        case .notEnoughMemory(_, let title, _, _, _), .notEnoughDisk(_, let title, _, _):
            title
        case .driverOld(let machine, _, _, let reported):
            reported == nil
                ? String(localized: "The graphics driver in \(machine) is missing")
                : String(localized: "The graphics driver in \(machine) is older than this Guesthouse")
        case .didNotStart(let machine, _):
            String(localized: "\(machine) didn't start")
        case .olderAgent:
            String(localized: "An older Guesthouse is still running machines")
        }
    }

    var message: String {
        switch self {
        case .noGPU(let has):
            String(localized: "Guesthouse needs a Mac with Apple silicon and macOS 27 or later. This one has \(has). Machines can still run, drawn by the processor, which is slow.")
        case .notEnoughMemory(_, _, let message, _, _), .notEnoughDisk(_, _, let message, _):
            message
        case .driverOld(_, _, _, let reported):
            reported == nil
                ? String(localized: "Without it the machine is drawn by the processor, which is slow. Installing takes about a minute and restarts the machine's screen, not the machine.")
                : String(localized: "It still works. Updating takes about a minute and restarts the machine's screen, not the machine.")
        case .olderAgent(let older, let machines, let installing, let canContinue):
            Self.olderAgentMessage(older: older, machines: machines, installing: installing, canContinue: canContinue)
        case .didNotStart:
            String(localized: "It stopped before its desktop came up. Nothing on its disk was changed. Starting again usually works. If it doesn't, send the report so it can be looked at.")
        }
    }

    /// From the core's answer to a check: a message when it found a problem, and nothing when it
    /// found none or could not say.
    static func from(core json: String) -> Problem? {
        let object = (try? JSONSerialization.jsonObject(with: Data(json.utf8))) as? [String: Any]
        guard let problem = object?["problem"] as? [String: Any],
              let title = problem["title"] as? String, let message = problem["message"] as? String,
              let machine = problem["machine"] as? String
        else { return nil }
        switch problem["kind"] as? String {
        case "not_enough_memory":
            return .notEnoughMemory(
                machine: machine, title: title, message: message,
                running: problem["running"] as? String, startAnyway: problem["start_anyway"] as? Bool ?? false)
        case "not_enough_disk":
            return .notEnoughDisk(
                machine: machine, title: title, message: message, startAnyway: problem["start_anyway"] as? Bool ?? false)
        default:
            return nil
        }
    }

    /// From the core's answer about the driver: a message when it is old or missing, and nothing
    /// when it is current or not known.
    static func driver(json: String, machine: String, machineID: String) -> Problem? {
        let object = (try? JSONSerialization.jsonObject(with: Data(json.utf8))) as? [String: Any]
        switch object?["state"] as? String {
        case "old":
            return .driverOld(machine: machine, machineID: machineID, released: object?["released"] as? String ?? "", reported: object?["reported"] as? String)
        case "missing":
            return .driverOld(machine: machine, machineID: machineID, released: object?["released"] as? String ?? "", reported: nil)
        default:
            return nil
        }
    }
}

extension Problem {
    /// What the sheet about an older agent says: who is still running, and what the choices do.
    nonisolated static func olderAgentMessage(older: String, machines: [String], installing: [String], canContinue: Bool) -> String {
        let names = ListFormatter.localizedString(byJoining: machines)
        var text = String(localized: "Guesthouse \(older) is still running \(names). It was started by an earlier copy of the app, and this one has to replace it.")
        if installing.isEmpty {
            text += " " + String(localized: "Shutting them down is the same as choosing Shut Down for each.")
        } else {
            let installs = ListFormatter.localizedString(byJoining: installing)
            text += " " + String(localized: "\(installs) is being installed. Wait for the install to finish, then open Guesthouse again.")
        }
        text += " " + (canContinue
            ? String(localized: "With Not Now, this app works with the older one until you quit.")
            : String(localized: "This version cannot work with the older one, so it shows no machines until they are shut down."))
        return text
    }
}

/// What the core says to do about the agent that answered a listing.
nonisolated enum AgentVerdict: Equatable {
    /// This app's own agent.
    case own
    /// Another build with nothing running in it: it can be replaced at once.
    case replace(older: String)
    /// Another build with machines running in it.
    case busy(older: String, running: [String], installing: [String], canContinue: Bool)

    static func from(core json: String) -> AgentVerdict? {
        guard let object = (try? JSONSerialization.jsonObject(with: Data(json.utf8))) as? [String: Any],
              let verdict = object["verdict"] as? String
        else { return nil }
        let older = object["older"] as? String ?? ""
        switch verdict {
        case "own": return .own
        case "replace": return .replace(older: older)
        case "busy":
            return .busy(
                older: older, running: object["running"] as? [String] ?? [],
                installing: object["installing"] as? [String] ?? [],
                canContinue: object["can_continue"] as? Bool ?? false)
        default: return nil
        }
    }
}

/// How long launchd is trusted to start this app's own agent. When it has started an agent that
/// is not (and that was replaced) this many times in a row, it is starting another copy's: the
/// app starts its own agent itself instead.
nonisolated struct LaunchdPatience {
    static let tries = 2
    private var replaced = 0

    /// Notes that launchd's agent was another build's and was replaced; true when to give up.
    mutating func replacedOne() -> Bool {
        replaced += 1
        return replaced >= Self.tries
    }
}
