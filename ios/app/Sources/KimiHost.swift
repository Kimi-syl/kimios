// KimiHost: the library-OS host backend for iOS.
//
// libkimi (compiled for aarch64-apple-ios) routes every KimiOS syscall to the
// C-ABI `kimi_host_syscall` below instead of trapping to EL1. This implements
// the same syscall contract as the bare-metal kernel using iOS primitives:
// a Metal-shared framebuffer, UIKit touches, mach time, and the app sandbox.
// The entire KimiOS userland (GUI + apps + shell) runs unchanged and native.

import Foundation
import Metal
import Darwin

final class KimiHost {
    static let shared = KimiHost()

    let width = 1080
    let height = 1920
    var stride: Int { width * 4 }

    // Double framebuffer: GUI renders into `back`, publishes dirty rects to
    // `front` on present; the Metal renderer reads `front`. Mirrors the proven
    // Android shared-memory design (no tearing, dirty-rect copies only).
    private(set) var backPtr: UnsafeMutableRawPointer?
    private(set) var frontPtr: UnsafeMutableRawPointer?
    private var backBuffer: MTLBuffer?
    private var frontBuffer: MTLBuffer?

    private var instrBox: UnsafeMutablePointer<UInt64>?
    private var heapArena: UnsafeMutableRawPointer?
    private var heapBreak: UInt64 = 0
    private let heapSize = 8 * 1024 * 1024

    private var inputQueue: [UInt64] = []
    private let inputLock = NSLock()

    private var startNs: UInt64 = 0
    private(set) var presentCount: UInt64 = 0

    // Called (on the GUI thread) after each present; the view controller hooks
    // this to trigger an on-demand Metal redraw on the main thread.
    var onPresent: (() -> Void)?

    func configure(device: MTLDevice) {
        let bytes = width * height * 4
        backBuffer = device.makeBuffer(length: bytes, options: .storageModeShared)
        frontBuffer = device.makeBuffer(length: bytes, options: .storageModeShared)
        backPtr = backBuffer?.contents()
        frontPtr = frontBuffer?.contents()
        if let b = backPtr { memset(b, 0, bytes) }
        if let f = frontPtr { memset(f, 0, bytes) }
        heapArena = malloc(heapSize)
        if let h = heapArena { memset(h, 0, heapSize) }
        instrBox = UnsafeMutablePointer<UInt64>.allocate(capacity: 16)
        instrBox?.initialize(repeating: 0, count: 16)
        startNs = clock_gettime_nsec_np(CLOCK_UPTIME_RAW)
    }

    var frontBufferMTL: MTLBuffer? { frontBuffer }

    private func nowNs() -> UInt64 { clock_gettime_nsec_np(CLOCK_UPTIME_RAW) }
    private func elapsedMs() -> UInt64 { (nowNs() - startNs) / 1_000_000 }

    // MARK: input (UIKit touches -> virtio-style events, ABS range 0..32767)

    func pushTouch(x: Int, y: Int, down: Bool) {
        let ax = UInt32(max(0, min(32767, x * 32767 / max(1, width))))
        let ay = UInt32(max(0, min(32767, y * 32767 / max(1, height))))
        var events: [UInt64] = []
        if down {
            events.append(pack(3, 0, ax))        // EV_ABS ABS_X
            events.append(pack(3, 1, ay))        // EV_ABS ABS_Y
            events.append(pack(1, 0x14a, 1))     // EV_KEY BTN_TOUCH down
        } else {
            events.append(pack(3, 0, ax))
            events.append(pack(3, 1, ay))
            events.append(pack(1, 0x14a, 0))     // EV_KEY BTN_TOUCH up
        }
        inputLock.lock()
        inputQueue.append(contentsOf: events)
        inputLock.unlock()
    }

    private func pack(_ kind: UInt16, _ code: UInt16, _ value: UInt32) -> UInt64 {
        (UInt64(kind) << 48) | (UInt64(code) << 32) | UInt64(value)
    }

    // Position update while a touch is held (drag / home-screen swipe).
    func pushMove(x: Int, y: Int) {
        let ax = UInt32(max(0, min(32767, x * 32767 / max(1, width))))
        let ay = UInt32(max(0, min(32767, y * 32767 / max(1, height))))
        inputLock.lock()
        inputQueue.append(pack(3, 0, ax))
        inputQueue.append(pack(3, 1, ay))
        inputLock.unlock()
    }

