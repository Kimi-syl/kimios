#!/usr/bin/env python3
"""Scaffold a new KimiOS plugin directory (plugin.json + entry template).

Usage:
  python3 tools/make_plugin.py ID NAME [--type app|service|ui]
                               [--runtime native|python|node]
                               [--version 1.0] [--description TEXT]
                               [--permissions fs,notify,...]
                               [-o DIR] [--force]

Defaults: --type app, --runtime python (ui forces native), output ./<id>.
Manifest limits mirror user/libkimi/src/kplugin.rs: id/name <= 48 bytes,
version <= 16 bytes, id must not contain '/' or '..', manifest < 2 KB.
Generated Python entries are self-contained (stdlib only); copy
tools/plugin_sdk/kimi.py next to the entry for the full SDK.

Examples:
  python3 tools/make_plugin.py org.kimios.demo "Demo" -o market/plugins/demo
  python3 tools/make_plugin.py heartbeat "Heartbeat" --type service
  python3 tools/make_plugin.py my-theme "My Theme" --type ui
"""

import argparse
import json
import os
import sys

PERMISSIONS = ("fs", "net", "notify", "settings")

DEFAULT_THEME = (
    "2e3440\n3b4252\n434c5e\n4c566a\n616e88\neceff4\n"
    "9aa5b8\n88c0d0\n3b5563\na3be8c\n272c36\n"
)

PYTHON_APP = '''#!/usr/bin/env python3
"""@NAME@ - KimiOS app plugin (scaffolded by tools/make_plugin.py).

Self-contained stdlib client below; for the full SDK copy
tools/plugin_sdk/kimi.py next to this file and use `from kimi import KimiPlugin`.
"""

import json
import os
import sys
import urllib.error
import urllib.parse
import urllib.request

PORT = int(os.environ.get("KIMI_API_PORT", "0") or 0)
PLUGIN_ID = os.environ.get("KIMI_PLUGIN_ID", "@ID@")
PLUGIN_DIR = os.environ.get(
    "KIMI_PLUGIN_DIR", os.path.dirname(os.path.abspath(__file__)))


def api(method, endpoint, params=None, body=None, ctype=None):
    if PORT <= 0:
        raise RuntimeError("KIMI_API_PORT not set - Plugin API unavailable")
    url = "http://127.0.0.1:%d%s" % (PORT, endpoint)
    if params:
        url += "?" + urllib.parse.urlencode(params)
    data = body.encode("utf-8") if isinstance(body, str) else body
    headers = {"Content-Type": ctype} if ctype else {}
    req = urllib.request.Request(url, data=data, headers=headers, method=method)
    with urllib.request.urlopen(req, timeout=5) as resp:
        raw = resp.read().decode("utf-8", "replace")
    try:
        return json.loads(raw)
    except ValueError:
        return raw


def main():
    print("@NAME@ (id=%s)" % PLUGIN_ID)
    try:
        print("info:", json.dumps(api("GET", "/api/info"), sort_keys=True))
        api("POST", "/api/notify",
            body=json.dumps({"title": "@NAME@", "body": "Hello from @ID@"}),
            ctype="application/json")
        print("notification sent")
    except (RuntimeError, urllib.error.URLError, OSError) as e:
        print("API unavailable (%s) - running offline" % e)
    out = os.path.join(PLUGIN_DIR, "out.txt")
    try:
        os.makedirs(PLUGIN_DIR, exist_ok=True)
        with open(out, "w", encoding="utf-8") as f:
            f.write("hello from @ID@\\n")
        print("wrote", out)
    except OSError as e:
        print("write failed:", e)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
'''

PYTHON_SERVICE = '''#!/usr/bin/env python3
"""@NAME@ - KimiOS service plugin (scaffolded by tools/make_plugin.py).

Background loop: appends a heartbeat line every 60s, exits cleanly on
SIGTERM/SIGINT. Add Plugin API calls via tools/plugin_sdk/kimi.py.
"""

import os
import signal
import sys
import time

PLUGIN_DIR = os.environ.get(
    "KIMI_PLUGIN_DIR", os.path.dirname(os.path.abspath(__file__)))
STOP = False


def _on_signal(signum, frame):
    global STOP
    STOP = True


def main():
    signal.signal(signal.SIGTERM, _on_signal)
    signal.signal(signal.SIGINT, _on_signal)
    interval = 60
    hb = os.path.join(PLUGIN_DIR, "heartbeat.txt")
    print("@ID@: service started (interval=%ss)" % interval)
    n = 0
    while not STOP:
        n += 1
        line = "beat %d %s\\n" % (n, time.strftime("%Y-%m-%d %H:%M:%S"))
        try:
            with open(hb, "a", encoding="utf-8") as f:
                f.write(line)
        except OSError as e:
            print("heartbeat write failed:", e)
        slept = 0
        while slept < interval and not STOP:
            time.sleep(1)
            slept += 1
    print("@ID@: stopped cleanly")
    return 0


if __name__ == "__main__":
    sys.exit(main())
'''

