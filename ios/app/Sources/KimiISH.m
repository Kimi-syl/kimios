//
//  KimiISH.m
//  KimiOS
//
//  Headless embedding of iSH (OpenMinis/ish-arm64) so the KimiOS terminal can
//  execute a real Linux userland on iOS. We boot the iSH kernel directly (no
//  UIKit / TerminalView), mount the bundled Alpine fakefs root, then run each
//  command as a fresh guest process with piped stdio and wait for its exit.
//
//  This mirrors iSH's own command-line runner (main.c: xX_main_Xx +
//  task_run_current) and ISHShellExecutor.m, minus the app UI.
//

#import "KimiISH.h"
#import <pthread.h>
#import <fcntl.h>
#import <unistd.h>
#import <errno.h>
#import <signal.h>
#import <dispatch/dispatch.h>

#if __has_include("kernel/init.h")
#define KIMI_HAVE_ISH 1

#include "kernel/init.h"
#include "kernel/fs.h"
#include "kernel/task.h"
#include "kernel/calls.h"
#include "fs/path.h"
#include "fs/fd.h"
#include "fs/real.h"
#include "fs/dev.h"
#include "fs/devices.h"

static dispatch_once_t g_bootOnce;
static int g_bootErr = -9999;
static NSLock *g_runLock = nil;

static dispatch_semaphore_t g_exitSem = nil;
static volatile int g_exitPid = 0;
static volatile int g_exitCode = 0;

// Called by the iSH kernel when a guest task exits (on its own thread).
static void kimi_exit_hook(struct task *task, int code) {
    if (task != NULL && task->pid == g_exitPid) {
        g_exitCode = code;
        dispatch_semaphore_t sem = g_exitSem;
        if (sem) dispatch_semaphore_signal(sem);
    }
}

static NSString *kimi_writable_root(void) {
    NSArray<NSString *> *docs =
        NSSearchPathForDirectoriesInDomains(NSDocumentDirectory, NSUserDomainMask, YES);
    NSString *base = docs.firstObject;
    return [base stringByAppendingPathComponent:@"ish/alpine"];
}

// Copy the bundled fakefs rootfs into Documents on first use (the .app bundle
// is read-only but fakefs writes meta.db + data/).
static BOOL kimi_ensure_rootfs(NSString **error) {
    NSString *dst = kimi_writable_root();
    NSString *marker = [dst stringByAppendingPathComponent:@"meta.db"];
    if ([[NSFileManager defaultManager] fileExistsAtPath:marker]) return YES;

    NSString *src = [[NSBundle mainBundle] pathForResource:@"alpine-rootfs" ofType:nil];
    if (src == nil) {
        if (error) *error = @"bundled alpine-rootfs missing";
        return NO;
    }
    NSFileManager *fm = [NSFileManager defaultManager];
    [fm removeItemAtPath:dst error:nil];
    [fm createDirectoryAtPath:[dst stringByDeletingLastPathComponent]
  withIntermediateDirectories:YES attributes:nil error:nil];
    NSError *e = nil;
    if (![fm copyItemAtPath:src toPath:dst error:&e]) {
        if (error) *error = e.localizedDescription ?: @"rootfs copy failed";
        return NO;
    }
    return YES;
}

