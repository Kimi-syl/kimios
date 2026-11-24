// KimiPlatform: iOS implementations of the host services that do not require
// running Linux guest binaries. These mirror the Android dispatcher so the
// Files/Market/Weather/Clock/Music/Browser apps behave the same in the IPA.

import Foundation
import UIKit
import CoreLocation
import AVFoundation
import UniformTypeIdentifiers

// MARK: - Location

final class KimiLocation: NSObject, CLLocationManagerDelegate {
    static let shared = KimiLocation()

    private let manager = CLLocationManager()
    private let lock = NSLock()
    private var last: CLLocation?
    private var started = false

    override init() {
        super.init()
        manager.delegate = self
        manager.desiredAccuracy = kCLLocationAccuracyHundredMeters
    }

    func start() {
        DispatchQueue.main.async {
            guard !self.started else { return }
            self.started = true
            switch self.manager.authorizationStatus {
            case .notDetermined:
                self.manager.requestWhenInUseAuthorization()
            case .authorizedWhenInUse, .authorizedAlways:
                self.manager.startUpdatingLocation()
            default:
                break
            }
        }
    }

    func snapshot() -> [UInt64] {
        start()
        lock.lock()
        let loc = last
        lock.unlock()
        guard let loc else { return [0, 0, 0, 0] }
        return [
            loc.coordinate.latitude.bitPattern,
            loc.coordinate.longitude.bitPattern,
            UInt64(max(0, loc.horizontalAccuracy)),
            1,
        ]
    }

    func locationManager(_ manager: CLLocationManager, didUpdateLocations locations: [CLLocation]) {
        guard let loc = locations.last else { return }
        lock.lock()
        last = loc
        lock.unlock()
    }

    func locationManager(_ manager: CLLocationManager, didFailWithError error: Error) {
        NSLog("[KimiOS] location error: %@", error.localizedDescription)
    }

    func locationManagerDidChangeAuthorization(_ manager: CLLocationManager) {
        switch manager.authorizationStatus {
        case .authorizedWhenInUse, .authorizedAlways:
            manager.startUpdatingLocation()
        default:
            break
        }
    }
}

// MARK: - Audio

final class KimiAudio: NSObject, AVAudioPlayerDelegate {
    static let shared = KimiAudio()

    private let lock = NSLock()
    private var player: AVAudioPlayer?

    func command(path: String?, cmd: UInt64) -> UInt64 {
        switch cmd {
        case 0:
            guard let path, let url = KimiFS.shared.sandboxURL(forDiskPath: path) else {
                return UInt64.max
            }
            do {
                try AVAudioSession.sharedInstance().setCategory(.playback, mode: .default,
                                                               options: [.mixWithOthers])
                try AVAudioSession.sharedInstance().setActive(true)
                let p = try AVAudioPlayer(contentsOf: url)
                p.delegate = self
                p.prepareToPlay()
                p.play()
                lock.lock(); player = p; lock.unlock()
                return 0
            } catch {
                NSLog("[KimiOS] audio play failed: %@", error.localizedDescription)
                return UInt64.max
            }
        case 1:
            lock.lock(); player?.pause(); lock.unlock()
            return 0
        case 2:
            lock.lock(); player?.stop(); player = nil; lock.unlock()
            return 0
        case 3:
            lock.lock(); player?.play(); lock.unlock()
            return 0
        case 4:
            return 0
        default:
            return UInt64.max
        }
    }

    func query(out: UnsafeMutablePointer<UInt64>) -> UInt64 {
        lock.lock()
        let p = player
        lock.unlock()
        guard let p else {
            out[0] = 0; out[1] = 0; out[2] = 0
            return 0
        }
        out[0] = UInt64(max(0, p.currentTime) * 1000.0)
        out[1] = UInt64(max(0, p.duration) * 1000.0)
        out[2] = p.isPlaying ? 1 : 0
        return 0
    }

    func audioPlayerDidFinishPlaying(_ player: AVAudioPlayer, successfully flag: Bool) {
        lock.lock()
        if self.player === player { self.player = nil }
        lock.unlock()
    }
}

// MARK: - Synchronous / async URL fetch

final class KimiFetch: NSObject, URLSessionDataDelegate {
    static let shared = KimiFetch()

    private let lock = NSLock()
    private var handle: FileHandle?
    private var task: URLSessionDataTask?
    private var done: UInt64 = 0
    private var total: UInt64 = 0
    private var state: UInt64 = 0       // 0 idle, 1 running, 2 done, 3 error
    private var session: URLSession!

    override init() {
        super.init()
        let config = URLSessionConfiguration.default
        config.timeoutIntervalForRequest = 60
        config.timeoutIntervalForResource = 3600
        session = URLSession(configuration: config, delegate: self, delegateQueue: nil)
    }

