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

    private KimiBridge() {}
}