// Create placeholder sysroot entries under /disk/alpine/root/usr/bin so the
// shared shell's command resolver (`resolve_sysroot_bin`) forwards bare names
// like `git`, `curl`, `apk`, `node` to the iSH guest. The files are empty
// markers; only their existence matters to the resolver.
static void kimi_install_stubs(void) {
    NSArray<NSString *> *docs =
        NSSearchPathForDirectoriesInDomains(NSDocumentDirectory, NSUserDomainMask, YES);
    NSString *dir = [[docs.firstObject stringByAppendingPathComponent:@"disk/alpine/root/usr/bin"] copy];
    NSFileManager *fm = [NSFileManager defaultManager];
    [fm createDirectoryAtPath:dir withIntermediateDirectories:YES attributes:nil error:nil];
    NSArray<NSString *> *tools = @[
        @"sh", @"bash", @"dash", @"ls", @"cat", @"cp", @"mv", @"rm", @"mkdir", @"rmdir",
        @"ln", @"touch", @"chmod", @"chown", @"stat", @"find", @"grep", @"egrep", @"sed",
        @"awk", @"sort", @"uniq", @"head", @"tail", @"wc", @"cut", @"tr", @"rev", @"seq",
        @"tee", @"xargs", @"env", @"printenv", @"uname", @"whoami", @"id", @"groups",
        @"pwd", @"date", @"sleep", @"true", @"false", @"yes", @"which", @"file", @"du",
        @"df", @"tar", @"gzip", @"gunzip", @"zip", @"unzip", @"curl", @"wget", @"git",
        @"make", @"gcc", @"cc", @"python3", @"python", @"node", @"npm", @"npx", @"apk",
        @"vi", @"vim", @"nano", @"less", @"more", @"top", @"htop", @"ps", @"kill",
        @"pgrep", @"pkill", @"ping", @"nc", @"ssh", @"ssh-keygen", @"openssl", @"jq",
        @"tree", @"diff", @"patch", @"md5sum", @"sha256sum", @"base64", @"xxd", @"hexdump",
        @"bc", @"dialog", @"tmux", @"screen", @"man"
    ];
    for (NSString *t in tools) {
        NSString *p = [dir stringByAppendingPathComponent:t];
        if (![fm fileExistsAtPath:p]) {
            [fm createFileAtPath:p contents:[NSData data] attributes:nil];
        }
    }
}

static int kimi_boot(NSString *rootDir) {
    NSString *dataDir = [rootDir stringByAppendingPathComponent:@"data"];
    const char *src = dataDir.fileSystemRepresentation;

    int err = mount_root(&fakefs, src);
    if (err < 0) return err;

    err = become_first_process();
    if (err < 0) return err;

    current->thread = pthread_self();

    generic_mknodat(AT_PWD, "/dev/null",   S_IFCHR | 0666, dev_make(MEM_MAJOR, DEV_NULL_MINOR));
    generic_mknodat(AT_PWD, "/dev/zero",   S_IFCHR | 0666, dev_make(MEM_MAJOR, DEV_ZERO_MINOR));
    generic_mknodat(AT_PWD, "/dev/full",   S_IFCHR | 0666, dev_make(MEM_MAJOR, DEV_FULL_MINOR));
    generic_mknodat(AT_PWD, "/dev/random", S_IFCHR | 0666, dev_make(MEM_MAJOR, DEV_RANDOM_MINOR));
    generic_mknodat(AT_PWD, "/dev/urandom",S_IFCHR | 0666, dev_make(MEM_MAJOR, DEV_URANDOM_MINOR));
    generic_mknodat(AT_PWD, "/dev/tty",    S_IFCHR | 0666, dev_make(TTY_ALTERNATE_MAJOR, DEV_TTY_MINOR));
    generic_mknodat(AT_PWD, "/dev/console",S_IFCHR | 0666, dev_make(TTY_ALTERNATE_MAJOR, DEV_CONSOLE_MINOR));
    generic_mknodat(AT_PWD, "/dev/ptmx",   S_IFCHR | 0666, dev_make(TTY_ALTERNATE_MAJOR, DEV_PTMX_MINOR));
    generic_mkdirat(AT_PWD, "/dev/pts", 0755);
    generic_setattrat(AT_PWD, "/", (struct attr) {.type = attr_mode, .mode = 0755}, false);

    do_mount(&procfs, "proc", "/proc", "", 0);
    do_mount(&devptsfs, "devpts", "/dev/pts", "", 0);

    exit_hook = kimi_exit_hook;
    return 0;
}

@implementation KimiISH

+ (BOOL)available { return YES; }

+ (BOOL)prepare:(NSString **)error {
    dispatch_once(&g_bootOnce, ^{
        NSString *rootfsErr = nil;
        if (!kimi_ensure_rootfs(&rootfsErr)) {
            g_bootErr = -1;
            NSLog(@"[KimiISH] rootfs: %@", rootfsErr);
            return;
        }
        g_bootErr = kimi_boot(kimi_writable_root());
        if (g_bootErr < 0) {
            NSLog(@"[KimiISH] boot failed: %d", g_bootErr);
        } else {
            kimi_install_stubs();
        }
    });
    if (g_bootErr < 0) {
        if (error) *error = [NSString stringWithFormat:@"ish boot error %d", g_bootErr];
        return NO;
    }
    return YES;
}

