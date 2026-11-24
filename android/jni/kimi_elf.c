// KimiOS libOS — native Linux ELF execution on the Android host kernel.
//
// Strategy (see docs/research): Android IS a Linux kernel. We fork a child,
// map a dynamically-linked musl aarch64 ELF + its PT_INTERP (ld-musl) into the
// child address space with mmap(NULL) reservations, build a SysV aarch64
// startup stack + auxv, and jump to the interpreter entry. The guest then
// issues REAL `svc #0` syscalls serviced by the Android kernel itself, so
// threads/futex/epoll/sockets/mmap/mprotect/dlopen and /proc all work natively.
//
// Three mandatory mitigations:
//   1. SIGSYS->ENOSYS shim: Android's app seccomp filter answers blocked probed
//      syscalls (io_uring_setup, faccessat2, fchmodat2, clone3, ...) with
//      SECCOMP_RET_TRAP (SIGSYS = process death), NOT ENOSYS. musl/libuv probe
//      these expecting ENOSYS fallback; without the shim node dies at startup.
//   2. targetSdk 28 (untrusted_app_27): grants execmod + sandbox execve.
//   3. 4 KiB pages: musl ldso file-mmaps 4 KiB-aligned Alpine libs.
//
// Forked child (not in-process thread): exit_group kills only the child, no
// TPIDR_EL0/bionic TLS conflict, no ART sigchain clobbering by V8. The child
// uses RAW syscalls only after fork (multithreaded-fork safety: no malloc/stdio).

#define _GNU_SOURCE
#ifndef KIMI_ELF_NO_JNI
#include <jni.h>
#endif
#include <android/log.h>
#include <stdint.h>
#include <stddef.h>
#include <stdio.h>
#include <string.h>
#include <signal.h>
#include <ucontext.h>
#include <sys/auxv.h>
#include <unistd.h>
#include <fcntl.h>
#include <sys/mman.h>
#include <sys/wait.h>
#include <sys/random.h>
#include <sys/ioctl.h>
#include <termios.h>
#include <errno.h>
#include <pthread.h>
#include <stdlib.h>

#define LOG_TAG "KimiOS-elf"
#define LOGI(...) __android_log_print(ANDROID_LOG_INFO, LOG_TAG, __VA_ARGS__)
#define LOGE(...) __android_log_print(ANDROID_LOG_ERROR, LOG_TAG, __VA_ARGS__)

#ifndef SYS_SECCOMP
#define SYS_SECCOMP 1
#endif

// ---------------- aarch64 raw syscalls (fork-safe) ----------------
#define SYS_io_uring_setup 425
#define SYS_io_uring_enter 426
#define SYS_io_uring_register 427
#define SYS_clone3 435
#define SYS_openat2 437
#define SYS_faccessat2 439
#define SYS_epoll_pwait2 441
#define SYS_fchmodat2 452
#define SYS_openat 56
#define SYS_close 57
#define SYS_read 63
#define SYS_write 64
#define SYS_pread64 67
#define SYS_exit 93
#define SYS_exit_group 94
#define SYS_rt_sigaction 134
#define SYS_rt_sigprocmask 135
#define SYS_ioctl 29
#define SYS_setsid 157
#define SYS_dup3 24
#define SYS_chdir 49
#define SYS_mmap 222
#define SYS_mprotect 226
#define SYS_getrandom 278
#ifndef AT_FDCWD
#define AT_FDCWD (-100)
#endif

static inline long k_sys(long n, long a, long b, long c, long d, long e, long f) {
    register long x8 __asm__("x8") = n;
    register long x0 __asm__("x0") = a;
    register long x1 __asm__("x1") = b;
    register long x2 __asm__("x2") = c;
    register long x3 __asm__("x3") = d;
    register long x4 __asm__("x4") = e;
    register long x5 __asm__("x5") = f;
    __asm__ volatile("svc #0"
                     : "+r"(x0)
                     : "r"(x1), "r"(x2), "r"(x3), "r"(x4), "r"(x5), "r"(x8)
                     : "memory", "cc");
    return x0;
}