    private func write(_ data: Data, to dest: String) -> UInt64? {
        guard let url = KimiFS.shared.ensureParents(forDiskPath: dest) else { return nil }
        if !FileManager.default.fileExists(atPath: url.path) {
            FileManager.default.createFile(atPath: url.path, contents: nil)
        }
        guard let h = try? FileHandle(forWritingTo: url) else { return nil }
        do { try h.write(contentsOf: data); try h.close(); return UInt64(data.count) }
        catch { return nil }
    }

    /// Blocking GET used by kpkg/marketplace/fetch. Returns bytes written or nil.
    func fetchSync(url: String, dest: String, bearer: String? = nil) -> UInt64? {
        guard let u = URL(string: url), let out = KimiFS.shared.ensureParents(forDiskPath: dest) else {
            return nil
        }
        var req = URLRequest(url: u)
        req.timeoutInterval = 60
        req.setValue("KimiOS-libos", forHTTPHeaderField: "User-Agent")
        if let bearer, !bearer.isEmpty {
            req.setValue("Bearer \(bearer)", forHTTPHeaderField: "Authorization")
        }
        let sem = DispatchSemaphore(value: 0)
        var result: UInt64?
        var failure = false
        URLSession.shared.dataTask(with: req) { data, response, error in
            defer { sem.signal() }
            if error != nil { failure = true; return }
            if let http = response as? HTTPURLResponse, !(200..<300).contains(http.statusCode) {
                failure = true; return
            }
            guard let data else { failure = true; return }
            FileManager.default.createFile(atPath: out.path, contents: data)
            result = UInt64(data.count)
        }.resume()
        _ = sem.wait(timeout: .now() + 65)
        return failure ? nil : result
    }

    func startAsync(url: String, dest: String) -> Bool {
        guard let u = URL(string: url), let out = KimiFS.shared.ensureParents(forDiskPath: dest) else {
            return false
        }
        lock.lock()
        task?.cancel()
        handle = nil
        done = 0; total = 0; state = 1
        try? FileManager.default.removeItem(at: out)
        FileManager.default.createFile(atPath: out.path, contents: nil)
        handle = try? FileHandle(forWritingTo: out)
        if handle == nil { state = 3; lock.unlock(); return false }
        lock.unlock()

        var req = URLRequest(url: u)
        req.timeoutInterval = 60
        req.setValue("KimiOS-libos", forHTTPHeaderField: "User-Agent")
        let t = session.dataTask(with: req)
        lock.lock(); task = t; lock.unlock()
        t.resume()
        return true
    }

    func progress(out: UnsafeMutablePointer<UInt64>) -> UInt64 {
        lock.lock()
        out[0] = done; out[1] = total; out[2] = state
        lock.unlock()
        return 0
    }

    func urlSession(_ session: URLSession, dataTask: URLSessionDataTask,
                    didReceive response: URLResponse,
                    completionHandler: @escaping (URLSession.ResponseDisposition) -> Void) {
        lock.lock()
        if response.expectedContentLength > 0 { total = UInt64(response.expectedContentLength) }
        lock.unlock()
        completionHandler(.allow)
    }

    func urlSession(_ session: URLSession, dataTask: URLSessionDataTask, didReceive data: Data) {
        lock.lock()
        let h = handle
        lock.unlock()
        guard let h else { return }
        do { try h.write(contentsOf: data) } catch { }
        lock.lock()
        done += UInt64(data.count)
        lock.unlock()
    }

    func urlSession(_ session: URLSession, task: URLSessionTask, didCompleteWithError error: Error?) {
        lock.lock()
        try? handle?.close()
        handle = nil
        state = error == nil ? 2 : 3
        lock.unlock()
    }
}

// MARK: - File import (UIDocumentPicker)

final class KimiImporter: NSObject, UIDocumentPickerDelegate {
    static let shared = KimiImporter()

    private var pendingDir: URL?

    func launch(dir: String) -> Bool {
        guard let out = KimiFS.shared.ensureParents(forDiskPath: dir) else { return false }
        if !FileManager.default.fileExists(atPath: out.path) {
            try? FileManager.default.createDirectory(at: out, withIntermediateDirectories: true)
        }
        pendingDir = out
        DispatchQueue.main.async {
            guard let top = Self.topViewController() else { return }
            let picker = UIDocumentPickerViewController(forOpeningContentTypes: [.item], asCopy: true)
            picker.delegate = self
            picker.allowsMultipleSelection = false
            top.present(picker, animated: true)
        }
        return true
    }

    func documentPicker(_ controller: UIDocumentPickerViewController,
                        didPickDocumentsAt urls: [URL]) {
        guard let src = urls.first, let dir = pendingDir else { return }
        pendingDir = nil
        let scoped = src.startAccessingSecurityScopedResource()
        defer { if scoped { src.stopAccessingSecurityScopedResource() } }
        let dest = dir.appendingPathComponent(src.lastPathComponent)
        if FileManager.default.fileExists(atPath: dest.path) {
            try? FileManager.default.removeItem(at: dest)
        }
        try? FileManager.default.copyItem(at: src, to: dest)
    }

