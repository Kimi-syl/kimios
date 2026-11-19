# Embedding a real Browser window in KimiOS (software framebuffer) — research report

RESEARCH ONLY. Goal: replace the full-screen `WebActivity` with a WebView whose pixels are
composited INSIDE the KimiOS framebuffer, so KimiOS keeps drawing title bar, URL bar, taskbar,
window controls and overlays around the page content.

Grounding in this repo:
- GUI runs on a detached native pthread: `android/jni/kimios_jni.c:534` (`gui_thread` → `kimi_gui_start`).
- Front/back buffers are 1080x1920 `0x00RRGGBB` (XRGB) u32; present converts to RGBX_8888 via
  `bswap32(p)>>8` (`kimios_jni.c:163-184`).
- JNI pattern already solved: `g_vm` from `JNI_OnLoad`, `g_bridge` global jclass + cached
  `jmethodID`s created in `nativeInit` on a Java thread (app classloader), `gui_env()`
  AttachCurrentThread per call (`kimios_jni.c:44-68, 547-559`).
- App space is 480x800, mapped NON-uniformly: `map_x = x*1080/480` (2.25x),
  `map_y = y*1920/800` (2.4x) (`user/kimi-gui/src/draw.rs:108-114`).
- Browser today: Rust text fetch + `SYS_OPEN_URL=45` → `KimiBridge.nativeOpenUrl` →
  `WebActivity` (`user/libkimi/src/lib.rs:74,628`, `KimiBridge.java:135-148`,
  `apps.rs:2107 browser_go`, `apps.rs:2497 draw_browser`).

---

## 1. RECOMMENDATION (decisive)

**Offscreen WebView → pixel readback → blit into the KimiOS back buffer (approach 1).
Keep the SurfaceView. Use the system Android WebView as the engine.**

Implement in two phases that share ONE JNI/threading/touch design:

