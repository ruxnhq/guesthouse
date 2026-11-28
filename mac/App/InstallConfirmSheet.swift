// SPDX-License-Identifier: GPL-2.0-or-later
import AppKit
import SwiftUI

/// The one question before an install: everything is already chosen for this Mac, and Customise
/// opens the few things a person might change. Windows also needs the installer file, which
/// Microsoft does not let an app fetch, so the sheet opens Microsoft's page and takes the file.
struct InstallConfirmSheet: View {
    let guest: InstallGuest
    @Environment(Library.self) private var library
    @Environment(\.dismiss) private var dismiss

    @State private var name = ""
    @State private var cores = 4
    @State private var memoryGB = 6
    @State private var diskGB = 64
    @State private var desktop = LinuxDesktop.gnome
    @State private var customising = false
    @State private var installer = ""
    @State private var problem: String?
    @State private var starting = false

    private let hostCores = ProcessInfo.processInfo.activeProcessorCount
    private let hostGB = Int(ProcessInfo.processInfo.physicalMemory / 1_073_741_824)
    private let freeGB = Library.freeSpaceGB()

    private var space: (text: String, enough: Bool, warning: String?) {
        InstallText.spaceSentence(needGB: guest.spaceNeededGB(diskGB: diskGB), freeGB: freeGB, diskGB: diskGB)
    }

    private var needsInstaller: Bool { guest == .windows }
    private var canInstall: Bool {
        !starting && space.enough && !name.trimmingCharacters(in: .whitespaces).isEmpty
            && (!needsInstaller || !installer.isEmpty)
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 20) {
            VStack(alignment: .leading, spacing: 6) {
                Text("Install \(guest.title)").font(.title2.weight(.semibold))
                Text("Everything below is already chosen for this Mac. Press Install and come back when it's done.")
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
            VStack(spacing: 0) {
                systemRow
                Divider()
                nameRow
                if guest == .ubuntu {
                    Divider()
                    desktopRow
                }
                Divider()
                sizeRow
                if customising { customiseRows }
                Divider()
                spaceRow
            }
            .card(padding: 0)
            if guest == .windows { testModeNote }
            if let problem {
                Label(problem, systemImage: "exclamationmark.circle")
                    .foregroundStyle(.red)
                    .fixedSize(horizontal: false, vertical: true)
            }
            HStack {
                if starting { ProgressView().controlSize(.small) }
                Spacer()
                Button("Cancel") { dismiss() }.keyboardShortcut(.cancelAction)
                Button("Install", action: install)
                    .buttonStyle(.borderedProminent)
                    .keyboardShortcut(.defaultAction)
                    .disabled(!canInstall)
            }
        }
        .padding(28)
        .frame(width: 560)
        .onAppear(perform: setDefaults)
    }

    // MARK: Rows

    private func row<Content: View>(_ title: LocalizedStringKey, @ViewBuilder _ content: () -> Content) -> some View {
        HStack(spacing: 12) {
            Text(title).foregroundStyle(.secondary).frame(width: 90, alignment: .leading)
            content()
        }
        .padding(.horizontal, 16)
        .frame(minHeight: 48)
    }

    private var systemRow: some View {
        row("System") {
            if guest == .windows {
                VStack(alignment: .leading, spacing: 2) {
                    Text("Windows 11 for ARM")
                    Text(installer.isEmpty ? "Get it from Microsoft, then choose it."
                         : URL(filePath: installer).lastPathComponent)
                        .font(.callout)
                        .foregroundStyle(.secondary)
                        .lineLimit(1)
                        .truncationMode(.middle)
                }
                Spacer()
                Button("Get It…") { NSWorkspace.shared.open(URL(string: InstallGuest.windowsInstallerPage)!) }
                    .help("Opens Microsoft's page in your browser")
                Button("Choose…", action: chooseInstaller)
            } else {
                Text("Ubuntu Server for ARM, downloaded from Ubuntu")
                Spacer()
            }
        }
    }

    private var nameRow: some View {
        row("Name") {
            if customising {
                TextField("Name", text: $name).labelsHidden()
            } else {
                Text(name)
                Spacer()
            }
        }
    }

