#!/usr/bin/env python3
"""Prepare an Alpine sysroot for native guest networking on Android.

Android has no /etc/resolv.conf, so musl's getaddrinfo (which reads that absolute
path) fails even though the app has INTERNET. This shim:

  1. Patches lib/ld-musl-aarch64.so.1, replacing the absolute strings
     "/etc/resolv.conf" and "/etc/hosts" with the relative "etc/resolv.conf" /
     "etc/hosts" (same byte length, NUL-padded, so ELF offsets are preserved).
  2. Writes <sysroot>/etc/resolv.conf (public nameservers) and etc/hosts.

The guest loader (android/jni/kimi_elf.c) chdir()s into the sysroot before
entering the guest, so musl resolves "etc/resolv.conf" relative to the sysroot.
The guest's own OpenSSL then provides HTTPS (the host browser/kpkg has no TLS).

Usage: python3 tools/prepare_sysroot.py /path/to/alpine/root
"""
import os
import sys


def main():
    sr = sys.argv[1]
    musl = os.path.join(sr, "lib", "ld-musl-aarch64.so.1")
    with open(musl, "rb") as f:
        data = f.read()
    n1 = data.count(b"/etc/resolv.conf\x00")
    n2 = data.count(b"/etc/hosts\x00")
    # same-length replacements keep every ELF offset intact
    data = data.replace(b"/etc/resolv.conf\x00", b"etc/resolv.conf\x00\x00")
    data = data.replace(b"/etc/hosts\x00", b"etc/hosts\x00\x00")
    with open(musl, "wb") as f:
        f.write(data)
    etc = os.path.join(sr, "etc")
    os.makedirs(etc, exist_ok=True)
    with open(os.path.join(etc, "resolv.conf"), "w") as f:
        f.write("nameserver 1.1.1.1\nnameserver 8.8.8.8\noptions timeout:2 attempts:2\n")
    with open(os.path.join(etc, "hosts"), "w") as f:
        f.write("127.0.0.1 localhost\n::1 localhost\n")
    # Bundle CA certificates: apkg skips Alpine's update-ca-certificates script,
    # so concatenate the Mozilla trust store into the OpenSSL bundle path. The
    # guest loader sets SSL_CERT_FILE/SSL_CERT_DIR to these sysroot paths
    # (Android has no /etc/ssl, so OpenSSL's default absolute paths fail).
    import glob
    mozilla = os.path.join(sr, "usr", "share", "ca-certificates", "mozilla")
    crts = sorted(glob.glob(os.path.join(mozilla, "*.crt")))
    if crts:
        sslcerts = os.path.join(etc, "ssl", "certs")
        os.makedirs(sslcerts, exist_ok=True)
        bundle = os.path.join(sslcerts, "ca-certificates.crt")
        with open(bundle, "wb") as out:
            for c in crts:
                with open(c, "rb") as inp:
                    out.write(inp.read())
                    out.write(b"\n")
        print(f"bundled {len(crts)} CA certs -> etc/ssl/certs/ca-certificates.crt")
    print(f"patched ld-musl: resolv.conf x{n1}, hosts x{n2}; wrote etc/resolv.conf + etc/hosts")
    if n1 == 0:
        print("WARNING: '/etc/resolv.conf' string not found (already patched, or different musl?)")


if __name__ == "__main__":
    main()
