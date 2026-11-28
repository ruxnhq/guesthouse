// SPDX-License-Identifier: GPL-2.0-or-later
import AppKit
import SwiftUI

/// A machine's pane while it is being installed: the steps and the time left, what stopped and
/// what to do about it, or that it is ready. Which one follows the install's state.
struct InstallView: View {
    let machine: Machine
    let install: InstallStatus
    @Environment(Library.self) private var library

    var body: some View {
        switch install.state {
        case "failed": InstallFailedView(machine: machine, install: install)
        case "done": InstallDoneView(machine: machine, install: install)
        default: InstallProgressView(machine: machine, install: install)
        }
    }
}

/// The steps as one list: done, the one going on with its bar, the ones to come, or the one that
/// stopped.
struct InstallStepList: View {
    let install: InstallStatus

    var body: some View {
        VStack(spacing: 0) {
            ForEach(Array(install.shown.enumerated()), id: \.element.name) { index, step in
                if index > 0 { Divider() }
                row(step)
            }
        }
        .card(padding: 0)
    }

    private func row(_ step: InstallStatus.Step) -> some View {
        let stopped = install.failure?.step == step.name
        let now = step.name == install.current && install.state == "running"
        return VStack(alignment: .leading, spacing: 8) {
            HStack(spacing: 14) {
                icon(step, stopped: stopped, now: now).frame(width: 22, height: 22)
                Text(install.title(of: step.name))
                    .fontWeight(now || stopped ? .semibold : .regular)
                    .foregroundStyle(step.status == "pending" ? .secondary : .primary)
                Spacer()
                if now { Text("\(install.percent)%").monospacedDigit() }
                if stopped { Text("Stopped").foregroundStyle(.red) }
            }
            if now {
                ProgressView(value: step.fraction).padding(.leading, 36)
                if let minutes = install.quietMinutes {
                    Text(InstallText.quietHint(minutes: minutes))
                        .font(.callout)
                        .foregroundStyle(.secondary)
                        .padding(.leading, 36)
                        .fixedSize(horizontal: false, vertical: true)
                }
            }
        }
        .padding(.horizontal, 20)
        .frame(minHeight: now ? 80 : 52)
        .accessibilityElement(children: .combine)
    }

    @ViewBuilder
    private func icon(_ step: InstallStatus.Step, stopped: Bool, now: Bool) -> some View {
        if stopped {
            Image(systemName: "exclamationmark.circle.fill").font(.title2).foregroundStyle(.red)
        } else if step.status == "done" {
            Image(systemName: "checkmark.circle.fill").font(.title2).foregroundStyle(.green)
        } else if now {
            ProgressView().controlSize(.small)
        } else {
            Image(systemName: "circle").font(.title2).foregroundStyle(.tertiary)
        }
    }
}

private struct InstallProgressView: View {
    let machine: Machine
    let install: InstallStatus
    @Environment(Library.self) private var library
    @Environment(\.openWindow) private var openWindow
    @State private var confirmingStop = false

    var body: some View {
        VStack(alignment: .leading, spacing: 26) {
            VStack(alignment: .leading, spacing: 8) {
                Text("Installing \(machine.name)").font(.largeTitle.weight(.semibold))
                Text(subtitle).foregroundStyle(.secondary)
            }
            InstallStepList(install: install)
            if install.current == "install_driver", install.guest == "ubuntu" {
                Label("The driver is added to Ubuntu as a kernel module with its own graphics libraries. Guesthouse keeps it up to date.",
                      systemImage: "info.circle")
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
            HStack {
                Button("Watch It Install") { openWindow(id: ScreenWindow.windowID, value: machine.id) }
                    .disabled(machine.frames == nil || !library.status(of: machine).isAlive)
                Spacer()
                Button("Stop Installing", role: .destructive) { confirmingStop = true }
            }
        }
        .frame(maxWidth: 640)
        .padding(.horizontal, 50)
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .center)
        .confirmationDialog("Stop installing \(machine.name)?", isPresented: $confirmingStop) {
            Button("Stop Installing", role: .destructive) {
                library.run { await library.agent.installCancel(machine: machine.agentID) }
            }
        } message: {
            Text("What is done is kept. The machine stays in the list.")
        }
    }