- **Phase 1 (ship first): detached WebView + `measure()/layout()` + `draw(Canvas→Bitmap)` +
  `copyPixelsToBuffer` into a shared native buffer.** Proven pattern (iZettle html2bitmap did
  exactly this in production). Zero APK size growth. Good enough for a text-first "lightweight
  browser". Known limits: Chromium's *software draw* path — no `<video>`/WebGL pixels, slower,
  may regress across WebView updates (Chromium's own doc, see §2.1).
- **Phase 2 (robust upgrade, same bridge): `VirtualDisplay` + `Presentation` + `ImageReader`
  (RGBA_8888) readback.** This is Chromium's *officially supported* snapshot architecture
  (VirtualDisplay backed by a surface; they describe SurfaceTexture+GL readback; ImageReader is
  the no-GL variant). WebView stays hardware-composited, video works (non-DRM), and the
  deprecated software-draw path is avoided.

**Reject TextureView overlay as the primary design** (§2.2): it breaks the
"everything is one framebuffer" invariant — KimiOS overlays (shade, dialogs, menus, taskbar)
would render UNDER a real Android view, and window drag/resize would need cross-thread view
layout updates that lag the native chrome.

**Reject alternative engines for now** (§2.3): GeckoView ≈ 233 MB AAR, Crosswalk dead since
2017, Ultralight has no Android support and is proprietary, Servo embedding is prototype-grade,
litehtml has no JavaScript. "Lightweight" should describe the KimiOS chrome, not the engine —
the system WebView is the only full-web engine that costs 0 bytes.

---

## 2. Approach analysis

### 2.1 Offscreen WebView → Bitmap → blit (RECOMMENDED)

**Does `draw()` work without a window? Yes — after measure/layout.**
The production library iZettle/android-html2bitmap creates a WebView from an application
context, NEVER attaches it to any window, calls
`webView.measure(EXACTLY w, EXACTLY h); webView.layout(0,0,measuredW,measuredH)`, waits for
load completion, then `webView.draw(new Canvas(bitmap))`. It also calls
`WebView.enableSlowWholeDocumentDraw()` (API 21+, must be called before any WebView is created)
when drawing beyond the visible viewport, and debounces ~300 ms after "page finished" because
completion fires multiple times.
Sources:
- https://github.com/iZettle/android-html2bitmap (README: "rendering the content inside an
  off screen webview… some methods on the webview are required to be run on the mainThread")
- https://github.com/iZettle/android-html2bitmap/blob/develop/html2bitmap/src/main/java/com/izettle/html2bitmap/Html2BitmapWebView.java
  (measure/layout/draw code; `screenshot()` = `Bitmap.createBitmap(...); webView.draw(canvas)`)
- Classic SO threads: https://stackoverflow.com/questions/6467214/drawing-offscreen-webview-to-bitmap
  (wait for `WebViewClient.onPageFinished` before drawing) and
  https://stackoverflow.com/questions/31344667/creating-bitmap-from-webview-with-content-offscreen-android
- Official sanction of draw-to-bitmap snapshots: `capturePicture()` deprecation note says
  "Use `WebView.onDraw(Canvas)` to obtain a bitmap snapshot of the WebView":
  https://developer.android.com/reference/android/webkit/WebView
- The View drawing-cache deprecation note likewise describes software snapshots as
  "create a Canvas from a Bitmap … call `View.draw(Canvas)`", while warning about
  hardware-only features: https://developer.android.com/reference/android/view/View

**The big caveat — Chromium deprecates software draw.** Primary source,
`android_webview/docs/software_draw_deprecated.md`:
- "Software draw in WebView is deprecated and only maintained for backward compatibility."
- "Software draw uses more memory. Software draw is very slow. Software draw has known missing
  features, such as `<video>` or `<webgl>`. … not maintained or well tested, and is often
  broken by updates. Fixes … low priority."
- "Do not call `setLayerType(View.LAYER_TYPE_SOFTWARE)` on WebView or any parent of WebView.
  Do not call `webview.onDraw` or `webview.draw` directly."
- "The officially supported way [to snapshot] is to create a VirtualDisplay backed by a
  SurfaceTexture … attach WebView to a Presentation … readback into a Bitmap."
URL: https://chromium.googlesource.com/chromium/src/+/HEAD/android_webview/docs/software_draw_deprecated.md

Consequences for KimiOS:
- A detached WebView drawn into a Bitmap canvas IS the software path (the canvas is not
  hardware-accelerated). Regular HTML/CSS/JS pages render fine; `<video>` and WebGL come out
  blank/black. Corroborated by
  https://stackoverflow.com/questions/72847300/android-webview-to-bitmap-but-hardware-accelerated
  (YouTube audio plays, video rectangle black with `webView.draw`) and
  https://sekin.in/why-android-webview-video-screenshots-are-blank-and-how-to-fix-them/
  (video is composited on a separate surface; canvas capture misses it; PixelCopy or
  VirtualDisplay+SurfaceTexture are the fixes).
- Do NOT try to "fix" blanks with `LAYER_TYPE_SOFTWARE` — it breaks HTML5 canvas rendering on
  modern WebView: https://stackoverflow.com/questions/57465334/android-webview-with-layer-type-software-not-showing-html5-canvas-content
- This is acceptable for Phase 1 of a small browser; Phase 2 (§6) removes the limitation
  without changing the KimiOS-side architecture.

**Keeping content live (dirty-flag loop).** A detached WebView has no ViewRootImpl to schedule
draws; `invalidate()` with no parent is a no-op. YOU must pull frames. What actually updates
`draw()` output: any internal WebView repaint (page load progress, scroll, JS mutation, CSS
animation) updates the compositor state that the next software `draw()` reads. Reliable
change signals:
- `WebViewClient.onPageStarted/onPageFinished`, `WebChromeClient.onProgressChanged`,
  `onReceivedTitle` — https://developer.android.com/reference/android/webkit/WebViewClient ,
  https://developer.android.com/reference/android/webkit/WebChromeClient
- `View.OnScrollChangeListener` (API 23+) / `WebView.onScrollChanged` — fires during scroll
  AND fling: https://developer.android.com/reference/android/view/View#setOnScrollChangeListener(android.view.View.OnScrollChangeListener)
- `WebView.postVisualStateCallback(id, cb)` — "called when the current state of the WebView is
  ready to be drawn": https://developer.android.com/reference/android/webkit/WebView#postVisualStateCallback(long,%20android.webkit.WebView.VisualStateCallback)
- `PictureListener`/`capturePicture()` are obsolete — do not use (interface deprecated,
  "now obsolete"; `setPictureListener` deprecated): same WebView reference page.
- JS-driven animation with no scroll produces NO callback. Options: (a) while the browser
  window is focused, force a redraw at a capped rate (e.g. 10 Hz) — cheap at this size;
  (b) optional `addJavascriptInterface` + injected rAF hook that marks dirty (note: JS bridge
  callbacks arrive on a private background thread — post to main; see `addJavascriptInterface`
  docs on the WebView page).
Recommended loop: event-driven dirty flag + coalesced pull from the GUI thread (§4.4). Draw
only when dirty/forced; never per-frame unconditionally.

**Performance / readback.**
- Cost per frame = software raster of the viewport + one memcpy of w*h*4 bytes. At the KimiOS
  content size (~990x1210 px ≈ 4.8 MB) expect single-digit ms on modern arm64 for typical
  pages; complex pages more. Chromium warns software draw is "very slow" relative to HW —
  measure on device; the dirty flag means static pages cost zero.
- Reuse ONE `Bitmap` (ARGB_8888) + ONE `Canvas` + ONE direct `ByteBuffer`; never allocate per
  frame. `copyPixelsToBuffer` is a raw memcpy; prefer it over `getPixels(int[])` (extra
  conversion + allocation).
  https://developer.android.com/reference/android/graphics/Bitmap#copyPixelsToBuffer(java.nio.Buffer)
- Pixel format: ARGB_8888 `copyPixelsToBuffer` yields **R,G,B,A byte order on devices**
  (little-endian memory layout), despite the "ARGB" name. Evidence:
  https://github.com/robolectric/robolectric/issues/8372 ("on a device, the bytes are RGBA"),
  https://stackoverflow.com/questions/57868130/packing-java-bitmap-into-bytebuffer-byte-order-doesnt-match-pixel-format-and ,
  packing formula in https://developer.android.com/reference/android/graphics/Bitmap.Config .
  This is convenient: KimiOS's window is RGBX_8888 and the GUI back buffer is XRGB u32 — one
  byte-shuffle in the Rust blit (§4.3).
- `PixelCopy` is NOT applicable to a detached WebView (it copies from Surface/SurfaceView/
  Window sources, API 24/26, async, converts format, `ERROR_SOURCE_NO_DATA` until a frame is
  queued): https://developer.android.com/reference/android/view/PixelCopy . It becomes useful
  only in surface-based variants.

**Touch forwarding.** Use `webView.dispatchTouchEvent(ev)` (full internal pipeline: scroll,
fling, clicks, text selection), NOT `onTouchEvent` directly. Synthetic events via
`MotionEvent.obtain(downTime, eventTime, action, x, y, metaState)`; coordinates are
view-local pixels; recycle after dispatch. Keep one `downTime` for the whole gesture; send
`ACTION_CANCEL` when KimiOS steals the gesture. Evidence this works for WebView incl.
scroll/fling:
- https://stackoverflow.com/questions/27600318/dispatch-touch-event-from-a-view-to-his-webview-sibling
  (dispatch to a WebView sibling reproduces default scroll with fling)
- https://forum.magicleap.cloud/t/touch-input-issues-using-webview/2245 (real product feeding
  synthetic touches to WebView via `dispatchTouchEvent`)
