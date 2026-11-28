// SPDX-License-Identifier: GPL-2.0-or-later
import Foundation
import GuesthouseCore

/// Where the app keeps the programs that ship inside it, beside the app's own.
nonisolated enum BundledHelpers {
    /// `Guesthouse.app/Contents/Helpers`.
    static var folder: URL {
        Bundle.main.bundleURL.appending(path: "Contents/Helpers", directoryHint: .isDirectory)
    }
}

/// A program the app runs that is not part of it.
nonisolated enum Tool: String {
    /// Runs the machines.
    case qemu
    /// Shares a machine's screen with other devices.
    case rmhost
    /// The graphics process of the protected path.
    case pvgHost = "pvg-host"
    /// A QEMU that has the protected graphics device.
    case pvgQemu = "pvg-qemu"

    var fileName: String {
        switch self {
        case .qemu, .pvgQemu: "qemu-system-aarch64"
        case .rmhost: "rmhost"
        case .pvgHost: "guesthouse-pvg-host"
        }
    }

    /// Said when it cannot be found.
    var fallbackSentence: String {
        switch self {
        case .qemu: String(localized: "Guesthouse cannot find QEMU, the program that runs the machines.")
        case .rmhost: String(localized: "Guesthouse cannot find rmhost, the program that shares a machine's screen.")
        case .pvgHost: String(localized: "Guesthouse cannot find guesthouse-pvg-host, the protected graphics process.")
        case .pvgQemu: String(localized: "Guesthouse cannot find a QEMU with the protected graphics device.")
        }
    }
}

/// The program that will run, and why that one. The search itself is in the core, where it is
/// tested; this is its answer in Swift types.
nonisolated enum ToolLocation: Equatable {
    case found(path: String, source: Source, why: String)
    /// A sentence saying what is missing.
    case missing(String)

    enum Source: String {
        case setting, environment, bundled, lab
    }

    /// Looks in the places the core knows, with `setting` as Settings holds it (QEMU only).
    static func find(_ tool: Tool, setting: String = "") -> ToolLocation {
        let bundled = BundledHelpers.folder.appending(path: tool.fileName).path(percentEncoded: false)
        guard let text = guesthouse_tool_locate(tool.rawValue, setting, bundled, isSignedBuild()),
              let answer = try? JSONDecoder().decode(Answer.self, from: Data(String(cString: text).utf8))
        else { return .missing(tool.fallbackSentence) }
        defer { guesthouse_string_free(text) }
        if let path = answer.path, let source = answer.source.flatMap(Source.init(rawValue:)) {
            return .found(path: path, source: source, why: answer.why ?? "")
        }
        return .missing(answer.error ?? tool.fallbackSentence)
    }

    /// The path to run, if one was found.
    var path: String? {
        if case .found(let path, _, _) = self { path } else { nil }
    }

    private struct Answer: Decodable {
        let path: String?
        let source: String?
        let why: String?
        let error: String?
    }
}
