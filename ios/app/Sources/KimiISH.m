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
#include "fs/fake.h"
#include "fs/tty.h"
#include "kernel/signal.h"

static dispatch_once_t g_bootOnce;
static int g_bootErr = -9999;
static NSLock *g_runLock = nil;

static dispatch_semaphore_t g_exitSem = nil;
static volatile int g_exitPid = 0;
static volatile int g_exitCode = 0;

// Called by the iSH kernel when a guest task exits (on its own thread).
static void kimi_session_note_exit(struct task *task, int code);

static void kimi_exit_hook(struct task *task, int code) {
    if (task != NULL && task->pid == g_exitPid) {
        g_exitCode = code;
        dispatch_semaphore_t sem = g_exitSem;
        if (sem) dispatch_semaphore_signal(sem);
    }
    kimi_session_note_exit(task, code);
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

    // Share the KimiOS /disk tree (projects, configs, downloads) with the guest
    // and expose /root through /disk/home so OpenCode sees the native config.
    NSArray<NSString *> *docs =
        NSSearchPathForDirectoriesInDomains(NSDocumentDirectory, NSUserDomainMask, YES);
    NSString *disk = [docs.firstObject stringByAppendingPathComponent:@"disk"];
    NSString *home = [disk stringByAppendingPathComponent:@"home"];
    NSString *sysroot = [disk stringByAppendingPathComponent:@"alpine/root"];
    NSFileManager *fm = [NSFileManager defaultManager];
    [fm createDirectoryAtPath:home withIntermediateDirectories:YES attributes:nil error:nil];
    [fm createDirectoryAtPath:sysroot withIntermediateDirectories:YES attributes:nil error:nil];
    fakefs_bind_mount("/disk", disk.fileSystemRepresentation, false);
    fakefs_bind_mount("/root", home.fileSystemRepresentation, false);

    exit_hook = kimi_exit_hook;
    return 0;
}

// ---------------------------------------------------------------- PTY sessions
// The UI-facing terminal is a fake slave tty whose driver callbacks feed a
// bounded in-memory byte queue. The guest shell/process gets this tty as
// stdin/stdout/stderr, so ANSI output streams immediately and keystrokes go
// through tty_input() without a host pipe or a 120-second one-shot wrapper.

#define KIMI_PTY_BUF (4u * 1024u * 1024u)

typedef struct kimi_pty_session {
    uint64_t id;
    struct tty *tty;
    int pid;
    int code;
    bool exited;
    bool closed;
    bool released;
    char *out;
    size_t out_len;
    pthread_mutex_t lock;
    struct kimi_pty_session *next;
} kimi_pty_session;

static kimi_pty_session *g_sessions = NULL;
static pthread_mutex_t g_sessions_lock = PTHREAD_MUTEX_INITIALIZER;
static pthread_mutex_t g_spawn_lock = PTHREAD_MUTEX_INITIALIZER;
static uint64_t g_next_session = 0x4953485F00000001ULL;

static void kimi_session_append(kimi_pty_session *s, const void *buf, size_t len) {
    if (s == NULL || buf == NULL || len == 0 || s->out == NULL) return;
    pthread_mutex_lock(&s->lock);
    if (len >= KIMI_PTY_BUF) {
        memcpy(s->out, (const char *)buf + (len - KIMI_PTY_BUF), KIMI_PTY_BUF);
        s->out_len = KIMI_PTY_BUF;
    } else {
        if (s->out_len + len > KIMI_PTY_BUF) {
            size_t drop = s->out_len + len - KIMI_PTY_BUF;
            memmove(s->out, s->out + drop, s->out_len - drop);
            s->out_len -= drop;
        }
        memcpy(s->out + s->out_len, buf, len);
        s->out_len += len;
    }
    pthread_mutex_unlock(&s->lock);
}

static int kimi_tty_init(struct tty *tty) {
    kimi_pty_session *s = calloc(1, sizeof(*s));
    if (s == NULL) return -1;
    s->out = malloc(KIMI_PTY_BUF);
    if (s->out == NULL) { free(s); return -1; }
    pthread_mutex_init(&s->lock, NULL);
    s->tty = tty;
    tty->data = s;
    return 0;
}

static int kimi_tty_write(struct tty *tty, const void *buf, size_t len, bool blocking) {
    (void)blocking;
    kimi_session_append((kimi_pty_session *)tty->data, buf, len);
    return (int)len;
}

