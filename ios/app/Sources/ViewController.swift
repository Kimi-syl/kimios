// ViewController: hosts the Metal view, forwards UIKit touches to the KimiOS
// input queue, and launches the native GUI loop on a dedicated thread.

import UIKit
import MetalKit

final class ViewController: UIViewController {
    private var mtkView: MTKView!
    private var renderer: Renderer?
    private var guiThread: Thread?
    private let aspect = 1080.0 / 1920.0

    override var prefersStatusBarHidden: Bool { true }
    override var prefersHomeIndicatorAutoHidden: Bool { true }

    override func viewDidLoad() {
        super.viewDidLoad()
        view.backgroundColor = .black

        guard let device = MTLCreateSystemDefaultDevice(),
              let renderer = Renderer(device: device) else {
            fatalError("Metal unavailable")
        }
        self.renderer = renderer

        let mv = MTKView(frame: view.bounds, device: device)
        mv.delegate = renderer
        mv.colorPixelFormat = .bgra8Unorm
        mv.clearColor = MTLClearColorMake(0, 0, 0, 1)
        mv.isPaused = true                 // render only when the GUI presents
        mv.enableSetNeedsDisplay = true
        mv.preferredFramesPerSecond = 120  // ProMotion where available
        mv.autoResizeDrawable = true
        mv.contentMode = .scaleToFill
        view.addSubview(mv)
        self.mtkView = mv

        KimiHost.shared.configure(device: device)
        KimiHost.shared.onPresent = { [weak mv] in
            DispatchQueue.main.async { mv?.setNeedsDisplay() }
        }

        let t = Thread { kimi_gui_start() }
        t.name = "kimios.gui"
        t.qualityOfService = .userInteractive
        t.start()
        guiThread = t
    }

    override func viewDidLayoutSubviews() {
        super.viewDidLayoutSubviews()
        // Aspect-fit the 9:16 framebuffer inside the screen (letterbox).
        let b = view.bounds
        var w = b.width
        var h = w / aspect
        if h > b.height { h = b.height; w = h * aspect }
        mtkView.frame = CGRect(x: (b.width - w) / 2, y: (b.height - h) / 2, width: w, height: h)
    }

    // MARK: touches -> KimiOS input

    private func fbPoint(_ touches: Set<UITouch>) -> (Int, Int)? {
        guard let t = touches.first else { return nil }
        let p = t.location(in: mtkView)
        let bw = max(1, mtkView.bounds.width), bh = max(1, mtkView.bounds.height)
        let x = Int(p.x / bw * 1080)
        let y = Int(p.y / bh * 1920)
        return (max(0, min(1079, x)), max(0, min(1919, y)))
    }

    override func touchesBegan(_ touches: Set<UITouch>, with event: UIEvent?) {
        if let (x, y) = fbPoint(touches) { KimiHost.shared.pushTouch(x: x, y: y, down: true) }
    }
    override func touchesMoved(_ touches: Set<UITouch>, with event: UIEvent?) {
        if let (x, y) = fbPoint(touches) { KimiHost.shared.pushMove(x: x, y: y) }
    }
    override func touchesEnded(_ touches: Set<UITouch>, with event: UIEvent?) {
        if let (x, y) = fbPoint(touches) { KimiHost.shared.pushTouch(x: x, y: y, down: false) }
    }
    override func touchesCancelled(_ touches: Set<UITouch>, with event: UIEvent?) {
        if let (x, y) = fbPoint(touches) { KimiHost.shared.pushTouch(x: x, y: y, down: false) }
    }
}
