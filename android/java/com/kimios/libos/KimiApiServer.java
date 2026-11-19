package com.kimios.libos;

import java.io.ByteArrayOutputStream;
import java.io.File;
import java.io.FileInputStream;
import java.io.FileOutputStream;
import java.io.IOException;
import java.io.InputStream;
import java.io.OutputStream;
import java.net.InetAddress;
import java.net.InetSocketAddress;
import java.net.ServerSocket;
import java.net.Socket;
import java.net.URLDecoder;
import java.nio.charset.StandardCharsets;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.HashMap;
import java.util.List;
import java.util.Map;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.concurrent.ThreadFactory;

import org.json.JSONArray;
import org.json.JSONObject;

/**
 * Localhost Plugin API server (SYS_PLUGIN_API=53). Tiny HTTP/1.1 server bound
 * to 127.0.0.1 on an ephemeral port; one daemon accept thread + a cached pool
 * of daemon workers, so neither the GUI pthread nor the main looper is ever
 * blocked by plugin traffic. Endpoints (JSON unless noted):
 *   GET  /api/info                  -> {"os":"KimiOS","version":"1.0","api":1}
 *   GET  /api/fs/list?path=/disk/.. -> [{"name","dir","size"}]
 *   GET  /api/fs/read?path=/disk/.. -> file bytes (text/plain)
 *   POST /api/fs/write?path=/disk/.. body=bytes -> {"ok","size"}
 *   POST /api/notify {"title","body"} -> stored for the GUI shade + logcat
 *   GET/POST /api/settings?plugin=<id> -> per-plugin settings.json key/values
 * All fs paths are sandboxed under diskRoot; ".." is rejected.
 */
public final class KimiApiServer {
    private static final String TAG = "KimiOS-plugin";
    private static final int MAX_BODY = 4 * 1024 * 1024;
    private static final int MAX_READ = 1024 * 1024;
    private static final int MAX_MANIFEST = 64 * 1024;
    private static final int MAX_NOTIFY = 64;

    private static final Object LOCK = new Object();
    private static ServerSocket server;
    private static volatile int port = -1;
    private static final List<String[]> notifications = new ArrayList<String[]>();

    private KimiApiServer() {}

    /** Lazy start; returns the bound port (cached), or -1 on failure. */
    static long start() {
        synchronized (LOCK) {
            if (server != null && !server.isClosed()) return port;
            if (KimiBridge.getDiskRoot() == null) return -1;
            try {
                ServerSocket ss = new ServerSocket();
                ss.setReuseAddress(true);
                ss.bind(new InetSocketAddress(InetAddress.getByName("127.0.0.1"), 0), 8);
                server = ss;
                port = ss.getLocalPort();
                ExecutorService pool = Executors.newCachedThreadPool(new ThreadFactory() {
                    @Override public Thread newThread(Runnable r) {
                        Thread t = new Thread(r, "kimi-api-worker");
                        t.setDaemon(true);
                        return t;
                    }
                });
                Thread accept = new Thread(new Runnable() {
                    @Override public void run() {
                        for (;;) {
                            try {
                                final Socket s = server.accept();
                                pool.execute(new Runnable() {
                                    @Override public void run() { handle(s); }
                                });
                            } catch (Exception e) {
                                return; // server socket closed
                            }
                        }
                    }
                }, "kimi-api-accept");
                accept.setDaemon(true);
                accept.start();
                android.util.Log.i(TAG, "plugin API listening on 127.0.0.1:" + port);
                return port;
            } catch (Exception e) {
                android.util.Log.e(TAG, "API server start failed: " + e);
                server = null;
                port = -1;
                return -1;
            }
        }
    }

    // ---- notifications (readable by the GUI via SYS_PLUGIN_API a0=1) ----

    private static void addNotification(String title, String body) {
        synchronized (notifications) {
            if (notifications.size() >= MAX_NOTIFY) notifications.remove(0);
            notifications.add(new String[] {title, body});
        }
        android.util.Log.i(TAG, "notify: " + title + ": " + body);
    }

    /** Drain pending notifications as flat NUL-terminated UTF-8 strings:
     *  title\0body\0title\0body\0... (empty array when none). */
    static byte[] drainNotifications() {
        ByteArrayOutputStream bos = new ByteArrayOutputStream();
        synchronized (notifications) {
            for (String[] n : notifications) {
                writeStr(bos, n[0]);
                writeStr(bos, n[1]);
            }
            notifications.clear();
        }
        return bos.toByteArray();
    }

    private static void writeStr(ByteArrayOutputStream bos, String s) {
        byte[] b = s.getBytes(StandardCharsets.UTF_8);
        bos.write(b, 0, b.length);
        bos.write(0);
    }

    // ---- HTTP plumbing ----

