#!/usr/bin/env python3
"""sysinfo-app — example KimiOS app plugin (type=app, runtime=python).

Demonstrates the FS API: reads GET /api/info and GET /api/fs/list?path=/disk
and prints a small system report. When the Plugin API server is unavailable
it degrades to local os.uname()/os.listdir() so it still produces a report.
"""

import os
import sys

from kimi import KimiApiError, KimiPlugin

HERE = os.path.dirname(os.path.abspath(__file__))


def local_report():
    lines = ["System report (local fallback — Plugin API unavailable)"]
    try:
        u = os.uname()
        lines.append("  sysname : %s" % u.sysname)
        lines.append("  release : %s" % u.release)
        lines.append("  machine : %s" % u.machine)
    except (AttributeError, OSError):
        lines.append("  sysname : %s" % sys.platform)
    lines.append("  cwd     : %s" % os.getcwd())
    return lines


def list_names(res):
    """Normalize a /api/fs/list payload into a list of entry names."""
    if isinstance(res, dict):
        for key in ("entries", "files", "list", "names"):
            if key in res:
                res = res[key]
                break
    if isinstance(res, str):
        return [ln for ln in res.splitlines() if ln.strip()]
    names = []
    if isinstance(res, list):
        for item in res:
            if isinstance(item, str):
                names.append(item)
            elif isinstance(item, dict):
                names.append(str(item.get("name") or item.get("path") or "?"))
    return names


def main():
    p = KimiPlugin()
    print("=== KimiOS system report (sysinfo-app) ===")

    info = None
    try:
        info = p.info()
    except KimiApiError as e:
        print("info: unavailable (%s)" % e)

    if isinstance(info, dict):
        print("OS      : %s" % info.get("os", "?"))
        print("Version : %s" % info.get("version", "?"))
        print("API     : %s" % info.get("api", "?"))
        print("Plugin  : %s" % info.get("plugin_id", p.plugin_id or "?"))
    elif info is not None:
        print("info    : %s" % info)
    else:
        for ln in local_report():
            print(ln)

    target = "/disk"
    print("--- listing %s ---" % target)
    try:
        names = list_names(p.fs_list(target))
        if names:
            for n in names:
                print("  %s" % n)
        else:
            print("  (empty)")
    except KimiApiError as e:
        print("  fs/list unavailable (%s)" % e)
        try:
            base = target if os.path.isdir(target) else HERE
            for n in sorted(os.listdir(base)):
                print("  %s" % n)
        except OSError as e2:
            print("  local listing failed: %s" % e2)

    print("=== done ===")
    return 0


if __name__ == "__main__":
    sys.exit(main())
