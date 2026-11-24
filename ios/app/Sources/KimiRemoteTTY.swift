// KimiRemoteTTY: lightweight remote PTY backend for the IPA.
//
// iOS cannot fork/exec Linux guests.  Instead the terminal can attach to a
// Kimi PTY bridge (tools/kimi-pty-bridge.py) running on any phone/Linux/macOS
// host.  The bridge owns the real PTY and executes opencode/shell commands; the
// iPad only transports bytes.  No emulator is involved.
//
// The bridge protocol is KPTY/2: the iPad asks the token-protected bridge to
// run the requested command line, then exchanges raw terminal frames.
// Configuration is read from Documents/disk/remote-tty.cfg:
//
//     host:port token
//
// (token is optional). The same file is visible in the Files app when file
// sharing is enabled by the IPA.

import Foundation
import Network

final class KimiRemoteTTY {
    static let shared = KimiRemoteTTY()

    struct Config {
        let host: String
        let port: UInt16
        let token: String
    }

    private final class Session {
        let id: UInt64
        let queue: DispatchQueue
        let lock = NSLock()
        var connection: NWConnection?
        var out = Data()
        var rx = Data()
        var handshaken = false
        var exited = false
        var exitCode: Int32 = 0
        let cols: UInt16
        let rows: UInt16
        let config: Config
        let bin: String
        let arg: String

        init(id: UInt64, config: Config, cols: UInt64, rows: UInt64,
             bin: String, arg: String) {
            self.id = id
            self.config = config
            self.cols = UInt16(clamping: cols)
            self.rows = UInt16(clamping: rows)
            self.bin = bin
            self.arg = arg
            self.queue = DispatchQueue(label: "kimios.remote.tty.\(id)")
        }

        func start() {
            guard let port = NWEndpoint.Port(rawValue: config.port) else {
                markExit(2); return
            }
            let conn = NWConnection(host: NWEndpoint.Host(config.host),
                                    port: port, using: .tcp)
            connection = conn
            conn.stateUpdateHandler = { [weak self] state in
                guard let self else { return }
                switch state {
                case .ready:
                    let token = self.config.token
                    let cmd = Data(self.bin.utf8).base64EncodedString()
                    let arg = Data(self.arg.utf8).base64EncodedString()
                    let hello = "KPTY/2 \(self.cols) \(self.rows) \(token) \(cmd) \(arg)\n"
                    conn.send(content: Data(hello.utf8), completion: .contentProcessed { _ in })
                    self.receiveLoop()
                case .failed, .cancelled:
                    self.markExit(3)
                default:
                    break
                }
            }
            conn.start(queue: queue)
        }

        private func receiveLoop() {
            connection?.receive(minimumIncompleteLength: 1, maximumLength: 65536) {
                [weak self] data, _, complete, error in
                guard let self else { return }
                if let data, !data.isEmpty { self.ingest(data) }
                if complete || error != nil { self.markExit(3); return }
                if !self.isExited() { self.receiveLoop() }
            }
        }

        private func ingest(_ data: Data) {
            lock.lock()
            rx.append(data)
            if !handshaken {
                if let nl = rx.firstIndex(of: 0x0a) {
                    let lineData = rx[..<nl]
                    let line = String(data: Data(lineData), encoding: .utf8) ?? ""
                    let remove = rx.distance(from: rx.startIndex, to: nl) + 1
                    rx.removeFirst(remove)
                    if line.hasPrefix("OK") {
                        handshaken = true
                    } else {
                        lock.unlock(); markExit(2); return
                    }
                } else {
                    lock.unlock(); return
                }
            }
            while rx.count >= 5 {
                let bytes = [UInt8](rx.prefix(5))
                let len = (Int(bytes[1]) << 24) | (Int(bytes[2]) << 16) |
                          (Int(bytes[3]) << 8) | Int(bytes[4])
                if rx.count < 5 + len { break }
                let typ = bytes[0]
                let payload = rx.subdata(in: 5..<(5 + len))
                rx.removeFirst(5 + len)
                if typ == 0 {
                    out.append(payload)
                } else if typ == 3 {
                    var code: Int32 = 0
                    if payload.count >= 4 {
                        code = Int32(bitPattern: (UInt32(payload[0]) << 24) |
                                                   (UInt32(payload[1]) << 16) |
                                                   (UInt32(payload[2]) << 8) |
                                                   UInt32(payload[3]))
                    }
                    exited = true; exitCode = code
                }
            }
            lock.unlock()
        }

        private func markExit(_ code: Int32) {
            lock.lock()
            exited = true
            exitCode = code
            lock.unlock()
        }

        private func isExited() -> Bool {
            lock.lock(); let e = exited; lock.unlock(); return e
        }