    private static void handle(Socket s) {
        try {
            s.setSoTimeout(30000);
            InputStream in = s.getInputStream();
            String line = readLine(in);
            if (line == null) return;
            String[] parts = line.split(" ");
            if (parts.length < 2) { sendJson(s, 400, err("bad request")); return; }
            String method = parts[0];
            String target = parts[1];
            int contentLength = 0;
            String hdr;
            while ((hdr = readLine(in)) != null && !hdr.isEmpty()) {
                int colon = hdr.indexOf(':');
                if (colon > 0 && hdr.substring(0, colon).trim().equalsIgnoreCase("Content-Length")) {
                    try { contentLength = Integer.parseInt(hdr.substring(colon + 1).trim()); }
                    catch (NumberFormatException ignored) {}
                }
            }
            if (contentLength < 0 || contentLength > MAX_BODY) {
                sendJson(s, 413, err("body too large"));
                return;
            }
            byte[] body = new byte[contentLength];
            int got = 0;
            while (got < contentLength) {
                int n = in.read(body, got, contentLength - got);
                if (n < 0) break;
                got += n;
            }
            route(s, method, target, body, got);
        } catch (Exception e) {
            try { sendJson(s, 500, err("internal: " + e)); } catch (Exception ignored) {}
        } finally {
            try { s.close(); } catch (Exception ignored) {}
        }
    }

    private static void route(Socket s, String method, String target, byte[] body, int len)
            throws IOException {
        String path = target, query = "";
        int q = target.indexOf('?');
        if (q >= 0) { path = target.substring(0, q); query = target.substring(q + 1); }
        Map<String, String> params = parseQuery(query);
        try {
            if (path.equals("/api/info")) {
                if (!method.equals("GET")) { sendJson(s, 405, err("GET only")); return; }
                JSONObject j = new JSONObject();
                j.put("os", "KimiOS");
                j.put("version", "1.0");
                j.put("api", 1);
                sendJson(s, 200, j.toString());
            } else if (path.equals("/api/fs/list")) {
                if (!method.equals("GET")) { sendJson(s, 405, err("GET only")); return; }
                File dir = resolve(params.get("path"));
                if (!dir.isDirectory()) { sendJson(s, 404, err("not a directory")); return; }
                File[] entries = dir.listFiles();
                JSONArray arr = new JSONArray();
                if (entries != null) {
                    Arrays.sort(entries);
                    for (File f : entries) {
                        JSONObject o = new JSONObject();
                        o.put("name", f.getName());
                        o.put("dir", f.isDirectory());
                        o.put("size", f.length());
                        arr.put(o);
                    }
                }
                sendJson(s, 200, arr.toString());
            } else if (path.equals("/api/fs/read")) {
                if (!method.equals("GET")) { sendJson(s, 405, err("GET only")); return; }
                File f = resolve(params.get("path"));
                if (!f.isFile()) { sendJson(s, 404, err("not a file")); return; }
                if (f.length() > MAX_READ) { sendJson(s, 413, err("file too large")); return; }
                byte[] data = readFile(f, MAX_READ);
                send(s, 200, "text/plain; charset=utf-8", data);
            } else if (path.equals("/api/fs/write")) {
                if (!method.equals("POST")) { sendJson(s, 405, err("POST only")); return; }
                File f = resolve(params.get("path"));
                if (f.isDirectory()) { sendJson(s, 400, err("is a directory")); return; }
                File parent = f.getParentFile();
                if (parent != null) parent.mkdirs();
                FileOutputStream out = new FileOutputStream(f);
                try { out.write(body, 0, len); } finally { out.close(); }
                JSONObject j = new JSONObject();
                j.put("ok", true);
                j.put("size", len);
                sendJson(s, 200, j.toString());
            } else if (path.equals("/api/notify")) {
                if (!method.equals("POST")) { sendJson(s, 405, err("POST only")); return; }
                JSONObject j = parseBody(body, len);
                String title = j.optString("title", "");
                String text = j.optString("body", "");
                if (title.isEmpty() && text.isEmpty()) {
                    sendJson(s, 400, err("title/body required"));
                    return;
                }
                addNotification(title, text);
                sendJson(s, 200, "{\"ok\":true}");
            } else if (path.equals("/api/settings")) {
                if (!method.equals("GET") && !method.equals("POST")) {
                    sendJson(s, 405, err("GET/POST only"));
                    return;
                }
                String id = params.get("plugin");
                if (id == null || id.isEmpty() || id.contains("..")
                        || id.indexOf('/') >= 0 || id.indexOf('\\') >= 0) {
                    sendJson(s, 400, err("plugin id required"));
                    return;
                }
                File dir = resolve("/disk/plugins/" + id);
                File sf = new File(dir, "settings.json");
                JSONObject cur = readSettings(sf);
                if (method.equals("POST")) {
                    JSONObject add = parseBody(body, len);
                    if (add.length() == 2 && add.has("key") && add.has("value")
                            && add.get("key") instanceof String) {
                        // SDK single-pair envelope: {"key":k,"value":v}
                        cur.put(add.getString("key"), add.get("value"));
                    } else {
                        java.util.Iterator<String> it = add.keys();
                        while (it.hasNext()) {
                            String k = it.next();
                            cur.put(k, add.get(k));
                        }
                    }
                    dir.mkdirs();
                    FileOutputStream out = new FileOutputStream(sf);
                    try { out.write(cur.toString().getBytes(StandardCharsets.UTF_8)); }
                    finally { out.close(); }
                }
                sendJson(s, 200, cur.toString());
            } else {
                sendJson(s, 404, err("not found"));
            }
        } catch (SecurityException se) {
            sendJson(s, 403, err(se.getMessage()));
        } catch (IllegalArgumentException ie) {
            sendJson(s, 400, err(ie.getMessage()));
        } catch (org.json.JSONException je) {
            sendJson(s, 400, err("bad json: " + je.getMessage()));
        } catch (Exception e) {
            sendJson(s, 500, err("internal: " + e));
        }
    }

