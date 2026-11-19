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

    /** Test hook: exercise sysinfo/proclist/copy syscalls (logs real values). */
    public static native int nativeSysTest();

    /** Test hook: open an app by AppKind discriminant (0=Music..14=Monitor). */
    public static native int nativeOpenApp(int kind);

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

    private static final long FIX_FRESH_MS = 30 * 60 * 1000;   // last-known freshness
    private static final long CACHE_REUSE_MS = 5 * 60 * 1000;  // skip slow path if cache fresh
    private static final long IP_RETRY_MS = 60 * 1000;         // backoff after IP-geo failure
    private static volatile long[] cachedFix;
    private static volatile long cachedFixAt;
    private static volatile long ipFailAt;

    private static long[] fixFromLocation(Location l) {
        long[] r = new long[] {
                Double.doubleToRawLongBits(l.getLatitude()),
                Double.doubleToRawLongBits(l.getLongitude()),
                (long) l.getAccuracy(), 1};
        cachedFix = r;
        cachedFixAt = System.currentTimeMillis();
        return r.clone();
    }

    /** HTTPS GET + JSON parse of an IP-geolocation endpoint. Returns
     *  [lat_bits, lon_bits, 0 (IP accuracy), 1] or null. No permission needed. */
    private static long[] ipGeoFix(String endpoint, int timeoutMs) {
        java.net.HttpURLConnection c = null;
        try {
            c = (java.net.HttpURLConnection) new java.net.URL(endpoint).openConnection();
            c.setInstanceFollowRedirects(true);
            c.setConnectTimeout(timeoutMs);
            c.setReadTimeout(timeoutMs);
            c.setRequestProperty("User-Agent", "KimiOS-libos");
            if (c.getResponseCode() != 200) return null;
            java.io.InputStream in = c.getInputStream();
            java.io.ByteArrayOutputStream bos = new java.io.ByteArrayOutputStream();
            byte[] buf = new byte[4096];
            int n; long total = 0;
            while ((n = in.read(buf)) > 0 && total < 65536) { bos.write(buf, 0, n); total += n; }
            in.close();
            org.json.JSONObject j = new org.json.JSONObject(bos.toString("UTF-8"));
            if (!j.optBoolean("success", true)) return null;   // ipwho.is failure payload
            if (j.optBoolean("error", false)) return null;     // ipapi.co failure payload
            if (!j.has("latitude") || !j.has("longitude")) return null;
            double lat = j.getDouble("latitude"), lon = j.getDouble("longitude");
            if (lat < -90 || lat > 90 || lon < -180 || lon > 180) return null;
            long[] r = new long[] {
                    Double.doubleToRawLongBits(lat), Double.doubleToRawLongBits(lon), 0, 1};
            cachedFix = r;
            cachedFixAt = System.currentTimeMillis();
            android.util.Log.i("KimiOS", "location via IP geo (" + endpoint + ")");
            return r.clone();
        } catch (Exception e) {
            android.util.Log.e("KimiOS", "ip geo " + endpoint + " failed: " + e);
            return null;
        } finally {
            if (c != null) c.disconnect();
        }
    }

    /** Called from C (SYS_LOCATION): layered fix as
     *  [lat_f64_bits, lon_f64_bits, accuracy_m, has_fix]. No fix -> has_fix=0.
     *  Order: fresh last-known (gps/network/passive) -> cached fix (5 min) ->
     *  active requestLocationUpdates (8 s timeout) -> ipwho.is / ipapi.co IP
     *  geolocation (no permission, works with zero GPS) -> stale last-known /
     *  any cached fix. May block the calling (GUI) thread up to ~10 s for a
     *  fix; never blocks forever. */
    public static long[] nativeLocation() {
        final long[] none = new long[] {0, 0, 0, 0};
        Context ctx = appContext;
        if (ctx == null) return cachedFix != null ? cachedFix.clone() : none;
        Location best = null;
        try {
            boolean permitted =
                    ctx.checkSelfPermission(android.Manifest.permission.ACCESS_FINE_LOCATION)
                            == PackageManager.PERMISSION_GRANTED
                    || ctx.checkSelfPermission(android.Manifest.permission.ACCESS_COARSE_LOCATION)
                            == PackageManager.PERMISSION_GRANTED;
            LocationManager lm = permitted
                    ? (LocationManager) ctx.getSystemService(Context.LOCATION_SERVICE) : null;
            if (lm != null) {
                String[] providers = {LocationManager.GPS_PROVIDER, LocationManager.NETWORK_PROVIDER,
                        LocationManager.PASSIVE_PROVIDER};
                for (String p : providers) {
                    try {
                        Location l = lm.getLastKnownLocation(p);
                        if (l != null && (best == null || l.getTime() > best.getTime())) best = l;
                    } catch (Exception ignored) {}
                }
                if (best != null && System.currentTimeMillis() - best.getTime() <= FIX_FRESH_MS)
                    return fixFromLocation(best);
            }
            long[] cache = cachedFix;
            if (cache != null && System.currentTimeMillis() - cachedFixAt <= CACHE_REUSE_MS)
                return cache.clone();
            if (lm != null) {
                final java.util.concurrent.CountDownLatch latch =
                        new java.util.concurrent.CountDownLatch(1);
                final java.util.concurrent.atomic.AtomicReference<Location> got =
                        new java.util.concurrent.atomic.AtomicReference<Location>();
                android.location.LocationListener listener = new android.location.LocationListener() {
                    @Override public void onLocationChanged(Location loc) {
                        if (loc != null && got.compareAndSet(null, loc)) latch.countDown();
                    }
                    @Override public void onStatusChanged(String p, int s, android.os.Bundle e) {}
                    @Override public void onProviderEnabled(String p) {}
                    @Override public void onProviderDisabled(String p) {}
                };
                boolean registered = false;
                for (String p : new String[]{LocationManager.GPS_PROVIDER,
                        LocationManager.NETWORK_PROVIDER}) {
                    try {
                        if (lm.isProviderEnabled(p)) {
                            lm.requestLocationUpdates(p, 0L, 0f, listener, android.os.Looper.getMainLooper());
                            registered = true;
                        }
                    } catch (Exception ignored) {}
                }
                if (registered) {
                    try {
                        latch.await(8, java.util.concurrent.TimeUnit.SECONDS);
                    } catch (InterruptedException ie) {
                        Thread.currentThread().interrupt();
                    } finally {
                        try { lm.removeUpdates(listener); } catch (Exception ignored) {}
                    }
                }
                Location active = got.get();
                if (active != null) return fixFromLocation(active);
            }
        } catch (Exception e) {
            android.util.Log.e("KimiOS", "location failed: " + e);
        }
        if (System.currentTimeMillis() - ipFailAt > IP_RETRY_MS) {
            long[] ip = ipGeoFix("https://ipwho.is/", 5000);
            if (ip == null) ip = ipGeoFix("https://ipapi.co/json/", 4000);
            if (ip != null) return ip;
            ipFailAt = System.currentTimeMillis();
        }
        if (best != null) return fixFromLocation(best);
        return cachedFix != null ? cachedFix.clone() : none;
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

    // ---- SYS_WEB=48: embedded offscreen browser (KimiWeb). Called from C on
    // the GUI pthread; every WebView operation hops to the main looper. ----

    /** Sub-op 0: create/resize the offscreen WebView (device px). 0 ok. */
    public static long nativeWebInit(int w, int h) {
        Context ctx = activity != null ? activity : appContext;
        return KimiWeb.init(ctx, w, h);
    }

    /** Sub-op 1: direct ByteBuffer holding RGBA pixels (C: GetDirectBufferAddress). */
    public static java.nio.ByteBuffer nativeWebBuf() { return KimiWeb.buf(); }

    /** Sub-op 2: pull-render; returns seqlock sequence (even = stable frame). */
    public static long nativeWebRender(boolean force) { return KimiWeb.render(force); }

    /** Sub-op 3: touch; action 0=down, 1=up, 2=move, 3=cancel. 0 ok. */
    public static long nativeWebTouch(int x, int y, int action) { return KimiWeb.touch(x, y, action); }

    /** Sub-op 4: navigate. 0 ok. */
    public static long nativeWebLoad(String url) { return KimiWeb.load(url); }

    /** Sub-op 5. 0 ok. */
    public static long nativeWebBack() { return KimiWeb.back(); }

    /** Sub-op 6. 0 ok. */
    public static long nativeWebForward() { return KimiWeb.forward(); }

    /** Sub-op 7. 0 ok. */
    public static long nativeWebReload() { return KimiWeb.reload(); }

    /** Sub-op 8: cached current URL (never null). */
    public static String nativeWebUrl() { return KimiWeb.url(); }

    /** Sub-op 9: cached page title (never null). */
    public static String nativeWebTitle() { return KimiWeb.title(); }

    /** Sub-op 10: 0/1 from volatile cache. */
    public static long nativeWebCanGoBack() { return KimiWeb.canGoBack(); }

    /** Sub-op 11: 0/1 from volatile cache. */
    public static long nativeWebCanGoForward() { return KimiWeb.canGoForward(); }

    /** Sub-op 12: resize viewport; reallocates bitmap + pixel buffer. 0 ok. */
    public static long nativeWebViewport(int w, int h) { return KimiWeb.viewport(w, h); }

    /** Sub-op 13: pause JS timers etc. 0 ok. */
    public static long nativeWebPause() { return KimiWeb.pause(); }

    /** Sub-op 14. 0 ok. */
    public static long nativeWebResume() { return KimiWeb.resume(); }

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

    // ---- SYS_SYSINFO=49 / SYS_PROCLIST=50: real host stats from /proc +
    // Android APIs. Called from C on the GUI pthread via cached method IDs. ----

    private static volatile String diskRoot;
    /** Set from C nativeInit: absolute sandbox disk root (<filesDir>/disk). */
    public static void setDiskRoot(String root) { diskRoot = root; }

    private static String firstLine(String path) {
        try (java.io.BufferedReader r = new java.io.BufferedReader(new java.io.FileReader(path))) {
            return r.readLine();
        } catch (Exception e) {
            return null;
        }
    }

    private static long parseKb(String line) {
        String[] t = line.trim().split("\\s+");
        try { return t.length >= 2 ? Long.parseLong(t[1]) : 0; }
        catch (NumberFormatException e) { return 0; }
    }

    private static long cpuPrevTotal = -1, cpuPrevIdle;

    /** [total_jiffies, idle_jiffies] from /proc/stat's first line, or null. */
    private static long[] cpuSample() {
        String line = firstLine("/proc/stat");
        if (line == null || !line.startsWith("cpu ")) return null;
        String[] t = line.trim().split("\\s+");
        if (t.length < 8) return null;
        try {
            long total = 0;
            for (int i = 1; i <= 7; i++) total += Long.parseLong(t[i]); // user..softirq
            long idle = Long.parseLong(t[4]) + Long.parseLong(t[5]);    // idle + iowait
            return new long[] {total, idle};
        } catch (NumberFormatException e) {
            return null;
        }
    }

    /** CPU usage % x10. Uses the delta against the previous call's cached
     *  /proc/stat sample; the very first call samples twice ~100 ms apart.
     *  Falls back to /proc/loadavg normalized by core count. */
    private static synchronized long cpuPctX10() {
        long[] s = cpuSample();
        if (s == null) {
            String la = firstLine("/proc/loadavg");
            if (la != null) {
                try {
                    double load = Double.parseDouble(la.split("\\s+")[0]);
                    int cores = Math.max(1, Runtime.getRuntime().availableProcessors());
                    long v = (long) (load * 1000 / cores);
                    return v < 0 ? 0 : (v > 1000 ? 1000 : v);
                } catch (Exception ignored) {}
            }
            return 0;
        }
        long total = s[0], idle = s[1], pct;
        if (cpuPrevTotal >= 0) {
            long dt = total - cpuPrevTotal, di = idle - cpuPrevIdle;
            pct = dt > 0 ? (dt - di) * 1000 / dt : 0;
        } else {
            try { Thread.sleep(100); } catch (InterruptedException ie) { Thread.currentThread().interrupt(); }
            long[] s2 = cpuSample();
            if (s2 == null) return 0;
            long dt = s2[0] - total, di = s2[1] - idle;
            pct = dt > 0 ? (dt - di) * 1000 / dt : 0;
            total = s2[0]; idle = s2[1];
        }
        cpuPrevTotal = total; cpuPrevIdle = idle;
        return pct < 0 ? 0 : (pct > 1000 ? 1000 : pct);
    }

    private static boolean allDigits(String s) {
        if (s.isEmpty()) return false;
        for (int i = 0; i < s.length(); i++)
            if (s.charAt(i) < '0' || s.charAt(i) > '9') return false;
        return true;
    }

    private static long procCount() {
        String[] pids = new java.io.File("/proc").list();
        long n = 0;
        if (pids != null) for (String p : pids) if (allDigits(p)) n++;
        if (n == 0) {
            // hidepid may hide other uids' processes; count our own threads.
            String[] tasks = new java.io.File("/proc/self/task").list();
            n = tasks != null ? tasks.length : 1;
        }
        return n;
    }

    /** Called from C (SYS_SYSINFO): real system stats as
     *  [cpu_pct_x10, ram_total_kb, ram_used_kb, storage_total_kb, storage_used_kb,
     *   uptime_s, proc_count, battery_pct, battery_charging, reserved]. */
    public static long[] nativeSysInfo() {
        long[] r = new long[10];
        r[0] = cpuPctX10();
        long memTotal = 0, memAvail = -1;
        try (java.io.BufferedReader br = new java.io.BufferedReader(new java.io.FileReader("/proc/meminfo"))) {
            String line;
            while ((line = br.readLine()) != null) {
                if (line.startsWith("MemTotal:")) memTotal = parseKb(line);
                else if (line.startsWith("MemAvailable:")) memAvail = parseKb(line);
            }
        } catch (Exception ignored) {}
        r[1] = memTotal;
        r[2] = memAvail >= 0 && memTotal > memAvail ? memTotal - memAvail : 0;
        String root = diskRoot;
        if (root == null) {
            Context ctx = appContext;
            root = ctx != null ? ctx.getFilesDir().getAbsolutePath() + "/disk" : "/data";
        }
        try {
            android.os.StatFs sf = new android.os.StatFs(root);
            long total = sf.getTotalBytes() / 1024;
            long avail = sf.getAvailableBytes() / 1024;
            r[3] = total;
            r[4] = total > avail ? total - avail : 0;
        } catch (Exception ignored) {}
        r[5] = android.os.SystemClock.elapsedRealtime() / 1000;
        r[6] = procCount();
        Context ctx = appContext;
        if (ctx != null) {
            try {
                Intent bi = ctx.registerReceiver(null,
                        new android.content.IntentFilter(Intent.ACTION_BATTERY_CHANGED));
                if (bi != null) {
                    int level = bi.getIntExtra(android.os.BatteryManager.EXTRA_LEVEL, -1);
                    int scale = bi.getIntExtra(android.os.BatteryManager.EXTRA_SCALE, 100);
                    int status = bi.getIntExtra(android.os.BatteryManager.EXTRA_STATUS, -1);
                    if (level >= 0 && scale > 0) r[7] = level * 100L / scale;
                    r[8] = (status == android.os.BatteryManager.BATTERY_STATUS_CHARGING
                            || status == android.os.BatteryManager.BATTERY_STATUS_FULL) ? 1 : 0;
                }
            } catch (Exception ignored) {}
        }
        return r;
    }

    // pid -> {utime+stime jiffies, elapsedRealtime ms} from the previous call.
    private static java.util.HashMap<Integer, long[]> procPrev = new java.util.HashMap<>();

    /** Called from C (SYS_PROCLIST): flat records of 8 longs each:
     *  [pid, ppid, ram_kb, cpu_x10, state, name[0..8], name[8..16], name[16..24]]
     *  (name = 24 bytes NUL-padded, little-endian in the last 3 u64;
     *   state 0=run, 1=sleep, 2=zombie). Sorted by ram desc, capped at max. */
    public static synchronized long[] nativeProcList(int max) {
        if (max <= 0) return new long[0];
        if (max > 256) max = 256;
        java.io.File[] dirs = new java.io.File("/proc").listFiles();
        if (dirs == null) return new long[0];
        long now = android.os.SystemClock.elapsedRealtime();
        java.util.ArrayList<long[]> recs = new java.util.ArrayList<>();
        java.util.HashMap<Integer, long[]> cur = new java.util.HashMap<>();
        for (java.io.File d : dirs) {
            String nm = d.getName();
            if (!allDigits(nm)) continue;
            int pid;
            try { pid = Integer.parseInt(nm); } catch (NumberFormatException e) { continue; }
            String stat = firstLine("/proc/" + nm + "/stat");
            if (stat == null) continue;
            int lo = stat.indexOf('('), hi = stat.lastIndexOf(')');
            if (lo < 0 || hi <= lo) continue;
            String comm = stat.substring(lo + 1, hi);
            String[] t = stat.substring(hi + 1).trim().split("\\s+");
            if (t.length < 13) continue;
            long ppid, ticks;
            try {
                ppid = Long.parseLong(t[1]);
                ticks = Long.parseLong(t[11]) + Long.parseLong(t[12]); // utime + stime
            } catch (NumberFormatException e) { continue; }
            char sc = t[0].charAt(0);
            long state = sc == 'R' ? 0 : (sc == 'Z' ? 2 : 1);
            long ramKb = 0;
            try (java.io.BufferedReader br = new java.io.BufferedReader(
                    new java.io.FileReader("/proc/" + nm + "/status"))) {
                String line;
                while ((line = br.readLine()) != null)
                    if (line.startsWith("VmRSS:")) { ramKb = parseKb(line); break; }
            } catch (Exception ignored) {}
            long cpuX10 = 0;
            long[] prev = procPrev.get(pid);
            if (prev != null && now > prev[1] && ticks >= prev[0]) {
                cpuX10 = (ticks - prev[0]) * 100 / (now - prev[1]);
                if (cpuX10 > 1000) cpuX10 = 1000;
            }
            cur.put(pid, new long[] {ticks, now});
            byte[] nb = new byte[24];
            byte[] cb = comm.getBytes(java.nio.charset.StandardCharsets.UTF_8);
            System.arraycopy(cb, 0, nb, 0, Math.min(cb.length, 24));
            long w0 = 0, w1 = 0, w2 = 0;
            for (int i = 0; i < 8; i++) {
                w0 |= (nb[i] & 0xffL) << (8 * i);
                w1 |= (nb[i + 8] & 0xffL) << (8 * i);
                w2 |= (nb[i + 16] & 0xffL) << (8 * i);
            }
            recs.add(new long[] {pid, ppid, ramKb, cpuX10, state, w0, w1, w2});
        }
        procPrev = cur;
        java.util.Collections.sort(recs, new java.util.Comparator<long[]>() {
            @Override public int compare(long[] a, long[] b) { return Long.compare(b[2], a[2]); }
        });
        int n = Math.min(max, recs.size());
        long[] out = new long[n * 8];
        for (int i = 0; i < n; i++) System.arraycopy(recs.get(i), 0, out, i * 8, 8);
        return out;
    }

    private KimiBridge() {}
}
