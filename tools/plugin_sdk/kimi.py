#!/usr/bin/env python3
"""KimiOS Plugin API SDK — Python stdlib only (urllib / json / os).

Wraps the localhost Plugin API server described in docs/plugin-system.md.
The host injects three env vars into every launched plugin:

    KIMI_API_PORT    port of the localhost Plugin API server (0/absent = down)
    KIMI_PLUGIN_ID   the plugin's manifest id
    KIMI_PLUGIN_DIR  install dir, /disk/plugins/<id>

Usage from a plugin (bundle this file next to your entry script):

    from kimi import KimiPlugin, KimiApiError
    p = KimiPlugin()
    try:
        print(p.info())
        p.notify("Hello", "from my plugin")
    except KimiApiError as e:
        print("API unavailable:", e)   # degrade gracefully

CLI demo:

    python3 kimi.py demo
    python3 kimi.py info
    python3 kimi.py list /disk
    python3 kimi.py read /disk/notes.txt
    python3 kimi.py write /disk/notes.txt "hello"
    python3 kimi.py notify "Title" "Body"
    python3 kimi.py settings [KEY]
    python3 kimi.py set KEY VALUE

All methods raise KimiApiError when the server is unreachable or returns
an HTTP error, so plugins can catch one exception type and fall back to
local behavior. See docs/plugin-sdk.md for the full guide.
"""

import json
import os
import urllib.error
import urllib.parse
import urllib.request

DEFAULT_TIMEOUT = 5.0


class KimiApiError(Exception):
    """A Plugin API call failed (server down, HTTP error, bad env)."""

    def __init__(self, message, status=None):
        super().__init__(message)
        self.status = status


class KimiPlugin:
    """Client for the KimiOS localhost Plugin API."""

    def __init__(self, port=None, plugin_id=None, plugin_dir=None,
                 timeout=DEFAULT_TIMEOUT):
        if port is None:
            port = os.environ.get("KIMI_API_PORT", "0")
        try:
            self.port = int(port)
        except (TypeError, ValueError):
            self.port = 0
        self.plugin_id = (plugin_id if plugin_id is not None
                          else os.environ.get("KIMI_PLUGIN_ID", ""))
        self.plugin_dir = (plugin_dir if plugin_dir is not None
                           else os.environ.get("KIMI_PLUGIN_DIR", ""))
        self.timeout = timeout
        self.base_url = "http://127.0.0.1:%d" % self.port

    @property
    def available(self):
        """True when KIMI_API_PORT points at a (presumed) running server."""
        return self.port > 0

    def path(self, name=""):
        """Absolute path inside this plugin's install dir (or cwd fallback)."""
        base = self.plugin_dir or os.getcwd()
        return os.path.join(base, name) if name else base

    def _request(self, method, endpoint, params=None, body=None,
                 content_type=None):
        if not self.available:
            raise KimiApiError(
                "KIMI_API_PORT not set - Plugin API server unavailable")
        url = self.base_url + endpoint
        if params:
            url += "?" + urllib.parse.urlencode(params)
        data = None
        headers = {}
        if body is not None:
            data = body.encode("utf-8") if isinstance(body, str) else body
            headers["Content-Type"] = content_type or "text/plain; charset=utf-8"
        req = urllib.request.Request(url, data=data, headers=headers,
                                     method=method)
        try:
            with urllib.request.urlopen(req, timeout=self.timeout) as resp:
                raw = resp.read().decode("utf-8", "replace")
        except urllib.error.HTTPError as e:
            detail = ""
            try:
                detail = e.read().decode("utf-8", "replace")[:200]
            except Exception:
                pass
            raise KimiApiError(
                "HTTP %d from %s: %s" % (e.code, endpoint, detail or e.reason),
                status=e.code) from e
        except (urllib.error.URLError, OSError, ValueError) as e:
            raise KimiApiError(
                "Plugin API unreachable at %s (%s)" % (self.base_url, e)) from e
        try:
            return json.loads(raw)
        except ValueError:
            return raw

    def info(self):
        """GET /api/info -> {os, version, plugin_id, api}."""
        return self._request("GET", "/api/info")

    def fs_list(self, path):
        """GET /api/fs/list?path= -> directory listing (JSON)."""
        return self._request("GET", "/api/fs/list", {"path": path})

    def fs_read(self, path):
        """GET /api/fs/read?path= -> file content.

        Returns the raw text, or the 'data'/'content' field when the
        server wraps the content in a JSON object.
        """
        res = self._request("GET", "/api/fs/read", {"path": path})
        if isinstance(res, dict):
            for key in ("data", "content"):
                if key in res:
                    return res[key]
        return res

    def fs_write(self, path, data):
        """POST /api/fs/write?path= with the content as the request body.

        The host sandboxes writes to /disk. `data` may be str or bytes.
        """
        return self._request("POST", "/api/fs/write", {"path": path},
                             body=data)

    def notify(self, title, body=""):
        """POST /api/notify {title, body} -> KimiOS shade notification."""
        return self._request(
            "POST", "/api/notify",
            body=json.dumps({"title": title, "body": body}),
            content_type="application/json")

    def settings_get(self, key=None):
        """GET /api/settings?plugin=<id> -> plugin-scoped settings dict.

        With `key`, returns just that value (None when unset).
        """
        res = self._request("GET", "/api/settings", {"plugin": self.plugin_id})
        if key is not None and isinstance(res, dict):
            return res.get(key)
        return res

    def settings_set(self, key, value):
        """POST /api/settings?plugin=<id> {key, value}."""
        return self._request(
            "POST", "/api/settings", {"plugin": self.plugin_id},
            body=json.dumps({"key": key, "value": value}),
            content_type="application/json")