- https://developer.android.com/reference/android/view/MotionEvent#obtain(long,%20long,%20int,%20float,%20float,%20int)
- https://developer.android.com/reference/android/view/View#dispatchTouchEvent(android.view.MotionEvent)
The boolean return tells you whether the WebView consumed the event; KimiOS hit-testing
decides routing BEFORE dispatch (content rect → WebView; chrome → Rust), so the return value
is informational only.

### 2.2 TextureView overlay (rejected as primary)

Facts (official docs):
- TextureView "can only be used in a hardware accelerated window. When rendered in software,
  TextureView will draw nothing." It "does not create a separate window but behaves as a
  regular View"; contents "must be copied, internally … may have slower performance than
  SurfaceView … SurfaceView is recommended as a more general solution."
  https://developer.android.com/reference/android/view/TextureView
- SurfaceView is Z-ordered BEHIND its window and punches a hole; "The view hierarchy will
  take care of correctly compositing … siblings … on top of it" — but at the cost of a full
  alpha-blended composite per surface change.
  https://developer.android.com/reference/android/view/SurfaceView

So the overlay design is: KimiOS renders chrome into a TextureView (via
`ANativeWindow_fromSurface(new Surface(textureView.getSurfaceTexture()))`), and a real WebView
sibling is positioned over the content rect. Touch then comes free from Android's input
pipeline, and text is crisp at native resolution. BUT:

1. **Z-order inversion of KimiOS overlays.** Everything KimiOS draws — notification shade,
   dialogs, context menus, taskbar, window-move previews — lives in the texture. A real
   WebView view ALWAYS composites above it. You'd have to hide the WebView whenever any
   overlay is up or the browser isn't frontmost: a second, Java-side window manager mirroring
   native state. That's exactly the "feels like Android, not KimiOS" failure mode.
2. **Window drag/resize desync.** Moving a KimiOS window would require JNI → UI-thread
   LayoutParams updates every frame; view layout lags the native chrome by ≥1 frame, so the
   page visibly detaches from its title bar during drags.
3. TextureView costs an extra internal copy and requires HW acceleration (docs above);
   SurfaceView is the documented recommendation.

Note: you don't even need TextureView to try this — with the existing SurfaceView, a WebView
sibling composites above the hole-punch by default (SurfaceView docs). The overlay problems
are identical. Keep this only as a last-resort fallback; the blit approach keeps ONE source
of truth for compositing, which is the whole point of KimiOS.

### 2.3 Alternative engines (all rejected for v1)

