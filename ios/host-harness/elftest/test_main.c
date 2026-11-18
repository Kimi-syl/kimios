// Host validation driver for android/jni/kimi_elf.c (compiled -DKIMI_ELF_NO_JNI).
// Runs a dynamic musl ELF guest through the same fork+mmap+auxv loader used on
// device, on aarch64 Linux. usage: elftest <exe> <sysroot> [arg] [flag]
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <sys/types.h>
#include <sys/wait.h>

extern pid_t kimi_guest_spawn(const char *, const char *, char *const *,
                              char *const *, int *, int *);
extern int kimi_guest_wait(pid_t, int *);

int main(int argc, char **argv) {
    if (argc < 3) { fprintf(stderr, "usage: %s exe sysroot [arg] [flag]\n", argv[0]); return 2; }
    const char *exe = argv[1];
    const char *sr = argv[2];
    char lib[1024], ph[1024], pp[2048], path[1024];
    snprintf(lib, sizeof lib, "LD_LIBRARY_PATH=%s/lib:%s/usr/lib", sr, sr);
    snprintf(ph, sizeof ph, "PYTHONHOME=%s/usr", sr);
    snprintf(pp, sizeof pp, "PYTHONPATH=%s/usr/lib/python3.14:%s/usr/lib/python3.14/lib-dynload", sr, sr);
    snprintf(path, sizeof path, "PATH=%s/usr/bin:%s/bin", sr, sr);
    char *av[32]; int n = 0;
    av[n++] = (char *)exe;
    for (int i = 3; i < argc && n < 30; i++) av[n++] = argv[i];
    av[n] = NULL;
    char *ev[] = { lib, ph, pp, path, "LANG=C.UTF-8", "PYTHONDONTWRITEBYTECODE=1",
                   "HOME=/tmp", "TMPDIR=/tmp", NULL };
    int in = -1, out = -1;
    pid_t pid = kimi_guest_spawn(exe, sr, av, ev, &in, &out);
    fprintf(stderr, "[host] spawn pid=%d\n", (int)pid);
    if (pid < 0) return 1;
    if (in >= 0) close(in);
    char buf[4096]; ssize_t r;
    while ((r = read(out, buf, sizeof buf)) > 0) write(1, buf, r);
    close(out);
    int st = 0;
    kimi_guest_wait(pid, &st);
    fprintf(stderr, "[host] exit raw=%#x code=%d sig=%d\n", st,
            WIFEXITED(st) ? WEXITSTATUS(st) : -1, WIFSIGNALED(st) ? WTERMSIG(st) : 0);
    return 0;
}
