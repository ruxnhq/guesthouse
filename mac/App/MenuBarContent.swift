// SPDX-License-Identifier: GPL-2.0-or-later
import AppKit
import SwiftUI

/// The menu bar item's window: each machine with the one thing to do with it, a device that is
/// waiting to pair with Accept and Decline, and the app's own commands. It is only in the menu
/// bar while a machine runs, and the person can turn it off in Settings.
struct MenuBarContent: View {
    @Environment(Library.self) private var library
    @Environment(\.openWindow) private var openWindow

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            ForEach(library.machines) { machine in
                machineRow(machine)
            }
            ForEach(library.machines.filter { library.agent.share(of: $0).pending != nil }) { machine in
                pairingRow(machine)
            }
            Divider()
            menuButton("Open Guesthouse") {
                openWindow(id: LibraryView.windowID)
                NSApp.activate()
            }
            // The same three the sidebar's New Machine button offers.
            menuButton("Install Windows…") {
                openWindow(id: LibraryView.windowID)
                NSApp.activate()
                library.installing = .windows
            }
            menuButton("Install Ubuntu…") {
                openWindow(id: LibraryView.windowID)
                NSApp.activate()
                library.installing = .ubuntu
            }
            menuButton("Use a Disk or Installer…") {
                openWindow(id: LibraryView.windowID)
                NSApp.activate()
                library.showingNewMachine = true
            }
            SettingsLink { Text("Settings…").frame(maxWidth: .infinity, alignment: .leading) }
                .buttonStyle(.plain)
                .padding(.horizontal, 8)
                .frame(minHeight: 26)
            Divider()
            menuButton("Quit Guesthouse and Shut Down Machines") { library.quitShuttingDown() }
        }
        .padding(10)
        .frame(width: 340)
    }

    private func machineRow(_ machine: Machine) -> some View {
        let status = library.status(of: machine)
        return HStack(spacing: 10) {
            MachineMark(kind: machine.kind)
            VStack(alignment: .leading, spacing: 1) {
                Text(machine.name).fontWeight(.semibold)
                HStack(spacing: 5) {
                    StateDot(color: status.color, size: 7)
                    Text(statusLine(machine, status)).font(.caption).foregroundStyle(.secondary)
                }
            }
            Spacer()
            if status.isAlive, machine.frames != nil {
                Button("Open Screen") {
                    openWindow(id: ScreenWindow.windowID, value: machine.id)
                    NSApp.activate()
                }
                .buttonStyle(.borderedProminent)
            } else if status.nextStep == .start, machine.startable {
                Button("Start") { library.start(machine) }
            }
        }
        .padding(.horizontal, 6)
        .frame(minHeight: 44)
    }

    private func statusLine(_ machine: Machine, _ status: MachineStatus) -> String {
        if status.state == "running", let viewer = library.agent.share(of: machine).viewer {
            return String(localized: "Running · \(viewer) is viewing")
        }
        return status.title
    }

    private func pairingRow(_ machine: Machine) -> some View {
        let device = library.agent.share(of: machine).pending
        return HStack(spacing: 10) {
            VStack(alignment: .leading, spacing: 2) {
                Text(Wording.text("remote_view.wants_to_pair", ["name": device?.name ?? "A device"])).fontWeight(.semibold)
                Text("With \(machine.name). Code \(device?.code ?? "")")
                    .font(.caption).foregroundStyle(.secondary)
            }
            Spacer()
            Button(Wording.text("remote_view.decline")) { library.declineDevice(machine) }
            Button(Wording.text("remote_view.accept")) { library.acceptDevice(machine) }.buttonStyle(.borderedProminent)
        }
        .padding(10)
        .background(Color.primary.opacity(0.08), in: RoundedRectangle(cornerRadius: 6))
    }

    private func menuButton(_ title: LocalizedStringKey, _ action: @escaping () -> Void) -> some View {
        Button(action: action) {
            Text(title).frame(maxWidth: .infinity, alignment: .leading)
        }
        .buttonStyle(.plain)
        .padding(.horizontal, 8)
        .frame(minHeight: 26)
    }
}