        func read(into ptr: UnsafeMutableRawPointer, cap: UInt64) -> UInt64 {
            lock.lock()
            let n = min(out.count, Int(cap))
            if n > 0 {
                out.copyBytes(to: ptr.assumingMemoryBound(to: UInt8.self), count: n)
                out.removeFirst(n)
            }
            lock.unlock()
            return UInt64(n)
        }

        private func sendFrame(_ type: UInt8, _ payload: Data) {
            var frame = Data([type])
            let n = UInt32(payload.count)
            frame.append(UInt8((n >> 24) & 0xff))
            frame.append(UInt8((n >> 16) & 0xff))
            frame.append(UInt8((n >> 8) & 0xff))
            frame.append(UInt8(n & 0xff))
            frame.append(payload)
            connection?.send(content: frame, completion: .contentProcessed { _ in })
        }

        func write(_ data: Data) -> UInt64 {
            guard !isExited() else { return 0 }
            sendFrame(1, data)
            return UInt64(data.count)
        }

        func resize(rows: UInt64, cols: UInt64) {
            var p = Data()
            let r = UInt16(clamping: rows), c = UInt16(clamping: cols)
            p.append(UInt8((r >> 8) & 0xff)); p.append(UInt8(r & 0xff))
            p.append(UInt8((c >> 8) & 0xff)); p.append(UInt8(c & 0xff))
            sendFrame(2, p)
        }

        func status() -> UInt64 {
            lock.lock(); let e = exited; let c = exitCode; lock.unlock()
            return e ? UInt64(UInt32(bitPattern: c)) + 1 : 0
        }

        func close() {
            connection?.cancel()
            markExit(0)
        }
    }

    private let lock = NSLock()
    private var sessions: [UInt64: Session] = [:]
    private var nextSlot: UInt64 = 1

    /// Read `Documents/disk/remote-tty.cfg`, or KIMI_REMOTE_TTY in the
    /// environment (useful for the host harness).
    func config() -> Config? {
        if let env = ProcessInfo.processInfo.environment["KIMI_REMOTE_TTY"], !env.isEmpty {
            if let c = parse(env) { return c }
        }
        let docs = FileManager.default.urls(for: .documentDirectory, in: .userDomainMask)[0]
        let file = docs.appendingPathComponent("disk/remote-tty.cfg")
        guard let text = try? String(contentsOf: file, encoding: .utf8) else { return nil }
        for line in text.split(whereSeparator: { $0 == "\n" || $0 == "\r" }) {
            let s = String(line).trimmingCharacters(in: .whitespaces)
            if s.isEmpty || s.hasPrefix("#") { continue }
            if let c = parse(s) { return c }
        }
        return nil
    }

    private func parse(_ text: String) -> Config? {
        var parts = text.split(whereSeparator: { $0 == " " || $0 == "\t" }).map(String.init)
        guard !parts.isEmpty else { return nil }
        var endpoint = parts.removeFirst()
        let token = parts.first ?? ""
        if endpoint.hasPrefix("tcp://") { endpoint.removeFirst(6) }
        let comps = endpoint.split(separator: ":", maxSplits: 1).map(String.init)
        guard let host = comps.first, !host.isEmpty else { return nil }
        let port = comps.count > 1 ? UInt16(comps[1]) : 7681
        guard let port else { return nil }
        return Config(host: host, port: port, token: token)
    }

    func supported() -> Bool { config() != nil }

    func spawn(bin: String, arg: String, rows: UInt64, cols: UInt64) -> UInt64? {
        guard let cfg = config() else { return nil }
        lock.lock()
        let id = nextSlot; nextSlot += 1
        let s = Session(id: id, config: cfg, cols: cols, rows: rows, bin: bin, arg: arg)
        sessions[id] = s
        lock.unlock()
        s.start()
        return id
    }

    func read(slot: UInt64, into ptr: UnsafeMutableRawPointer, cap: UInt64) -> UInt64 {
        lock.lock(); let s = sessions[slot]; lock.unlock()
        return s?.read(into: ptr, cap: cap) ?? UInt64.max
    }

    func write(slot: UInt64, data: Data) -> UInt64 {
        lock.lock(); let s = sessions[slot]; lock.unlock()
        return s?.write(data) ?? UInt64.max
    }

    func resize(slot: UInt64, rows: UInt64, cols: UInt64) -> Bool {
        lock.lock(); let s = sessions[slot]; lock.unlock()
        guard let s else { return false }
        s.resize(rows: rows, cols: cols)
        return true
    }

    func status(slot: UInt64) -> UInt64 {
        lock.lock(); let s = sessions[slot]; lock.unlock()
        return s?.status() ?? UInt64.max
    }

    func close(slot: UInt64) -> Bool {
        lock.lock(); let s = sessions.removeValue(forKey: slot); lock.unlock()
        guard let s else { return false }
        s.close()
        return true
    }
}