static void kimi_tty_cleanup(struct tty *tty) {
    kimi_pty_session *s = (kimi_pty_session *)tty->data;
    if (s != NULL) {
        pthread_mutex_lock(&s->lock);
        s->tty = NULL;
        s->released = true;
        if (!s->exited) { s->exited = true; s->code = 0; }
        pthread_mutex_unlock(&s->lock);
        tty->data = NULL;
    }
}

static struct tty_driver_ops kimi_tty_ops = {
    .init = kimi_tty_init,
    .write = kimi_tty_write,
    .cleanup = kimi_tty_cleanup,
};
static struct tty_driver kimi_pty_driver = {.ops = &kimi_tty_ops};

static void kimi_hangup_release(struct tty *tty) {
    if (tty == NULL) return;
    lock(&tty->lock);
    tty_hangup(tty);
    unlock(&tty->lock);
    tty_release(tty);
}

static void kimi_session_note_exit(struct task *task, int code) {
    if (task == NULL) return;
    pthread_mutex_lock(&g_sessions_lock);
    for (kimi_pty_session *s = g_sessions; s != NULL; s = s->next) {
        if (s->pid == task->pid) {
            pthread_mutex_lock(&s->lock);
            s->exited = true;
            s->code = code;
            pthread_mutex_unlock(&s->lock);
            break;
        }
    }
    pthread_mutex_unlock(&g_sessions_lock);
}

static kimi_pty_session *kimi_session_find(uint64_t id) {
    kimi_pty_session *found = NULL;
    pthread_mutex_lock(&g_sessions_lock);
    for (kimi_pty_session *s = g_sessions; s != NULL; s = s->next) {
        if (s->id == id) { found = s; break; }
    }
    pthread_mutex_unlock(&g_sessions_lock);
    return found;
}

int kimi_pty_spawn(const char *command, const char *cwd, int rows, int cols, uint64_t *slot) {
    if (command == NULL || slot == NULL || ![KimiISH available]) return -1;
    if (![KimiISH prepare:NULL]) return -2;
    pthread_mutex_lock(&g_spawn_lock);
    struct task *saved = current;
    int err = become_new_init_child();
    if (err < 0) { current = saved; pthread_mutex_unlock(&g_spawn_lock); return err; }

    struct tty *tty = pty_open_fake(&kimi_pty_driver);
    if (IS_ERR(tty)) {
        int e = (int)PTR_ERR(tty);
        current = saved;
        pthread_mutex_unlock(&g_spawn_lock);
        return e;
    }
    kimi_pty_session *s = (kimi_pty_session *)tty->data;
    char stdio[64];
    snprintf(stdio, sizeof(stdio), "/dev/pts/%d", tty->num);
    err = create_stdio(stdio, TTY_PSEUDO_SLAVE_MAJOR, tty->num);
    if (err < 0) {
        tty_release(tty);
        current = saved;
        pthread_mutex_unlock(&g_spawn_lock);
        return err;
    }

    tty_set_winsize(tty, (struct winsize_) {
        .row = (word_t)(rows > 0 ? rows : 24),
        .col = (word_t)(cols > 0 ? cols : 80),
    });
    // The app/agent sets its own raw mode; start without echo/newline mangling.
    tty->termios.iflags = 0;
    tty->termios.oflags = 0;
    tty->termios.lflags = 0;

    const char *cd = (cwd != NULL && cwd[0] != '\0') ? cwd : "/";
    char wrapped[512];
    snprintf(wrapped, sizeof(wrapped),
             "cd '%s' 2>/dev/null || cd /; exec /bin/sh -c \"$KIMI_COMMAND\"", cd);
    char *env_cmd = malloc(strlen(command) + 16);
    if (env_cmd == NULL) {
        kimi_hangup_release(tty);
        current = saved;
        pthread_mutex_unlock(&g_spawn_lock);
        return -1;
    }
    snprintf(env_cmd, strlen(command) + 16, "KIMI_COMMAND=%s", command);
    const char *envp[] = {
        "TERM=xterm-256color",
        "HOME=/root",
        "PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin",
        "COLORTERM=truecolor",
        "LANG=C.UTF-8",
        env_cmd,
        NULL,
    };
    char *argv[] = { "/bin/sh", "-c", wrapped, NULL };
    err = do_execve("/bin/sh", 3, argv, envp);
    free(env_cmd);
    if (err < 0) {
        kimi_hangup_release(tty);
        current = saved;
        pthread_mutex_unlock(&g_spawn_lock);
        return err;
    }

    s->pid = current->pid;
    s->id = g_next_session++;
    pthread_mutex_lock(&g_sessions_lock);
    s->next = g_sessions;
    g_sessions = s;
    pthread_mutex_unlock(&g_sessions_lock);

    err = task_start(current);
    current = saved;
    pthread_mutex_unlock(&g_spawn_lock);
    if (err < 0) {
        tty_hangup(tty);
        tty_release(tty);
        return err;
    }
    *slot = s->id;
    return 0;
}