+ (int)runCommand:(NSString *)command
           output:(uint8_t *)out
         capacity:(size_t)capacity
           length:(size_t *)length
          timeout:(double)timeout {
    if (length) *length = 0;
    if (![self prepare:NULL]) return -100;
    if (g_runLock == nil) {
        @synchronized(self) { if (g_runLock == nil) g_runLock = [NSLock new]; }
    }
    [g_runLock lock];
    int result = -101;
    int p[2] = { -1, -1 };

    if (pipe(p) < 0) goto done;
    fcntl(p[0], F_SETFL, fcntl(p[0], F_GETFL, 0) | O_NONBLOCK);

    struct task *saved = current;
    int err = become_new_init_child();
    if (err < 0) { current = saved; result = -102; goto done; }
    struct task *task = current;

    // stdin = /dev/null; stdout & stderr = pipe write end.
    struct fd *f0 = adhoc_fd_create(&realfs_fdops);
    if (f0) { f0->real_fd = open("/dev/null", O_RDONLY); task->files->files[0] = f0; }
    struct fd *f1 = adhoc_fd_create(&realfs_fdops);
    if (f1) { f1->real_fd = dup(p[1]); task->files->files[1] = f1; }
    struct fd *f2 = adhoc_fd_create(&realfs_fdops);
    if (f2) { f2->real_fd = dup(p[1]); task->files->files[2] = f2; }
    close(p[1]); p[1] = -1;

    const char *cmd = command.UTF8String ?: "";
    size_t clen = strlen(cmd);
    char *argv = malloc(clen + 32);
    if (!argv) { current = saved; result = -103; goto done; }
    size_t pos = 0;
    memcpy(argv + pos, "/bin/sh", 8); pos += 8;   // includes NUL
    memcpy(argv + pos, "-c", 3);      pos += 3;   // includes NUL
    memcpy(argv + pos, cmd, clen + 1); pos += clen + 1;
    argv[pos] = '\0';                             // final NUL (double-NUL end)

    static const char envp[] =
        "TERM=xterm-256color\0"
        "HOME=/root\0"
        "PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin\0"
        "PYTHONMALLOC=malloc\0"
        "NO_COLOR=1\0"
        "\0";

    err = do_execve("/bin/sh", 3, argv, envp);
    free(argv);
    if (err < 0) { current = saved; result = -104; goto done; }

    g_exitPid = task->pid;
    g_exitCode = -1;
    g_exitSem = dispatch_semaphore_create(0);

    err = task_start(task);
    current = saved;
    if (err < 0) { result = -105; goto done; }

    double to = timeout > 0 ? timeout : 120.0;
    long waited = dispatch_semaphore_wait(g_exitSem,
        dispatch_time(DISPATCH_TIME_NOW, (int64_t)(to * NSEC_PER_SEC)));
    if (waited != 0) { result = -106; goto done; }   // timed out

    // Drain captured output (write ends are gone once the task exited).
    size_t total = 0;
    for (int i = 0; i < 1000 && out != NULL && total < capacity; i++) {
        ssize_t n = read(p[0], out + total, capacity - total);
        if (n > 0) { total += (size_t)n; }
        else if (n == 0) { break; }
        else if (errno == EAGAIN || errno == EWOULDBLOCK) { usleep(500); }
        else { break; }
    }
    if (length) *length = total;

    int code = g_exitCode;
    result = (code & 0xff) ? (128 + (code & 0xff)) : ((code >> 8) & 0xff);

done:
    if (p[0] >= 0) close(p[0]);
    if (p[1] >= 0) close(p[1]);
    [g_runLock unlock];
    return result;
}

@end

#else  // no iSH headers/libs in this build

@implementation KimiISH
+ (BOOL)available { return NO; }
+ (BOOL)prepare:(NSString **)error {
    if (error) *error = @"built without iSH";
    return NO;
}
+ (int)runCommand:(NSString *)command
           output:(uint8_t *)out
         capacity:(size_t)capacity
           length:(size_t *)length
          timeout:(double)timeout {
    if (length) *length = 0;
    return -1;
}
@end

#endif