#!/usr/bin/env python3
"""hello-python — example KimiOS app plugin (type=app, runtime=python).

Uses the bundled SDK (kimi.py) to:
  1. print system info from GET /api/info
  2. post a "Hello from <name>" notification
  3. write out.txt into the plugin dir (/disk/plugins/<id>/)

Everything degrades gracefully when the Plugin API server is down:
info is skipped, the notification is reported as skipped, and out.txt
is written with plain local file I/O instead.
"""

import json
import os
import sys
import time

from kimi import KimiApiError, KimiPlugin

HERE = os.path.dirname(os.path.abspath(__file__))


def plugin_name():
    try:
        with open(os.path.join(HERE, "plugin.json"), "r", encoding="utf-8") as f:
            return json.load(f).get("name", "hello-python")
    except (OSError, ValueError):
        return "hello-python"


def main():
    p = KimiPlugin()
    name = plugin_name()
    print("hello-python: %s (id=%s)" % (name, p.plugin_id or "?"))
    print("plugin dir: %s" % (p.plugin_dir or HERE))

    try:
        info = p.info()
        print("system info: %s" % json.dumps(info, sort_keys=True))
    except KimiApiError as e:
        print("system info: unavailable (%s)" % e)

    try:
        p.notify("Hello from %s" % name, "KimiOS example plugin is running.")
        print("notification: sent")
    except KimiApiError as e:
        print("notification: skipped (%s)" % e)

    text = "Hello from %s\nplugin_id: %s\nwritten: %s\n" % (
        name, p.plugin_id or "?", time.strftime("%Y-%m-%d %H:%M:%S"))
    out = os.path.join(p.plugin_dir or HERE, "out.txt")
    try:
        p.fs_write(out, text)
        print("wrote %s via Plugin API" % out)
    except KimiApiError as e:
        try:
            os.makedirs(os.path.dirname(out), exist_ok=True)
            with open(out, "w", encoding="utf-8") as f:
                f.write(text)
            print("wrote %s locally (API unavailable: %s)" % (out, e))
        except OSError as e2:
            print("write failed: %s" % e2)
            return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