    private func pollInput() -> UInt64 {
        inputLock.lock()
        defer { inputLock.unlock() }
        if inputQueue.isEmpty { return UInt64.max }
        return inputQueue.removeFirst()
    }

    fileprivate func cntpct() -> UInt64 { nowNs() - startNs }

    // MARK: syscall dispatch

    fileprivate func syscall(_ n: UInt64, _ a0: UInt64, _ a1: UInt64,
                             _ a2: UInt64, _ a3: UInt64, _ a4: UInt64) -> UInt64 {
        switch Int(n) {
        case 16: // SYS_FB_INFO
            let info = UnsafeMutablePointer<UInt64>(bitPattern: UInt(a0))!
            info[0] = UInt64(width); info[1] = UInt64(height)
            info[2] = UInt64(stride); info[3] = 0
            return 0
        case 31: // SYS_FB_MAP (front)
            return UInt64(UInt(bitPattern: frontPtr))
        case 36: // SYS_FB_BACK
            return UInt64(UInt(bitPattern: backPtr))
        case 37: // SYS_FB_PRESENT(x0,y0,x1,y1)
            present(rect: a0, a1, a2, a3)
            return 0
        case 17: // SYS_FB_FILL
            fill(x: a0, y: a1, w: a2, h: a3, color: UInt32(a4))
            return 0
        case 32: // SYS_INPUT_POLL
            return pollInput()
        case 33: // SYS_INPUT_WAIT
            return UInt64.max
        case 39: // SYS_INSTR_MAP
            guard let ib = instrBox else { return 0 }
            return UInt64(UInt(bitPattern: UnsafeMutableRawPointer(ib)))
        case 20: // SYS_UPTIME
            return elapsedMs()
        case 21: // SYS_SLEEP(ms)
            usleep(UInt32(min(a0, 1000)) * 1000)
            return 0
        case 26: // SYS_RTC -> [epoch,y,m,d,h,min,s]
            let f = UnsafeMutablePointer<UInt64>(bitPattern: UInt(a0))!
            var t = time(nil)
            var tmb = tm()
            gmtime_r(&t, &tmb)
            f[0] = UInt64(t); f[1] = UInt64(tmb.tm_year + 1900); f[2] = UInt64(tmb.tm_mon + 1)
            f[3] = UInt64(tmb.tm_mday); f[4] = UInt64(tmb.tm_hour); f[5] = UInt64(tmb.tm_min); f[6] = UInt64(tmb.tm_sec)
            return 0
        case 1: // SYS_WRITE(fd,buf,len)
            if let p = UnsafeMutableRawPointer(bitPattern: UInt(a1)) {
                FileHandle.standardOutput.write(Data(bytes: p, count: Int(a2)))
            }
            return a2
        case 3: // SYS_READ
            return 0
        case 11: // SYS_GETPID
            return 1
        case 2: // SYS_EXIT
            NSLog("[KimiOS] GUI exit(%llu)", a0)
            pthread_exit(nil)
            return 0
        case 22: // SYS_BRK
            guard let base = heapArena else { return 0 }
            let baseAddr = UInt64(UInt(bitPattern: base))
            if a0 == 0 { return heapBreak != 0 ? heapBreak : baseAddr }
            if a0 > baseAddr + UInt64(heapSize) { return heapBreak }
            heapBreak = a0
            return heapBreak
        case 12: // SYS_STATS (free frames for `mem`)
            return UInt64(heapSize / 4096)

        // ---- filesystem (sandboxed /disk + RamFS) ----
        case 4: // SYS_OPEN(path,len)
            return KimiFS.shared.open(userStr(a0, a1))
        case 5: // SYS_READ_FILE(fd,offset,buf,len)
            guard let p = userPtr(a2) else { return 0 }
            return KimiFS.shared.readFile(a0, offset: a1, into: p, len: a3)
        case 6: // SYS_CLOSE(fd)
            return KimiFS.shared.close(a0)
        case 8: // SYS_LIST(buf,len)
            guard let p = userPtr(a0) else { return UInt64.max }
            return KimiFS.shared.list(into: p, len: a1)
        case 13: // SYS_CREATE(path,len)
            return KimiFS.shared.create(userStr(a0, a1))
        case 14: // SYS_WRITE_FILE(fd,offset,buf,len)
            guard let p = userPtr(a2) else { return 0 }
            return KimiFS.shared.writeFile(a0, offset: a1, from: p, len: a3)
        case 15: // SYS_DISKINFO
            return KimiFS.shared.diskinfo()
        case 18: // SYS_UNLINK(path,len)
            return KimiFS.shared.unlink(userStr(a0, a1))
        case 19: // SYS_FREEBLOCKS
            return KimiFS.shared.freeblocks()
        case 25: // SYS_RENAME(old,oldlen,new,newlen)
            return KimiFS.shared.rename(userStr(a0, a1), userStr(a2, a3))
        case 27: // SYS_STAT(path,len,out[3])
            guard let out = UnsafeMutablePointer<UInt64>(bitPattern: UInt(a2)) else { return UInt64.max }
            return KimiFS.shared.stat(userStr(a0, a1), out: out)
        case 28: // SYS_LISTDIR(path,len,out,outlen)
            guard let p = userPtr(a2) else { return UInt64.max }
            return KimiFS.shared.listdir(userStr(a0, a1), into: p, len: a3)
        case 29: // SYS_MKDIR(path,len)
            return KimiFS.shared.mkdir(userStr(a0, a1))
        case 30: // SYS_RMDIR(path,len)
            return KimiFS.shared.rmdir(userStr(a0, a1))

        // ---- networking (BSD sockets) ----
        case 34: // SYS_NET(op,...)
            return netSyscall(a0, a1, a2, a3, a4)

        // ---- process services (single native GUI process) ----
        case 23: // SYS_PS(out,capacity) -> one running process record
            guard a1 >= 1, let out = UnsafeMutablePointer<UInt64>(bitPattern: UInt(a0)) else { return UInt64.max }
            out[0] = 1; out[1] = 0; out[2] = 2; out[3] = 0   // pid ppid state=run exit
            return 1
        case 38: // SYS_GFX_BENCH (no kernel display path to benchmark here)
            return 0
        case 7, 9, 35: // SYS_EXEC / SYS_SPAWN / SYS_SPAWN_ARGS
            // Loading external ELF/PE images needs the JIT ELF loader (mode B);
            // fail gracefully so the shell reports "spawn failed".
            return UInt64.max
        case 10, 24: // SYS_WAIT / SYS_KILL (no child processes)
            return UInt64.max

        // ---- host services shared with the Android dispatcher ----
        case 40: // SYS_SYMLINK(target,targetlen,path,pathlen)
            return KimiSystem.symlink(target: userStr(a0, a1), path: userStr(a2, a3))
                ? 0 : UInt64.max
        case 42: // SYS_FETCH_URL(url,urlLen,dest,destLen)
            return KimiFetch.shared.fetchSync(url: userStr(a0, a1), dest: userStr(a2, a3))
                ?? UInt64.max
        case 43: // SYS_LOCAL_TIME(out[9])
            guard let out = UnsafeMutablePointer<UInt64>(bitPattern: UInt(a0)) else {
                return UInt64.max
            }
            KimiSystem.localTime(out: out)
            return 0
        case 44: // SYS_LOCATION(out[4])
            guard let out = UnsafeMutablePointer<UInt64>(bitPattern: UInt(a0)) else {
                return UInt64.max
            }
            let fix = KimiLocation.shared.snapshot()
            for i in 0..<4 { out[i] = fix[i] }
            return 0
        case 45: // SYS_OPEN_URL(url,len)
            return KimiSystem.openURL(userStr(a0, a1)) ? 0 : UInt64.max
        case 46: // SYS_AUDIO(path,len,cmd)
            if a2 == 4 {
                guard let out = UnsafeMutablePointer<UInt64>(bitPattern: UInt(a0)) else {
                    return UInt64.max
                }
                return KimiAudio.shared.query(out: out)
            }
            let path = a2 == 0 ? userStr(a0, a1) : nil
            return KimiAudio.shared.command(path: path, cmd: a2)
        case 47: // SYS_IMPORT_FILE(dir,len)
            return KimiImporter.shared.launch(dir: userStr(a0, a1)) ? 0 : UInt64.max
        case 49: // SYS_SYSINFO(out[10])
            guard let out = UnsafeMutablePointer<UInt64>(bitPattern: UInt(a0)) else {
                return UInt64.max
            }
            KimiSystem.sysinfo(out: out)
            return 0
        case 50: // SYS_PROCLIST(out,capacity)
            guard let out = UnsafeMutablePointer<UInt64>(bitPattern: UInt(a0)) else {
                return UInt64.max
            }
            return KimiSystem.procList(out: out, capacity: a1)
        case 52: // SYS_COPY(src,srclen,dst,dstlen)
            return KimiSystem.copyFile(src: userStr(a0, a1), dst: userStr(a2, a3))
                ? 0 : UInt64.max
        case 57: // SYS_FETCH_ASYNC(url,urlLen,dest,destLen)
            return KimiFetch.shared.startAsync(url: userStr(a0, a1), dest: userStr(a2, a3))
                ? 0 : UInt64.max
        case 58: // SYS_FETCH_PROGRESS(out[3])
            guard let out = UnsafeMutablePointer<UInt64>(bitPattern: UInt(a0)) else {
                return UInt64.max
            }
            return KimiFetch.shared.progress(out: out)
        case 59: // SYS_FETCH_AUTH(req{up,ul,dp,dl,kp,kl})
            guard let base = UnsafeMutableRawPointer(bitPattern: UInt(a0)) else {
                return UInt64.max
            }
            let f = base.assumingMemoryBound(to: UInt64.self)
            let url = userStr(f[0], f[1])
            let dest = userStr(f[2], f[3])
            let key = userStr(f[4], f[5])
            return KimiFetch.shared.fetchSync(url: url, dest: dest, bearer: key) ?? UInt64.max
        default:
            return UInt64.max
        }
    }

