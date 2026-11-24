// KimiNode: local OpenCode server runtime for iOS.
//
// NodeMobile (Node 24.5) executes a Node-target OpenCode bundle inside the app
// process. The shared Rust OpenCode client talks to it at 127.0.0.1:4199 over
// the existing SYS_NET implementation. No emulator, no fork/exec, no remote
// server.

import Foundation
import Darwin
import NodeMobile

final class KimiNode {
    static let shared = KimiNode()

    private let lock = NSLock()
    private var started = false
    private var ready = false
    private var failed = false
    private var password = ""
    private var port: UInt16 = 4199
    private var output = Data()
    private var activeSlot: UInt64 = 0
    private var nextSlot: UInt64 = 0x4E4F4445_0000_0001

    private init() {}

    /// Directory copied by tools/build-opencode-node.sh into the app bundle.
    private var resourceRoot: URL? {
        guard let base = Bundle.main.resourceURL else { return nil }
        for candidate in ["opencode", "Resources/opencode"] {
            let url = base.appendingPathComponent(candidate, isDirectory: true)
            if FileManager.default.fileExists(
                atPath: url.appendingPathComponent("opencode.mjs").path) {
                return url
            }
        }
        return base.appendingPathComponent("opencode", isDirectory: true)
    }

    var supported: Bool {
        guard let root = resourceRoot else { return false }
        return FileManager.default.fileExists(
            atPath: root.appendingPathComponent("opencode.mjs").path)
    }

    private var disk: URL {
        FileManager.default.urls(for: .documentDirectory, in: .userDomainMask)[0]
            .appendingPathComponent("disk", isDirectory: true)
    }

    /// Create the marker consumed by user/kimi-gui/src/opencode.rs so the app
    /// knows the local Node runtime is installed instead of the Alpine binary.
    func prepare() {
        guard supported else { return }
        let markerDir = disk.appendingPathComponent("opencode", isDirectory: true)
        try? FileManager.default.createDirectory(at: markerDir, withIntermediateDirectories: true)
        let marker = markerDir.appendingPathComponent("node-runtime.marker")
        if !FileManager.default.fileExists(atPath: marker.path) {
            FileManager.default.createFile(atPath: marker.path, contents: Data("node-24.5\n".utf8))
        }
    }

    func canRun(bin: String, arg: String) -> Bool {
        let name = (bin as NSString).lastPathComponent
        let first = arg.split(whereSeparator: { $0 == " " || $0 == "\t" }).first.map(String.init) ?? ""
        return name == "opencode" && first == "serve"
    }

    func isLocalSlot(_ slot: UInt64) -> Bool {
        (slot >> 32) == 0x4E4F4445
    }

    /// Start (or attach to) the single local server session. Returns a SYS_PROC
    /// slot whose read end contains the log lines the Rust client scans.
    func spawnServer(port requestedPort: UInt16) -> UInt64? {
        lock.lock()
        if !started {
            if !startLocked(port: requestedPort) {
                lock.unlock()
                return nil
            }
        }
        if failed {
            lock.unlock()
            return nil
        }
        let slot = nextSlot
        nextSlot &+= 1
        activeSlot = slot
        if ready {
            appendReadyLocked()
        }
        lock.unlock()
        return slot
    }

    func read(slot: UInt64, into ptr: UnsafeMutableRawPointer, cap: UInt64) -> UInt64 {
        lock.lock()
        guard slot == activeSlot else { lock.unlock(); return UInt64.max }
        let n = min(output.count, Int(cap))
        if n > 0 {
            output.copyBytes(to: ptr.assumingMemoryBound(to: UInt8.self), count: n)
            output.removeFirst(n)
        }
        lock.unlock()
        return UInt64(n)
    }

    func write(slot: UInt64, data: Data) -> UInt64 {
        lock.lock(); let ok = slot == activeSlot; lock.unlock()
        return ok ? UInt64(data.count) : UInt64.max
    }

    func resize(slot: UInt64, rows: UInt64, cols: UInt64) -> Bool {
        lock.lock(); let ok = slot == activeSlot; lock.unlock()
        return ok
    }

    func status(slot: UInt64) -> UInt64 {
        lock.lock()
        guard slot == activeSlot else { lock.unlock(); return UInt64.max }
        let f = failed
        lock.unlock()
        return f ? 2 : 0       // 0 = running; 2 = status code 1 + 1
    }