    private var subtitle: String {
        let left = install.secondsLeft.map { InstallStatus.timeLeft(seconds: $0) + " " } ?? ""
        return left + String(localized: "You can close this window, it carries on.")
    }
}

private struct InstallFailedView: View {
    let machine: Machine
    let install: InstallStatus
    @Environment(Library.self) private var library

    var body: some View {
        VStack(alignment: .leading, spacing: 22) {
            VStack(alignment: .leading, spacing: 8) {
                Text(install.failureTitle).font(.largeTitle.weight(.semibold))
                Text(install.failureSentence).foregroundStyle(.secondary)
                if let code = install.failure?.code {
                    Text("Error \(code)").font(.callout).foregroundStyle(.tertiary)
                }
            }
            InstallStepList(install: install)
            HStack {
                ForEach(Array((install.failure?.choices ?? []).enumerated()), id: \.element) { index, choice in
                    if index == 0 {
                        Button(install.label(forChoice: choice)) { choose(choice) }
                            .buttonStyle(.borderedProminent)
                    } else {
                        Button(install.label(forChoice: choice)) { choose(choice) }
                    }
                }
            }
        }
        .frame(maxWidth: 640)
        .padding(.horizontal, 50)
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .center)
    }

    private func choose(_ choice: String) {
        switch choice {
        case "choose_installer":
            let panel = NSOpenPanel()
            panel.canChooseDirectories = false
            panel.message = String(localized: "Choose an installer (.iso) you already have")
            guard panel.runModal() == .OK, let url = panel.url else { return }
            send(choice, path: url.path(percentEncoded: false))
        case "send_report":
            library.report(machine: machine, kind: .installFailed)
        default:
            send(choice, path: nil)
        }
    }

    private func send(_ choice: String, path: String?) {
        // The agent keeps the password in memory only, so a new one asks for it again here.
        let password = PasswordKeychain.read(for: machine.agentID)
        library.run {
            await library.agent.installChoose(machine: machine.agentID, choice: choice, path: path, password: password)
        }
    }
}

private struct InstallDoneView: View {
    let machine: Machine
    let install: InstallStatus
    @Environment(Library.self) private var library
    @Environment(\.openWindow) private var openWindow

    var body: some View {
        VStack(spacing: 26) {
            Image(systemName: "checkmark.circle.fill")
                .font(.system(size: 64))
                .foregroundStyle(.green)
                .accessibilityHidden(true)
            VStack(spacing: 10) {
                Text("\(machine.name) is ready").font(.largeTitle.weight(.semibold))
                Text("It is running and drawn by this Mac's GPU. Its screen opens in a window of its own, and its size follows that window.")
                    .foregroundStyle(.secondary)
                    .multilineTextAlignment(.center)
                    .frame(maxWidth: 520)
            }
            VStack(spacing: 12) {
                Button("Open Screen") {
                    openWindow(id: ScreenWindow.windowID, value: machine.id)
                    library.markInstallSeen(machine)
                }
                .buttonStyle(.borderedProminent)
                .controlSize(.large)
                Button("Show the Machine") { library.markInstallSeen(machine) }
                    .buttonStyle(.link)
            }
            VStack(spacing: 10) {
                Text("Sign in with this account. Guesthouse made the password and keeps it in your Keychain; it is also under Details.")
                    .foregroundStyle(.secondary)
                    .multilineTextAlignment(.center)
                GuestSignIn(machine: machine, account: install.account)
                    .card(padding: 14)
                    .fixedSize()
                Text("Viewing from your other devices is under Sharing")
                    .foregroundStyle(.secondary)
            }
            if install.guest == "windows" {
                Text("This copy of Windows runs in test mode, so it can load the Guesthouse graphics driver. Windows says \"Test Mode\" in the corner of its desktop.")
                    .foregroundStyle(.secondary)
                    .multilineTextAlignment(.center)
                    .frame(maxWidth: 520)
            }
        }
        .padding(40)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }
}
