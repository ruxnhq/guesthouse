// SPDX-License-Identifier: GPL-2.0-or-later
import AppKit
import SwiftUI

/// A message for something going wrong: a sign, a title, a few sentences and the buttons that do
/// something about it.
struct ProblemSheet: View {
    let problem: Problem
    @Environment(Library.self) private var library
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            Image(systemName: symbol)
                .font(.system(size: 30))
                .foregroundStyle(tint)
                .accessibilityHidden(true)
            Text(problem.title).font(.headline)
            Text(problem.message)
                .foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
            HStack {
                Spacer()
                buttons
            }
        }
        .padding(28)
        .frame(width: 520)
    }

    private var symbol: String {
        if case .didNotStart = problem { return "exclamationmark.circle" }
        return "exclamationmark.triangle"
    }

    private var tint: Color {
        if case .didNotStart = problem { return .red }
        return .orange
    }

    @ViewBuilder
    private var buttons: some View {
        switch problem {
        case .noGPU:
            Button("Carry On Without It") { dismiss() }
            Button("Open Software Update") {
                NSWorkspace.shared.open(URL(string: "x-apple.systempreferences:com.apple.Software-Update-Settings.extension")!)
                dismiss()
            }
            .buttonStyle(.borderedProminent)
        case .notEnoughMemory(let machine, _, _, let running, let startAnyway):
            Button("Cancel") { dismiss() }
            if startAnyway, let target = library.machines.first(where: { $0.name == machine }) {
                Button("Start Anyway") {
                    dismiss()
                    library.start(target, ignoring: [.memory])
                }
            }
            if let target = library.machines.first(where: { $0.name == machine }) {
                Button("Change \(machine)'s Memory") {
                    library.show(target, tab: .hardware)
                    dismiss()
                }
            }
            if let running, let other = library.machines.first(where: { $0.name == running }) {
                Button("Shut Down \(running)") {
                    library.shutDown(other)
                    dismiss()
                }
                .buttonStyle(.borderedProminent)
            }
        case .notEnoughDisk(let machine, _, _, let startAnyway):
            Button("Cancel") { dismiss() }
            if startAnyway, let target = library.machines.first(where: { $0.name == machine }) {
                Button("Start Anyway") {
                    dismiss()
                    library.start(target, ignoring: [.disk])
                }
            }
            Button("Show in Finder") {
                if let folder = Core.machinesFolder { NSWorkspace.shared.activateFileViewerSelecting([folder]) }
                dismiss()
            }
            .buttonStyle(.borderedProminent)
        case .driverOld(_, let machineID, _, let reported):
            Button("Later") {
                dismiss()
                if let target = library.machines.first(where: { $0.id == machineID }) {
                    library.start(target, ignoring: [.driver])
                }
            }
            Button(reported == nil ? "Install Driver" : "Update Driver") {}
                .buttonStyle(.borderedProminent)
                .disabled(true)
                .help("Needs the guest's help, which is not built yet")
        case .didNotStart(_, let machineID):
            if let target = library.machines.first(where: { $0.id == machineID }) {
                Button("Send Report") { library.report(machine: target, kind: .wontStart) }
                Button("Start Again") {
                    dismiss()
                    library.start(target)
                }
                .buttonStyle(.borderedProminent)
            } else {
                Button("OK") { dismiss() }
            }
        }
    }
}
