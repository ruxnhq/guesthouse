// SPDX-License-Identifier: GPL-2.0-or-later
import Foundation

/// Where an install stands, as the agent says it, for the install screens and for
/// `--print-states`. Read from the agent's `install` event and the `installs` of a listing.
nonisolated struct InstallStatus: Equatable {
    struct Step: Equatable {
        var name: String
        var status: String
        var fraction: Double
    }

    struct Failure: Equatable {
        var step: String
        var code: String?
        var reason: String
        var choices: [String]
    }

    var machine: String
    var guest: String
    /// `running`, `failed`, `done` or `cancelled`.
    var state: String
    var steps: [Step]
    var current: String?
    var failure: Failure?
    var secondsLeft: Int?
    /// The guest account's name, for saying how to sign in.
    var account: String

    init?(json: [String: Any]) {
        guard let machine = json["machine"] as? String, let guest = json["guest"] as? String,
              let state = json["state"] as? String, let steps = json["steps"] as? [[String: Any]]
        else { return nil }
        self.machine = machine
        self.guest = guest
        self.state = state
        self.steps = steps.compactMap { entry in
            guard let name = entry["step"] as? String, let status = entry["status"] as? String else { return nil }
            let progress = entry["progress"] as? [String: Any]
            return Step(name: name, status: status, fraction: progress?["fraction"] as? Double ?? 0)
        }
        current = json["current"] as? String
        secondsLeft = json["seconds_left"] as? Int
        account = json["account"] as? String ?? ""
        if let failure = json["failure"] as? [String: Any], let step = failure["step"] as? String {
            self.failure = Failure(
                step: step, code: failure["code"] as? String,
                reason: failure["reason"] as? String ?? "",
                choices: failure["choices"] as? [String] ?? [])
        }
    }

    /// The time left in the words of the progress screen: "About 14 minutes left."
    static func timeLeft(seconds: Int) -> String {
        if seconds < 60 { return String(localized: "Less than a minute left.") }
        let minutes = Int((Double(seconds) / 60).rounded())
        return minutes == 1 ? String(localized: "About a minute left.") : String(localized: "About \(minutes) minutes left.")
    }

    /// One line for `--print-states`.
    func printLine() -> String {
        var parts = ["install: \(machine) \(guest) \(state)"]
        if let current, state == "running" {
            let fraction = steps.first { $0.name == current }?.fraction ?? 0
            parts.append("\(current) \(Int((fraction * 100).rounded()))%")
        }
        if let secondsLeft, state == "running" { parts.append(Self.timeLeft(seconds: secondsLeft)) }
        if let failure {
            parts.append("stopped at \(failure.step)\(failure.code.map { " (\($0))" } ?? ""): \(failure.reason)")
            parts.append("choices: \(failure.choices.joined(separator: ", "))")
        }
        return parts.joined(separator: " ")
    }
}
