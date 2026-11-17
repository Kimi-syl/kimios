// KimiNet: BSD-socket networking backend for the library-OS.
//
// Implements the kernel's SYS_NET sub-operations (src/usermode.rs sys_net) using
// the iOS host network stack: DNS via getaddrinfo, TCP via BSD sockets with
// connect/recv timeouts. This makes Browser, `dns`, `fetch` and kpkg work.

import Foundation
import Darwin

final class KimiNet {
    static let shared = KimiNet()

    private var sockets: [UInt64: Int32] = [:]
    private var nextSlot: UInt64 = 0
    private let lock = NSLock()

    private func socketFor(_ slot: UInt64) -> Int32? {
        lock.lock(); defer { lock.unlock() }
        return sockets[slot]
    }

    // op 0: status. The iOS host always has a network path (WiFi / cellular).
    func status() -> UInt64 { 0 }

    // op 1: resolve(name) -> write 4 network-order IPv4 octets to out.
    func resolve(_ host: String, out: UnsafeMutableRawPointer) -> UInt64 {
        var hints = addrinfo()
        hints.ai_family = AF_INET
        hints.ai_socktype = SOCK_STREAM
        var res: UnsafeMutablePointer<addrinfo>? = nil
        let rc = host.withCString { Darwin.getaddrinfo($0, nil, &hints, &res) }
        if rc != 0 { return UInt64.max }
        defer { Darwin.freeaddrinfo(res) }
        var node = res
        while let n = node {
            let ai = n.pointee
            if ai.ai_family == AF_INET, let addr = ai.ai_addr {
                let sin = addr.assumingMemoryBound(to: sockaddr_in.self)
                var saddr = sin.pointee.sin_addr.s_addr   // already network order
                memcpy(out, &saddr, 4)
                return 0
            }
            node = ai.ai_next
        }
        return UInt64.max
    }

    // op 2: tcp_connect(ip_bigendian_u32, port, timeout) -> slot.
    func connect(ip: UInt64, port: UInt64, timeoutMs: UInt64) -> UInt64 {
        let a = (ip >> 24) & 0xff, b = (ip >> 16) & 0xff
        let c = (ip >> 8) & 0xff, d = ip & 0xff
        let ipstr = "\(a).\(b).\(c).\(d)"
        let fd = Darwin.socket(AF_INET, SOCK_STREAM, 0)
        if fd < 0 { return UInt64.max }

        var addr = sockaddr_in()
        addr.sin_len = UInt8(MemoryLayout<sockaddr_in>.size)
        addr.sin_family = sa_family_t(AF_INET)
        addr.sin_port = UInt16(port & 0xffff).bigEndian
        addr.sin_addr.s_addr = ipstr.withCString { Darwin.inet_addr($0) }

        let ms = timeoutMs == 0 ? 5000 : Int(timeoutMs)
        if !connectWithTimeout(fd, &addr, ms) {
            Darwin.close(fd)
            return UInt64.max
        }
        lock.lock()
        let slot = nextSlot; nextSlot += 1; sockets[slot] = fd
        lock.unlock()
        return slot
    }

    private func connectWithTimeout(_ fd: Int32, _ addr: inout sockaddr_in, _ ms: Int) -> Bool {
        let flags = Darwin.fcntl(fd, F_GETFL, 0)
        _ = Darwin.fcntl(fd, F_SETFL, flags | O_NONBLOCK)
        var a = addr
        let r = withUnsafePointer(to: &a) { p in
            p.withMemoryRebound(to: sockaddr.self, capacity: 1) { sa in
                Darwin.connect(fd, sa, socklen_t(MemoryLayout<sockaddr_in>.size))
            }
        }
        var ok = false
        if r == 0 {
            ok = true
        } else if errno == EINPROGRESS {
            var pfd = pollfd(fd: fd, events: Int16(POLLOUT), revents: 0)
            let pr = Darwin.poll(&pfd, 1, Int32(ms))
            if pr > 0 {
                var err: Int32 = 0
                var len = socklen_t(MemoryLayout<Int32>.size)
                Darwin.getsockopt(fd, SOL_SOCKET, SO_ERROR, &err, &len)
                ok = (err == 0)
            }
        }
        _ = Darwin.fcntl(fd, F_SETFL, flags)   // restore blocking mode
        return ok
    }

    // op 3: tcp_send(slot, buf, len) -> bytes accepted.
    func send(slot: UInt64, buf: UnsafeRawPointer, len: UInt64) -> UInt64 {
        guard let fd = socketFor(slot) else { return 0 }
        let n = Darwin.send(fd, buf, Int(len), 0)
        return n < 0 ? 0 : UInt64(n)
    }

    // op 4: tcp_recv(slot, buf, len, timeout) -> bytes read (0 = timeout/close).
    func recv(slot: UInt64, buf: UnsafeMutableRawPointer, len: UInt64, timeoutMs: UInt64) -> UInt64 {
        guard let fd = socketFor(slot) else { return 0 }
        let ms = timeoutMs == 0 ? 5000 : Int(timeoutMs)
        var tv = timeval(tv_sec: ms / 1000, tv_usec: Int32((ms % 1000) * 1000))
        Darwin.setsockopt(fd, SOL_SOCKET, SO_RCVTIMEO, &tv, socklen_t(MemoryLayout<timeval>.size))
        let n = Darwin.recv(fd, buf, Int(len), 0)
        return n <= 0 ? 0 : UInt64(n)
    }

    // op 5: tcp_close(slot) -> 0.
    func close(slot: UInt64) -> UInt64 {
        lock.lock()
        let fd = sockets.removeValue(forKey: slot)
        lock.unlock()
        if let fd = fd { Darwin.close(fd) }
        return 0
    }
}
