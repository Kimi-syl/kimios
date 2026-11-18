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

        KimiBridge.nativeInit(getFilesDir().getAbsolutePath());
        maybeRunGuest(getIntent());
    }

    @Override
    protected void onNewIntent(android.content.Intent intent) {
        super.onNewIntent(intent);
        setIntent(intent);
        maybeRunGuest(intent);
    }

    private void maybeRunGuest(android.content.Intent intent) {
        if (intent == null) return;
        if (intent.getStringExtra("kpkg") != null) {
            new Thread(new Runnable() {
                @Override public void run() {
                    int r = KimiBridge.nativeKpkgUpdate();
                    android.util.Log.i("KimiOS-guest", "kpkg update result=" + r);
                }
            }, "kimi-kpkg").start();
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
