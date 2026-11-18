package com.kimios.libos;

import java.io.File;

/** Builds argv/envp and launches Alpine Linux guests natively via KimiBridge. */
public final class KimiGuest {
    private KimiGuest() {}

    public static String sysroot(File filesDir) {
        return new File(filesDir, "disk/alpine/root").getAbsolutePath();
    }

    /** which = "hello" | "python" | "node"; arg = code/script payload. */
    public static int run(File filesDir, String which, String arg) {
        String sr = sysroot(filesDir);
        String lib = sr + "/usr/lib:" + sr + "/lib";
        String bin = sr + "/usr/bin:" + sr + "/bin";
        new File(filesDir, "home").mkdirs();
        new File(filesDir, "tmp").mkdirs();
        String exe;
        String[] argv, envp;
        if ("python".equals(which)) {
            exe = sr + "/usr/bin/python3.14";
            argv = new String[] { exe, "-u", "-c", arg };
            envp = new String[] {
                "LD_LIBRARY_PATH=" + lib,
                "PYTHONHOME=" + sr + "/usr",
                "PYTHONPATH=" + sr + "/usr/lib/python3.14:" + sr + "/usr/lib/python3.14/lib-dynload",
                "PATH=" + bin,
                "HOME=" + filesDir + "/home",
                "TMPDIR=" + filesDir + "/tmp",
                "LANG=C.UTF-8",
                "PYTHONDONTWRITEBYTECODE=1",
            };
        } else if ("node".equals(which)) {
            exe = sr + "/usr/bin/node";
            argv = new String[] { exe, "-e", arg };
            envp = new String[] {
                "LD_LIBRARY_PATH=" + lib, "PATH=" + bin,
                "HOME=" + filesDir + "/home", "TMPDIR=" + filesDir + "/tmp", "LANG=C.UTF-8",
            };
        } else {
            exe = sr + "/hello_dyn";
            argv = new String[] { exe };
            envp = new String[] { "LD_LIBRARY_PATH=" + lib };
        }
        return KimiBridge.nativeRunElf(exe, sr, argv, envp);
    }
}