// ---------------- ELF64 ----------------
#include <elf.h>
#ifndef PT_PHDR
#define PT_PHDR 6
#endif
#ifndef EM_AARCH64
#define EM_AARCH64 183
#endif

// captured in parent before fork (async-signal-safe to read in child)
static uint64_t g_hwcap, g_hwcap2, g_pagesz = 4096;
static uint64_t g_uid, g_gid;
static uint8_t g_random[16];

typedef struct {
    uint64_t entry; // absolute interpreter (or exe) entry
    uint64_t sp;    // initial stack pointer (points at argc)
} kimi_image;

static long k_pread(long fd, void *buf, long n, uint64_t off) {
    long got = 0;
    while (got < n) {
        long r = k_sys(SYS_pread64, fd, (long)((uint8_t *)buf + got), n - got,
                       (long)(off + got), 0, 0);
        if (r <= 0) break;
        got += r;
    }
    return got;
}

// Map one ELF (exe or interpreter) at a fresh mmap(NULL) reservation.
static int load_one(const char *path, uint64_t *base_out, uint64_t *entry_out,
                    uint64_t *phdr_out, uint64_t *phent_out, uint64_t *phnum_out,
                    char *interp_out, long interp_cap) {
    long fd = k_sys(SYS_openat, AT_FDCWD, (long)path, O_RDONLY, 0, 0, 0);
    if (fd < 0) { LOGE("open %s -> %ld", path, fd); return -1; }
    Elf64_Ehdr eh;
    if (k_pread(fd, &eh, sizeof eh, 0) != (long)sizeof eh) { k_sys(SYS_close, fd, 0, 0, 0, 0, 0); return -1; }
    if (eh.e_ident[0] != 0x7f || eh.e_ident[1] != 'E' || eh.e_ident[2] != 'L' ||
        eh.e_ident[3] != 'F' || eh.e_ident[4] != 2 || eh.e_ident[5] != 1 ||
        eh.e_machine != EM_AARCH64 || (eh.e_type != ET_DYN && eh.e_type != ET_EXEC)) {
        k_sys(SYS_close, fd, 0, 0, 0, 0, 0); LOGE("not aarch64 ELF: %s", path); return -1;
    }
    const uint64_t PG = g_pagesz;
    static uint8_t phbuf[16384];
    long phsz = (long)eh.e_phnum * eh.e_phentsize;
    if (phsz > (long)sizeof phbuf) phsz = sizeof phbuf;
    if (k_pread(fd, phbuf, phsz, eh.e_phoff) != phsz) { k_sys(SYS_close, fd, 0, 0, 0, 0, 0); return -1; }
    Elf64_Phdr *ph = (Elf64_Phdr *)phbuf;
    int nph = eh.e_phnum;

    uint64_t lo = ~0ull, hi = 0;
    for (int i = 0; i < nph; i++) {
        if (ph[i].p_type != PT_LOAD || ph[i].p_memsz == 0) continue;
        uint64_t s = ph[i].p_vaddr & ~(PG - 1);
        uint64_t e = (ph[i].p_vaddr + ph[i].p_memsz + PG - 1) & ~(PG - 1);
        if (s < lo) lo = s;
        if (e > hi) hi = e;
    }
    if (lo == ~0ull) { k_sys(SYS_close, fd, 0, 0, 0, 0, 0); return -1; }
    void *res = (void *)k_sys(SYS_mmap, 0, (long)(hi - lo), PROT_NONE,
                              MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    if ((long)res < 0) { k_sys(SYS_close, fd, 0, 0, 0, 0, 0); LOGE("reserve mmap failed"); return -1; }
    uint64_t base = (uint64_t)res - lo;

    uint64_t phdr_va = 0;
    int found_phdr = 0;
    for (int i = 0; i < nph; i++) {
        if (ph[i].p_type == PT_PHDR) { phdr_va = base + ph[i].p_vaddr; found_phdr = 1; }
        if (ph[i].p_type == PT_INTERP && interp_out && interp_cap > 0) {
            long n = (long)ph[i].p_filesz;
            if (n >= interp_cap) n = interp_cap - 1;
            k_pread(fd, interp_out, n, ph[i].p_offset);
            interp_out[n] = 0;
        }
        if (ph[i].p_type != PT_LOAD || ph[i].p_memsz == 0) continue;
        uint64_t va = base + ph[i].p_vaddr;
        uint64_t pg_start = va & ~(PG - 1);
        uint64_t pg_end = (va + ph[i].p_memsz + PG - 1) & ~(PG - 1);
        uint64_t off_page = ph[i].p_offset & ~(PG - 1);
        int prot = (ph[i].p_flags & PF_R ? PROT_READ : 0) |
                   (ph[i].p_flags & PF_W ? PROT_WRITE : 0) |
                   (ph[i].p_flags & PF_X ? PROT_EXEC : 0);
        if (!found_phdr && ph[i].p_offset <= eh.e_phoff &&
            eh.e_phoff < ph[i].p_offset + ph[i].p_filesz) {
            phdr_va = va + (eh.e_phoff - ph[i].p_offset);
            found_phdr = 1;
        }
        if (ph[i].p_filesz) {
            uint64_t file_end = (va + ph[i].p_filesz + PG - 1) & ~(PG - 1);
            void *m = (void *)k_sys(SYS_mmap, (long)pg_start, (long)(file_end - pg_start),
                                    prot, MAP_PRIVATE | MAP_FIXED, fd, (long)off_page);
            if ((long)m < 0) { k_sys(SYS_close, fd, 0, 0, 0, 0, 0); LOGE("seg mmap failed"); return -1; }
            if (ph[i].p_memsz > ph[i].p_filesz) {
                uint64_t bss_start = va + ph[i].p_filesz;
                uint64_t ze = (va + ph[i].p_memsz) < file_end ? (va + ph[i].p_memsz) : file_end;
                if (ze > bss_start && (prot & PROT_WRITE))
                    memset((void *)bss_start, 0, (size_t)(ze - bss_start));
                if (pg_end > file_end)
                    k_sys(SYS_mmap, (long)file_end, (long)(pg_end - file_end), prot,
                          MAP_PRIVATE | MAP_ANONYMOUS | MAP_FIXED, -1, 0);
            }
        } else {
            k_sys(SYS_mmap, (long)pg_start, (long)(pg_end - pg_start), prot,
                  MAP_PRIVATE | MAP_ANONYMOUS | MAP_FIXED, -1, 0);
        }
    }
    k_sys(SYS_close, fd, 0, 0, 0, 0, 0);
    *base_out = base;
    *entry_out = base + eh.e_entry;
    *phdr_out = phdr_va;
    *phent_out = eh.e_phentsize;
    *phnum_out = eh.e_phnum;
    return 0;
}

// ---------------- SysV aarch64 startup stack + auxv ----------------
#define STACK_SZ (8u * 1024u * 1024u)
#define FRAME_BUF 16384

static uint64_t build_stack(uint64_t stack_top, uint64_t phdr, uint64_t phent,
                            uint64_t phnum, uint64_t entry, uint64_t at_base,
                            char *const argv[], char *const envp[]) {
    static uint8_t buf[FRAME_BUF];
    memset(buf, 0, sizeof buf);
    long cur = FRAME_BUF;
    int argc = 0, envc = 0;
    while (argv && argv[argc] && argc < 64) argc++;
    while (envp && envp[envc] && envc < 128) envc++;
    if (argc < 1) argc = 1;

    cur -= 16;
    long random_off = cur;
    memcpy(buf + cur, g_random, 16);
    const char *plat = "aarch64";
    cur -= (long)strlen(plat) + 1;
    memcpy(buf + cur, plat, strlen(plat) + 1);

    // Lay argv then envp strings ASCENDING and contiguous (kernel convention).
    // libuv's uv_set_process_title (node `process.title`, set by npm) computes
    // the argv block size as argv[last]+len - argv[0]; a descending layout
    // underflows it to a negative memset size and crashes. argv[0] must be the
    // lowest string address, with argv[1..] and envp[0..] following upward.
    long env_off[128];
    long arg_off[64];
    long strbytes = 0;
    for (int i = 0; i < argc; i++) strbytes += (long)strlen(argv[i] ? argv[i] : "") + 1;
    for (int i = 0; i < envc; i++) strbytes += (long)strlen(envp[i]) + 1;
    long pos = cur - strbytes;
    for (int i = 0; i < argc; i++) {
        const char *s = argv[i] ? argv[i] : "";
        long l = (long)strlen(s) + 1;
        arg_off[i] = pos; memcpy(buf + pos, s, l); pos += l;
    }
    for (int i = 0; i < envc; i++) {
        long l = (long)strlen(envp[i]) + 1;
        env_off[i] = pos; memcpy(buf + pos, envp[i], l); pos += l;
    }
    cur -= strbytes;

    // auxv pairs (terminated by AT_NULL)
    uint64_t auxv[][2] = {
        {3, phdr}, {4, phent}, {5, phnum}, {6, g_pagesz}, {7, at_base},
        {8, 0}, {9, entry}, {11, g_uid}, {12, g_uid}, {13, g_gid}, {14, g_gid},
        {16, g_hwcap}, {26, g_hwcap2}, {17, 100}, {23, 0},
        {25, 0 /*random va, fixed below*/}, {31, 0 /*execfn va*/}, {0, 0},
    };
    int nauxv = (int)(sizeof auxv / sizeof auxv[0]);

    cur &= ~15l;
    long structured = 8 + 8 * (argc + 1) + 8 * (envc + 1) + 16 * nauxv;
    if ((cur - structured) & 15) cur -= 8;

    uint64_t base = stack_top - FRAME_BUF;
    uint64_t random_va = base + (uint64_t)random_off;
    uint64_t argv0_va = base + (uint64_t)arg_off[0];
    for (int i = 0; i < nauxv; i++) {
        if (auxv[i][0] == 25) auxv[i][1] = random_va;
        if (auxv[i][0] == 31) auxv[i][1] = argv0_va;
    }

    cur -= 16 * nauxv;
    long auxv_off = cur;
    for (int i = 0; i < nauxv; i++) {
        memcpy(buf + auxv_off + i * 16, &auxv[i][0], 8);
        memcpy(buf + auxv_off + i * 16 + 8, &auxv[i][1], 8);
    }
    cur -= 8; // envp NULL terminator (buf zeroed)
    cur -= 8 * envc;
    long envp_off = cur;
    for (int i = 0; i < envc; i++) {
        uint64_t va = base + (uint64_t)env_off[i];
        memcpy(buf + envp_off + i * 8, &va, 8);
    }
    cur -= 8; // argv NULL terminator
    cur -= 8 * argc;
    long argv_off = cur;
    for (int i = 0; i < argc; i++) {
        uint64_t va = base + (uint64_t)arg_off[i];
        memcpy(buf + argv_off + i * 8, &va, 8);
    }
    cur -= 8;
    long argc_off = cur;
    uint64_t ac = (uint64_t)argc;
    memcpy(buf + argc_off, &ac, 8);

    uint64_t sp = base + (uint64_t)argc_off;
    memcpy((void *)(stack_top - (FRAME_BUF - cur)), buf + cur, (size_t)(FRAME_BUF - cur));
    return sp;
}

// ---------------- SIGSYS -> ENOSYS shim ----------------
// Guest signal handlers MUST be installed with raw rt_sigaction + SA_RESTORER,
// NOT bionic sigaction(): bionic wraps handlers in its sigchain dispatcher,
// which reads bionic TLS via TPIDR_EL0. The guest thread's TPIDR_EL0 belongs to
// musl, so the sigchain dispatcher null-derefs (fault 0x308) the moment any
// signal (e.g. the io_uring SIGSYS probe) is delivered. A raw kernel handler
// with our own rt_sigreturn restorer bypasses sigchain; the handler itself is
// compiled no_stack_protector so it never touches TPIDR-relative TLS.
#define SA_RESTORER 0x04000000UL
#define K_SA_SIGINFO 4UL
struct k_sigaction {
    void *handler;
    unsigned long flags;
    void *restorer;
    uint64_t mask;
};
__attribute__((naked)) static void k_sigreturn(void) {
    __asm__ volatile("mov x8, #139\n\tsvc #0\n\t"); // rt_sigreturn
}
static void k_install_sig(int sig, void *handler) {
    struct k_sigaction ksa;
    ksa.handler = handler;
    ksa.flags = K_SA_SIGINFO | SA_RESTORER;
    ksa.restorer = (void *)k_sigreturn;
    ksa.mask = 0;
    k_sys(SYS_rt_sigaction, sig, (long)&ksa, 0, 8, 0, 0);
}
static void k_sig_default(int sig) {
    struct k_sigaction dfl;
    dfl.handler = (void *)0; // SIG_DFL
    dfl.flags = 0;
    dfl.restorer = 0;
    dfl.mask = 0;
    k_sys(SYS_rt_sigaction, sig, (long)&dfl, 0, 8, 0, 0);
}

__attribute__((no_stack_protector))
static void sigsys_handler(int sig, siginfo_t *si, void *ucv) {
    if (si->si_code == SYS_SECCOMP) {
        int nr = si->si_syscall;
        if (nr == SYS_io_uring_setup || nr == SYS_io_uring_enter ||
            nr == SYS_io_uring_register || nr == SYS_openat2 ||
            nr == SYS_faccessat2 || nr == SYS_fchmodat2 || nr == SYS_clone3 ||
            nr == SYS_epoll_pwait2 || nr == 334 /*statx on old kernels*/) {
            ucontext_t *uc = (ucontext_t *)ucv;
            uc->uc_mcontext.regs[0] = (uint64_t)(-38); // -ENOSYS
            return;
        }
    }
    k_sig_default(sig); // unhandled: restore default, return -> re-fatal
}

// ---------------- guest fatal-signal diagnostics ----------------
static char *hex64(char *p, uint64_t v) {
    static const char d[] = "0123456789abcdef";
    *p++ = '0'; *p++ = 'x';
    for (int i = 60; i >= 0; i -= 4) *p++ = d[(v >> i) & 15];
    return p;
}
__attribute__((no_stack_protector))
static void fatal_handler(int sig, siginfo_t *si, void *ucv) {
    ucontext_t *uc = (ucontext_t *)ucv;
    char buf[192];
    char *p = buf;
    const char *m = "GUEST-FATAL sig=";
    while (*m) *p++ = *m++;
    *p++ = '0' + (sig / 10 % 10); *p++ = '0' + (sig % 10);
    m = " addr="; while (*m) *p++ = *m++;
    p = hex64(p, (uint64_t)(uintptr_t)si->si_addr);
    m = " pc="; while (*m) *p++ = *m++;
    p = hex64(p, (uint64_t)uc->uc_mcontext.pc);
    *p++ = '\n';
    k_sys(SYS_write, 2, (long)buf, (long)(p - buf), 0, 0, 0);
    k_sig_default(sig);
}
static void kimi_fatal_diagnose_install(void) {
    int sigs[] = { SIGSEGV, SIGBUS, SIGILL, SIGABRT, SIGFPE };
    for (unsigned i = 0; i < sizeof sigs / sizeof sigs[0]; i++)
        k_install_sig(sigs[i], (void *)fatal_handler);
}

static void install_sigsys_shim(void) {
    k_install_sig(SIGSYS, (void *)sigsys_handler);
    kimi_fatal_diagnose_install();
}

// ---------------- load + enter ----------------
static int kimi_elf_load(const char *exe, const char *sysroot,
                         char *const argv[], char *const envp[], kimi_image *img) {
    uint64_t ebase, eentry, ephdr, ephent, ephnum;
    char interp[256];
    interp[0] = 0;
    if (load_one(exe, &ebase, &eentry, &ephdr, &ephent, &ephnum, interp, sizeof interp) != 0)
        return -1;
    uint64_t ibase = 0, ientry = eentry, iphdr = 0, iphent = 0, iphnum = 0;
    if (interp[0]) {
        char ipath[512];
        long sl = (long)strlen(sysroot);
        if (sl > 400) sl = 400;
        memcpy(ipath, sysroot, sl);
        long il = (long)strlen(interp);
        if (sl + il >= (long)sizeof ipath) il = (long)sizeof ipath - sl - 1;
        memcpy(ipath + sl, interp, il);
        ipath[sl + il] = 0;
        if (load_one(ipath, &ibase, &ientry, &iphdr, &iphent, &iphnum, NULL, 0) != 0)
            return -1;
        LOGI("loaded exe base=%#lx interp=%s base=%#lx", (unsigned long)ebase, ipath, (unsigned long)ibase);
    }
    void *st = (void *)k_sys(SYS_mmap, 0, STACK_SZ + g_pagesz, PROT_NONE,
                             MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    if ((long)st < 0) return -1;
    k_sys(SYS_mprotect, (long)((uint8_t *)st + g_pagesz), STACK_SZ,
          PROT_READ | PROT_WRITE, 0, 0, 0);
    uint64_t stack_top = (uint64_t)st + g_pagesz + STACK_SZ;
    uint64_t sp = build_stack(stack_top, ephdr, ephent, ephnum, eentry, ibase, argv, envp);
    img->entry = interp[0] ? ientry : eentry;
    img->sp = sp;
    return 0;
}

static void kimi_elf_enter(const kimi_image *img) {
    uint64_t empty = 0;
    k_sys(SYS_rt_sigprocmask, 0 /*SIG_SETMASK*/, (long)&empty, 0, 8, 0, 0);
    __asm__ volatile("mov sp, %0\n\tbr %1" ::"r"(img->sp), "r"(img->entry) : "memory", "cc");
    __builtin_unreachable();
}

// ---------------- parent orchestration ----------------
pid_t kimi_guest_spawn(const char *exe, const char *sysroot,
                       char *const argv[], char *const envp[],
                       int *out_stdin, int *out_stdout) {
    g_hwcap = getauxval(AT_HWCAP);
    g_hwcap2 = getauxval(AT_HWCAP2);
    long ps = sysconf(_SC_PAGESIZE);
    if (ps > 0) g_pagesz = (uint64_t)ps;
    g_uid = getuid();
    g_gid = getgid();
    if (k_sys(SYS_getrandom, (long)g_random, sizeof g_random, 0, 0, 0, 0) != (long)sizeof g_random)
        for (int i = 0; i < 16; i++) g_random[i] = (uint8_t)(i * 7 + 13);
    if (g_pagesz != 4096)
        LOGE("WARNING: page size %lu != 4096; musl 4KiB-aligned libs may fail", (unsigned long)g_pagesz);

    int inpipe[2], outpipe[2];
    if (pipe2(inpipe, O_CLOEXEC) != 0 || pipe2(outpipe, O_CLOEXEC) != 0) return -1;
    pid_t pid = fork();
    if (pid == 0) {
        // child: raw syscalls only from here
        k_sys(SYS_close, inpipe[1], 0, 0, 0, 0, 0);
        k_sys(SYS_close, outpipe[0], 0, 0, 0, 0, 0);
        k_sys(SYS_dup3, inpipe[0], 0, 0, 0, 0, 0);
        k_sys(SYS_dup3, outpipe[1], 1, 0, 0, 0, 0);
        k_sys(SYS_dup3, outpipe[1], 2, 0, 0, 0, 0);
        k_sys(SYS_close, inpipe[0], 0, 0, 0, 0, 0);
        k_sys(SYS_close, outpipe[1], 0, 0, 0, 0, 0);
        for (int fd = 3; fd < 1024; fd++) k_sys(SYS_close, fd, 0, 0, 0, 0, 0);
        install_sigsys_shim();
        // cwd = sysroot so musl reads the relative "etc/resolv.conf" shim
        // (Android has no /etc/resolv.conf; see tools/prepare_sysroot.py).
        k_sys(SYS_chdir, (long)sysroot, 0, 0, 0, 0, 0);
        kimi_image img;
        if (kimi_elf_load(exe, sysroot, argv, envp, &img) != 0)
            k_sys(SYS_exit, 127, 0, 0, 0, 0, 0);
        kimi_elf_enter(&img);
        k_sys(SYS_exit, 127, 0, 0, 0, 0, 0);
    }
    k_sys(SYS_close, inpipe[0], 0, 0, 0, 0, 0);
    k_sys(SYS_close, outpipe[1], 0, 0, 0, 0, 0);
    if (out_stdin) *out_stdin = inpipe[1];
    if (out_stdout) *out_stdout = outpipe[0];
    return pid;
}

int kimi_guest_wait(pid_t pid, int *status) {
    int st = 0;
    long r = wait4(pid, &st, 0, NULL);
    if (status) *status = st;
    return (int)r;
}

// Synchronous run: capture guest stdout/stderr into `out` (cap bytes), wait for
// exit, report length + code. Used by the shell's SYS_LINUX_RUN path.
int kimi_guest_capture(const char *exe, const char *sysroot, char *const argv[],
                       char *const envp[], char *out, size_t cap, size_t *out_len,
                       int *code) {
    int in = -1, of = -1;
    pid_t pid = kimi_guest_spawn(exe, sysroot, argv, envp, &in, &of);
    if (pid < 0) return -1;
    if (in >= 0) close(in);
    size_t total = 0;
    ssize_t r;
    if (cap > 0) {
        while (total < cap - 1 && (r = read(of, out + total, cap - 1 - total)) > 0)
            total += (size_t)r;
    }
    char scratch[4096];
    while ((r = read(of, scratch, sizeof scratch)) > 0) {} // drain so guest never blocks
    close(of);
    int st = 0;
    kimi_guest_wait(pid, &st);
    if (cap > 0) out[total] = 0;
    if (out_len) *out_len = total;
    int c = WIFEXITED(st) ? WEXITSTATUS(st) : (128 + WTERMSIG(st));
    if (code) *code = c;
    return c;
}

// ---------------- pseudoterminal sessions ----------------
//
// Pipes are enough for `node -e`; they are not enough for a TUI.  opencode (and
// vim/htop/lazygit) check isatty() and switch to the alternate screen only when
// stdout is a terminal.  This spawns the same fork+mmap guest behind a pts so
// the guest sees a real tty and can receive terminal resize events.

int kimi_guest_spawn_pty(const char *exe, const char *sysroot,
                         char *const argv[], char *const envp[],
                         int cols, int rows, int *master_out) {
    g_hwcap = getauxval(AT_HWCAP);
    g_hwcap2 = getauxval(AT_HWCAP2);
    long ps = sysconf(_SC_PAGESIZE);
    if (ps > 0) g_pagesz = (uint64_t)ps;
    g_uid = getuid();
    g_gid = getgid();
    if (k_sys(SYS_getrandom, (long)g_random, sizeof g_random, 0, 0, 0, 0) != (long)sizeof g_random)
        for (int i = 0; i < 16; i++) g_random[i] = (uint8_t)(i * 7 + 13);

    int master = posix_openpt(O_RDWR | O_NOCTTY);
    if (master < 0) { LOGE("posix_openpt failed: %s", strerror(errno)); return -1; }
    if (grantpt(master) != 0 || unlockpt(master) != 0) {
        LOGE("grantpt/unlockpt failed: %s", strerror(errno));
        close(master);
        return -1;
    }
    const char *slave_name = ptsname(master);
    if (!slave_name) { LOGE("ptsname failed"); close(master); return -1; }
    int slave = open(slave_name, O_RDWR | O_NOCTTY);
    if (slave < 0) { LOGE("open pty slave failed: %s", strerror(errno)); close(master); return -1; }

    struct winsize ws;
    memset(&ws, 0, sizeof ws);
    ws.ws_col = (unsigned short)(cols > 0 ? cols : 80);
    ws.ws_row = (unsigned short)(rows > 0 ? rows : 24);
    ioctl(master, TIOCSWINSZ, &ws);

    pid_t pid = fork();
    if (pid == 0) {
        // Child: raw syscalls only from here (multithreaded fork safety).
        k_sys(SYS_close, master, 0, 0, 0, 0, 0);
        k_sys(SYS_setsid, 0, 0, 0, 0, 0, 0);
        k_sys(SYS_ioctl, slave, TIOCSCTTY, 0, 0, 0, 0);
        k_sys(SYS_dup3, slave, 0, 0, 0, 0, 0);
        k_sys(SYS_dup3, slave, 1, 0, 0, 0, 0);
        k_sys(SYS_dup3, slave, 2, 0, 0, 0, 0);
        k_sys(SYS_close, slave, 0, 0, 0, 0, 0);
        for (int fd = 3; fd < 1024; fd++) k_sys(SYS_close, fd, 0, 0, 0, 0, 0);
        install_sigsys_shim();
        k_sys(SYS_chdir, (long)sysroot, 0, 0, 0, 0, 0);
        kimi_image img;
        if (kimi_elf_load(exe, sysroot, argv, envp, &img) != 0)
            k_sys(SYS_exit, 127, 0, 0, 0, 0, 0);
        kimi_elf_enter(&img);
        k_sys(SYS_exit, 127, 0, 0, 0, 0, 0);
    }

    close(slave);
    if (pid < 0) { close(master); return -1; }
    int flags = fcntl(master, F_GETFL, 0);
    if (flags >= 0) fcntl(master, F_SETFL, flags | O_NONBLOCK);
    *master_out = master;
    return pid;
}

int kimi_guest_set_winsize(int fd, int cols, int rows) {
    struct winsize ws;
    memset(&ws, 0, sizeof ws);
    ws.ws_col = (unsigned short)(cols > 0 ? cols : 80);
    ws.ws_row = (unsigned short)(rows > 0 ? rows : 24);
    return ioctl(fd, TIOCSWINSZ, &ws);
}

// ---------------- JNI ----------------
#ifndef KIMI_ELF_NO_JNI
typedef struct { int fd; } reader_arg;
static void *stdout_reader(void *arg) {
    reader_arg *ra = (reader_arg *)arg;
    char buf[4096];
    for (;;) {
        ssize_t n = read(ra->fd, buf, sizeof buf - 1);
        if (n <= 0) break;
        buf[n] = 0;
        __android_log_write(ANDROID_LOG_INFO, "KimiOS-guest", buf);
    }
    close(ra->fd);
    free(ra);
    return NULL;
}

JNIEXPORT jint JNICALL
Java_com_kimios_libos_KimiBridge_nativeRunElf(JNIEnv *env, jclass cls,
                                              jstring jexe, jstring jsysroot,
                                              jobjectArray jargv, jobjectArray jenvp) {
    (void)cls;
    const char *exe = (*env)->GetStringUTFChars(env, jexe, NULL);
    const char *sysroot = (*env)->GetStringUTFChars(env, jsysroot, NULL);
    int argc = jargv ? (*env)->GetArrayLength(env, jargv) : 0;
    int envc = jenvp ? (*env)->GetArrayLength(env, jenvp) : 0;
    char **argv = calloc(argc + 1, sizeof(char *));
    char **envp = calloc(envc + 1, sizeof(char *));
    for (int i = 0; i < argc; i++) {
        jstring s = (*env)->GetObjectArrayElement(env, jargv, i);
        argv[i] = (char *)(*env)->GetStringUTFChars(env, s, NULL);
    }
    for (int i = 0; i < envc; i++) {
        jstring s = (*env)->GetObjectArrayElement(env, jenvp, i);
        envp[i] = (char *)(*env)->GetStringUTFChars(env, s, NULL);
    }
    int in_fd = -1, out_fd = -1;
    pid_t pid = kimi_guest_spawn(exe, sysroot, argv, envp, &in_fd, &out_fd);
    LOGI("guest spawn pid=%d exe=%s", (int)pid, exe);
    int code = -1;
    if (pid > 0) {
        if (in_fd >= 0) close(in_fd); // no stdin for v1
        reader_arg *ra = malloc(sizeof(reader_arg));
        ra->fd = out_fd;
        pthread_t th;
        pthread_create(&th, NULL, stdout_reader, ra);
        int st = 0;
        kimi_guest_wait(pid, &st);
        pthread_join(th, NULL);
        code = WIFEXITED(st) ? WEXITSTATUS(st) : (128 + WTERMSIG(st));
        LOGI("guest pid=%d exited code=%d (raw status=%#x)", (int)pid, code, st);
    }
    (*env)->ReleaseStringUTFChars(env, jexe, exe);
    (*env)->ReleaseStringUTFChars(env, jsysroot, sysroot);
    free(argv);
    free(envp);
    return code;
}

#endif // KIMI_ELF_NO_JNI
