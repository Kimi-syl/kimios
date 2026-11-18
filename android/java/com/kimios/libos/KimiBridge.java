package com.kimios.libos;

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

    private KimiBridge() {}
}