    private var desktopRow: some View {
        row("Desktop") {
            VStack(alignment: .leading, spacing: 4) {
                Picker("Desktop", selection: $desktop) {
                    ForEach(LinuxDesktop.allCases) { Text($0.title).tag($0) }
                }
                .pickerStyle(.segmented).labelsHidden().fixedSize()
                Text(desktop.detail).font(.callout).foregroundStyle(.secondary)
            }
            .padding(.vertical, 8)
            Spacer()
        }
    }

    private var sizeRow: some View {
        row("Size") {
            Text(InstallText.sizeSummary(cores: cores, memoryGB: memoryGB, diskGB: diskGB))
            Spacer()
            Button("Customise") { withAnimation { customising.toggle() } }
                .disabled(customising)
        }
    }

    /// Processors, memory and disk, the way the board's New Machine has them.
    private var customiseRows: some View {
        VStack(spacing: 0) {
            Divider()
            row("Processors") {
                Picker("Processors", selection: $cores) {
                    ForEach(processorChoices, id: \.self) { Text("\($0)").tag($0) }
                }
                .pickerStyle(.segmented).labelsHidden().fixedSize()
                Text("of this Mac's \(hostCores)").foregroundStyle(.secondary)
            }
            Divider()
            row("Memory") {
                Slider(value: Binding(get: { Double(memoryGB) }, set: { memoryGB = Int($0) }),
                       in: 2...Double(max(3, hostGB - 2)), step: 1)
                    .accessibilityLabel("Memory")
                Text("\(memoryGB) GB").frame(width: 70, alignment: .trailing)
            }
            Divider()
            row("Disk") {
                Slider(value: Binding(get: { Double(diskGB) }, set: { diskGB = Int($0) }), in: 32...256, step: 8)
                    .accessibilityLabel("Disk")
                Text("\(diskGB) GB").frame(width: 70, alignment: .trailing)
            }
        }
    }

    private var spaceRow: some View {
        row("Space") {
            Image(systemName: space.enough && space.warning == nil ? "checkmark" : "exclamationmark.triangle.fill")
                .foregroundStyle(space.enough && space.warning == nil ? Color.green : Color.orange)
                .accessibilityHidden(true)
            VStack(alignment: .leading, spacing: 4) {
                Text(space.text)
                if let warning = space.warning {
                    Text(warning)
                        .font(.callout)
                        .foregroundStyle(.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                }
            }
            Spacer()
        }
    }

    private var testModeNote: some View {
        Label {
            Text("This copy of Windows runs in test mode, so it can load the Guesthouse graphics driver. Windows says \"Test Mode\" in the corner of its desktop.")
                .foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
        } icon: {
            Image(systemName: "info.circle").foregroundStyle(.orange)
        }
    }

    // MARK: Actions

    private var processorChoices: [Int] {
        let choices = [2, 4, 6, 8, 12, 16].filter { $0 <= hostCores }
        return choices.isEmpty ? [1] : choices
    }

    private func setDefaults() {
        name = InstallText.freeName(guest.defaultName, taken: library.machines.map(\.name))
        cores = guest.cores(hostCores: hostCores)
        if !processorChoices.contains(cores) { cores = processorChoices.last ?? 1 }
        memoryGB = min(guest.memoryGB, max(3, hostGB - 2))
        diskGB = guest.diskGB
    }

    private func chooseInstaller() {
        let panel = NSOpenPanel()
        panel.canChooseFiles = true
        panel.canChooseDirectories = false
        panel.allowedContentTypes = [.init(filenameExtension: "iso")].compactMap { $0 }
        panel.message = String(localized: "Choose the Windows 11 ARM installer (.iso) you downloaded")
        if panel.runModal() == .OK, let url = panel.url { installer = url.path(percentEncoded: false) }
    }

    private func install() {
        starting = true
        problem = nil
        let sizes = InstallSizes(cores: cores, memoryGB: memoryGB, diskGB: diskGB)
        Task {
            let error = await library.startInstall(
                guest: guest, name: name.trimmingCharacters(in: .whitespaces),
                file: needsInstaller ? installer : nil, sizes: sizes,
                desktop: guest == .ubuntu ? desktop.rawValue : nil)
            starting = false
            if let error { problem = error } else { dismiss() }
        }
    }
}
