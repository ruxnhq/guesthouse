// SPDX-License-Identifier: GPL-2.0-or-later
import Foundation

/// The two systems Guesthouse can install, with the sizes it chooses for this Mac and the words
/// the install screens use for them. Plain data and sentences, so the tests can reach it.
nonisolated enum InstallGuest: String, CaseIterable, Identifiable {
    case windows, ubuntu

    var id: String { rawValue }

    /// What the machine is called until the person changes it.
    var defaultName: String { self == .windows ? "Windows 11" : "Ubuntu" }

    /// The name in headings: "Install Windows 11".
    var title: String { defaultName }

    /// The short name the step titles use: "Download Windows".
    var shortName: String { self == .windows ? "Windows" : "Ubuntu" }

    /// Memory and disk in gigabytes, as the core's `default_sizes` chooses them, and the most
    /// processors it gives by default.
    var memoryGB: Int { self == .windows ? 6 : 4 }
    var diskGB: Int { self == .windows ? 64 : 32 }
    static let defaultMostCores = 4

    func cores(hostCores: Int) -> Int { min(max(hostCores, 1), Self.defaultMostCores) }

    /// Room the install needs on the Mac, as the core counts it (`plan.rs`): the installer, what
    /// the system writes and a margin. The disk is a sparse file that holds only what the guest
    /// writes, so its full size is not needed, only warned about (`InstallText.spaceSentence`).
    func spaceNeededGB(diskGB: Int) -> Int {
        let (installer, system) = self == .windows ? (6, 25) : (3, 12)
        return installer + min(system, diskGB) + 5
    }

    /// Microsoft's page for the ARM installer. Its download cannot be automated (the page checks
    /// that a person is there), so the app opens it in the browser. The same address the core
    /// has as `windows::INSTALLER_PAGE`.
    static let windowsInstallerPage = "https://www.microsoft.com/software-download/windows11arm64"
}

/// The desktop an Ubuntu machine starts to. The agent passes the word on to the guest, where the
/// driver package installs that desktop.
nonisolated enum LinuxDesktop: String, CaseIterable, Identifiable {
    case gnome, plasma, weston

    var id: String { rawValue }

    var title: String {
        switch self {
        case .gnome: "GNOME"
        case .plasma: "KDE Plasma"
        case .weston: "Minimal"
        }
    }

    /// One line under the choice.
    var detail: String {
        switch self {
        case .gnome: String(localized: "Ubuntu's own desktop.")
        case .plasma: String(localized: "KDE's desktop, in the style of Windows.")
        case .weston: String(localized: "A plain desktop with a terminal. Quickest to install, least memory.")
        }
    }
}

/// What Customise chose.
nonisolated struct InstallSizes: Equatable {
    var cores: Int
    var memoryGB: Int
    var diskGB: Int
}

nonisolated enum InstallText {
    /// Decimal gigabytes, as the core says them.
    static let bytesPerGB: Double = 1_000_000_000

    static func gigabytes(_ bytes: Int64) -> Int { Int((Double(bytes) / bytesPerGB).rounded(.down)) }

    /// The size row of the confirmation: "4 processors, 6 GB memory, 64 GB disk".
    static func sizeSummary(cores: Int, memoryGB: Int, diskGB: Int) -> String {
        String(localized: "\(cores) processors, \(memoryGB) GB memory, \(diskGB) GB disk")
    }

    /// The space row: "Needs 36 GB. This Mac has 212 GB free." and whether it is enough. When it
    /// is, but the whole disk would not fit, a second sentence says so as a warning, not a stop
    /// (the disk takes only what the guest writes).
    static func spaceSentence(needGB: Int, freeGB: Int?, diskGB: Int = 0) -> (text: String, enough: Bool, warning: String?) {
        guard let freeGB else { return (String(localized: "Needs \(needGB) GB."), true, nil) }
        let enough = freeGB >= needGB
        let text = String(localized: "Needs \(needGB) GB. This Mac has \(freeGB) GB free.")
        let warning = enough && diskGB > freeGB
            ? String(localized: "The disk can grow by up to \(diskGB) GB and this Mac has \(freeGB) GB free. It only takes the room the guest writes, so it may never get that big. If this Mac fills up, the machine stops.")
            : nil
        return (text, enough, warning)
    }

    /// A name no machine has: the wanted name, or the wanted name and the first number after it.
    static func freeName(_ wanted: String, taken: [String]) -> String {
        if !taken.contains(wanted) { return wanted }
        var number = 2
        while taken.contains("\(wanted) \(number)") { number += 1 }
        return "\(wanted) \(number)"
    }
}

extension InstallStatus {
    /// The steps the screens list, in order. Making the disk has no line of its own: it is quick
    /// and the person has nothing to do or see in it.
    static let shownSteps = ["fetch", "install_system", "install_driver", "first_start"]

    var shown: [Step] { steps.filter { Self.shownSteps.contains($0.name) } }

    var installGuest: InstallGuest { InstallGuest(rawValue: guest) ?? .windows }

    /// The line a step has on the progress screen.
    func title(of step: String) -> String {
        let name = installGuest.shortName
        switch step {
        case "fetch": return String(localized: "Download \(name)")
        case "install_system": return String(localized: "Install \(name)")
        case "install_driver": return String(localized: "Install the Guesthouse graphics driver")
        case "first_start": return String(localized: "First start")
        default: return step
        }
    }

    /// How far the current step is, in whole percent.
    var percent: Int {
        let fraction = steps.first { $0.name == current }?.fraction ?? 0
        return Int((fraction * 100).rounded())
    }

    /// The sidebar's second line for a machine that is being installed.
    var sidebarLine: String {
        switch state {
        case "failed": String(localized: "Install stopped")
        case "done": String(localized: "Ready")
        default: String(localized: "Installing, \(percent)%")
        }
    }

    /// The failure's heading, from the step that stopped.
    var failureTitle: String {
        switch failure?.step {
        case "fetch": String(localized: "The download stopped")
        case "install_system": String(localized: "The install stopped")
        case "install_driver": String(localized: "The graphics driver didn't install")
        case "first_start": String(localized: "The first start didn't work")
        default: String(localized: "The install stopped")
        }
    }

    /// The sentence under the heading: what is safe, and the reason the core gave.
    var failureSentence: String {
        guard let failure else { return "" }
        let reason = failure.reason.isEmpty ? "" : " " + failure.reason
        if failure.step == "install_driver" {
            let name = installGuest.shortName
            return String(localized: "\(name) is installed and nothing is lost.") + reason
        }
        return reason.trimmingCharacters(in: .whitespaces)
    }

    /// The install stopped because a new agent took it up and does not have the account's password
    /// (it is only ever in an agent's memory). The app repairs this itself from the Keychain.
    var needsPassword: Bool {
        state == "failed" && failure?.code == "password_missing"
    }

    /// The button for a choice the core offers.
    func label(forChoice choice: String) -> String {
        switch choice {
        case "try_again": failure?.step == "first_start" ? String(localized: "Start Again") : String(localized: "Try Again")
        case "choose_installer": String(localized: "Choose an Installer…")
        case "open_without_driver": String(localized: "Open \(installGuest.shortName) Without It")
        case "send_report": String(localized: "Send Report")
        default: choice
        }
    }
}
