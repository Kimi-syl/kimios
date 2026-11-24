# Local OpenCode runtime on iOS — feasibility report

**Question:** can the IPA run OpenCode locally, with no QEMU/UTM-style emulator,
and reuse the existing Kimi VT terminal?

**Short answer:** not with the current codebase. There is one plausible
no-emulator family (a JavaScript runtime port), but it is a new runtime
integration, not a dispatcher patch. The remote PTY bridge remains the working
no-emulator path today.

## Constraints from iOS

- Sandboxed apps cannot `fork`/`exec` downloaded executables.
- iOS does not expose a Linux syscall ABI (`svc #0` with Linux numbers), and
  `TPIDR_EL0`/thread-local storage is owned by Darwin.
- JIT is restricted; it needs special sideload entitlements and does not solve
  the ELF/kernel-ABI problem by itself.
- `child_process.spawn()`/`fork()` are unavailable in Node-on-iOS runtimes.

## Option A — nodejs-mobile + OpenCode `build:node`

This is the most credible local, no-emulator route.

What exists:

- OpenCode has a Node-target build script: `packages/cli/package.json`
  (`build:node`), but the current builder pins **Node 26.4.0** and emits a Node
  SEA executable for Linux/macOS/Windows. There is no iOS target.
- OpenCode already has Node platform packages (`@effect/platform-node`,
  `@effect/sql-sqlite-node`) and runtime-specific import maps.
- `nodejs-mobile` provides a Node.js runtime as an iOS framework (current public
  release is Node 18.20.4, an iOS zip/framework of roughly 51 MB).

What is missing:

1. **Node version gap** — public nodejs-mobile is Node 18; OpenCode's current
   Node builder pins **Node 26.4.0** and uses Node SEA. Porting Node 26 plus SEA
   support to iOS (or backporting OpenCode to Node 18 without SEA) is a major
   separate effort.
2. **Native modules** — OpenCode uses `@lydell/node-pty`, `@parcel/watcher`,
   `photon-node`, `web-tree-sitter` and platform binaries. There are no iOS
   builds for these. Each needs a JS shim, a cross-compiled iOS module, or
   removal.
3. **PTY semantics** — `node-pty` is unavailable on iOS. A shim must connect
   OpenCode's stdin/stdout to the Kimi terminal: fake TTY properties, raw-mode
   handling, resize events, ANSI output and key input. This is feasible but
   OpenCode/OpenTUI must be tested against the shim.
4. **Tool execution** — OpenCode's bash/shell tools require `child_process`.
   Those must be replaced with Kimi host services (`/disk` file APIs, `SYS_NET`,
   the built-in shell) or disabled. This changes a core part of the agent.
5. **OpenCode bundle** — the Bun standalone executable is not a portable JS
   file. The Node build must be produced from the OpenCode repository with Bun
   and vendored into the IPA, with all dependencies bundled or shimmed.
6. **Build infrastructure** — NodeMobile.xcframework, the OpenCode Node bundle,
   a Swift bridge, and an Xcode build step would all need to be added. This is
   multi-hundred-MB build surface and needs a dedicated CI job.

Estimated effort: a focused multi-week port, with a hard risk that OpenCode's
tool layer cannot be made feature-complete without a real process model.

## Option B — extract the Bun/JSC standalone bundle and run it in JavaScriptCore

Bun standalone executables embed a Bun runtime plus a module graph/bytecode;
they are not a portable JSC script. OpenCode also depends on Bun APIs, native
packages, worker threads, process spawning and terminal PTYs. Reimplementing
that runtime in JavaScriptCore is larger than Option A and has no clear
compatibility guarantee. **No-go.**

## Option C — in-process AArch64 Linux syscall translation on Darwin

The guest and host are both AArch64, so instruction translation is not needed,
but this is not a small shim:

- emulate Linux syscall numbers and semantics on Darwin (mmap/mprotect, futex,
  clone, epoll, eventfd, io_uring probes, `/proc`, sockets, ioctl/TTY, signals);
- load musl and its TLS without breaking Darwin's per-thread TLS;
- handle executable memory/JIT entitlements;
- translate Bun/JSC's native runtime expectations.

This is a user-mode Linux compatibility layer, not an emulator, but it is a
large systems project comparable in scope to the existing Kimi kernel ABI work.
It cannot be responsibly claimed as a small iOS patch.

## Recommendation

- Keep the **remote PTY bridge** as the shipping no-emulator path.
- If local execution is a hard requirement, fund **Option A** as a dedicated
  runtime port: Node 22/24 mobile framework, OpenCode Node bundle, PTY shim,
  and tool-provider shims. Do not present it as complete until it is demonstrated
  on an actual iPad with the OpenCode TUI.
- Do not use the Linux ELF path as a shortcut; it does not exist on iOS.