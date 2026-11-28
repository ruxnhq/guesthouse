// SPDX-License-Identifier: GPL-2.0-or-later
import AppKit
import SwiftUI

/// How to sign in to a machine that Guesthouse installed: the account and its password, which is
/// kept in the Mac's Keychain and shown only when asked for, with a button to copy it.
struct GuestSignIn: View {
    let machine: Machine
    let account: String
    @State private var shown = false
    @State private var copied = false

    private var password: String? { PasswordKeychain.read(for: machine.agentID) }

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            line("Account") { Text(account).textSelection(.enabled) }
            line("Password") {
                if let password {
                    Text(shown ? password : String(repeating: "•", count: password.count))
                        .font(.body.monospaced())
                        .textSelection(.enabled)
                    Button(shown ? "Hide" : "Show") { shown.toggle() }
                    Button(copied ? "Copied" : "Copy") { copy(password) }
                } else {
                    Text("Not in this Mac's Keychain.").foregroundStyle(.secondary)
                }
            }
        }
    }

    private func line<Content: View>(_ title: LocalizedStringKey, @ViewBuilder _ content: () -> Content) -> some View {
        HStack(spacing: 10) {
            Text(title).foregroundStyle(.secondary).frame(width: 70, alignment: .leading)
            content()
        }
    }

    /// Copies the password for this Mac only, marked as a secret for clipboard tools (the
    /// nspasteboard.org marker), and takes it off the pasteboard after a minute unless something
    /// else was copied since.
    private func copy(_ password: String) {
        let pasteboard = NSPasteboard.general
        pasteboard.prepareForNewContents(with: .currentHostOnly)
        pasteboard.setString(password, forType: .string)
        pasteboard.setString("", forType: NSPasteboard.PasteboardType("org.nspasteboard.ConcealedType"))
        let ours = pasteboard.changeCount
        copied = true
        Task {
            try? await Task.sleep(for: .seconds(2))
            copied = false
            try? await Task.sleep(for: .seconds(58))
            if pasteboard.changeCount == ours {
                pasteboard.clearContents()
            }
        }
    }
}
