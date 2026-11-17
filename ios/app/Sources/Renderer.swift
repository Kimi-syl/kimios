// Renderer: presents the KimiOS framebuffer to the screen with Metal.
//
// The GUI renders 1080x1920 XRGB8888 pixels into a shared MTLBuffer (byte order
// B,G,R,X = BGRA). Each present blits that buffer into a BGRA texture and draws
// a full-screen quad. Rendering is on-demand (MTKView paused + setNeedsDisplay
// on present) so the GPU only works when the GUI actually draws a new frame.

import Metal
import MetalKit

final class Renderer: NSObject, MTKViewDelegate {
    let device: MTLDevice
    private let queue: MTLCommandQueue
    private var pipeline: MTLRenderPipelineState?
    private var texture: MTLTexture?
    private var vertexBuffer: MTLBuffer?
    private let fbWidth = 1080
    private let fbHeight = 1920

    private static let shaderSource = """
    #include <metal_stdlib>
    using namespace metal;
    struct VOut { float4 pos [[position]]; float2 uv; };
    vertex VOut vmain(uint vid [[vertex_id]], constant float4* verts [[buffer(0)]]) {
        VOut o; float4 v = verts[vid];
        o.pos = float4(v.xy, 0.0, 1.0); o.uv = v.zw; return o;
    }
    fragment float4 fmain(VOut in [[stage_in]], texture2d<float> tex [[texture(0)]]) {
        constexpr sampler s(mag_filter::linear, min_filter::linear);
        float4 c = tex.sample(s, in.uv);
        return float4(c.rgb, 1.0);
    }
    """

    init?(device: MTLDevice) {
        self.device = device
        guard let q = device.makeCommandQueue() else { return nil }
        self.queue = q
        super.init()

        guard let library = try? device.makeLibrary(source: Self.shaderSource, options: nil),
              let vfn = library.makeFunction(name: "vmain"),
              let ffn = library.makeFunction(name: "fmain") else { return nil }

        let desc = MTLRenderPipelineDescriptor()
        desc.vertexFunction = vfn
        desc.fragmentFunction = ffn
        desc.colorAttachments[0].pixelFormat = .bgra8Unorm
        pipeline = try? device.makeRenderPipelineState(descriptor: desc)

        let td = MTLTextureDescriptor.texture2DDescriptor(
            pixelFormat: .bgra8Unorm, width: fbWidth, height: fbHeight, mipmapped: false)
        td.usage = [.shaderRead, .renderTarget]
        td.storageMode = .private
        texture = device.makeTexture(descriptor: td)

        // Triangle strip: clip-space xy + texture uv (origin top-left).
        let verts: [Float] = [
            -1, -1, 0, 1,
             1, -1, 1, 1,
            -1,  1, 0, 0,
             1,  1, 1, 0,
        ]
        vertexBuffer = device.makeBuffer(bytes: verts, length: verts.count * 4, options: .storageModeShared)
        if pipeline == nil || texture == nil || vertexBuffer == nil { return nil }
    }

    func draw(in view: MTKView) {
        guard let fb = KimiHost.shared.frontBufferMTL,
              let tex = texture, let pipe = pipeline, let vb = vertexBuffer,
              let drawable = view.currentDrawable,
              let rpd = view.currentRenderPassDescriptor,
              let cmd = queue.makeCommandBuffer() else { return }

        if let blit = cmd.makeBlitCommandEncoder() {
            blit.copy(from: fb, sourceOffset: 0,
                      sourceBytesPerRow: fbWidth * 4,
                      sourceBytesPerImage: fbWidth * fbHeight * 4,
                      sourceSize: MTLSizeMake(fbWidth, fbHeight, 1),
                      to: tex, destinationSlice: 0, destinationLevel: 0,
                      destinationOrigin: MTLOriginMake(0, 0, 0))
            blit.endEncoding()
        }

        if let enc = cmd.makeRenderCommandEncoder(descriptor: rpd) {
            enc.setRenderPipelineState(pipe)
            enc.setVertexBuffer(vb, offset: 0, index: 0)
            enc.setFragmentTexture(tex, index: 0)
            enc.drawPrimitives(type: .triangleStrip, vertexStart: 0, vertexCount: 4)
            enc.endEncoding()
        }
        cmd.present(drawable)
        cmd.commit()
    }

    func mtkView(_ view: MTKView, drawableSizeWillChange size: CGSize) {}
}
