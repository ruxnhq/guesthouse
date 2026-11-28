// SPDX-License-Identifier: GPL-2.0-or-later
import Foundation
import Security

/// The code-signing requirement the app and the agent hold each other to: the other's identifier,
/// and, when this program is signed by a team, a certificate Apple issued to that same team. An
/// ad hoc build has no team, and its peer is then held to the identifier.
nonisolated func peerRequirement(identifier: String) -> String {
    codeRequirement(identifier: identifier, team: signingTeam())
}

/// The requirement for `identifier`, signed by `team` when there is one. A team identifier is
/// letters and digits; anything else is taken for no team, as it is no team Apple gives.
nonisolated func codeRequirement(identifier: String, team: String?) -> String {
    let named = "identifier \"\(identifier)\""
    guard let team, !team.isEmpty, team.allSatisfy({ $0.isASCII && ($0.isLetter || $0.isNumber) }) else {
        return named
    }
    return "\(named) and anchor apple generic and certificate leaf[subject.OU] = \"\(team)\""
}

/// The team this program is signed by, or nil when it is signed ad hoc or not at all.
nonisolated func signingTeam() -> String? {
    var code: SecCode?
    guard SecCodeCopySelf([], &code) == errSecSuccess, let code else { return nil }
    var staticCode: SecStaticCode?
    guard SecCodeCopyStaticCode(code, [], &staticCode) == errSecSuccess, let staticCode else { return nil }
    var information: CFDictionary?
    let flags = SecCSFlags(rawValue: kSecCSSigningInformation)
    guard SecCodeCopySigningInformation(staticCode, flags, &information) == errSecSuccess,
        let information = information as? [String: Any]
    else { return nil }
    return information[kSecCodeInfoTeamIdentifier as String] as? String
}
