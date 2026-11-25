package com.kimios.libos;

import android.accessibilityservice.AccessibilityService;
import android.accessibilityservice.AccessibilityServiceInfo;
import android.graphics.Rect;
import android.view.accessibility.AccessibilityEvent;
import android.view.accessibility.AccessibilityNodeInfo;

import java.util.ArrayList;
import java.util.List;

/**
 * Instant UI-element reader. When enabled (Android Settings > Accessibility >
 * KimiOS), this service captures the active window's accessibility node tree on
 * every window/content change and keeps the latest compact snapshot. The KimiOS
 * userland reads it through SYS_UI_READ (the `ui` shell command), so the agent /
 * terminal can "see" the current screen's real interactive elements instantly.
 *
 * This is the Android equivalent of dsh-Go's phone-control screen reading; it
 * exposes the genuine accessibility element tree (not OCR).
 */
public final class KimiA11yService extends AccessibilityService {

    private static volatile String latest = "";
    private static volatile boolean connected = false;

    /** Latest snapshot, or a help string when the service is off / no window yet. */
    public static String snapshot() {
        if (!connected) {
            return "ui: KimiOS accessibility service is not enabled.\n"
                 + "Enable it in Android Settings > Accessibility > KimiOS.\n";
        }
        String s = latest;
        if (s == null || s.isEmpty()) {
            return "ui: no window captured yet. Open an app and run `ui` again.\n";
        }
        return s;
    }

    public static boolean isConnected() {
        return connected;
    }

    @Override
    public void onServiceConnected() {
        connected = true;
        AccessibilityServiceInfo info = getServiceInfo();
        if (info == null) info = new AccessibilityServiceInfo();
        info.eventTypes = AccessibilityEvent.TYPE_WINDOW_STATE_CHANGED
                        | AccessibilityEvent.TYPE_WINDOW_CONTENT_CHANGED;
        info.feedbackType = AccessibilityServiceInfo.FEEDBACK_GENERIC;
        info.flags |= AccessibilityServiceInfo.FLAG_REPORT_VIEW_IDS
                    | AccessibilityServiceInfo.FLAG_RETRIEVE_INTERACTIVE_WINDOWS;
        info.notificationTimeout = 100;
        setServiceInfo(info);
    }

    @Override
    public void onAccessibilityEvent(AccessibilityEvent event) {
        if (event == null) return;
        int t = event.getEventType();
        if (t != AccessibilityEvent.TYPE_WINDOW_STATE_CHANGED
                && t != AccessibilityEvent.TYPE_WINDOW_CONTENT_CHANGED) {
            return;
        }
        AccessibilityNodeInfo root = getRootInActiveWindow();
        if (root == null) return;
        List<UiSnapshot.Elem> els = new ArrayList<>();
        traverse(root, els, 0);
        CharSequence pn = root.getPackageName();
        latest = UiSnapshot.format(pn == null ? "?" : pn.toString(), els);
        root.recycle();
    }

    private void traverse(AccessibilityNodeInfo n, List<UiSnapshot.Elem> out, int depth) {
        if (n == null || out.size() >= 400 || depth > 40) return;
        Rect r = new Rect();
        n.getBoundsInScreen(r);
        CharSequence txt = n.getText();
        String text = txt != null ? txt.toString()
                : (n.getContentDescription() != null ? n.getContentDescription().toString() : "");
        CharSequence cn = n.getClassName();
        String cls = cn != null ? cn.toString() : "";
        if (!text.isEmpty() || n.isClickable() || n.isEditable()) {
            out.add(new UiSnapshot.Elem(text, cls, r.left, r.top, r.right, r.bottom,
                    n.isClickable(), n.isEditable(), n.isFocused(), n.isScrollable()));
        }
        int count = n.getChildCount();
        for (int i = 0; i < count; i++) {
            AccessibilityNodeInfo c = n.getChild(i);
            if (c != null) {
                traverse(c, out, depth + 1);
                c.recycle();
            }
        }
    }

    @Override
    public void onInterrupt() {}

    @Override
    public void onDestroy() {
        connected = false;
        super.onDestroy();
    }
}