    private func netSyscall(_ op: UInt64, _ a1: UInt64, _ a2: UInt64, _ a3: UInt64, _ a4: UInt64) -> UInt64 {
        switch Int(op) {
        case 0: return KimiNet.shared.status()
        case 1:
            guard let out = userPtr(a3) else { return UInt64.max }
            return KimiNet.shared.resolve(userStr(a1, a2), out: out)
        case 2: return KimiNet.shared.connect(ip: a1, port: a2, timeoutMs: a3)
        case 3:
            guard let p = userPtr(a2) else { return 0 }
            return KimiNet.shared.send(slot: a1, buf: p, len: a3)
        case 4:
            guard let p = userPtr(a2) else { return 0 }
            return KimiNet.shared.recv(slot: a1, buf: p, len: a3, timeoutMs: a4)
        case 5: return KimiNet.shared.close(slot: a1)
        default: return UInt64.max
        }
    }

    private func userStr(_ ptr: UInt64, _ len: UInt64) -> String {
        guard len > 0, let p = UnsafeMutableRawPointer(bitPattern: UInt(ptr)) else { return "" }
        return String(data: Data(bytes: p, count: Int(len)), encoding: .utf8) ?? ""
    }

    private func userPtr(_ p: UInt64) -> UnsafeMutableRawPointer? {
        UnsafeMutableRawPointer(bitPattern: UInt(p))
    }

