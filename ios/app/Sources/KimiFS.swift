// KimiFS: sandboxed filesystem backend for the library-OS.
//
// Replicates the KimiOS kernel's two namespaces:
//   * RamFS  — small read-only built-in text files (kind 0).
//   * /disk/ — persistent files backed by the app sandbox Documents/disk
//              (file kind 1, directory kind 2).
// Every method matches the exact syscall semantics in src/usermode.rs so the
// unmodified shell / Files / Notes / APK / DVM / kpkg code works unchanged.

import Foundation

final class KimiFS {
    static let shared = KimiFS()

    private let fm = FileManager.default
    private let diskRoot: URL
    private var handles: [UInt64: FD] = [:]
    private var nextHandle: UInt64 = 100
    private let lock = NSLock()

    private enum FD { case ram(String); case disk(String) }

    // Built-in read-only RamFS (text, genuinely readable via cat / Files).
    private let ramfs: [String: String] = [
        "/readme":
            "KimiOS for iPad — library-OS edition.\n\n" +
            "The same pure-Rust userland that runs bare-metal, compiled to native\n" +
            "ARM64 and hosted by iOS (Metal + UIKit). No QEMU, no EL1/MMU.\n\n" +
            "Persistent files live under /disk/ (the app sandbox). Open the\n" +
            "Terminal and type 'help' for commands, or use the Files app.\n",
        "/version": "KimiOS 0.9.0-ios (library-OS, native ARM64, no QEMU)\n",
        "/help":
            "ls cat write rm mkdir rmdir mv stat df net dns fetch date time\n" +
            "sleep uptime uname mem ps kpkg dvm apk help\n",
    ]

    init() {
        let docs = fm.urls(for: .documentDirectory, in: .userDomainMask)[0]
        diskRoot = docs.appendingPathComponent("disk", isDirectory: true)
        try? fm.createDirectory(at: diskRoot, withIntermediateDirectories: true)
    }

    // MARK: helpers

    private func diskURL(_ rel: String) -> URL? {
        // Reject traversal / absolute components; keep everything inside disk/.
        var parts: [String] = []
        for comp in rel.split(separator: "/") {
            if comp == ".." || comp == "." || comp.isEmpty { continue }
            parts.append(String(comp))
        }
        var url = diskRoot
        for p in parts { url.appendPathComponent(p) }
        return url
    }

    private func relPath(_ path: String) -> String? {
        if path == "/disk" { return "" }
        if path.hasPrefix("/disk/") { return String(path.dropFirst(6)) }
        return nil
    }

    private func isDir(_ url: URL) -> Bool {
        var d: ObjCBool = false
        return fm.fileExists(atPath: url.path, isDirectory: &d) && d.boolValue
    }

    private func allocFD(_ fd: FD) -> UInt64 {
        lock.lock(); defer { lock.unlock() }
        let h = nextHandle; nextHandle += 1
        handles[h] = fd
        return h
    }

    private func lookupFD(_ h: UInt64) -> FD? {
        lock.lock(); defer { lock.unlock() }
        return handles[h]
    }

    private func readDisk(_ rel: String) -> Data? {
        guard let url = diskURL(rel), !isDir(url) else { return nil }
        return try? Data(contentsOf: url)
    }

    // MARK: open / create / read / write / close

    func open(_ path: String) -> UInt64 {
        if ramfs[path] != nil { return allocFD(.ram(path)) }
        guard let rel = relPath(path), let url = diskURL(rel),
              fm.fileExists(atPath: url.path), !isDir(url) else { return UInt64.max }
        return allocFD(.disk(rel))
    }

    func create(_ path: String) -> UInt64 {
        guard let rel = relPath(path), !rel.isEmpty, let url = diskURL(rel) else { return UInt64.max }
        let parent = url.deletingLastPathComponent()
        try? fm.createDirectory(at: parent, withIntermediateDirectories: true)
        // create or truncate
        fm.createFile(atPath: url.path, contents: Data(), attributes: nil)
        return allocFD(.disk(rel))
    }

    func readFile(_ h: UInt64, offset: UInt64, into buf: UnsafeMutableRawPointer, len: UInt64) -> UInt64 {
        switch lookupFD(h) {
        case .ram(let name):
            guard let d = ramfs[name]?.data(using: .utf8) else { return 0 }
            let off = Int(offset)
            if off >= d.count { return 0 }
            let n = min(Int(len), d.count - off)
            d.withUnsafeBytes { raw in
                if let base = raw.baseAddress { memcpy(buf, base + off, n) }
            }
            return UInt64(n)
        case .disk(let rel):
            // Efficient positional read (no whole-file load) for large packages.
            guard let url = diskURL(rel), !isDir(url),
                  let fh = FileHandle(forReadingFrom: url) else { return 0 }
            defer { fh.closeFile() }
            fh.seek(toFileOffset: offset)
            let d = fh.readData(ofLength: Int(len))
            guard !d.isEmpty, let base = d.withUnsafeBytes({ $0.baseAddress }) else { return 0 }
            memcpy(buf, base, d.count)
            return UInt64(d.count)
        case .none:
            return 0
        }
    }

