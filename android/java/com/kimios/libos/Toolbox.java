package com.kimios.libos;

import android.content.Context;
import java.io.File;
import java.io.FileOutputStream;
import java.io.InputStream;
import java.io.ByteArrayOutputStream;
import java.util.zip.GZIPInputStream;

/**
 * Prebundled TUI toolbox (dash + coreutils + libs), shipped as
 * assets/toolbox.tar.gz and extracted into <files>/disk/alpine/root on first
 * run. Real musl ELFs are used because busybox *applets* die under Android's
 * per-app seccomp (SIGSYS), while dash/coreutils run like any other guest.
 */
public final class Toolbox {
    public static void ensure(Context ctx) {
        File root = new File(ctx.getFilesDir(), "disk/alpine/root");
        File marker = new File(ctx.getFilesDir(), "disk/.toolbox.v1");
        if (marker.exists()) return;
        try {
            InputStream raw = ctx.getAssets().open("toolbox.tar.gz");
            GZIPInputStream gz = new GZIPInputStream(raw);
            ByteArrayOutputStream bos = new ByteArrayOutputStream();
            byte[] buf = new byte[65536];
            int r;
            while ((r = gz.read(buf)) > 0) bos.write(buf, 0, r);
            gz.close();
            root.mkdirs();
            extractTar(bos.toByteArray(), root);
            marker.getParentFile().mkdirs();
            if (!marker.createNewFile() && !marker.exists()) {
                android.util.Log.i("KimiOS", "toolbox marker create failed");
            }
            android.util.Log.i("KimiOS", "toolbox extracted");
        } catch (Throwable t) {
            android.util.Log.i("KimiOS", "toolbox extract failed: " + t);
        }
    }

    private static void extractTar(byte[] t, File root) throws Exception {
        int off = 0;
        while (off + 512 <= t.length) {
            boolean zero = true;
            for (int i = 0; i < 512; i++) { if (t[off + i] != 0) { zero = false; break; } }
            if (zero) break;
            String name = str(t, off, 100);
            String prefix = str(t, off + 345, 155);
            if (prefix.length() > 0) name = prefix + "/" + name;
            long size = octal(t, off + 124, 12);
            char type = (char) (t[off + 156] & 0xff);
            String link = str(t, off + 157, 100);
            long mode = octal(t, off + 100, 8);
            off += 512;
            name = clean(name);
            if (name.isEmpty() || name.startsWith("..") || name.indexOf("..") >= 0) {
                off += pad(size);
                continue;
            }
            File dest = new File(root, name);
            if (type == '5' || (size == 0 && name.endsWith("/"))) {
                dest.mkdirs();
            } else if (type == '2') {
                dest.getParentFile().mkdirs();
                try { android.system.Os.symlink(link, dest.getAbsolutePath()); } catch (Throwable e) {}
            } else if (type == '0' || type == 0 || type == '7') {
                dest.getParentFile().mkdirs();
                FileOutputStream fo = new FileOutputStream(dest);
                fo.write(t, off, (int) size);
                fo.close();
                if ((mode & 0111) != 0) {
                    try { android.system.Os.chmod(dest.getAbsolutePath(), 0755); } catch (Throwable e) {}
                }
            }
            off += pad(size);
        }
    }

    private static int pad(long size) { return (int) (((size + 511) / 512) * 512); }

    private static String clean(String n) {
        if (n.startsWith("./")) n = n.substring(2);
        return n.replace('\\', '/');
    }

    private static String str(byte[] b, int o, int len) {
        int end = o;
        while (end < o + len && b[end] != 0) end++;
        return new String(b, o, end - o);
    }

    private static long octal(byte[] b, int o, int len) {
        if ((b[o] & 0x80) != 0) { // GNU base-256
            long v = 0;
            for (int i = o + 1; i < o + len; i++) v = (v << 8) | (b[i] & 0xff);
            return v;
        }
        long v = 0;
        int i = o;
        while (i < o + len && b[i] == ' ') i++;
        for (; i < o + len; i++) {
            int c = b[i];
            if (c < '0' || c > '7') break;
            v = v * 8 + (c - '0');
        }
        return v;
    }
}