package com.kimios.libos;

import android.app.Activity;
import android.content.Context;
import android.content.Intent;
import android.content.pm.PackageManager;
import android.location.Location;
import android.location.LocationManager;
import android.media.MediaPlayer;
import android.net.Uri;
import android.view.Surface;

/** JNI bridge to the native KimiOS library-OS (libkimios.so). */
public final class KimiBridge {
    static { System.loadLibrary("kimios"); }

    public static native void nativeInit(String filesDir);
    public static native void nativeSetSurface(Surface surface);
    public static native void nativeStart();
    public static native void nativeTouch(int x, int y, int down);
    public static native void nativeMove(int x, int y);

    /** Run a Linux ELF guest natively (forked child on the Android kernel).
     *  Returns the guest exit code. Blocks; call from a background thread. */
    public static native int nativeRunElf(String exe, String sysroot,
                                          String[] argv, String[] envp);

    /** Run an Alpine sysroot binary via SYS_LINUX_RUN (shell node/python path). */
    public static native int nativeLinuxRun(String bin, String arg);

    /** Test hook: fetch+cache the kpkg repo from GitHub (1 = success). */
    public static native int nativeKpkgUpdate();

    /** Test hook: install a repo asset to a /disk path (1 = success). */
    public static native int nativeInstall(String name, String dest);

    // Host services for GUI-thread syscalls. Set by MainActivity on the Java
    // side so the static methods below never need a Context passed through C.
    private static volatile Context appContext;
    private static volatile Activity activity;
    private static volatile String pendingImportDir;
    public static final int REQ_IMPORT = 4700;

    public static void setContext(Context c) { appContext = c; }
    public static void setActivity(Activity a) { activity = a; }
    public static void clearActivity(Activity a) { if (activity == a) activity = null; }

    /** Called from C (SYS_FETCH_URL): GET url (HTTP/HTTPS) and write the body to
     *  the absolute sandbox path dest. Returns bytes written, or -1 on error. */
    public static long nativeFetchUrl(String url, String dest) {
        try {
            java.io.File f = new java.io.File(dest);
            java.io.File parent = f.getParentFile();
            if (parent != null) parent.mkdirs();
            java.net.HttpURLConnection c =
                    (java.net.HttpURLConnection) new java.net.URL(url).openConnection();
            c.setInstanceFollowRedirects(true);
            c.setConnectTimeout(15000);
            c.setReadTimeout(20000);
            c.setRequestProperty("User-Agent", "KimiOS-libos");
            int code = c.getResponseCode();
            if (code != 200) { c.disconnect(); return -1; }
            java.io.InputStream in = c.getInputStream();
            java.io.FileOutputStream out = new java.io.FileOutputStream(f);
            byte[] buf = new byte[8192];
            long total = 0; int n;
            while ((n = in.read(buf)) > 0) { out.write(buf, 0, n); total += n; }
            out.close(); in.close(); c.disconnect();
            return total;
        } catch (Exception e) {
            android.util.Log.e("KimiOS", "fetchUrl failed: " + e);
            return -1;
        }
    }

    /** Called from C (SYS_LOCAL_TIME): local device time as
     *  [epoch_s, year, month(1-12), day, hour, minute, second,
     *   weekday(0=Sunday..6), tz_offset_minutes]. */
    public static long[] nativeLocalTime() {
        long now = System.currentTimeMillis();
        java.util.Calendar c = java.util.Calendar.getInstance();
        c.setTimeInMillis(now);
        int offMs = c.get(java.util.Calendar.ZONE_OFFSET) + c.get(java.util.Calendar.DST_OFFSET);
        return new long[] {
                now / 1000,
                c.get(java.util.Calendar.YEAR),
                c.get(java.util.Calendar.MONTH) + 1,
                c.get(java.util.Calendar.DAY_OF_MONTH),
                c.get(java.util.Calendar.HOUR_OF_DAY),
                c.get(java.util.Calendar.MINUTE),
                c.get(java.util.Calendar.SECOND),
                c.get(java.util.Calendar.DAY_OF_WEEK) - 1,
                offMs / 60000
        };
    }

