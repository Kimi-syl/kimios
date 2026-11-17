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
            let t = time(nil)
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
        case 12: // SYS_STATS
            return UInt64(heapSize)
        default:
            // Filesystem / net / process syscalls are stubbed in this first
            // library-OS build (home, clock, calculator, settings render fine).
            return UInt64.max
        }
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
