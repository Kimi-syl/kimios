# Local iSH Linux runtime on iOS

The IPA now embeds iSH-ARM64 and an Alpine fakefs rootfs. This is a real Linux
userland, not a remote bridge and not a hardware VM.

## Verified

- iSH ARM64 native smoke: boots Alpine and executes `/bin/sh` commands.
- Real OpenCode Linux ARM64 musl binary: `opencode --version` prints `1.19.6`
  inside iSH on the CI arm64 host.
- The real OpenCode Linux ARM64 musl binary runs inside iSH and reports
  `1.19.6`. Full TUI rendering still has to be exercised through the in-app
  KimiISH devpts driver and shared VT emulator on a device.
- iOS IPA compiles and links `libish.a`, `libish_emu.a`, `libfakefs.a`.
- `SYS_LINUX_RUN` and `SYS_RUN_PATH` are routed to the guest.
- `SYS_PROC` has streaming PTY sessions, stdin writes, resize, status and close.
- `/disk` is bind-mounted into the guest; `/root` maps to `/disk/home`.
- libstdc++, libgcc, CA certificates and OpenCode are seeded into the rootfs by
  `ios/deps/install_guest_runtime.sh`.

## Not yet verified on a physical iPad

- The full OpenCode TUI through the **in-app** KimiISH PTY on a physical iPad.
  The guest process, PTY API and VT response path are implemented, but headless
  CI cannot emulate the terminal query responses OpenCode waits for.
- Long-running server/agent sessions and thermal/performance behavior.
- Guest networking inside the iOS app (iSH host-socket translation).

## Licensing

iSH is GPLv3. The IPA is a combined GPLv3 work when this backend is enabled.
See the repository `NOTICE` and the in-app `Resources/licenses` directory.