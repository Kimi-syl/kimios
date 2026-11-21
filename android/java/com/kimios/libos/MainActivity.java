package com.kimios.libos;

import android.app.Activity;
import android.content.pm.ActivityInfo;
import android.graphics.Color;
import android.os.Bundle;
import android.view.Gravity;
import android.view.MotionEvent;
import android.view.SurfaceHolder;
import android.view.SurfaceView;
import android.view.View;
import android.view.Window;
import android.view.WindowManager;
import android.widget.FrameLayout;

/**
 * Thin host for the native KimiOS library-OS. The whole GUI runs in Rust on a
 * native thread; this Activity just provides a Surface (ANativeWindow framebuffer),
 * forwards touches, and keeps the 9:16 framebuffer aspect-fit (letterboxed).
 */
public class MainActivity extends Activity
        implements SurfaceHolder.Callback, View.OnTouchListener {

    private KimiView view;

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);
        requestWindowFeature(Window.FEATURE_NO_TITLE);
        getWindow().setFlags(WindowManager.LayoutParams.FLAG_FULLSCREEN,
                WindowManager.LayoutParams.FLAG_FULLSCREEN);
        setRequestedOrientation(ActivityInfo.SCREEN_ORIENTATION_PORTRAIT);

        FrameLayout root = new FrameLayout(this);
        root.setBackgroundColor(Color.BLACK);
        view = new KimiView(this);
        root.addView(view, new FrameLayout.LayoutParams(
                FrameLayout.LayoutParams.MATCH_PARENT,
                FrameLayout.LayoutParams.MATCH_PARENT, Gravity.CENTER));
        setContentView(root);

        view.getHolder().addCallback(this);
        view.setOnTouchListener(this);

        KimiBridge.setContext(getApplicationContext());
        KimiBridge.setActivity(this);
        requestLocationPermissions();
        KimiBridge.nativeInit(getFilesDir().getAbsolutePath());
        maybeRunGuest(getIntent());
    }

    private void requestLocationPermissions() {
        if (checkSelfPermission(android.Manifest.permission.ACCESS_FINE_LOCATION)
                != android.content.pm.PackageManager.PERMISSION_GRANTED) {
            requestPermissions(new String[]{
                    android.Manifest.permission.ACCESS_FINE_LOCATION,
                    android.Manifest.permission.ACCESS_COARSE_LOCATION}, 1);
        }
    }

    @Override
    protected void onActivityResult(int requestCode, int resultCode, android.content.Intent data) {
        super.onActivityResult(requestCode, resultCode, data);
        if (requestCode == KimiBridge.REQ_IMPORT) KimiBridge.handleImportResult(resultCode, data);
    }

    @Override
    protected void onDestroy() {
        KimiBridge.nativeAudio(null, 2);
        KimiBridge.clearActivity(this);
        super.onDestroy();
    }

    @Override
    protected void onNewIntent(android.content.Intent intent) {
        super.onNewIntent(intent);
        setIntent(intent);
        maybeRunGuest(intent);
    }

    private void maybeRunGuest(android.content.Intent intent) {
        if (intent == null) return;
        if (intent.getStringExtra("openapp") != null) {
            final int kind = Integer.parseInt(intent.getStringExtra("openapp"));
            new Thread(new Runnable() {
                @Override public void run() { KimiBridge.nativeOpenApp(kind); }
            }, "kimi-open").start();
            return;
        }
        if (intent.getStringExtra("sys") != null) {
            new Thread(new Runnable() {
                @Override public void run() { KimiBridge.nativeSysTest(); }
            }, "kimi-sys").start();
            return;
        }
        if (intent.getStringExtra("kpkg") != null) {
            new Thread(new Runnable() {
                @Override public void run() {
                    int r = KimiBridge.nativeKpkgUpdate();
                    android.util.Log.i("KimiOS-guest", "kpkg update result=" + r);
                }
            }, "kimi-kpkg").start();
            return;
        }
        final String inst = intent.getStringExtra("install");
        if (inst != null) {
            final String dest = intent.getStringExtra("dest");
            new Thread(new Runnable() {
                @Override public void run() {
                    KimiBridge.nativeKpkgUpdate();
                    String d = dest != null ? dest : "/disk/fonts/" + inst + ".kfont";
                    int r = KimiBridge.nativeInstall(inst, d);
                    android.util.Log.i("KimiOS-guest", "install " + inst + " result=" + r);
                }
            }, "kimi-install").start();
            return;
        }
        final String lb = intent.getStringExtra("linux");
        if (lb != null) {
            final String la = intent.getStringExtra("arg");
            new Thread(new Runnable() {
                @Override public void run() {
                    int code = KimiBridge.nativeLinuxRun(lb, la == null ? "" : la);
                    android.util.Log.i("KimiOS-guest", "linux_run " + lb + " exit=" + code);
                }
            }, "kimi-linux").start();
            return;
        }
        if (intent.getStringExtra("api") != null) {
            new Thread(new Runnable() {
                @Override public void run() {
                    long port = KimiBridge.nativePluginTest(null, 0);
                    android.util.Log.i("KimiOS-guest", "plugin api port=" + port);
                }
            }, "kimi-plugin-api").start();
            return;
        }
        final String ialp = intent.getStringExtra("imalpine");
        if (ialp != null) {
            new Thread(new Runnable() {
                @Override public void run() {
                    int r = KimiBridge.nativeImportAlpine(ialp);
                    android.util.Log.i("KimiOS-guest", "import-alpine " + ialp + " result=" + r);
                }
            }, "kimi-imalpine").start();
            return;
        }
        final String mstore = intent.getStringExtra("mstore");
        if (mstore != null) {
            final String mq = intent.getStringExtra("q");
            new Thread(new Runnable() {
                @Override public void run() {
                    int f = 0;
                    try { f = Integer.parseInt(mstore); } catch (Exception e) {}
                    long n = KimiBridge.nativeStoreCount(f, mq == null ? "" : mq);
                    android.util.Log.i("KimiOS-guest", "mstore filter=" + f + " count=" + n);
                }
            }, "kimi-mstore").start();
            return;
        }
        final String mrun = intent.getStringExtra("mrun");
        if (mrun != null) {
            final String mn = intent.getStringExtra("name");
            new Thread(new Runnable() {
                @Override public void run() {
                    int s = 0;
                    try { s = Integer.parseInt(mrun); } catch (Exception e) {}
                    long r = KimiBridge.nativeStoreRun(mn == null ? "" : mn, s);
                    android.util.Log.i("KimiOS-guest", "mstore run name=" + mn + " src=" + s + " result=" + r);
                }
            }, "kimi-mrun").start();
            return;
        }
        if (intent.getStringExtra("term") != null) {
            new Thread(new Runnable() {
                @Override public void run() {
                    long n = KimiBridge.nativeTermTest();
                    android.util.Log.i("KimiOS-guest", "term-test result=" + n);
                }
            }, "kimi-term").start();
            return;
        }
        final String pl = intent.getStringExtra("plugin");
        if (pl != null) {
            final int bg = "1".equals(intent.getStringExtra("bg")) ? 1 : 0;
            new Thread(new Runnable() {
                @Override public void run() {
                    long r = KimiBridge.nativePluginTest(pl, bg);
                    android.util.Log.i("KimiOS-guest", "plugin " + pl + " result=" + r);
                }
            }, "kimi-plugin").start();
            return;
        }
        final String g = intent.getStringExtra("guest");
        if (g == null) return;
        String a = intent.getStringExtra("arg");
        if (a == null) a = "node".equals(g) ? "console.log('hello from node')"
                                            : "print('hello from python')";
        final String arg = a;
        new Thread(new Runnable() {
            @Override public void run() {
                int code = KimiGuest.run(getFilesDir(), g, arg);
                android.util.Log.i("KimiOS-guest", "guest " + g + " exit=" + code);
            }
        }, "kimi-guest").start();
    }

    @Override public void surfaceCreated(SurfaceHolder holder) {
        KimiBridge.nativeSetSurface(holder.getSurface());
        KimiBridge.nativeStart();
    }
    @Override public void surfaceChanged(SurfaceHolder holder, int format, int w, int h) {}
    @Override public void surfaceDestroyed(SurfaceHolder holder) {
        KimiBridge.nativeSetSurface(null);
    }

    @Override
    public boolean onTouch(View v, MotionEvent ev) {
        int w = Math.max(1, v.getWidth()), h = Math.max(1, v.getHeight());
        int x = clamp((int) (ev.getX() / w * 1080), 0, 1079);
        int y = clamp((int) (ev.getY() / h * 1920), 0, 1919);
        switch (ev.getActionMasked()) {
            case MotionEvent.ACTION_DOWN:
            case MotionEvent.ACTION_POINTER_DOWN:
                KimiBridge.nativeTouch(x, y, 1); break;
            case MotionEvent.ACTION_MOVE:
                KimiBridge.nativeMove(x, y); break;
            case MotionEvent.ACTION_UP:
            case MotionEvent.ACTION_POINTER_UP:
            case MotionEvent.ACTION_CANCEL:
                KimiBridge.nativeTouch(x, y, 0); break;
        }
        return true;
    }

    private static int clamp(int v, int lo, int hi) { return v < lo ? lo : (v > hi ? hi : v); }

    /** SurfaceView that measures itself to a 9:16 aspect fit inside its parent. */
    private static class KimiView extends SurfaceView {
        KimiView(android.content.Context c) { super(c); }
        @Override protected void onMeasure(int widthSpec, int heightSpec) {
            int pw = MeasureSpec.getSize(widthSpec), ph = MeasureSpec.getSize(heightSpec);
            float aspect = 1080f / 1920f;
            int w = pw, h = (int) (w / aspect);
            if (h > ph) { h = ph; w = (int) (h * aspect); }
            setMeasuredDimension(w, h);
        }
    }
}
