package com.kimios.libos;

import java.util.List;

/**
 * Pure (Android-free) UI-element snapshot model + formatter. Kept separate from
 * the AccessibilityService so the formatting logic can be unit-tested on a host
 * JVM without any Android runtime.
 */
public final class UiSnapshot {

    public static final class Elem {
        public final String text;
        public final String cls;
        public final int l, t, r, b;
        public final boolean clickable, editable, focused, scrollable;

        public Elem(String text, String cls, int l, int t, int r, int b,
                    boolean clickable, boolean editable, boolean focused, boolean scrollable) {
            this.text = text;
            this.cls = cls;
            this.l = l; this.t = t; this.r = r; this.b = b;
            this.clickable = clickable;
            this.editable = editable;
            this.focused = focused;
            this.scrollable = scrollable;
        }
    }

    /** Format a captured element list into the compact text the `ui` command prints. */
    public static String format(String pkg, List<Elem> els) {
        StringBuilder sb = new StringBuilder();
        sb.append("package ").append(pkg == null || pkg.isEmpty() ? "?" : pkg).append('\n');
        sb.append(els.size()).append(" elements\n");
        int i = 0;
        for (Elem e : els) {
            sb.append('[').append(i++).append("] ").append(shortClass(e.cls));
            String label = e.text == null ? "" : e.text.replace('\n', ' ').trim();
            if (label.length() > 80) label = label.substring(0, 80) + "...";
            if (!label.isEmpty()) sb.append(" \"").append(label).append('"');
            sb.append(" (").append(e.l).append(',').append(e.t).append(',')
              .append(e.r).append(',').append(e.b).append(')');
            if (e.clickable) sb.append(" click");
            if (e.editable) sb.append(" edit");
            if (e.focused) sb.append(" focus");
            if (e.scrollable) sb.append(" scroll");
            sb.append('\n');
        }
        return sb.toString();
    }

    static String shortClass(String c) {
        if (c == null || c.isEmpty()) return "?";
        int dot = c.lastIndexOf('.');
        return dot >= 0 && dot < c.length() - 1 ? c.substring(dot + 1) : c;
    }

    private UiSnapshot() {}
}