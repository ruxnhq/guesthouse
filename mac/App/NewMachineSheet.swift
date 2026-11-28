// SPDX-License-Identifier: GPL-2.0-or-later
import AppKit
import SwiftUI

/// Adds a machine from a disk image that already has Windows on it. The files are cloned into
/// the machine's own folder (copied where cloning is not possible), so the originals are never
/// touched. Installing Windows or Ubuntu from a download is the Install choices' (`InstallConfirmSheet`).
struct NewMachineSheet: View {
    @Environment(Library.self) private var library
    @Environment(\.dismiss) private var dismiss
    @State private var request = NewMachineRequest()
    /// What is wrong with the request as it stands; shown once the person has tried to add.
    @State private var problems: [String] = []
    @State private var tried = false
    @State private var creating = false
    @State private var firmware = FirmwareFiles()

    /// The processor counts offered: those of the choices that this Mac has.
    private var processorChoices: [Int] {
        let choices = [2, 4, 6, 8, 12, 16].filter { $0 <= request.hostCores }
        return choices.isEmpty ? [1] : choices
    }

    /// Fewest and most gigabytes the slider offers; the core checks the same limits.
    private var memoryRange: ClosedRange<Int> {
        2...max(2, request.hostMemoryMB / 1024 - 2)
    }

    private var runningMemoryMB: Int {
        library.machines.filter { library.status(of: $0).isAlive }.reduce(0) { $0 + $1.memoryMB }
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            VStack(alignment: .leading, spacing: 4) {
                Text("Use a Disk or Installer").font(.title2.weight(.semibold))
                Text("Make a machine from a disk that already has Windows on it. Guesthouse copies the files into its own folder; the originals are left alone.")
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
            .padding([.top, .horizontal], 24)
            Form {
                Section {
                    TextField("Name", text: $request.name)
                    FileChooserRow(title: "Disk image", message: String(localized: "Choose the raw disk image (.img) of the machine"), path: $request.disk, detail: diskSize)
                    firmwareRow
                }
                Section {
                    LabeledContent("Processors") {
                        HStack {
                            Picker("Processors", selection: $request.cores) {
                                ForEach(processorChoices, id: \.self) { Text("\($0)").tag($0) }
                            }
                            .pickerStyle(.segmented)
                            .labelsHidden()
                            .fixedSize()
                            Text("of this Mac's \(request.hostCores)").foregroundStyle(.secondary)
                        }
                    }
                    LabeledContent("Memory") {
                        HStack {
                            Slider(value: memorySlider, in: Double(memoryRange.lowerBound)...Double(memoryRange.upperBound), step: 1)
                                .accessibilityLabel("Memory")
                            Text("\(request.memoryMB / 1024) GB").fontWeight(.semibold)
                            Text("of \(request.hostMemoryMB / 1024) GB").foregroundStyle(.secondary)
                        }
                    }
                }
                if let warning = Core.memoryAdvice(newMB: request.memoryMB, runningMB: runningMemoryMB, hostMB: request.hostMemoryMB) {
                    Label(warning, systemImage: "exclamationmark.triangle")
                        .foregroundStyle(.orange)
                        .fixedSize(horizontal: false, vertical: true)
                }
                if tried && !problems.isEmpty {
                    ForEach(problems, id: \.self) { problem in
                        Label(problem, systemImage: "exclamationmark.circle").foregroundStyle(.red)
                    }
                }
            }
            .formStyle(.columns)
            .padding(.horizontal, 24)
            .padding(.vertical, 14)
            HStack {
                Spacer()
                if creating {
                    ProgressView().controlSize(.small)
                    Text("Copying the files…").foregroundStyle(.secondary)
                }
                Button("Cancel") { dismiss() }
                    .keyboardShortcut(.cancelAction)
                    .disabled(creating)
                Button("Create", action: add)
                    .buttonStyle(.borderedProminent)
                    .keyboardShortcut(.defaultAction)
                    .disabled(creating)
            }
            .padding([.horizontal, .bottom], 24)
        }
        .frame(width: 620)
        .onChange(of: request) { old, new in
            // The name follows the disk image's file name until the person types their own.
            if old.disk != new.disk {
                if new.name.isEmpty || new.name == Self.name(fromFile: old.disk) {
                    request.name = Self.name(fromFile: new.disk)
                }
                useFirmware(FirmwareFiles.beside(disk: new.disk))
            }
            problems = Core.problems(with: request)
        }
        .onAppear {
            request.cores = processorChoices.contains(4) ? 4 : (processorChoices.last ?? 1)
            request.memoryMB = min(6, memoryRange.upperBound) * 1024
            problems = Core.problems(with: request)
        }
    }

    // MARK: Firmware

    /// Found beside the disk image, or chosen: the names of the files, with Choose….
    private var firmwareRow: some View {
        LabeledContent("Firmware") {
            HStack {
                Text(firmwareText)
                    .foregroundStyle(firmware.complete ? Color.primary : Color.secondary)
                    .truncationMode(.middle)
                Button("Choose…", action: chooseFirmware)
            }
        }
    }

    private var firmwareText: String {
        if firmware.complete {
            return request.disk.isEmpty ? "" : String(localized: "Found beside the disk image")
        }
        return request.disk.isEmpty ? String(localized: "Found beside the disk image once it is chosen") : String(localized: "Not found beside the disk image")
    }

    private func useFirmware(_ files: FirmwareFiles) {
        firmware = files
        request.firmwareCode = files.code ?? ""
        request.firmwareVars = files.vars ?? ""
        request.optionROM = files.optionROM ?? ""
    }

    /// Choosing by hand: the code, variables and (optionally) option ROM files together, told
    /// apart by their names.
    private func chooseFirmware() {
        let panel = NSOpenPanel()
        panel.canChooseFiles = true
        panel.canChooseDirectories = false
        panel.allowsMultipleSelection = true
        panel.message = String(localized: "Choose the firmware code and variables files (and the option ROM, if there is one) together")
        guard panel.runModal() == .OK else { return }
        useFirmware(FirmwareFiles.sorted(panel.urls.map { $0.path(percentEncoded: false) }))
    }

    // MARK: The rest

    private var memorySlider: Binding<Double> {
        Binding(get: { Double(request.memoryMB / 1024) }, set: { request.memoryMB = Int($0) * 1024 })
    }

    private var diskSize: String? {
        DiskUsage.read(path: request.disk).map { MachineFormat.fileSize($0.size) }
    }

    /// A first name for the machine, from the disk image's file name.
    private static func name(fromFile path: String) -> String {
        path.isEmpty ? "" : URL(filePath: path).deletingPathExtension().lastPathComponent
    }

    private func add() {
        tried = true
        problems = Core.problems(with: request)
        guard problems.isEmpty else { return }
        creating = true
        let chosen = request
        Task {
            let result = await Task.detached { Core.create(chosen) }.value
            creating = false
            switch result {
            case .success(let id):
                library.reload()
                library.selection = library.machines.first(where: { $0.agentID == id })?.id
                dismiss()
            case .failure(let failure):
                problems = failure.problems
            }
        }
    }
}
