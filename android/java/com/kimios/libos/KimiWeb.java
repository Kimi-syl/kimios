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
import android.webkit.WebResourceError;
import android.webkit.WebResourceRequest;
import android.webkit.WebSettings;
import android.webkit.WebView;
import android.webkit.WebViewClient;
import java.nio.ByteBuffer;

/**
 * Offscreen WebView rendered into a direct ByteBuffer for KimiOS (SYS_WEB=48).
 *
 * Threading: the WebView is created and driven ONLY on the Android main thread
 * (Chromium checkThread). The KimiOS GUI pthread calls the static entry points
 * below, which post fire-and-forget Runnables to the main looper and read
 * volatile caches. Pixels are published through a seqlock: seq odd = a frame
 * is being written, seq even = stable frame the GUI thread may blit.
 */
public final class KimiWeb {
    private static final Handler MAIN = new Handler(Looper.getMainLooper());

    private static volatile WebView web;
    private static volatile ByteBuffer pixels;
    private static Bitmap bmp;
    private static Canvas canvas;
    private static int w, h;
    private static long downTime;

    private static volatile int seq;
    private static volatile boolean dirty = true;
    private static volatile boolean renderPosted;
    private static volatile String url = "";
    private static volatile String title = "";
    private static volatile boolean canBack, canFwd;

    /** GUI thread. Allocates the pixel buffer synchronously (so buf() works
     *  immediately), creates/resizes the WebView on the main thread. 0 ok. */
    public static long init(final Context ctx, final int pw, final int ph) {
        if (ctx == null || pw <= 0 || ph <= 0 || pw > 4096 || ph > 4096) return -1;
        pixels = ByteBuffer.allocateDirect(pw * ph * 4);
        MAIN.post(new Runnable() {
            @Override public void run() {
                try {
                    if (web == null) createWeb(ctx);
                    sizeTo(pw, ph);
                } catch (Throwable t) {
                    android.util.Log.e("KimiOS", "web init failed: " + t);
                }
            }
        });
        return 0;
    }

    /** GUI thread. Direct buffer holding RGBA pixels (main thread writes it). */
    public static ByteBuffer buf() { return pixels; }

    /** Main thread only. */
    private static void createWeb(Context ctx) {
        WebView v = new WebView(ctx);
        WebSettings s = v.getSettings();
        s.setJavaScriptEnabled(true);
        s.setDomStorageEnabled(true);
        s.setUseWideViewPort(true);
        s.setLoadWithOverviewMode(true);
        s.setBuiltInZoomControls(false);
        s.setDisplayZoomControls(false);
        s.setSupportZoom(false);
        s.setSupportMultipleWindows(false);
        s.setMixedContentMode(WebSettings.MIXED_CONTENT_ALWAYS_ALLOW);
        v.setWebViewClient(new WebViewClient() {
            @Override public boolean shouldOverrideUrlLoading(WebView view, WebResourceRequest req) {
                if (req.isForMainFrame()) { view.loadUrl(req.getUrl().toString()); return true; }
                return false;
            }
            @Override public void onPageStarted(WebView view, String u, Bitmap favicon) {
                url = u;
                dirty = true;
            }
            @Override public void onPageFinished(WebView view, String u) {
                url = u;
                canBack = view.canGoBack();
                canFwd = view.canGoForward();
                dirty = true;
            }
            @Override public void doUpdateVisitedHistory(WebView view, String u, boolean reload) {
                url = u;
                canBack = view.canGoBack();
                canFwd = view.canGoForward();
                dirty = true;
            }
            @Override public void onReceivedError(WebView view, WebResourceRequest req, WebResourceError err) {
                if (req.isForMainFrame()) dirty = true;
            }
        });
        v.setWebChromeClient(new WebChromeClient() {
            @Override public void onReceivedTitle(WebView view, String t) {
                title = t == null ? "" : t;
                dirty = true;
            }
            @Override public void onProgressChanged(WebView view, int progress) {
                dirty = true;
            }
        });
        v.setOnScrollChangeListener(new View.OnScrollChangeListener() {
            @Override public void onScrollChange(View view, int x, int y, int ox, int oy) {
                dirty = true;
            }
        });
        web = v;
    }