    private func present(rect x0: UInt64, _ y0: UInt64, _ x1: UInt64, _ y1: UInt64) {
        guard let b = backPtr, let f = frontPtr else { return }
        let xa = Int(min(x0, UInt64(width))), xb = Int(min(x1, UInt64(width)))
        let ya = Int(min(y0, UInt64(height))), yb = Int(min(y1, UInt64(height)))
        if xb <= xa || yb <= ya { return }
        let rowBytes = (xb - xa) * 4
        for y in ya..<yb {
            let off = y * stride + xa * 4
            memcpy(f + off, b + off, rowBytes)
        }
        presentCount &+= 1
        onPresent?()
    }

    private func fill(x: UInt64, y: UInt64, w: UInt64, h: UInt64, color: UInt32) {
        guard let b = backPtr else { return }
        let px = b.assumingMemoryBound(to: UInt32.self)
        let xe = min(Int(x + w), width), ye = min(Int(y + h), height)
        for j in Int(y)..<ye {
            for i in Int(x)..<xe { px[j * width + i] = color }
        }
    }
}

// MARK: - C-ABI entry points resolved by the Rust staticlib

@_cdecl("kimi_host_syscall")
func kimi_host_syscall(_ n: UInt64, _ a0: UInt64, _ a1: UInt64,
                       _ a2: UInt64, _ a3: UInt64, _ a4: UInt64) -> UInt64 {
    KimiHost.shared.syscall(n, a0, a1, a2, a3, a4)
}

@_cdecl("kimi_host_cntpct")
func kimi_host_cntpct() -> UInt64 {
    KimiHost.shared.cntpct()
}
