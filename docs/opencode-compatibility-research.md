# OpenCode compatibility research (P6)

**Status: RESEARCH.** Key finding: OpenCode is **TypeScript compiled to a single-file Bun 1.4.4 executable** (JavaScriptCore), *not* Go (the Go TUI was deleted 2025-11-02, commit `f68374ad`). Current release v1.19.6.

## Two paths, very different cost

### Linux ARM64 musl — TRACTABLE (recommended, ~days)
- Release asset `opencode-linux-arm64-musl.tar.gz` → `opencode` (219 MB), a dynamic musl ELF — the exact binary class the Android libOS loader already runs (node/python verified on device).
- **Verified running in the phone-hosted aarch64 environment**: `--version` → 1.19.6 exit 0; `--help` exit 0; `opencode serve --port 4199` → HTTP 200 on `/app` (embedded web UI) and `/config`. Exercises JSC, JIT, threads, sockets, HTTP.
- Bun reads its embedded bundle via `dl_iterate_phdr` + `getauxval(AT_PHDR)`, **not** `/proc/self/exe` — so the `app_process64` exe-path caveat is cosmetic, not fatal.
- Gaps to close: (1) binary is non-PIE `ET_EXEC` linked at `0x200000` with 104 RELA relocs → `kimi_elf.c` must map `ET_EXEC` at its link address (currently reserves via `mmap(NULL)`); (2) sysroot needs `libstdc++`/`libgcc` (Alpine, already pulled by nodejs); (3) env (`HOME/TMPDIR/LD_LIBRARY_PATH/SSL_CERT_FILE`); (4) stdin closed in v1 → start with `opencode serve` (headless) or `opencode run`; wire a PTY later for the TUI; (5) phantom-process killer → `:guest` service process; (6) bump guest stack (GNU_STACK hint 12.8 MB).
- **GUI shortcut:** `opencode serve`'s web UI opened in the KimiOS embedded browser = OpenCode with a GUI, zero user32.

### Windows ARM64 — WINE-SCALE GAP (research track, months)
Measured from `opencode.exe` (194 MB): machine `0xAA64`, PE32+, 14 sections, subsystem 3 (console), base `0x140000000`, `SizeOfImage` 196.6 MB, stack reserve 18 MB.
- **Imports: 4 DLLs, 353 functions** (kernel32 320, ntdll 29, api-ms-win-core-synch 3, bcryptprimitives 1) + 11 delay-load DLLs (WS2_32, USER32, CRYPT32, ADVAPI32, SHELL32, ole32, …). KimiOS implements **9** kernel32 imports (`src/pe.rs:84-97`).
- **110,865 DIR64 relocations** (886 blocks); KimiOS has **no relocation engine** and rejects base `0x140000000` (hard window `[0x40000000,0x63000000)`).
- TLS directory + 5 callbacks; `.pdata` ARM64 unwind (JSC JIT needs `RtlAddFunctionTable`/`RtlVirtualUnwind`); **x18 = TEB** (Windows ARM64 ABI) — KimiOS sets x18=0.
- Runtime needs: VEH, IOCP (libuv), `WaitOnAddress` (futex-equiv), ConPTY (shell), WS2_32 (network), heap/SRW/condvars.
- Current PE loader: 9 imports, no relocs, no TEB/PEB, 64 KiB stack, 3 MiB `/disk` exec cap, 128 MiB QEMU RAM, 64-inode KimiFS. Only a synthetic ~2 KB test PE runs.
- Verdict: cannot load/launch today. A "print version" spike needs the loader work + ~150–200 startup APIs; `serve` needs IOCP/threads/files/WS2_32. user32/GUI (Electron desktop) = full Chromium stack, out of reach. Prior art: Wine/Hangover on aarch64.

## Plan
1. **Phase 1 (deliver OpenCode):** Linux ARM64 musl on Android libOS — fix `ET_EXEC` link-address mapping, add libstdc++/libgcc, `opencode serve` guest, verify `--version` + HTTP 200 on device, open web UI in KimiOS browser. Meets "OpenCode Linux build runs".
2. **Phase 2 (research):** Windows ARM64 console spike — relocation engine, any-base load, TEB/PEB + x18, big stack + guard pages, TLS callbacks, `.pdata` registry, gap-tracer logging unresolved APIs, then startup-critical kernel32/ntdll/sync/bcrypt set.
3. **Phase 3 (long-horizon):** Win32 GUI/user32 — served web UI is the pragmatic answer.

Full import/reloc dump: `/tmp/opencode-research/imports.txt`; live run proof: `/tmp/opencode-research/opencode-serve-proof.log`.
Sources: anomalyco/opencode v1.19.6 releases + `script/build.ts`; oven-sh/bun `StandaloneModuleGraph.rs`/`sys/lib.rs`; Microsoft Learn ARM64 ABI; `src/pe.rs`, `src/linux.rs`, `android/jni/kimi_elf.c`.
