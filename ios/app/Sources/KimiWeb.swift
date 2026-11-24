// KimiWeb: offscreen WKWebView bridge for the shared Browser app.
//
// The userland expects a stable RGBA pixel buffer plus a monotonically
// increasing sequence number.  WKWebView renders on the main thread; the GUI
// thread polls snapshots and blits the buffer into the Metal framebuffer.

import Foundation
import UIKit
import WebKit

final class KimiWeb: NSObject, WKNavigationDelegate {
    static let shared = KimiWeb()

    private let lock = NSLock()
    private var view: WKWebView?
    private var pixels: UnsafeMutableRawPointer?
    private var width = 0
    private var height = 0
    private var seq: UInt64 = 0
    private var snapshotting = false
    private var lastSnapshotMs: UInt64 = 0
    private var cachedURL = ""
    private var cachedTitle = ""
    private var canBack = false
    private var canForward = false
    private var touchStart: CGPoint?

    private func ensureView(_ w: Int, _ h: Int) -> Bool {
        if view != nil && width == w && height == h { return true }
        if w <= 0 || h <= 0 { return false }
        width = w; height = h
        let old = pixels
        pixels = malloc(w * h * 4)
        if let p = pixels { memset(p, 0, w * h * 4) }
        if let old { free(old) }

        var made = false
        DispatchQueue.main.sync {
            let cfg = WKWebViewConfiguration()
            cfg.allowsInlineMediaPlayback = true
            let wv = WKWebView(frame: CGRect(x: 0, y: 0, width: w, height: h), configuration: cfg)
            wv.navigationDelegate = self
            wv.isOpaque = true
            wv.backgroundColor = .white
            if let root = Self.rootView() {
                wv.frame = CGRect(x: -CGFloat(w) * 2, y: -CGFloat(h) * 2,
                                  width: CGFloat(w), height: CGFloat(h))
                root.addSubview(wv)
            }
            self.view = wv
            made = true
        }
        return made
    }

    private static func rootView() -> UIView? {
        let scenes = UIApplication.shared.connectedScenes.compactMap { $0 as? UIWindowScene }
        let window = scenes.flatMap { $0.windows }.first(where: { $0.isKeyWindow })
            ?? scenes.first?.windows.first
        return window?.rootViewController?.view
    }

    func initialize(w: Int, h: Int) -> Bool {
        lock.lock(); defer { lock.unlock() }
        if view != nil && width == w && height == h { return true }
        return ensureView(w, h)
    }

    func buffer() -> UnsafeMutableRawPointer? {
        lock.lock(); defer { lock.unlock() }
        return pixels
    }

    func load(_ url: String) -> Bool {
        guard let u = URL(string: url) else { return false }
        lock.lock()
        cachedURL = url
        lock.unlock()
        DispatchQueue.main.async {
            guard let v = self.view else { return }
            v.load(URLRequest(url: u, cachePolicy: .useProtocolCachePolicy, timeoutInterval: 30))
        }
        return true
    }

    func render(force: Bool) -> UInt64 {
        lock.lock()
        if snapshotting || (!force && nowMs() - lastSnapshotMs < 80) {
            let s = seq; lock.unlock(); return s
        }
        snapshotting = true
        lastSnapshotMs = nowMs()
        lock.unlock()

        DispatchQueue.main.async {
            guard let v = self.view else {
                self.lock.lock(); self.snapshotting = false; self.lock.unlock(); return
            }
            let cfg = WKSnapshotConfiguration()
            cfg.rect = v.bounds
            v.takeSnapshot(with: cfg) { image, _ in
                if let image { self.copy(image: image) }
                self.lock.lock()
                self.seq &+= 2
                self.snapshotting = false
                self.lock.unlock()
            }
        }
        lock.lock()
        let s = seq
        lock.unlock()
        return s
    }

    private func copy(image: UIImage) {
        guard let cg = image.cgImage else { return }
        lock.lock()
        let w = width, h = height
        let dst = pixels
        lock.unlock()
        guard w > 0, h > 0, let dst else { return }
        let color = CGColorSpaceCreateDeviceRGB()
        guard let ctx = CGContext(data: dst, width: w, height: h, bitsPerComponent: 8,
                                  bytesPerRow: w * 4, space: color,
                                  bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue) else { return }
        ctx.interpolationQuality = .medium
        ctx.draw(cg, in: CGRect(x: 0, y: 0, width: w, height: h))
    }

    func touch(x: UInt32, y: UInt32, action: UInt32) -> Bool {
        let p = CGPoint(x: Double(x), y: Double(y))
        switch action {
        case 0:
            touchStart = p
            return true
        case 1:
            let start = touchStart ?? p
            touchStart = nil
            let dx = p.x - start.x, dy = p.y - start.y
            if abs(dx) < 12 && abs(dy) < 12 {
                tap(p)
            } else {
                scroll(dx: dx, dy: dy)
            }
            return true
        case 2:
            return true
        case 3:
            touchStart = nil
            return true
        default:
            return false
        }
    }

    private func tap(_ p: CGPoint) {
        let js = "var e=document.elementFromPoint(\(p.x),\(p.y)); if(e){e.click();}"
        DispatchQueue.main.async { self.view?.evaluateJavaScript(js) }
    }

    private func scroll(dx: CGFloat, dy: CGFloat) {
        let js = "window.scrollBy(\(-dx),\(-dy));"
        DispatchQueue.main.async { self.view?.evaluateJavaScript(js) }
    }

    func back() -> Bool {
        lock.lock(); let ok = canBack; lock.unlock()
        if ok { DispatchQueue.main.async { self.view?.goBack() } }
        return ok
    }

    func forward() -> Bool {
        lock.lock(); let ok = canForward; lock.unlock()
        if ok { DispatchQueue.main.async { self.view?.goForward() } }
        return ok
    }

    func reload() -> Bool {
        DispatchQueue.main.async { self.view?.reload() }
        return true
    }

    func url() -> String { lock.lock(); let s = cachedURL; lock.unlock(); return s }
    func title() -> String { lock.lock(); let s = cachedTitle; lock.unlock(); return s }
    func canGoBack() -> Bool { lock.lock(); let b = canBack; lock.unlock(); return b }
    func canGoForward() -> Bool { lock.lock(); let b = canForward; lock.unlock(); return b }

    func viewport(w: Int, h: Int) -> Bool {
        if w == 0 || h == 0 { return false }
        lock.lock(); let same = (width == w && height == h); lock.unlock()
        if same { return true }
        return initialize(w: w, h: h)
    }

    func pause() -> Bool { true }
    func resume() -> Bool { true }

    private func nowMs() -> UInt64 { UInt64(Date().timeIntervalSince1970 * 1000) }

    // MARK: WKNavigationDelegate

    func webView(_ webView: WKWebView, didCommit navigation: WKNavigation!) {
        syncMeta(webView)
    }

    func webView(_ webView: WKWebView, didFinish navigation: WKNavigation!) {
        syncMeta(webView)
    }

    func webView(_ webView: WKWebView, didFail navigation: WKNavigation!, withError error: Error) {
        syncMeta(webView)
    }

    private func syncMeta(_ v: WKWebView) {
        lock.lock()
        cachedURL = v.url?.absoluteString ?? cachedURL
        cachedTitle = v.title ?? cachedTitle
        canBack = v.canGoBack
        canForward = v.canGoForward
        lock.unlock()
    }
}