// SPDX-License-Identifier: GPL-2.0-or-later
import Foundation
import GuesthouseCore

/// Which agent is this app's own. The core makes the identity (`agent_build.rs`) of the agent
/// program in this app's bundle, as the agent makes it of itself, and judges what an agent that
/// answered a listing is.
nonisolated enum AgentBuild {
    private static var program: String {
        Bundle.main.bundleURL.appending(path: "Contents/MacOS/guesthouse-agent").path(percentEncoded: false)
    }

    private static func ask(_ request: [String: Any]) -> [String: Any]? {
        guard let data = try? JSONSerialization.data(withJSONObject: request),
              let text = String(data: data, encoding: .utf8),
              let answer = guesthouse_agent_build(text)
        else { return nil }
        defer { guesthouse_string_free(answer) }
        return (try? JSONSerialization.jsonObject(with: Data(String(cString: answer).utf8))) as? [String: Any]
    }

    /// The identity of this app's own agent program, or nil when the bundle has none (a build
    /// run from a folder without the agent), when every agent is taken as it is.
    static func expected() -> [String: Any]? {
        let info = Bundle.main.infoDictionary
        let found = ask([
            "what": "identity", "program": program,
            "version": info?["CFBundleShortVersionString"] as? String ?? "0",
            "build": info?["CFBundleVersion"] as? String ?? "0",
        ])
        return found?["identity"] as? [String: Any]
    }

    /// What to do about the agent whose answer to `list` is `line`.
    static func verdict(ofListing line: String) -> AgentVerdict {
        guard let expected = expected(),
              let answer = try? JSONSerialization.jsonObject(with: Data(line.utf8)) as? [String: Any],
              let judged = ask(["what": "judge", "expected": expected, "answer": answer]),
              let data = try? JSONSerialization.data(withJSONObject: judged),
              let verdict = AgentVerdict.from(core: String(decoding: data, as: UTF8.self))
        else { return .own }
        return verdict
    }
}