    // ---- sandbox ----

    /** Resolve a /disk/... path (or an absolute path under diskRoot, e.g.
     *  KIMI_PLUGIN_DIR) under diskRoot; reject traversal + escapes. */
    private static File resolve(String path) {
        String root = KimiBridge.getDiskRoot();
        if (root == null) throw new IllegalStateException("no disk root");
        if (path == null) throw new IllegalArgumentException("path required");
        String rel;
        if (path.equals("/disk") || path.equals(root)) rel = "";
        else if (path.startsWith("/disk/")) rel = path.substring(6);
        else if (path.startsWith(root + "/")) rel = path.substring(root.length() + 1);
        else throw new IllegalArgumentException("path must be /disk/... or under the sandbox");
        if (rel.contains("..")) throw new SecurityException("path traversal rejected");
        File rootF = new File(root);
        File f = rel.isEmpty() ? rootF : new File(rootF, rel);
        try {
            String canon = f.getCanonicalPath();
            String rootCanon = rootF.getCanonicalPath();
            if (!canon.equals(rootCanon) && !canon.startsWith(rootCanon + File.separator))
                throw new SecurityException("outside sandbox");
        } catch (IOException e) {
            throw new SecurityException("path resolution failed");
        }
        return f;
    }

    // ---- helpers ----

    private static JSONObject parseBody(byte[] body, int len) throws org.json.JSONException {
        return new JSONObject(new String(body, 0, len, StandardCharsets.UTF_8));
    }

    private static JSONObject readSettings(File f) {
        try {
            if (f.isFile() && f.length() <= MAX_MANIFEST)
                return new JSONObject(new String(readFile(f, MAX_MANIFEST), StandardCharsets.UTF_8));
        } catch (Exception ignored) {}
        return new JSONObject();
    }

    private static byte[] readFile(File f, int cap) throws IOException {
        FileInputStream in = new FileInputStream(f);
        try {
            ByteArrayOutputStream bos = new ByteArrayOutputStream();
            byte[] buf = new byte[8192];
            int n, total = 0;
            while ((n = in.read(buf)) > 0 && total < cap) {
                if (total + n > cap) n = cap - total;
                bos.write(buf, 0, n);
                total += n;
            }
            return bos.toByteArray();
        } finally {
            in.close();
        }
    }

    private static Map<String, String> parseQuery(String q) {
        Map<String, String> m = new HashMap<String, String>();
        if (q == null || q.isEmpty()) return m;
        for (String kv : q.split("&")) {
            int eq = kv.indexOf('=');
            try {
                if (eq > 0) m.put(URLDecoder.decode(kv.substring(0, eq), "UTF-8"),
                        URLDecoder.decode(kv.substring(eq + 1), "UTF-8"));
                else if (!kv.isEmpty()) m.put(URLDecoder.decode(kv, "UTF-8"), "");
            } catch (Exception ignored) {}
        }
        return m;
    }

    private static String readLine(InputStream in) throws IOException {
        ByteArrayOutputStream bos = new ByteArrayOutputStream();
        int c;
        while ((c = in.read()) != -1) {
            if (c == '\n') break;
            if (c != '\r') bos.write(c);
            if (bos.size() > 8192) throw new IOException("header too long");
        }
        if (c == -1 && bos.size() == 0) return null;
        return bos.toString("UTF-8");
    }

    private static String reason(int status) {
        switch (status) {
            case 200: return "OK";
            case 400: return "Bad Request";
            case 403: return "Forbidden";
            case 404: return "Not Found";
            case 405: return "Method Not Allowed";
            case 413: return "Payload Too Large";
            default: return "Internal Server Error";
        }
    }

    private static void send(Socket s, int status, String type, byte[] body) throws IOException {
        OutputStream out = s.getOutputStream();
        String head = "HTTP/1.1 " + status + " " + reason(status) + "\r\n"
                + "Content-Type: " + type + "\r\n"
                + "Content-Length: " + body.length + "\r\n"
                + "Connection: close\r\n\r\n";
        out.write(head.getBytes(StandardCharsets.UTF_8));
        out.write(body);
        out.flush();
    }

    private static void sendJson(Socket s, int status, String json) throws IOException {
        send(s, status, "application/json; charset=utf-8", json.getBytes(StandardCharsets.UTF_8));
    }

    private static String err(String msg) {
        try { return new JSONObject().put("error", msg == null ? "error" : msg).toString(); }
        catch (Exception e) { return "{\"error\":\"internal\"}"; }
    }
}