def _print_result(res):
    if isinstance(res, (dict, list)):
        print(json.dumps(res, indent=2, sort_keys=True))
    else:
        print(res)


def _demo(p):
    ok = True
    try:
        print("info:")
        _print_result(p.info())
    except KimiApiError as e:
        ok = False
        print("  info failed: %s" % e)
    try:
        p.notify("KimiOS SDK", "Hello from the Python SDK demo")
        print("notify: sent")
    except KimiApiError as e:
        ok = False
        print("notify failed: %s" % e)
    try:
        p.settings_set("demo_ran", "yes")
        print("settings demo_ran = %r" % p.settings_get("demo_ran"))
    except KimiApiError as e:
        ok = False
        print("settings failed: %s" % e)
    if not ok:
        print("demo: API server unavailable - this is fine, plugins should "
              "degrade gracefully")
    return 0 if ok else 1


def _cli(argv=None):
    import argparse
    ap = argparse.ArgumentParser(
        prog="kimi.py", description="KimiOS Plugin API CLI demo (stdlib only)")
    sub = ap.add_subparsers(dest="cmd", required=True)
    sub.add_parser("info", help="GET /api/info")
    sp = sub.add_parser("list", help="GET /api/fs/list")
    sp.add_argument("path")
    sp = sub.add_parser("read", help="GET /api/fs/read")
    sp.add_argument("path")
    sp = sub.add_parser("write", help="POST /api/fs/write")
    sp.add_argument("path")
    sp.add_argument("text")
    sp = sub.add_parser("notify", help="POST /api/notify")
    sp.add_argument("title")
    sp.add_argument("body", nargs="?", default="")
    sp = sub.add_parser("settings", help="GET /api/settings")
    sp.add_argument("key", nargs="?")
    sp = sub.add_parser("set", help="POST /api/settings")
    sp.add_argument("key")
    sp.add_argument("value")
    sub.add_parser("demo", help="info + notify + settings round-trip")
    args = ap.parse_args(argv)

    p = KimiPlugin()
    if args.cmd == "demo":
        return _demo(p)
    try:
        if args.cmd == "info":
            _print_result(p.info())
        elif args.cmd == "list":
            _print_result(p.fs_list(args.path))
        elif args.cmd == "read":
            _print_result(p.fs_read(args.path))
        elif args.cmd == "write":
            _print_result(p.fs_write(args.path, args.text))
        elif args.cmd == "notify":
            _print_result(p.notify(args.title, args.body))
        elif args.cmd == "settings":
            _print_result(p.settings_get(args.key))
        elif args.cmd == "set":
            _print_result(p.settings_set(args.key, args.value))
    except KimiApiError as e:
        print("error: %s" % e)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(_cli())