long kimi_pty_read(uint64_t slot, void *buf, size_t cap) {
    kimi_pty_session *s = kimi_session_find(slot);
    if (s == NULL || buf == NULL) return -1;
    pthread_mutex_lock(&s->lock);
    size_t n = s->out_len < cap ? s->out_len : cap;
    if (n > 0) {
        memcpy(buf, s->out, n);
        memmove(s->out, s->out + n, s->out_len - n);
        s->out_len -= n;
    }
    pthread_mutex_unlock(&s->lock);
    return (long)n;
}

long kimi_pty_write(uint64_t slot, const void *buf, size_t len) {
    kimi_pty_session *s = kimi_session_find(slot);
    if (s == NULL || buf == NULL) return -1;
    pthread_mutex_lock(&s->lock);
    struct tty *tty = s->tty;
    pthread_mutex_unlock(&s->lock);
    if (tty == NULL) return 0;
    return tty_input(tty, buf, len, false);
}

int kimi_pty_resize(uint64_t slot, int rows, int cols) {
    kimi_pty_session *s = kimi_session_find(slot);
    if (s == NULL || rows <= 0 || cols <= 0) return -1;
    pthread_mutex_lock(&s->lock);
    struct tty *tty = s->tty;
    pthread_mutex_unlock(&s->lock);
    if (tty == NULL) return -1;
    lock(&tty->lock);
    tty_set_winsize(tty, (struct winsize_) {.row = (word_t)rows, .col = (word_t)cols});
    unlock(&tty->lock);
    return 0;
}

int kimi_pty_status(uint64_t slot) {
    kimi_pty_session *s = kimi_session_find(slot);
    if (s == NULL) return -1;
    pthread_mutex_lock(&s->lock);
    bool exited = s->exited;
    int code = s->code;
    pthread_mutex_unlock(&s->lock);
    return exited ? code + 1 : 0;
}

int kimi_pty_close(uint64_t slot) {
    kimi_pty_session *s = kimi_session_find(slot);
    if (s == NULL) return -1;
    pthread_mutex_lock(&s->lock);
    if (s->closed) { pthread_mutex_unlock(&s->lock); return 0; }
    s->closed = true;
    int pid = s->pid;
    struct tty *tty = s->tty;
    s->tty = NULL;
    pthread_mutex_unlock(&s->lock);

    if (pid > 0) {
        struct task *task = pid_get_task((dword_t)pid);
        if (task != NULL) send_signal(task, SIGKILL_, SIGINFO_NIL);
    }
    if (tty != NULL) {
        lock(&tty->lock);
        tty_hangup(tty);
        unlock(&tty->lock);
        tty_release(tty);
    }
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
    uint64_t slot = 0;
    const char *cmd = command.UTF8String ?: "";
    int err = kimi_pty_spawn(cmd, "/disk/alpine/root", 24, 80, &slot);
    if (err < 0) return err;

    size_t total = 0;
    double to = timeout > 0 ? timeout : 120.0;
    NSDate *deadline = [NSDate dateWithTimeIntervalSinceNow:to];
    for (;;) {
        uint8_t tmp[8192];
        long n = kimi_pty_read(slot, tmp, sizeof(tmp));
        if (n > 0 && out != NULL && total < capacity) {
            size_t take = (size_t)n;
            if (take > capacity - total) take = capacity - total;
            memcpy(out + total, tmp, take);
            total += take;
        }
        int st = kimi_pty_status(slot);
        if (st > 0) {
            while ((n = kimi_pty_read(slot, tmp, sizeof(tmp))) > 0 &&
                   out != NULL && total < capacity) {
                size_t take = (size_t)n;
                if (take > capacity - total) take = capacity - total;
                memcpy(out + total, tmp, take);
                total += take;
            }
            if (length) *length = total;
            int code = st - 1;
            kimi_pty_close(slot);
            return (code & 0xff) ? (128 + (code & 0xff)) : ((code >> 8) & 0xff);
        }
        if ([deadline timeIntervalSinceNow] <= 0) {
            kimi_pty_close(slot);
            if (length) *length = total;
            return -106;
        }
        usleep(2000);
    }
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