    /** Called from C (SYS_LOCATION): last known fix as
     *  [lat_f64_bits, lon_f64_bits, accuracy_m, has_fix]. No fix -> has_fix=0. */
    public static long[] nativeLocation() {
        long[] r = new long[] {0, 0, 0, 0};
        Context ctx = appContext;
        if (ctx == null) return r;
        try {
            if (ctx.checkSelfPermission(android.Manifest.permission.ACCESS_FINE_LOCATION)
                        != PackageManager.PERMISSION_GRANTED
                    && ctx.checkSelfPermission(android.Manifest.permission.ACCESS_COARSE_LOCATION)
                        != PackageManager.PERMISSION_GRANTED) {
                return r;
            }
            LocationManager lm = (LocationManager) ctx.getSystemService(Context.LOCATION_SERVICE);
            if (lm == null) return r;
            Location best = null;
            String[] providers = {LocationManager.GPS_PROVIDER, LocationManager.NETWORK_PROVIDER,
                    LocationManager.PASSIVE_PROVIDER};
            for (String p : providers) {
                try {
                    Location l = lm.getLastKnownLocation(p);
                    if (l != null && (best == null || l.getTime() > best.getTime())) best = l;
                } catch (Exception ignored) {}
            }
            if (best != null) {
                r[0] = Double.doubleToRawLongBits(best.getLatitude());
                r[1] = Double.doubleToRawLongBits(best.getLongitude());
                r[2] = (long) best.getAccuracy();
                r[3] = 1;
            }
        } catch (Exception e) {
            android.util.Log.e("KimiOS", "location failed: " + e);
        }
        return r;
    }