    func writeFile(_ h: UInt64, offset: UInt64, from buf: UnsafeRawPointer, len: UInt64) -> UInt64 {
        guard case .disk(let rel) = lookupFD(h), let url = diskURL(rel) else { return 0 }
        let off = Int(offset), n = Int(len)
        let chunk = Data(bytes: buf, count: n)
        // Sequential append fast path (streaming extraction): no whole-file read.
        if let fh = FileHandle(forWritingTo: url) {
            let size = Int(fh.seekToEndOfFile())
            if off == size {
                fh.seek(toFileOffset: offset)
                fh.write(chunk)
                fh.closeFile()
                return UInt64(n)
            }
            fh.closeFile()
        }
        // Random write: read-modify-write (small files only).
        var existing = (try? Data(contentsOf: url)) ?? Data()
        if existing.count < off + n { existing.append(Data(count: off + n - existing.count)) }
        existing.replaceSubrange(off..<(off + n), with: chunk)
        do { try existing.write(to: url); return UInt64(n) } catch { return 0 }
    }

    func close(_ h: UInt64) -> UInt64 {
        lock.lock(); defer { lock.unlock() }
        if handles.removeValue(forKey: h) != nil { return 0 }
        return UInt64.max
    }

    // MARK: listing

    // list(buf,len): NUL-separated RamFS names + "/disk/<root entry>" (dirs end '/').
    func list(into buf: UnsafeMutableRawPointer, len: UInt64) -> UInt64 {
        var out = Data()
        func push(_ s: String) {
            out.append(s.data(using: .utf8) ?? Data())
            out.append(0)
        }
        for name in ramfs.keys.sorted() { push(name) }
        if let entries = try? fm.contentsOfDirectory(atPath: diskRoot.path) {
            for name in entries.sorted() {
                let url = diskRoot.appendingPathComponent(name)
                push("/disk/" + name + (isDir(url) ? "/" : ""))
            }
        }
        let n = min(out.count, Int(len))
        out.withUnsafeBytes { raw in
            if let base = raw.baseAddress { memcpy(buf, base, n) }
        }
        return UInt64(n)
    }

    // listdir(path,out,len): NUL-terminated children of a /disk dir (dirs end '/').
    func listdir(_ path: String, into buf: UnsafeMutableRawPointer, len: UInt64) -> UInt64 {
        guard let rel = relPath(path), let url = diskURL(rel), isDir(url) else { return UInt64.max }
        var out = Data()
        if let entries = try? fm.contentsOfDirectory(atPath: url.path) {
            for name in entries.sorted() {
                let child = url.appendingPathComponent(name)
                out.append(name.data(using: .utf8) ?? Data())
                if isDir(child) { out.append(UInt8(ascii: "/")) }
                out.append(0)
            }
        }
        let n = min(out.count, Int(len))
        out.withUnsafeBytes { raw in
            if let base = raw.baseAddress { memcpy(buf, base, n) }
        }
        return UInt64(n)
    }

    // stat(path,out[3]) -> [kind, size, blocks]; kind 0 ram, 1 file, 2 dir.
    func stat(_ path: String, out: UnsafeMutablePointer<UInt64>) -> UInt64 {
        if let text = ramfs[path] {
            out[0] = 0; out[1] = UInt64(text.utf8.count); out[2] = 0
            return 0
        }
        guard let rel = relPath(path), let url = diskURL(rel),
              fm.fileExists(atPath: url.path) else { return UInt64.max }
        if isDir(url) {
            out[0] = 2; out[1] = 0; out[2] = 0
        } else {
            let attrs = try? fm.attributesOfItem(atPath: url.path)
            let size = (attrs?[.size] as? UInt64) ?? 0
            out[0] = 1; out[1] = size; out[2] = (size + 511) / 512
        }
        return 0
    }

    // MARK: directory / file management

    func unlink(_ path: String) -> UInt64 {
        guard let rel = relPath(path), let url = diskURL(rel),
              fm.fileExists(atPath: url.path), !isDir(url) else { return UInt64.max }
        return (try? fm.removeItem(at: url)) != nil ? 0 : UInt64.max
    }

    func mkdir(_ path: String) -> UInt64 {
        guard let rel = relPath(path), let url = diskURL(rel) else { return UInt64.max }
        if isDir(url) { return 0 }
        return (try? fm.createDirectory(at: url, withIntermediateDirectories: false)) != nil ? 0 : UInt64.max
    }

    func rmdir(_ path: String) -> UInt64 {
        guard let rel = relPath(path), let url = diskURL(rel), isDir(url) else { return UInt64.max }
        let contents = (try? fm.contentsOfDirectory(atPath: url.path)) ?? ["x"]
        if !contents.isEmpty { return UInt64.max }
        return (try? fm.removeItem(at: url)) != nil ? 0 : UInt64.max
    }

    func rename(_ old: String, _ new: String) -> UInt64 {
        guard let orel = relPath(old), let nrel = relPath(new),
              let ourl = diskURL(orel), let nurl = diskURL(nrel),
              fm.fileExists(atPath: ourl.path), !fm.fileExists(atPath: nurl.path) else { return UInt64.max }
        return (try? fm.moveItem(at: ourl, to: nurl)) != nil ? 0 : UInt64.max
    }

    // MARK: disk geometry

    private func diskFileCount() -> UInt64 {
        guard let e = fm.enumerator(at: diskRoot, includingPropertiesForKeys: [.isDirectoryKey]) else { return 0 }
        var n: UInt64 = 0
        for case let url as URL in e where !isDir(url) { n += 1 }
        return n
    }

    // diskinfo -> (total_sectors << 32) | file_count
    func diskinfo() -> UInt64 {
        let totalSectors: UInt64 = 131072   // nominal 64 MiB disk (512-byte sectors)
        return (totalSectors << 32) | (diskFileCount() & 0xffff_ffff)
    }

    func freeblocks() -> UInt64 {
        let used = diskFileCount()
        let total: UInt64 = 131072
        return total > used ? total - used : 0
    }
}