    func documentPickerWasCancelled(_ controller: UIDocumentPickerViewController) {
        pendingDir = nil
    }

    private static func topViewController() -> UIViewController? {
        let scenes = UIApplication.shared.connectedScenes.compactMap { $0 as? UIWindowScene }
        let window = scenes.flatMap { $0.windows }.first(where: { $0.isKeyWindow })
            ?? scenes.first?.windows.first
        var top = window?.rootViewController
        while let presented = top?.presentedViewController { top = presented }
        if let nav = top as? UINavigationController { return nav.visibleViewController }
        if let tab = top as? UITabBarController { return tab.selectedViewController }
        return top
    }
}

// MARK: - System services

enum KimiSystem {
    static func localTime(out: UnsafeMutablePointer<UInt64>) {
        let now = Date()
        let cal = Calendar.current
        let c = cal.dateComponents([.year, .month, .day, .hour, .minute, .second, .weekday], from: now)
        out[0] = UInt64(now.timeIntervalSince1970)
        out[1] = UInt64(c.year ?? 1970)
        out[2] = UInt64(c.month ?? 1)
        out[3] = UInt64(c.day ?? 1)
        out[4] = UInt64(c.hour ?? 0)
        out[5] = UInt64(c.minute ?? 0)
        out[6] = UInt64(c.second ?? 0)
        out[7] = UInt64(max(0, (c.weekday ?? 1) - 1))
        out[8] = UInt64(TimeZone.current.secondsFromGMT(for: now) / 60)
    }

    static func sysinfo(out: UnsafeMutablePointer<UInt64>) {
        let total = ProcessInfo.processInfo.physicalMemory
        let attrs = try? FileManager.default.attributesOfFileSystem(forPath: NSHomeDirectory())
        let storageTotal = (attrs?[.systemSize] as? NSNumber)?.uint64Value ?? 0
        let storageFree = (attrs?[.systemFreeSize] as? NSNumber)?.uint64Value ?? 0
        UIDevice.current.isBatteryMonitoringEnabled = true
        let battery = UIDevice.current.batteryLevel
        out[0] = 0
        out[1] = total / 1024
        out[2] = (total / 2) / 1024
        out[3] = storageTotal / 1024
        out[4] = (storageTotal > storageFree ? storageTotal - storageFree : 0) / 1024
        out[5] = UInt64(ProcessInfo.processInfo.systemUptime)
        out[6] = 1
        out[7] = battery < 0 ? 0 : UInt64(battery * 100)
        out[8] = (UIDevice.current.batteryState == .charging || UIDevice.current.batteryState == .full) ? 1 : 0
        out[9] = 0
    }

    static func procList(out: UnsafeMutablePointer<UInt64>, capacity: UInt64) -> UInt64 {
        if capacity < 1 { return 0 }
        let name = Array("KimiOS".utf8)
        for i in 0..<8 { out[i] = 0 }
        out[0] = 1          // pid
        out[1] = 0          // ppid
        out[2] = 0          // ram_kb
        out[3] = 0          // cpu_x10
        out[4] = 0          // running
        for (i, b) in name.prefix(24).enumerated() {
            out[5 + i / 8] |= UInt64(b) << UInt64((i % 8) * 8)
        }
        return 1
    }

    static func copyFile(src: String, dst: String) -> Bool {
        guard let s = KimiFS.shared.sandboxURL(forDiskPath: src),
              let d = KimiFS.shared.ensureParents(forDiskPath: dst) else { return false }
        if FileManager.default.fileExists(atPath: d.path) {
            try? FileManager.default.removeItem(at: d)
        }
        do { try FileManager.default.copyItem(at: s, to: d); return true } catch { return false }
    }

    static func symlink(target: String, path: String) -> Bool {
        guard let d = KimiFS.shared.ensureParents(forDiskPath: path) else { return false }
        if FileManager.default.fileExists(atPath: d.path) {
            try? FileManager.default.removeItem(at: d)
        }
        do {
            try FileManager.default.createSymbolicLink(atPath: d.path,
                                                        withDestinationPath: target)
            return true
        } catch { return false }
    }

    static func openURL(_ text: String) -> Bool {
        guard let u = URL(string: text) else { return false }
        if let host = u.host, (host == "127.0.0.1" || host == "localhost"),
           KimiNode.shared.serverReady {
            KimiWebSheet.present(url: u, password: KimiNode.shared.webPassword)
            return true
        }
        DispatchQueue.main.async { UIApplication.shared.open(u) }
        return true
    }
}