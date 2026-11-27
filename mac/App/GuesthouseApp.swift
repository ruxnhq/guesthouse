// SPDX-License-Identifier: GPL-2.0-or-later
import AppKit
import SwiftUI

@main
struct GuesthouseApp: App {
    @State private var library = Library.shared
    @AppStorage(SettingsKey.menuBarItem) private var menuBarItem = true
    @NSApplicationDelegateAdaptor(AppDelegate.self) private var delegate

    init() {
        // The --print options write to a file or a pipe as often as to a terminal: a line is
        // seen as it is printed, not when a buffer fills.
        setvbuf(stdout, nil, _IOLBF, 0)
        if LaunchOptions.unregisterAgent {
            print("agent: \(AgentLauncher.unregisterFromLaunchd())")
            exit(0)
        }
    }

    var body: some Scene {
        Window("Guesthouse", id: LibraryView.windowID) {
            LibraryView()
                .environment(library)
                .guesthouseLook()
        }
        .defaultSize(width: 1100, height: 720)
        // Always there at launch, even when the last run ended with it closed: the library is
        // the app, and `--screen` opens its window from it.
        .defaultLaunchBehavior(.presented)
        .restorationBehavior(.disabled)
        .commands {
            AppCommands(library: library)
        }

        WindowGroup("Screen", id: ScreenWindow.windowID, for: Machine.ID.self) { $machineID in
            if let machineID {
                ScreenWindow(machineID: machineID)
                    .environment(library)
                    .guesthouseLook()
            }
        }
        .defaultSize(width: 1280, height: 760)
        // A screen opens when asked, not by itself at launch: the machine may not be running.
        .restorationBehavior(.disabled)

        Settings {
            SettingsView()
                .environment(library)
                .guesthouseLook()
        }

        // In the menu bar only while a machine runs (and the person has not turned it off).
        MenuBarExtra("Guesthouse", image: "MenuBarIcon", isInserted: Binding(
            get: { menuBarItem && library.agent.anyAlive }, set: { _ in })
        ) {
            MenuBarContent()
                .environment(library)
                .guesthouseLook()
        }
        .menuBarExtraStyle(.window)
    }
}

struct AppCommands: Commands {
    let library: Library
    @Environment(\.openWindow) private var openWindow
    @AppStorage(SettingsKey.showScreenStatistics) private var showStatistics = false

    var body: some Commands {
        CommandGroup(replacing: .appInfo) {
            Button("About Guesthouse") { showAboutPanel() }
        }
        CommandGroup(replacing: .newItem) {
            Button("New Machine…") {
                openWindow(id: LibraryView.windowID)
                library.showingNewMachine = true
            }
            .keyboardShortcut("n")
        }
        CommandGroup(after: .toolbar) {
            Toggle("Show Statistics", isOn: $showStatistics)
                .keyboardShortcut("s", modifiers: [.command, .option])
        }
        CommandMenu("Machine") {
            Button("Start") { act { library.start($0) } }
                .keyboardShortcut("r")
                .disabled(!canStart)
            Button("Shut Down") { act { library.shutDown($0) } }
                .keyboardShortcut(.delete, modifiers: [.command, .option])
                .disabled(selectedStatus?.state != "running")
            Button("Open Screen") { act { openWindow(id: ScreenWindow.windowID, value: $0.id) } }
                .keyboardShortcut("o", modifiers: [.command, .shift])
                .disabled(!canOpenScreen)
        }
        // There is no help book yet, so no Help item that would only say so.
        CommandGroup(replacing: .help) {}
    }

    private var selected: Machine? {
        library.machines.first { $0.id == library.selection }
    }

    private var selectedStatus: MachineStatus? {
        selected.map { library.status(of: $0) }
    }

    private func act(_ action: (Machine) -> Void) {
        if let selected { action(selected) }
    }

    private var canStart: Bool {
        guard let selected, let status = selectedStatus else { return false }
        return Rules.canStart(
            startable: selected.startable, problem: library.startPlan(for: selected).plan == nil,
            agentConnected: library.agent.connected, state: status.state)
    }

    private var canOpenScreen: Bool {
        guard let selected, let status = selectedStatus else { return false }
        return Rules.canOpenScreen(hasScreen: selected.frames != nil, startable: selected.startable, state: status.state)
    }

    /// The standard About panel, with the core's version under the app's.
    private func showAboutPanel() {
        let credits = NSAttributedString(
            string: String(localized: "Core \(Core.version)"),
            attributes: [
                .font: NSFont.systemFont(ofSize: NSFont.smallSystemFontSize),
                .foregroundColor: NSColor.secondaryLabelColor,
            ]
        )
        NSApplication.shared.orderFrontStandardAboutPanel(options: [.credits: credits])
    }
}