    /** Main thread only. measure/layout + reallocate bitmap and pixel buffer. */
    private static void sizeTo(int pw, int ph) {
        if (web == null || pw <= 0 || ph <= 0) return;
        w = pw; h = ph;
        web.measure(View.MeasureSpec.makeMeasureSpec(pw, View.MeasureSpec.EXACTLY),
                    View.MeasureSpec.makeMeasureSpec(ph, View.MeasureSpec.EXACTLY));
        web.layout(0, 0, web.getMeasuredWidth(), web.getMeasuredHeight());
        if (bmp != null) bmp.recycle();
        bmp = Bitmap.createBitmap(pw, ph, Bitmap.Config.ARGB_8888);
        canvas = new Canvas(bmp);
        ByteBuffer cur = pixels;
        if (cur == null || cur.capacity() != pw * ph * 4)
            pixels = ByteBuffer.allocateDirect(pw * ph * 4);
        dirty = true;
    }

    /** GUI thread. Coalesced dirty-gated pull render; returns current seq. */
    public static long render(final boolean force) {
        if (web == null || pixels == null || renderPosted || (!dirty && !force)) return seq;
        renderPosted = true;
        MAIN.post(new Runnable() {
            @Override public void run() {
                renderPosted = false;
                if (web == null || bmp == null || pixels == null) return;
                if (!dirty && !force) return;
                if (pixels.capacity() < bmp.getWidth() * bmp.getHeight() * 4) return;
                seq++;
                try {
                    web.draw(canvas);
                    pixels.clear();
                    bmp.copyPixelsToBuffer(pixels);
                    dirty = false;
                } catch (Throwable t) {
                    android.util.Log.e("KimiOS", "web draw failed: " + t);
                } finally {
                    seq++;
                }
            }
        });
        return seq;
    }

    /** GUI thread. action: 0=down, 1=up, 2=move, 3=cancel; view-local device px. */
    public static long touch(final int x, final int y, final int action) {
        MAIN.post(new Runnable() {
            @Override public void run() {
                WebView v = web;
                if (v == null) return;
                long now = SystemClock.uptimeMillis();
                int a;
                switch (action) {
                    case 0: downTime = now; a = MotionEvent.ACTION_DOWN; break;
                    case 1: a = MotionEvent.ACTION_UP; break;
                    case 2: a = MotionEvent.ACTION_MOVE; break;
                    default: a = MotionEvent.ACTION_CANCEL; break;
                }
                if (downTime == 0) downTime = now;
                MotionEvent ev = MotionEvent.obtain(downTime, now, a, x, y, 0);
                try { v.dispatchTouchEvent(ev); } finally { ev.recycle(); }
                dirty = true;
            }
        });
        return 0;
    }

    public static long load(final String u) {
        if (u == null || u.isEmpty()) return -1;
        MAIN.post(new Runnable() {
            @Override public void run() { if (web != null) web.loadUrl(u); }
        });
        return 0;
    }

    public static long back() {
        MAIN.post(new Runnable() {
            @Override public void run() { if (web != null && web.canGoBack()) web.goBack(); }
        });
        return 0;
    }

    public static long forward() {
        MAIN.post(new Runnable() {
            @Override public void run() { if (web != null && web.canGoForward()) web.goForward(); }
        });
        return 0;
    }

    public static long reload() {
        MAIN.post(new Runnable() {
            @Override public void run() { if (web != null) web.reload(); }
        });
        return 0;
    }

    public static long viewport(final int pw, final int ph) {
        if (pw <= 0 || ph <= 0 || pw > 4096 || ph > 4096) return -1;
        MAIN.post(new Runnable() {
            @Override public void run() { sizeTo(pw, ph); }
        });
        return 0;
    }

    public static long pause() {
        MAIN.post(new Runnable() {
            @Override public void run() { if (web != null) web.onPause(); }
        });
        return 0;
    }

    public static long resume() {
        MAIN.post(new Runnable() {
            @Override public void run() { if (web != null) web.onResume(); }
        });
        return 0;
    }

    public static long destroy() {
        MAIN.post(new Runnable() {
            @Override public void run() {
                if (web != null) { web.destroy(); web = null; }
                if (bmp != null) { bmp.recycle(); bmp = null; }
                canvas = null;
            }
        });
        return 0;
    }

    // Cached getters: safe from the GUI pthread (no WebView access).
    public static String url() { return url; }
    public static String title() { return title; }
    public static long canGoBack() { return canBack ? 1 : 0; }
    public static long canGoForward() { return canFwd ? 1 : 0; }

    private KimiWeb() {}
}