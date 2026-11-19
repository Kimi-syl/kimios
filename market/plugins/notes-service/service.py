#!/usr/bin/env python3
"""notes-service — example KimiOS background service (type=service).

Runs forever, sleeping between beats. Every `interval` seconds it:
  * writes a heartbeat line to /disk/plugins/<id>/heartbeat.txt
  * posts a notification (throttled so it is not spammy)

The interval is read from plugin settings (key "interval"), defaulting
to 60s. SIGTERM/SIGINT set a stop flag so the loop exits cleanly. When
the Plugin API is down it still writes the heartbeat via local file I/O.
"""

import os
import signal
import sys
import time

from kimi import KimiApiError, KimiPlugin

HERE = os.path.dirname(os.path.abspath(__file__))
_STOP = False


def _on_signal(signum, frame):
    global _STOP
    _STOP = True


def read_interval(p):
    try:
        val = p.settings_get("interval")
        if val is not None:
            n = int(val)
            if n > 0:
                return n
    except (KimiApiError, TypeError, ValueError):
        pass
    return 60


def write_heartbeat(p, path, text):
    try:
        p.fs_write(path, text)
        return "api"
    except KimiApiError:
        try:
            os.makedirs(os.path.dirname(path), exist_ok=True)
            with open(path, "a", encoding="utf-8") as f:
                f.write(text)
            return "local"
        except OSError:
            return "failed"


def main():
    signal.signal(signal.SIGTERM, _on_signal)
    signal.signal(signal.SIGINT, _on_signal)

    p = KimiPlugin()
    interval = read_interval(p)
    hb = os.path.join(p.plugin_dir or HERE, "heartbeat.txt")
    print("notes-service: started (interval=%ss, heartbeat=%s)" % (interval, hb))

    try:
        p.notify("Notes service", "Heartbeat service started.")
    except KimiApiError as e:
        print("notify: skipped (%s)" % e)

    beat = 0
    while not _STOP:
        beat += 1
        stamp = time.strftime("%Y-%m-%d %H:%M:%S")
        line = "beat %d %s\n" % (beat, stamp)
        how = write_heartbeat(p, hb, line)
        print("heartbeat %d (%s)" % (beat, how))

        if beat == 1 or beat % 10 == 0:
            try:
                p.notify("Notes heartbeat", "beat %d at %s" % (beat, stamp))
            except KimiApiError:
                pass

        slept = 0
        while slept < interval and not _STOP:
            time.sleep(1)
            slept += 1

    print("notes-service: stopped cleanly after %d beats" % beat)
    return 0


if __name__ == "__main__":
    sys.exit(main())
