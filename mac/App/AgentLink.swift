// SPDX-License-Identifier: GPL-2.0-or-later
import Foundation
import GuesthouseCore

/// A way to reach the agent: lines go out, and lines (answers and state changes) come back to
/// `onLine`. The words are the same on both, so nothing above this file knows which it has.
nonisolated protocol AgentLink: AnyObject, Sendable {
    func send(_ line: String)
    func close()
    /// The process id of the agent at the other end, or 0 when it cannot be told.
    var peerPID: Int32 { get }
}

/// The agent's Unix socket, for an agent the app started itself.
nonisolated final class SocketLink: AgentLink, @unchecked Sendable {
    private let lock = NSLock()
    private nonisolated(unsafe) var connection: OpaquePointer?
    private var closing = false
    nonisolated let peerPID: Int32

    /// nil when no agent is listening on `path`.
    init?(path: String, onLine: @escaping @Sendable (String) -> Void, onClose: @escaping @Sendable () -> Void) {
        guard let opened = guesthouse_connection_open(path) else { return nil }
        connection = opened
        peerPID = guesthouse_connection_peer_pid(opened)
        let handle = ConnectionHandle(raw: opened)
        // The thread owns the connection's end of life: it closes it once `close()` was called
        // or the agent hung up, so no send can be using a freed connection.
        Thread.detachNewThread { [self] in
            var hungUp = false
            while !isClosing {
                if let line = guesthouse_connection_receive(handle.raw, 500, &hungUp) {
                    onLine(String(cString: line))
                    guesthouse_string_free(line)
                } else if hungUp {
                    break
                }
            }
            lock.lock()
            self.connection = nil
            guesthouse_connection_close(handle.raw)
            lock.unlock()
            if hungUp { onClose() }
        }
    }

    func send(_ line: String) {
        lock.lock()
        defer { lock.unlock() }
        if let connection {
            _ = guesthouse_connection_send(connection, line)
        }
    }

    func close() {
        lock.lock()
        closing = true
        lock.unlock()
    }

    private var isClosing: Bool {
        lock.lock()
        defer { lock.unlock() }
        return closing
    }
}

/// The agent's mach service, for the agent launchd runs.
nonisolated final class XPCLink: NSObject, AgentLink, AgentListener, @unchecked Sendable {
    private let connection: NSXPCConnection
    private let onLine: @Sendable (String) -> Void

    /// Known once the service has answered something.
    var peerPID: Int32 { connection.processIdentifier }

    init(onLine: @escaping @Sendable (String) -> Void, onClose: @escaping @Sendable () -> Void) {
        self.onLine = onLine
        connection = NSXPCConnection(machServiceName: agentMachService)
        super.init()
        connection.remoteObjectInterface = agentServiceInterface()
        connection.exportedInterface = NSXPCInterface(with: AgentListener.self)
        connection.exportedObject = self
        // Only our own agent may answer: its identifier, signed by the app's own team.
        connection.setCodeSigningRequirement(peerRequirement(identifier: "com.ruxn.guesthouse.agent"))
        connection.invalidationHandler = onClose
        connection.interruptionHandler = onClose
        connection.resume()
        service?.subscribe(self)
    }

    private var service: AgentService? {
        connection.remoteObjectProxyWithErrorHandler { _ in } as? AgentService
    }

    func send(_ line: String) {
        let onLine = onLine
        service?.request(line) { onLine($0) }
    }

    func event(_ line: String) {
        onLine(line)
    }

    func close() {
        connection.invalidationHandler = nil
        connection.interruptionHandler = nil
        connection.invalidate()
    }
}

/// The core's connection, which it allows to be used from the reading thread and the sending one.
private nonisolated struct ConnectionHandle: @unchecked Sendable {
    let raw: OpaquePointer
}