NODE_APP = '''// @NAME@ - KimiOS app plugin (node), scaffolded by tools/make_plugin.py.
const http = require("http");
const port = parseInt(process.env.KIMI_API_PORT || "0", 10);
const pluginId = process.env.KIMI_PLUGIN_ID || "@ID@";

function api(method, path, body) {
  return new Promise((resolve, reject) => {
    if (!port) return reject(new Error("KIMI_API_PORT not set"));
    const data = body ? JSON.stringify(body) : null;
    const req = http.request({
      host: "127.0.0.1", port, path, method,
      headers: data ? { "Content-Type": "application/json" } : {},
    }, (res) => {
      let raw = "";
      res.on("data", (c) => { raw += c; });
      res.on("end", () => resolve(raw));
    });
    req.on("error", reject);
    if (data) req.write(data);
    req.end();
  });
}

(async () => {
  console.log(pluginId + " (node)");
  try {
    console.log("info:", await api("GET", "/api/info"));
    await api("POST", "/api/notify",
              { title: "@NAME@", body: "Hello from @ID@" });
    console.log("notification sent");
  } catch (e) {
    console.log("API unavailable (" + e.message + ") - running offline");
  }
})();
'''

NODE_SERVICE = '''// @NAME@ - KimiOS service plugin (node), scaffolded by tools/make_plugin.py.
const fs = require("fs");
const path = require("path");
const dir = process.env.KIMI_PLUGIN_DIR || __dirname;
let n = 0;
console.log("@ID@: service started");
const timer = setInterval(() => {
  n += 1;
  const line = "beat " + n + " " + new Date().toISOString() + "\\n";
  try { fs.appendFileSync(path.join(dir, "heartbeat.txt"), line); } catch (e) {}
  console.log("heartbeat " + n);
}, 60000);
function stop() {
  clearInterval(timer);
  console.log("@ID@: stopped cleanly");
  process.exit(0);
}
process.on("SIGTERM", stop);
process.on("SIGINT", stop);
'''

NATIVE_APP = '''#!/bin/sh
# @NAME@ - KimiOS native app plugin (scaffolded by tools/make_plugin.py).
echo "@ID@ running (native)"
echo "KIMI_PLUGIN_DIR=${KIMI_PLUGIN_DIR:-unset}"
echo "KIMI_API_PORT=${KIMI_API_PORT:-unset}"
exit 0
'''

NATIVE_SERVICE = '''#!/bin/sh
# @NAME@ - KimiOS native service plugin (scaffolded by tools/make_plugin.py).
trap 'exit 0' TERM INT
DIR="${KIMI_PLUGIN_DIR:-$(dirname "$0")}"
N=0
while :; do
  N=$((N + 1))
  printf 'beat %s\\n' "$N" >> "$DIR/heartbeat.txt" 2>/dev/null
  sleep 60 &
  wait $!
done
'''

UI_APPLY = '''#!/bin/sh
# @NAME@ - KimiOS UI plugin entry (scaffolded by tools/make_plugin.py).
# The host applies the bundled theme via plugin.json ui.theme (apply_ui).
DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
THEME="$DIR/theme.ktheme"
echo "@ID@: applying bundled theme"
if [ -f "$THEME" ]; then
  cat "$THEME"
else
  echo "@ID@: theme.ktheme not found" >&2
  exit 1
fi
exit 0
'''


def fail(msg):
    print("error: %s" % msg)
    raise SystemExit(1)


def render(template, pid, name):
    return template.replace("@ID@", pid).replace("@NAME@", name)