    /** Called from C (SYS_OPEN_URL): open url in a real WebView (WebActivity).
     *  Returns 0 on launch, -1 on error. */
    public static long nativeOpenUrl(String url) {
        Context ctx = appContext;
        if (ctx == null || url == null || url.isEmpty()) return -1;
        try {
            Intent i = new Intent(ctx, WebActivity.class);
            i.putExtra("url", url);
            i.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK);
            ctx.startActivity(i);
            return 0;
        } catch (Exception e) {
            android.util.Log.e("KimiOS", "openUrl failed: " + e);
            return -1;
        }
    }

    private static final Object AUDIO_LOCK = new Object();
    private static MediaPlayer player;

    /** Called from C (SYS_AUDIO): cmd 0=play(path), 1=pause, 2=stop/release,
     *  3=resume. Returns 0 ok, -1 error. */
    public static long nativeAudio(String path, int cmd) {
        synchronized (AUDIO_LOCK) {
            try {
                switch (cmd) {
                    case 0: {
                        if (path == null) return -1;
                        releasePlayerLocked();
                        MediaPlayer mp = new MediaPlayer();
                        mp.setOnErrorListener(new MediaPlayer.OnErrorListener() {
                            @Override public boolean onError(MediaPlayer m, int what, int extra) {
                                android.util.Log.e("KimiOS", "audio error " + what + "/" + extra);
                                synchronized (AUDIO_LOCK) {
                                    if (player == m) player = null;
                                    try { m.release(); } catch (Exception ignored) {}
                                }
                                return true;
                            }
                        });
                        mp.setDataSource(path);
                        mp.prepare();
                        mp.start();
                        player = mp;
                        return 0;
                    }
                    case 1:
                        if (player == null) return -1;
                        if (player.isPlaying()) player.pause();
                        return 0;
                    case 2:
                        releasePlayerLocked();
                        return 0;
                    case 3:
                        if (player == null) return -1;
                        if (!player.isPlaying()) player.start();
                        return 0;
                    default:
                        return -1;
                }
            } catch (Exception e) {
                android.util.Log.e("KimiOS", "audio cmd " + cmd + " failed: " + e);
                releasePlayerLocked();
                return -1;
            }
        }
    }

    private static void releasePlayerLocked() {
        if (player != null) {
            try { player.stop(); } catch (Exception ignored) {}
            try { player.release(); } catch (Exception ignored) {}
            player = null;
        }
    }

    /** Called from C (SYS_AUDIO cmd 4): [position_ms, duration_ms, is_playing]. */
    public static long[] nativeAudioQuery() {
        synchronized (AUDIO_LOCK) {
            long pos = 0, dur = 0, playing = 0;
            if (player != null) {
                try { pos = player.getCurrentPosition(); } catch (Exception ignored) {}
                try { dur = player.getDuration(); } catch (Exception ignored) {}
                try { playing = player.isPlaying() ? 1 : 0; } catch (Exception ignored) {}
            }
            return new long[] {pos, dur, playing};
        }
    }

    /** Called from C (SYS_IMPORT_FILE): launch the SAF picker; the picked file
     *  is copied async into absDir by handleImportResult. Returns 0 if launched. */
    public static long nativeImportFile(String absDir) {
        final Activity a = activity;
        if (a == null || absDir == null) return -1;
        try {
            new java.io.File(absDir).mkdirs();
            final Intent i = new Intent(Intent.ACTION_OPEN_DOCUMENT);
            i.addCategory(Intent.CATEGORY_OPENABLE);
            i.setType("*/*");
            if (i.resolveActivity(a.getPackageManager()) == null) return -1;
            pendingImportDir = absDir;
            a.runOnUiThread(new Runnable() {
                @Override public void run() {
                    try {
                        a.startActivityForResult(i, REQ_IMPORT);
                    } catch (Exception e) {
                        pendingImportDir = null;
                        android.util.Log.e("KimiOS", "import picker failed: " + e);
                    }
                }
            });
            return 0;
        } catch (Exception e) {
            android.util.Log.e("KimiOS", "importFile failed: " + e);
            return -1;
        }
    }

    /** MainActivity.onActivityResult hook: copy the picked Uri into the pending
     *  /disk dir on a background thread (userland refreshes the listing later). */
    public static void handleImportResult(int resultCode, Intent data) {
        final String dir = pendingImportDir;
        pendingImportDir = null;
        if (resultCode != Activity.RESULT_OK || data == null || data.getData() == null || dir == null)
            return;
        final Uri uri = data.getData();
        final Context ctx = appContext;
        if (ctx == null) return;
        new Thread(new Runnable() {
            @Override public void run() {
                try {
                    String name = null;
                    android.database.Cursor c =
                            ctx.getContentResolver().query(uri, null, null, null, null);
                    if (c != null) {
                        try {
                            int idx = c.getColumnIndex(android.provider.OpenableColumns.DISPLAY_NAME);
                            if (idx >= 0 && c.moveToFirst()) name = c.getString(idx);
                        } finally { c.close(); }
                    }
                    if (name == null || name.isEmpty()) name = "imported-" + System.currentTimeMillis();
                    name = name.replace('/', '_');
                    java.io.File dirF = new java.io.File(dir);
                    dirF.mkdirs();
                    java.io.File out = new java.io.File(dirF, name);
                    java.io.InputStream in = ctx.getContentResolver().openInputStream(uri);
                    if (in == null) return;
                    java.io.FileOutputStream os = new java.io.FileOutputStream(out);
                    byte[] buf = new byte[8192]; int n;
                    while ((n = in.read(buf)) > 0) os.write(buf, 0, n);
                    os.close(); in.close();
                    android.util.Log.i("KimiOS", "imported " + out.getAbsolutePath());
                } catch (Exception e) {
                    android.util.Log.e("KimiOS", "import copy failed: " + e);
                }
            }
        }, "kimi-import").start();
    }

    private KimiBridge() {}
}