| Engine | Verdict | Evidence |
|---|---|---|
| **Android WebView (system)** | **USE.** 0 added bytes, evergreen, full JS/DOM. Embedding is exactly what it's for ("display web content as part of your activity layout… increased control over the UI"). | https://developer.android.com/reference/android/webkit/WebView |
| GeckoView | Full Firefox engine, self-contained, but the current AAR is **244,578,463 bytes (~233 MB**, all ABIs; per-ABI still tens of MB) — measured via HTTP HEAD on maven.mozilla.org. Also renders through its own compositor to SurfaceView/TextureView, so you'd STILL need surface readback, plus Gecko runtime init. Absurd for a hobby-OS APK. | https://mozilla.github.io/geckoview/ ; https://maven.mozilla.org/maven2/org/mozilla/geckoview/geckoview/160.0.20261116130455/geckoview-160.0.20261116130455.aar |
| Crosswalk | **Dead.** "As of February 2017, Crosswalk is not being developed anymore. The last … release is 23." Repo archived Apr 2020. | https://github.com/crosswalk-project/crosswalk |
| Ultralight | WebKit fork with a CPU renderer (v2.0) that can render to an offscreen bitmap — architecturally attractive — but supported platforms are Windows/macOS/Linux/Xbox/PlayStation/Switch: **no Android**. Proprietary (partial LGPL source; full source = enterprise license). | https://ultralig.ht/ (FAQ platform list, pricing) |
| Servo | Rust prototype engine; Android builds exist (servoshell) but embedding is experimental and the build is a large mach/NDK toolchain. Interesting long-term for a Rust OS, not a browser dependency today. | https://github.com/servo/servo (README), https://book.servo.org |
| litehtml | C++17 HTML/CSS2/3 **layout only**: no JavaScript, draws nothing itself (you implement `document_container`); README: "Using it as a full-featured browser engine is not recommended." Could later render offline help/docs inside KimiOS; not a web browser. | https://github.com/litehtml/litehtml |
| Headless Chromium/CDP screenshots | Full Chromium build + IPC + per-shot latency; no interactivity without a full input bridge. Worst of all worlds here. | (general; cf. WebView2 offscreen discussion https://github.com/MicrosoftEdge/WebView2Feedback/issues/547) |

---

## 3. THREADING MODEL (the key constraint)

**Hard facts:**
1. WebView docs: `getWebViewLooper()` "Returns the Looper corresponding to the thread on which
   WebView calls must be made"; per-method notes: "This method must be called on the thread
   that originally created this UI element. This is typically the main thread."
   https://developer.android.com/reference/android/webkit/WebView
2. Chromium enforces it: `WebViewChromium.checkThread()` throws
   `IllegalStateException("Calling methods on another thread than the UI thread.")` when any
   WebView method runs off the WebView's UI thread:
   https://chromium.googlesource.com/chromium/src/+/main/android_webview/glue/java/src/com/android/webview/chromium/WebViewChromium.java
   (lines ~363-389). Older builds logged "All WebView methods must be called on the same
   thread": https://stackoverflow.com/questions/21955593/android-webview-loadurl-wont-load-another-webpage
3. Creating a WebView on a thread without a Looper crashes (`Handler` →
   "Can't create handler inside thread that has not called Looper.prepare()"):
   https://stackoverflow.com/questions/3875184/cant-create-handler-inside-thread-that-has-not-called-looper-prepare .
   KimiOS's GUI pthread is attached via `AttachCurrentThread` but has **no Looper** — it can
   never create or drive a WebView directly.

**Model: WebView lives on the Android main thread; KimiOS GUI pthread only consumes pixels.**

```
Android main/UI thread                    KimiOS GUI pthread (no Looper)
─────────────────────────                 ──────────────────────────────
owns WebView lifecycle                    owns front/back buffers, chrome,
ALL WebView calls (loadUrl, draw,         window manager, hit-testing
dispatchTouchEvent, goBack…)                     │
        ▲                                        │ JNI (cached g_bridge,
        │ Handler(Looper.getMainLooper()).post   │  gui_env attach/detach)
        │                                 commands: webLoad/webTouch/webBack…
        │                                 (fire-and-forget posts)
        │                                        │
   draw → Bitmap → copyPixelsToBuffer ──► shared native buffer + seqlock
                                                 │
                                          webRender() pulls seq; blit when
                                          a new stable frame exists
```

Rules:
- Every `KimiBridge.nativeWeb*` static method posts a Runnable to
  `new Handler(Looper.getMainLooper())` (or `activity.runOnUiThread`). Commands are
  fire-and-forget; the GUI thread never blocks on the UI thread.
- Render is **asynchronous produce/consume**: UI thread draws into a reused Bitmap, copies
  into a direct ByteBuffer that wraps native memory (`NewDirectByteBuffer`), and publishes
  via a sequence counter (seqlock: odd = writing, even = stable). GUI thread compares
  sequence numbers and blits only complete frames. No locks shared across the JNI boundary,
  no deadlock possibility.
- Queries (`url`, `title`, `canGoBack/Forward`) must NOT call WebView from the GUI pthread
  (would throw). Cache them in volatile Java fields, updated on the UI thread by
  `WebViewClient`/`WebChromeClient` callbacks; the getters return the cache.
- JNI plumbing reuses the existing pattern verbatim: cache `jclass` + method IDs in
  `nativeInit` (Java thread, app classloader — `FindClass` from the attached GUI pthread
  resolves against the SYSTEM classloader; this is already documented at
  `kimios_jni.c:48-51`), attach/detach per call via `gui_env`/`gui_done`.
- Lifecycle: create/destroy WebView on the UI thread; call `web.onPause()/onResume()` when
  the KimiOS browser window is backgrounded/foregrounded to stop JS timers (battery).

---

## 4. Concrete implementation patterns (Phase 1)

### 4.1 Java — `KimiWeb.java` (new file next to KimiBridge; all WebView ops on main)

```java
package com.kimios.libos;

import android.content.Context;
import android.graphics.Bitmap;
import android.graphics.Canvas;
import android.os.Handler;
import android.os.Looper;
import android.os.SystemClock;
import android.view.MotionEvent;
import android.view.View;
import android.webkit.WebChromeClient;
import android.webkit.WebSettings;
import android.webkit.WebView;
import android.webkit.WebViewClient;
import java.nio.ByteBuffer;

/** Offscreen WebView rendered into a native-shared pixel buffer for KimiOS. */
public final class KimiWeb {
    private static final Handler MAIN = new Handler(Looper.getMainLooper());
    private static WebView web;
    private static Bitmap bmp;
    private static Canvas canvas;
    private static ByteBuffer pixels;      // direct buffer over native memory
    private static volatile int seq;       // seqlock: odd = write in progress
    private static volatile boolean dirty = true, renderPosted;
    private static volatile String url = "", title = "";
    private static volatile boolean canBack, canFwd;
    private static int w, h;
    private static long downTime;

    /** Called from native (GUI thread) — everything below hops to main. */
    public static long init(final Context ctx, final int pw, final int ph, final ByteBuffer buf) {
        MAIN.post(() -> {
            w = pw; h = ph; pixels = buf;
            web = new WebView(ctx);                 // MainActivity context (dialogs/IME)
            WebSettings s = web.getSettings();
            s.setJavaScriptEnabled(true);
            s.setDomStorageEnabled(true);
            s.setUseWideViewPort(true);
            s.setLoadWithOverviewMode(true);
            s.setBuiltInZoomControls(false);
            s.setSupportZoom(false);                // single-pointer bridge in v1
            s.setMixedContentMode(WebSettings.MIXED_CONTENT_ALWAYS_ALLOW);
            web.setWebViewClient(new WebViewClient() {
                @Override public void onPageFinished(WebView v, String u) {
                    url = u; canBack = v.canGoBack(); canFwd = v.canGoForward();
                    v.postVisualStateCallback(1, () -> dirty = true);
                    dirty = true;
                }
                @Override public void doUpdateVisitedHistory(WebView v, String u, boolean r) {
                    url = u; canBack = v.canGoBack(); canFwd = v.canGoForward();
                }
            });
            web.setWebChromeClient(new WebChromeClient() {
                @Override public void onReceivedTitle(WebView v, String t) { title = t; }
                @Override public void onProgressChanged(WebView v, int p) { dirty = true; }
            });
            web.setOnScrollChangeListener((v, x, y, ox, oy) -> dirty = true); // API 23+
            sizeTo(pw, ph);
        });
        return 0;
    }

    private static void sizeTo(int pw, int ph) {    // main thread
        w = pw; h = ph;
        web.measure(View.MeasureSpec.makeMeasureSpec(pw, View.MeasureSpec.EXACTLY),
                    View.MeasureSpec.makeMeasureSpec(ph, View.MeasureSpec.EXACTLY));
        web.layout(0, 0, web.getMeasuredWidth(), web.getMeasuredHeight());
        if (bmp != null) bmp.recycle();
        bmp = Bitmap.createBitmap(pw, ph, Bitmap.Config.ARGB_8888);
        canvas = new Canvas(bmp);
        dirty = true;
    }

    public static long load(final String u) { MAIN.post(() -> { if (web != null) web.loadUrl(u); }); return 0; }
    public static long back()    { MAIN.post(() -> { if (web != null && web.canGoBack())    web.goBack(); });    return 0; }
    public static long forward() { MAIN.post(() -> { if (web != null && web.canGoForward()) web.goForward(); }); return 0; }
    public static long reload()  { MAIN.post(() -> { if (web != null) web.reload(); }); return 0; }
    public static long setViewport(final int pw, final int ph) { MAIN.post(() -> { if (web != null) sizeTo(pw, ph); }); return 0; }

    /** Pull-model render: coalesced, dirty-gated. Returns current seq. */
    public static long render(final boolean force) {
        if (web == null || renderPosted || (!dirty && !force)) return seq;
        renderPosted = true;
        MAIN.post(() -> {
            renderPosted = false;
            if (web == null || (!dirty && !force)) return;
            seq++;                                   // odd: writing
            web.draw(canvas);                        // software draw (Phase 1)
            pixels.clear();
            bmp.copyPixelsToBuffer(pixels);          // RGBA bytes on device
            dirty = false;
            seq++;                                   // even: stable
        });
        return seq;
    }

    /** action: 0=down 1=move 2=up 3=cancel; x,y in WebView-local device px. */
    public static long touch(final int x, final int y, final int action) {
        MAIN.post(() -> {
            if (web == null) return;
            long now = SystemClock.uptimeMillis();
            int a;
            switch (action) {
                case 0: downTime = now; a = MotionEvent.ACTION_DOWN; break;
                case 1: a = MotionEvent.ACTION_MOVE; break;
                case 2: a = MotionEvent.ACTION_UP; break;
                default: a = MotionEvent.ACTION_CANCEL; break;
            }
            MotionEvent ev = MotionEvent.obtain(downTime, now, a, x, y, 0);
            web.dispatchTouchEvent(ev);
            ev.recycle();
            dirty = true;
        });
        return 0;
    }

    // Cached getters — safe to call from the GUI pthread (no WebView access).
    public static String url()   { return url; }
    public static String title() { return title; }
    public static long canGoBack()    { return canBack ? 1 : 0; }
    public static long canGoForward() { return canFwd ? 1 : 0; }

    public static long pause()  { MAIN.post(() -> { if (web != null) web.onPause(); });  return 0; }
    public static long resume() { MAIN.post(() -> { if (web != null) web.onResume(); }); return 0; }
    public static long destroy() {
        MAIN.post(() -> { if (web != null) { web.destroy(); web = null; }
                          if (bmp != null) { bmp.recycle(); bmp = null; } });
        return 0;
    }
}
```

Notes:
- `web.draw(canvas)` before the first paint yields blank — gate the first blit on
  `onPageFinished` + `postVisualStateCallback` (both set `dirty` only when content exists).
- html2bitmap debounces 300 ms after completion because "done" fires repeatedly; for an
  interactive browser you don't need the debounce — progressive redraws during load are
  desirable (progress bar in KimiOS chrome).
- If you later want whole-document capture (long screenshots), call
  `WebView.enableSlowWholeDocumentDraw()` BEFORE creating any WebView (API 21+); it has a
  significant performance cost (WebView docs).

### 4.2 JNI surface (C, mirrors existing KimiBridge conventions)

Java static methods on `KimiBridge` (thin wrappers around `KimiWeb`, so the cached
`g_bridge` jclass keeps working):

```java
public static long nativeWebInit(int w, int h, java.nio.ByteBuffer buf) { return KimiWeb.init(activity, w, h, buf); }
public static long nativeWebLoad(String url)        { return KimiWeb.load(url); }
public static long nativeWebRender(boolean force)   { return KimiWeb.render(force); }
public static long nativeWebTouch(int x, int y, int action) { return KimiWeb.touch(x, y, action); }
public static long nativeWebBack()                  { return KimiWeb.back(); }
public static long nativeWebForward()               { return KimiWeb.forward(); }
public static long nativeWebReload()                { return KimiWeb.reload(); }
public static long nativeWebSetViewport(int w, int h) { return KimiWeb.setViewport(w, h); }
public static String nativeWebUrl()                 { return KimiWeb.url(); }
public static String nativeWebTitle()               { return KimiWeb.title(); }
public static long nativeWebCanGoBack()             { return KimiWeb.canGoBack(); }
public static long nativeWebCanGoForward()          { return KimiWeb.canGoForward(); }
public static long nativeWebPause()                 { return KimiWeb.pause(); }
public static long nativeWebResume()                { return KimiWeb.resume(); }
public static long nativeWebDestroy()               { return KimiWeb.destroy(); }
```

C side (`kimios_jni.c`), exactly like `g_fetch_mid` etc.:

```c
static jmethodID g_web_init_mid, g_web_load_mid, g_web_render_mid, g_web_touch_mid,
                 g_web_back_mid, g_web_fwd_mid, g_web_reload_mid, g_web_viewport_mid,
                 g_web_url_mid, g_web_title_mid, g_web_canback_mid, g_web_canfwd_mid,
                 g_web_pause_mid, g_web_resume_mid, g_web_destroy_mid;
// in nativeInit (Java thread!):
g_web_init_mid   = (*env)->GetStaticMethodID(env, g_bridge, "nativeWebInit",
                       "(IILjava/nio/ByteBuffer;)J");
g_web_render_mid = (*env)->GetStaticMethodID(env, g_bridge, "nativeWebRender", "(Z)J");
g_web_touch_mid  = (*env)->GetStaticMethodID(env, g_bridge, "nativeWebTouch", "(III)J");
// ... etc.

static uint8_t *web_pixels; static int web_w, web_h;

// SYS_WEB_INIT (new syscall, e.g. 48): a0=w a1=h
//   web_pixels = calloc(w*h, 4);
//   jobject buf = (*env)->NewDirectByteBuffer(env, web_pixels, w*h*4);
//   CallStaticLongMethod(g_bridge, g_web_init_mid, w, h, buf); DeleteLocalRef(buf);
// SYS_WEB_BUF  (49): return (uint64_t)web_pixels;   // like SYS_FB_BACK=36
// SYS_WEB_RENDER (50): a0=force → CallStaticLongMethod → seq
// SYS_WEB_TOUCH (51): a0=x a1=y a2=action → fire-and-forget
// SYS_WEB_LOAD/BACK/FORWARD/RELOAD/VIEWPORT/PAUSE/RESUME/DESTROY: thin call_str_long-style
// SYS_WEB_URL/TITLE: CallStaticObjectMethod → GetStringUTFChars into caller buffer
// SYS_WEB_CANBACK/CANFWD: cached longs
```

All calls go through the existing `gui_env()`/`gui_done()` attach helpers. No `FindClass`
on the GUI pthread. No blocking waits on the UI thread anywhere.

### 4.3 Rust side (libkimi + kimi-gui)

```rust
// libkimi/src/lib.rs — new syscalls (48+; SYS_OPEN_URL=45 stays as fallback)
pub const SYS_WEB_INIT: u64 = 48;  pub const SYS_WEB_BUF: u64 = 49;
pub const SYS_WEB_RENDER: u64 = 50; pub const SYS_WEB_TOUCH: u64 = 51;
pub const SYS_WEB_LOAD: u64 = 52;  pub const SYS_WEB_BACK: u64 = 53;
pub const SYS_WEB_FORWARD: u64 = 54; pub const SYS_WEB_RELOAD: u64 = 55;
pub const SYS_WEB_VIEWPORT: u64 = 56; pub const SYS_WEB_URL: u64 = 57;
pub const SYS_WEB_TITLE: u64 = 58; pub const SYS_WEB_CANBACK: u64 = 59;
pub const SYS_WEB_CANFWD: u64 = 60; pub const SYS_WEB_PAUSE: u64 = 61;
pub const SYS_WEB_RESUME: u64 = 62; pub const SYS_WEB_DESTROY: u64 = 63;

pub fn web_init(w: u32, h: u32) -> bool { syscall2(SYS_WEB_INIT, w as u64, h as u64) == 0 }
pub fn web_buf() -> *const u8 { syscall0(SYS_WEB_BUF) as *const u8 }
pub fn web_render(force: bool) -> u64 { syscall1(SYS_WEB_RENDER, force as u64) }
pub fn web_touch(x: i32, y: i32, action: u32) { syscall3(SYS_WEB_TOUCH, x as u64, y as u64, action as u64); }
pub fn web_load(url: &str) -> bool { /* str+ptr,len convention like open_url */ }
```

Browser app flow (`apps.rs`):
- On first open / window resize: compute the content rect in DEVICE pixels from app-space
  coords using the same math as `Screen::map_x/map_y`
  (app rect ≈ (20,196)-(460,700) → device ≈ (45,470)-(1035,1680), ~990x1210):
  `k::web_init(w_px, h_px)` (or `web_set_viewport` on resize). Size the WebView in DEVICE
  pixels so the blit is 1:1 (crisp text; pages lay out at device density).
- `browser_go`: `k::web_load(url)` instead of `k::open_url` (keep `open_url` only as an
  explicit "open externally" fallback).
- Each frame, in `draw_browser`:

```rust
// chrome (URL bar, GO, title, status) drawn exactly as today, around the rect
let seq = k::web_render(force_at_10hz_when_focused);
if seq != last_seq && seq & 1 == 0 {          // new stable frame
    let src = k::web_buf();
    for y in 0..h_px {
        let s = unsafe { src.add((y * w_px * 4) as usize) };
        let d = unsafe { back.add(((rect_y0 + y) * 1080 + rect_x0) as usize) };
        for x in 0..w_px {
            let (r, g, b) = unsafe { (*s.add(x*4) as u32, *s.add(x*4+1) as u32, *s.add(x*4+2) as u32) };
            unsafe { *d.add(x as usize) = (r << 16) | (g << 8) | b };  // XRGB
        }
    }
    last_seq = seq;
    screen.mark_dirty(rect);                  // existing dirty-box present path
}
```

- Touch: in `browser_tap`/move handling, if the point is inside the content rect:
  `k::web_touch(map_x(px) - rect_x0, map_y(py) - rect_y0, action)` (view-local device px).
  Chrome hits (URL bar y≈104..148, GO button, scroll buttons) stay in Rust. When a gesture
  that started in the content leaves it or KimiOS steals it (shade pull, window drag), send
  action=3 (CANCEL). Existing single-pointer input bridge is sufficient for scroll/click;
  pinch-zoom is out of scope for v1 (zoom controls disabled in settings).
- Title/URL for the chrome: poll `k::web_title()` / `k::web_url()` (cached strings, cheap);
  back/forward button enablement from `web_can_go_back/forward`.

### 4.4 Efficient update loop (summary)

1. GUI thread calls `web_render(force)` once per frame WHILE the browser window is visible.
2. Java coalesces (`renderPosted`) and skips when `!dirty && !force` → static pages cost one
   JNI call, zero pixels.
3. `dirty` is set by: page start/finish, progress, title, scroll/fling, visual-state callback,
   every forwarded touch.
4. `force` at ~10 Hz while focused catches JS/CSS animation without callbacks; drop to 0 when
   the window is unfocused/backgrounded (also call `web_pause`).
5. Seqlock guarantees the GUI thread never blits a half-written frame; no mutexes cross JNI.

---

## 5. Pitfalls checklist

1. **Looper/UI thread**: WebView creation or any method off the main thread → crash/throw
   (§3 sources). Never call WebView from the GUI pthread — even getters: use caches.
2. **FindClass on attached pthread** hits the system classloader — reuse cached `g_bridge`
   (already solved in `kimios_jni.c:48-51`).
3. **measure()/layout() before draw()**, and again on every resize; first draw before paint
   is blank → gate on `onPageFinished`/`postVisualStateCallback`.
4. **Software draw limits**: no `<video>`/WebGL; slow; may break on WebView updates
   (Chromium doc). Don't use `LAYER_TYPE_SOFTWARE` to paper over issues (breaks HTML canvas).
   Phase 2 is the escape hatch.
5. **PictureListener/capturePicture are obsolete** — ignore old blog posts that use them.
6. **Pixel format**: ARGB_8888 buffer bytes are RGBA on device; KimiOS back buffer is XRGB
   u32, window RGBX. Convert once in the Rust blit. `getPixels()` endian/confusion extra cost
   — use `copyPixelsToBuffer`. (ByteBuffer.order only matters if you read ints, not bytes.)
7. **Coordinate scaling is non-uniform**: 480→1080 = 2.25x, 800→1920 = 2.4x
   (`draw.rs map_x/map_y`). Size the WebView in device pixels; touches are view-local device
   px after subtracting the rect origin; never feed app-space coords to MotionEvent.
8. **No blocking the GUI pthread on the UI thread** (and vice versa): async posts + seqlock.
9. **Memory**: reuse Bitmap/Canvas/ByteBuffer (~4.8 MB at 990x1210 + software-draw overhead);
   recycle on destroy/resize; `WebView.destroy()` on main thread only.
10. **IME/soft keyboard**: a detached WebView has no window focus, so in-page text fields
    can't summon the IME in Phase 1. Keep URL entry in the KimiOS OSK; document the limit.
    (Phase 2's Presentation has a real window on the virtual display and behaves better.)
11. **Dialogs/file chooser/JS alerts** need an Activity context and WebChromeClient handlers;
    create the WebView with MainActivity, disable multi-window
    (`setSupportMultipleWindows(false)`), intercept `target=_blank` via
    `WebViewClient.shouldOverrideUrlLoading` and load in-place (or spawn a new KimiOS window).
12. **DRM/FLAG_SECURE content is intentionally blank** in any readback path — not a bug.
13. **`enableSlowWholeDocumentDraw`** only if drawing beyond the viewport; call before any
    WebView exists; significant perf cost (WebView docs).
14. **Lifecycle**: pause/resume with window focus; on `surfaceDestroyed`/Activity destroy,
    destroy the WebView on the UI thread; handle process death of the WebView renderer
    (`WebViewRenderProcessClient`, optional).

---

## 6. Phase 2 — VirtualDisplay + Presentation + ImageReader (officially supported path)

Same JNI surface; only the Java internals change (`render()` becomes "copy latest
ImageReader image", `dirty` becomes "new image available"):

```java
// main thread:
ImageReader reader = ImageReader.newInstance(w, h, PixelFormat.RGBA_8888, 2);
DisplayManager dm = (DisplayManager) ctx.getSystemService(Context.DISPLAY_SERVICE);
VirtualDisplay vd = dm.createVirtualDisplay("kimi-web", w, h, densityDpi,
        reader.getSurface(), DisplayManager.VIRTUAL_DISPLAY_FLAG_OWN_CONTENT_ONLY);
Presentation p = new Presentation(activity, vd.getDisplay());
p.setContentView(web);            // WebView now attached to a REAL window → HW compositor
p.show();
reader.setOnImageAvailableListener(r -> {
    Image img = r.acquireLatestImage();
    if (img == null) return;
    Image.Plane pl = img.getPlanes()[0];
    seq++;                        // odd
    // copy row-by-row: pl.getRowStride() may exceed w*4
    copyRows(pl.getBuffer(), pl.getRowStride(), pixels, w * 4, h);
    seq++;                        // even
    img.close();
}, backgroundHandler);            // HandlerThread, NOT the GUI pthread
// touch: p.dispatchTouchEvent(ev) or web.dispatchTouchEvent(ev), same coordinate math
// resize: vd.resize(w, h, dpi) (API 21+) + new ImageReader + vd.setSurface(newSurface)
```

- Chromium's documented snapshot architecture is VirtualDisplay + Presentation +
  SurfaceTexture + GL readback; ImageReader is the same idea with CPU readback and no EGL
  context: https://chromium.googlesource.com/chromium/src/+/HEAD/android_webview/docs/software_draw_deprecated.md
- VirtualDisplay renders into an app-provided Surface, API 19+, no special permission for an
  app-owned display; `resize()`/`setSurface()` for window resizes:
  https://developer.android.com/reference/android/hardware/display/VirtualDisplay ,
  https://developer.android.com/reference/android/hardware/display/DisplayManager#createVirtualDisplay(java.lang.String,%20int,%20int,%20int,%20android.view.Surface,%20int)
- Presentation (API 17) is a Dialog bound to a display:
  https://developer.android.com/reference/android/app/Presentation
- ImageReader (API 19) gives CPU-accessible RGBA_8888 planes:
  https://developer.android.com/reference/android/media/ImageReader
- Keeps hardware acceleration (video plays, minus DRM), avoids the deprecated software-draw
  path, and costs ~2 extra buffer copies. Validate on the target device/WebView version
  before switching; keep Phase 1 as a runtime fallback flag.

---

## 7. Source index

Android official:
- WebView (threading, getWebViewLooper, PictureListener obsolete, capturePicture→onDraw,
  postVisualStateCallback, enableSlowWholeDocumentDraw, addJavascriptInterface threading):
  https://developer.android.com/reference/android/webkit/WebView
- WebViewClient / WebChromeClient:
  https://developer.android.com/reference/android/webkit/WebViewClient ,
  https://developer.android.com/reference/android/webkit/WebChromeClient
- View (draw, measure/layout, dispatchTouchEvent, drawing-cache deprecation note,
  OnScrollChangeListener): https://developer.android.com/reference/android/view/View
- MotionEvent.obtain: https://developer.android.com/reference/android/view/MotionEvent
- Bitmap / Bitmap.Config / copyPixelsToBuffer:
  https://developer.android.com/reference/android/graphics/Bitmap ,
  https://developer.android.com/reference/android/graphics/Bitmap.Config
- SurfaceView (z-order/hole punch): https://developer.android.com/reference/android/view/SurfaceView
- TextureView (HW-only, regular view, slower): https://developer.android.com/reference/android/view/TextureView
- PixelCopy: https://developer.android.com/reference/android/view/PixelCopy
- VirtualDisplay / DisplayManager / Presentation / ImageReader:
  https://developer.android.com/reference/android/hardware/display/VirtualDisplay ,
  https://developer.android.com/reference/android/hardware/display/DisplayManager ,
  https://developer.android.com/reference/android/app/Presentation ,
  https://developer.android.com/reference/android/media/ImageReader
- Hardware acceleration guide: https://developer.android.com/develop/ui/views/graphics/hardware-accel

Chromium primary sources:
- Software draw deprecated + official snapshot architecture:
  https://chromium.googlesource.com/chromium/src/+/HEAD/android_webview/docs/software_draw_deprecated.md
- WebViewChromium.checkThread() enforcement:
  https://chromium.googlesource.com/chromium/src/+/main/android_webview/glue/java/src/com/android/webview/chromium/WebViewChromium.java

Production reference implementation (offscreen, unattached WebView → bitmap):
- https://github.com/iZettle/android-html2bitmap (archived 2025, pattern still valid)
- https://github.com/iZettle/android-html2bitmap/blob/develop/html2bitmap/src/main/java/com/izettle/html2bitmap/Html2BitmapWebView.java

StackOverflow / community:
- Offscreen draw: https://stackoverflow.com/questions/6467214/drawing-offscreen-webview-to-bitmap ,
  https://stackoverflow.com/questions/31344667/creating-bitmap-from-webview-with-content-offscreen-android
- Video black with draw(): https://stackoverflow.com/questions/72847300/android-webview-to-bitmap-but-hardware-accelerated ;
  analysis + PixelCopy/VirtualDisplay fixes: https://sekin.in/why-android-webview-video-screenshots-are-blank-and-how-to-fix-them/
- LAYER_TYPE_SOFTWARE breaks canvas: https://stackoverflow.com/questions/57465334/android-webview-with-layer-type-software-not-showing-html5-canvas-content
- Touch dispatch to WebView: https://stackoverflow.com/questions/27600318/dispatch-touch-event-from-a-view-to-his-webview-sibling ,
  https://forum.magicleap.cloud/t/touch-input-issues-using-webview/2245
- Same-thread enforcement: https://stackoverflow.com/questions/21955593/android-webview-loadurl-wont-load-another-webpage
- Looper.prepare crash: https://stackoverflow.com/questions/3875184/cant-create-handler-inside-thread-that-has-not-called-looper-prepare
- RGBA byte order on device: https://github.com/robolectric/robolectric/issues/8372 ,
  https://stackoverflow.com/questions/57868130/packing-java-bitmap-into-bytebuffer-byte-order-doesnt-match-pixel-format-and ,
  https://stackoverflow.com/questions/47970384/why-is-copypixelsfrombuffer-giving-incorrect-color-setpixels-is-correct-but-slo

Engines:
- GeckoView: https://mozilla.github.io/geckoview/ ; AAR size (244,578,463 bytes, HEAD request):
  https://maven.mozilla.org/maven2/org/mozilla/geckoview/geckoview/160.0.20261116130455/geckoview-160.0.20261116130455.aar
- Crosswalk (dead Feb 2017, archived): https://github.com/crosswalk-project/crosswalk
- litehtml (layout-only, no JS): https://github.com/litehtml/litehtml
- Ultralight (no Android, proprietary): https://ultralig.ht/
- Servo (prototype, Android builds exist): https://github.com/servo/servo , https://book.servo.org