def main(argv=None):
    ap = argparse.ArgumentParser(
        prog="make_plugin.py",
        description="Scaffold a KimiOS plugin directory (manifest + entry).")
    ap.add_argument("id", help="plugin id (<=48 bytes, no '/' or '..')")
    ap.add_argument("name", help="display name (<=48 bytes)")
    ap.add_argument("--type", choices=["app", "service", "ui"], default="app")
    ap.add_argument("--runtime", choices=["native", "python", "node"],
                    default=None, help="default: python (native for ui)")
    ap.add_argument("--version", default="1.0")
    ap.add_argument("--description", default=None)
    ap.add_argument("--permissions", default=None,
                    help="comma list of fs,net,notify,settings")
    ap.add_argument("-o", "--output", default=None,
                    help="output directory (default: ./<id>)")
    ap.add_argument("--force", action="store_true",
                    help="write into an existing directory")
    args = ap.parse_args(argv)

    pid, name = args.id, args.name
    if not pid or len(pid.encode("utf-8")) > 48 or "/" in pid or ".." in pid:
        fail("id must be 1-48 bytes and contain no '/' or '..'")
    if not name or len(name.encode("utf-8")) > 48:
        fail("name must be 1-48 bytes")
    if len(args.version.encode("utf-8")) > 16:
        fail("version must be <= 16 bytes")

    ptype = args.type
    runtime = args.runtime or ("native" if ptype == "ui" else "python")
    if ptype == "ui" and runtime != "native":
        fail("ui plugins use runtime=native with an apply.sh entry")

    if args.permissions is None:
        perms = {"app": ["fs", "notify"],
                 "service": ["fs", "notify", "settings"],
                 "ui": []}[ptype]
    else:
        perms = [p.strip() for p in args.permissions.split(",") if p.strip()]
        for perm in perms:
            if perm not in PERMISSIONS:
                fail("unknown permission %r (choose from %s)"
                     % (perm, ",".join(PERMISSIONS)))

    files = {}
    if ptype == "ui":
        entry = "apply.sh"
        files[entry] = render(UI_APPLY, pid, name)
        files["theme.ktheme"] = DEFAULT_THEME
    elif runtime == "python":
        entry = "service.py" if ptype == "service" else "main.py"
        tmpl = PYTHON_SERVICE if ptype == "service" else PYTHON_APP
        files[entry] = render(tmpl, pid, name)
    elif runtime == "node":
        entry = "service.js" if ptype == "service" else "main.js"
        tmpl = NODE_SERVICE if ptype == "service" else NODE_APP
        files[entry] = render(tmpl, pid, name)
    else:
        entry = "service.sh" if ptype == "service" else "main.sh"
        tmpl = NATIVE_SERVICE if ptype == "service" else NATIVE_APP
        files[entry] = render(tmpl, pid, name)

    manifest = {
        "id": pid,
        "name": name,
        "version": args.version,
        "type": ptype,
        "runtime": runtime,
        "entry": entry,
        "args": [],
        "permissions": perms,
        "description": args.description or "%s (%s/%s)" % (name, ptype, runtime),
    }
    if ptype == "ui":
        manifest["ui"] = {"theme": "theme.ktheme"}
    manifest_text = json.dumps(manifest, indent=2) + "\n"
    if len(manifest_text.encode("utf-8")) >= 2048:
        fail("manifest exceeds the 2048-byte parse buffer; shorten fields")

    outdir = args.output or pid
    if os.path.exists(outdir) and os.listdir(outdir) and not args.force:
        fail("%s exists and is not empty (use --force)" % outdir)
    os.makedirs(outdir, exist_ok=True)

    written = []
    path = os.path.join(outdir, "plugin.json")
    with open(path, "w", encoding="utf-8") as f:
        f.write(manifest_text)
    written.append(path)
    for fname, content in files.items():
        path = os.path.join(outdir, fname)
        with open(path, "w", encoding="utf-8") as f:
            f.write(content)
        if fname.endswith(".sh"):
            os.chmod(path, 0o755)
        written.append(path)

    print("scaffolded %s plugin %r in %s:" % (ptype, pid, outdir))
    for path in written:
        print("  %s" % path)
    print("next: edit the entry, then install via Plugin Manager / Marketplace")
    print("      (extracted directory under /disk/import). See docs/plugin-sdk.md")
    return 0


if __name__ == "__main__":
    sys.exit(main())
