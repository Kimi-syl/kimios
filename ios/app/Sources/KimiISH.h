//
//  KimiISH.h
//  KimiOS
//
//  Headless iSH embedding: boots an Alpine fakefs guest and executes commands
//  through the emulated Linux ARM64 userland. Backs SYS_LINUX_RUN / SYS_RUN_PATH
//  on iOS (real Linux binaries, no fork/exec of guest ELF, no JIT).
//

#import <Foundation/Foundation.h>

NS_ASSUME_NONNULL_BEGIN

@interface KimiISH : NSObject

/// YES if this build was compiled with the iSH libraries available.
+ (BOOL)available;

/// Prepare the guest once (copy the bundled rootfs to a writable location and
/// boot the iSH kernel). Safe to call repeatedly. Returns YES on success.
+ (BOOL)prepare:(NSString *_Nullable *_Nullable)error;

/// Run a shell command (`/bin/sh -c <command>`) in the guest, synchronously.
/// Captures combined stdout+stderr into `out` (up to `capacity`). Returns the
/// guest exit status (>=0), or a negative error code on failure/timeout.
+ (int)runCommand:(NSString *)command
           output:(uint8_t *)out
         capacity:(size_t)capacity
           length:(size_t *)length
          timeout:(double)timeout;

@end

/// PTY-backed guest sessions used by SYS_PROC. `command` is passed to
/// /bin/sh -c; `cwd` should be a guest path such as /disk.
int kimi_pty_spawn(const char *command, const char *cwd, int rows, int cols,
                   uint64_t *slot);
long kimi_pty_read(uint64_t slot, void *buf, size_t cap);
long kimi_pty_write(uint64_t slot, const void *buf, size_t len);
int kimi_pty_resize(uint64_t slot, int rows, int cols);
/// 0 = running, >0 = exit code + 1, -1 = invalid slot.
int kimi_pty_status(uint64_t slot);
int kimi_pty_close(uint64_t slot);

NS_ASSUME_NONNULL_END