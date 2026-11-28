// SPDX-License-Identifier: GPL-2.0-or-later
import Foundation
import GuesthouseCore

/// The agent core's handle, which the core allows to be used from any thread.
struct AgentHandle: @unchecked Sendable {
    let raw: OpaquePointer
}

/// How long with nothing running and nobody connected before the agent leaves. launchd starts
/// it again when the app next asks.
private let idleLimit = 10

/// Answers on the mach service `agentMachService` and never returns.
func serveXPC(agent: OpaquePointer) -> Never {
    let handle = AgentHandle(raw: agent)
    let server = XPCServer(agent: handle)
    let listener = NSXPCListener(machServiceName: agentMachService)
    listener.delegate = server
    // Only our own app may talk to the agent: its identifier, signed by the agent's own team.
    listener.setConnectionCodeSigningRequirement(peerRequirement(identifier: "com.ruxn.guesthouse"))
    listener.resume()
    Thread.detachNewThread {
        var idleSeconds = 0
        while true {
            sleep(1)
            idleSeconds = guesthouse_agent_is_idle(handle.raw) ? idleSeconds + 1 : 0
            if idleSeconds >= idleLimit {
                fputs("guesthouse-agent: idle, leaving\n", stderr)
                exit(0)
            }
        }
    }
    RunLoop.main.run()
    exit(0)
}

final class XPCServer: NSObject, NSXPCListenerDelegate {
    let agent: AgentHandle

    init(agent: AgentHandle) {
        self.agent = agent
    }

    func listener(_ listener: NSXPCListener, shouldAcceptNewConnection connection: NSXPCConnection) -> Bool {
        let service = XPCService(agent: agent, connection: connection)
        connection.exportedInterface = agentServiceInterface()
        connection.remoteObjectInterface = NSXPCInterface(with: AgentListener.self)
        connection.exportedObject = service
        connection.invalidationHandler = { service.stopListening() }
        connection.resume()
        return true
    }
}

/// One app connection.
final class XPCService: NSObject, AgentService, @unchecked Sendable {
    private let agent: AgentHandle
    private let connection: NSXPCConnection
    private let lock = NSLock()
    private var listening = true

    init(agent: AgentHandle, connection: NSXPCConnection) {
        self.agent = agent
        self.connection = connection
    }

    func request(_ line: String, reply: @escaping (String) -> Void) {
        guard let answer = guesthouse_agent_handle(agent.raw, line) else {
            reply(#"{"v":1,"id":0,"ok":false,"error":"the agent gave no answer"}"#)
            return
        }
        reply(String(cString: answer))
        guesthouse_string_free(answer)
    }

    func subscribe(_ listener: AgentListener) {
        guard let events = guesthouse_agent_events(agent.raw) else { return }
        let box = EventsBox(raw: events)
        nonisolated(unsafe) let listener = listener
        Thread.detachNewThread { [self] in
            while isListening {
                guard let line = guesthouse_events_next(box.raw, 1000) else { continue }
                listener.event(String(cString: line))
                guesthouse_string_free(line)
            }
            guesthouse_events_close(box.raw)
        }
    }

    func stopListening() {
        lock.lock()
        listening = false
        lock.unlock()
    }

    private var isListening: Bool {
        lock.lock()
        defer { lock.unlock() }
        return listening
    }
}

private struct EventsBox: @unchecked Sendable {
    let raw: OpaquePointer
}