    func close(slot: UInt64) -> Bool {
        lock.lock()
        let ok = slot == activeSlot
        if ok { activeSlot = 0 }
        lock.unlock()
        return ok
    }

    // MARK: - startup

    private func startLocked(port: UInt16) -> Bool {
        guard supported, let root = resourceRoot else { return false }
        let mjs = root.appendingPathComponent("opencode.mjs")
        let assets = root.appendingPathComponent("assets", isDirectory: true)
        let loader = root.appendingPathComponent("loader.mjs")
        let ptyShim = root.appendingPathComponent("pty-shim.cjs")
        let watcherShim = root.appendingPathComponent("watcher-shim.cjs")
        for p in [mjs, loader, ptyShim, watcherShim] {
            guard FileManager.default.fileExists(atPath: p.path) else { return false }
        }

        self.port = port
        self.password = String(UUID().uuidString.replacingOccurrences(of: "-", with: "").prefix(32))
        let home = disk.appendingPathComponent("home", isDirectory: true)
        let config = home.appendingPathComponent(".config", isDirectory: true)
        try? FileManager.default.createDirectory(at: config, withIntermediateDirectories: true)
        try? FileManager.default.createDirectory(at: disk, withIntermediateDirectories: true)

        setEnv("HOME", home.path)
        setEnv("XDG_CONFIG_HOME", config.path)
        setEnv("OPENCODE_PASSWORD", password)
        setEnv("OPENCODE_NODE_ASSETS_DIR", assets.path)
        setEnv("OTUI_ASSET_ROOT", assets.path)
        setEnv("OPENCODE_NODE_PTY_PATH", ptyShim.path)
        setEnv("OPENCODE_PARCEL_WATCHER_PATH", watcherShim.path)
        setEnv("OPENCODE_DISABLE_TELEMETRY", "1")
        setEnv("TERM", "xterm-256color")
        setEnv("COLORTERM", "truecolor")
        _ = disk.path.withCString { chdir($0) }

        let args = [
            "node",
            "--experimental-loader", loader.path,
            mjs.path,
            "serve", "--hostname", "127.0.0.1", "--port", String(port),
        ]
        started = true
        ready = false
        failed = false
        output.removeAll(keepingCapacity: true)

        Thread.detachNewThread { [weak self] in
            var argv: [UnsafeMutablePointer<CChar>?] = args.map { strdup($0) }
            argv.append(nil)
            let rc = node_start(Int32(args.count), &argv)
            for p in argv { if let p { free(p) } }
            guard let self else { return }
            self.lock.lock()
            if !self.ready {
                self.failed = true
                self.output.append(Data("openCode local server exited (\(rc))\n".utf8))
            }
            self.lock.unlock()
        }

        DispatchQueue.global(qos: .userInitiated).async { [weak self] in
            guard let self else { return }
            for _ in 0..<80 {
                if self.portOpen(port) {
                    self.lock.lock()
                    if !self.ready {
                        self.ready = true
                        self.appendReadyLocked()
                    }
                    self.lock.unlock()
                    return
                }
                Thread.sleep(forTimeInterval: 0.25)
            }
            self.lock.lock()
            if !self.ready {
                self.failed = true
                self.output.append(Data("openCode local server did not start\n".utf8))
            }
            self.lock.unlock()
        }
        return true
    }

    private func appendReadyLocked() {
        output.append(Data("server listening on http://127.0.0.1:\(port)\nserver password \(password)\n".utf8))
    }

    private func setEnv(_ key: String, _ value: String) {
        key.withCString { k in value.withCString { v in setenv(k, v, 1) } }
    }

    private func portOpen(_ port: UInt16) -> Bool {
        var addr = sockaddr_in()
        addr.sin_len = UInt8(MemoryLayout<sockaddr_in>.size)
        addr.sin_family = sa_family_t(AF_INET)
        addr.sin_port = port.bigEndian
        addr.sin_addr.s_addr = inet_addr("127.0.0.1")
        let fd = socket(AF_INET, SOCK_STREAM, 0)
        if fd < 0 { return false }
        defer { close(fd) }
        let r = withUnsafePointer(to: &addr) {
            $0.withMemoryRebound(to: sockaddr.self, capacity: 1) {
                connect(fd, $0, socklen_t(MemoryLayout<sockaddr_in>.size))
            }
        }
        return r == 0
    }
}