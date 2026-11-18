// KimiOS library-OS Android host: JNI bridge + C syscall dispatcher.
//
// The pure-Rust userland (libkimi + kimi-gui) is compiled as a no_std staticlib
// for aarch64-linux-android and linked into this shared library. Every KimiOS
// syscall routes here (kimi_host_syscall) instead of trapping to EL1. Display
// uses ANativeWindow (CPU framebuffer from the SurfaceView), input comes from
// UIKit-style touches via JNI, FS is the app sandbox, net is BSD sockets.
// No QEMU — native ARM64. Mirrors ios/app/Sources/KimiHost.swift + KimiFS/KimiNet.

#include <jni.h>
#include <android/native_window.h>
#include <android/native_window_jni.h>
#include <android/log.h>
#include <pthread.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <fcntl.h>
#include <dirent.h>
#include <errno.h>
#include <time.h>
#include <sys/stat.h>
#include <sys/types.h>
#include <sys/socket.h>
#include <netdb.h>
#include <arpa/inet.h>

#define LOG(...) __android_log_print(ANDROID_LOG_INFO, "KimiOS", __VA_ARGS__)
#define ERROR UINT64_MAX
#define FB_W 1080
#define FB_H 1920

extern void kimi_gui_start(void);

static uint32_t *front, *back;
static ANativeWindow *window;
static char diskRoot[512];
static uint64_t start_ns;
static int gui_started;
static JavaVM *g_vm;

JNIEXPORT jint JNICALL JNI_OnLoad(JavaVM *vm, void *reserved) {
    (void)reserved; g_vm = vm; return JNI_VERSION_1_6;
}

// ===================== time =====================
static uint64_t now_ns(void) {
    struct timespec t; clock_gettime(CLOCK_MONOTONIC, &t);
    return (uint64_t)t.tv_sec * 1000000000ull + t.tv_nsec;
}
uint64_t kimi_host_cntpct(void) { return now_ns() - start_ns; }

// ===================== input =====================
static uint64_t input_q[256];
static int input_head, input_tail;
static pthread_mutex_t input_lock = PTHREAD_MUTEX_INITIALIZER;
static uint64_t pack_ev(uint16_t kind, uint16_t code, uint32_t val) {
    return ((uint64_t)kind << 48) | ((uint64_t)code << 32) | (uint64_t)val;
}
static void input_push(uint64_t e) {
    pthread_mutex_lock(&input_lock);
    int next = (input_tail + 1) % 256;
    if (next != input_head) { input_q[input_tail] = e; input_tail = next; }
    pthread_mutex_unlock(&input_lock);
}
static uint64_t input_poll(void) {
    pthread_mutex_lock(&input_lock);
    uint64_t r = ERROR;
    if (input_head != input_tail) { r = input_q[input_head]; input_head = (input_head + 1) % 256; }
    pthread_mutex_unlock(&input_lock);
    return r;
}
static void touch(int x, int y, int down) {
    uint32_t ax = (uint32_t)((int64_t)x * 32767 / FB_W);
    uint32_t ay = (uint32_t)((int64_t)y * 32767 / FB_H);
    input_push(pack_ev(3, 0, ax));
    input_push(pack_ev(3, 1, ay));
    input_push(pack_ev(1, 0x14a, down ? 1 : 0));
}
static void move(int x, int y) {
    uint32_t ax = (uint32_t)((int64_t)x * 32767 / FB_W);
    uint32_t ay = (uint32_t)((int64_t)y * 32767 / FB_H);
    input_push(pack_ev(3, 0, ax));
    input_push(pack_ev(3, 1, ay));
}

// ===================== display =====================
static void blit_to_window(void) {
    if (!window) return;
    ANativeWindow_Buffer buf;
    if (ANativeWindow_lock(window, &buf, NULL) != 0) return;
    uint32_t *dst = (uint32_t *)buf.bits;
    int w = FB_W < buf.width ? FB_W : buf.width;
    int h = FB_H < buf.height ? FB_H : buf.height;
    // GUI pixels are 0x00RRGGBB (XRGB); window is RGBX_8888 (R,G,B,X bytes).
    // bswap32(p) >> 8 swaps R and B into RGBX order.
    for (int y = 0; y < h; y++) {
        uint32_t *d = dst + (size_t)y * buf.stride;
        const uint32_t *s = front + (size_t)y * FB_W;
        for (int x = 0; x < w; x++) d[x] = __builtin_bswap32(s[x]) >> 8;
    }
    ANativeWindow_unlockAndPost(window);
}
static void fb_present(uint64_t x0, uint64_t y0, uint64_t x1, uint64_t y1) {
    if (x1 > FB_W) x1 = FB_W; if (y1 > FB_H) y1 = FB_H;
    for (uint64_t y = y0; y < y1; y++)
        memcpy(front + y * FB_W + x0, back + y * FB_W + x0, (size_t)(x1 - x0) * 4);
    blit_to_window();
}

// ===================== ramfs =====================
static const char *ram_names[] = { "/readme", "/version", "/help" };
static const char *ram_data[] = {
    "KimiOS for Android - library-OS edition.\nThe same pure-Rust userland as bare metal, native ARM64, no QEMU.\nPersistent files live under /disk/ (app sandbox).\n",
    "KimiOS 0.9.0-android (library-OS, native ARM64, no QEMU)\n",
    "ls cat write rm mkdir rmdir mv stat df net dns fetch date time\nsleep uptime uname mem ps kpkg dvm apk help\n",
};
static int ram_index(const char *p) { for (int i = 0; i < 3; i++) if (!strcmp(p, ram_names[i])) return i; return -1; }

// ===================== disk fs =====================
typedef struct { int used, kind; char path[256]; } Handle; // kind 0 ram, 1 disk
static Handle handles[64];
static int alloc_handle(int kind, const char *path) {
    for (int i = 0; i < 64; i++) if (!handles[i].used) {
        handles[i].used = 1; handles[i].kind = kind;
        snprintf(handles[i].path, 256, "%s", path); return i + 100;
    }
    return -1;
}
static int safe_rel(const char *r) { return strstr(r, "..") == NULL; }
static void disk_path(const char *rel, char *out, size_t n) { snprintf(out, n, "%s/%s", diskRoot, rel); }
static int is_dir(const char *p) { struct stat st; return stat(p, &st) == 0 && S_ISDIR(st.st_mode); }
static int file_exists(const char *p) { struct stat st; return stat(p, &st) == 0; }
static char *read_all(const char *p, size_t *len) {
    int fd = open(p, O_RDONLY); if (fd < 0) return NULL;
    struct stat st; fstat(fd, &st); size_t sz = st.st_size;
    char *b = malloc(sz + 1); if (!b) { close(fd); return NULL; }
    size_t got = read(fd, b, sz); close(fd); b[got] = 0; *len = got; return b;
}
static void mkdir_p(const char *dir) {
    char tmp[512]; snprintf(tmp, sizeof tmp, "%s", dir);
    size_t len = strlen(tmp); if (len && tmp[len - 1] == '/') tmp[len - 1] = 0;
    for (char *p = tmp + 1; *p; p++) if (*p == '/') { *p = 0; mkdir(tmp, 0755); *p = '/'; }
    mkdir(tmp, 0755);
}
static uint64_t fs_open(const char *path) {
    if (ram_index(path) >= 0) return alloc_handle(0, path);
    if (strncmp(path, "/disk/", 6)) return ERROR;
    const char *rel = path + 6; if (!safe_rel(rel)) return ERROR;
    char p[512]; disk_path(rel, p, sizeof p);
    if (!file_exists(p) || is_dir(p)) return ERROR;
    return alloc_handle(1, rel);
}
static uint64_t fs_create(const char *path) {
    if (strncmp(path, "/disk/", 6)) return ERROR;
    const char *rel = path + 6; if (!*rel || !safe_rel(rel)) return ERROR;
    char p[512]; disk_path(rel, p, sizeof p);
    char dir[512]; snprintf(dir, sizeof dir, "%s", p);
    char *slash = strrchr(dir, '/'); if (slash) { *slash = 0; mkdir_p(dir); }
    int fd = open(p, O_WRONLY | O_CREAT | O_TRUNC, 0644); if (fd < 0) return ERROR; close(fd);
    return alloc_handle(1, rel);
}
static uint64_t fs_read(uint64_t h, uint64_t off, void *buf, uint64_t len) {
    int idx = (int)h - 100; if (idx < 0 || idx >= 64 || !handles[idx].used) return 0;
    if (handles[idx].kind == 0) {
        int ri = ram_index(handles[idx].path); if (ri < 0) return 0;
        const char *d = ram_data[ri]; size_t sz = strlen(d);
        if (off >= sz) return 0;
        uint64_t n = len < (sz - off) ? len : (sz - off); memcpy(buf, d + off, n); return n;
    }
    char p[512]; disk_path(handles[idx].path, p, sizeof p);
    int fd = open(p, O_RDONLY); if (fd < 0) return 0;
    if (lseek(fd, (off_t)off, SEEK_SET) < 0) { close(fd); return 0; }
    ssize_t n = read(fd, buf, len); close(fd); return n < 0 ? 0 : (uint64_t)n;
}
static uint64_t fs_write(uint64_t h, uint64_t off, const void *buf, uint64_t len) {
    int idx = (int)h - 100; if (idx < 0 || idx >= 64 || !handles[idx].used || handles[idx].kind != 1) return 0;
    char p[512]; disk_path(handles[idx].path, p, sizeof p);
    struct stat st; size_t cursz = (stat(p, &st) == 0) ? (size_t)st.st_size : 0;
    if (off == cursz) {
        int fd = open(p, O_WRONLY | O_CREAT, 0644); if (fd < 0) return 0;
        lseek(fd, (off_t)off, SEEK_SET); ssize_t w = write(fd, buf, len); close(fd);
        return w == (ssize_t)len ? len : 0;
    }
    size_t sz = 0; char *data = read_all(p, &sz); if (!data) { data = malloc(1); sz = 0; }
    size_t need = off + len;
    if (need > sz) { data = realloc(data, need + 1); memset(data + sz, 0, need - sz); sz = need; }
    memcpy(data + off, buf, len);
    int fd = open(p, O_WRONLY | O_CREAT | O_TRUNC, 0644); if (fd < 0) { free(data); return 0; }
    ssize_t w = write(fd, data, sz); close(fd); free(data);
    return w == (ssize_t)sz ? len : 0;
}
static uint64_t fs_close(uint64_t h) { int idx = (int)h - 100; if (idx < 0 || idx >= 64 || !handles[idx].used) return ERROR; handles[idx].used = 0; return 0; }
static uint64_t push_str(void *buf, uint64_t cap, uint64_t *w, const char *s) {
    uint64_t n = strlen(s);
    for (uint64_t i = 0; i < n && *w < cap; i++) ((char *)buf)[(*w)++] = s[i];
    if (*w < cap) ((char *)buf)[(*w)++] = 0;
    return *w;
}
static uint64_t fs_list(void *buf, uint64_t cap) {
    uint64_t w = 0;
    for (int i = 0; i < 3; i++) push_str(buf, cap, &w, ram_names[i]);
    DIR *d = opendir(diskRoot);
    if (d) { struct dirent *e; while ((e = readdir(d))) { if (e->d_name[0] == '.') continue; char p[512]; disk_path(e->d_name, p, sizeof p); char nm[300]; snprintf(nm, sizeof nm, "/disk/%s%s", e->d_name, is_dir(p) ? "/" : ""); push_str(buf, cap, &w, nm); } closedir(d); }
    return w;
}
static uint64_t fs_listdir(const char *path, void *buf, uint64_t cap) {
    const char *rel;
    if (!strcmp(path, "/disk")) rel = ""; else if (!strncmp(path, "/disk/", 6)) rel = path + 6; else return ERROR;
    char p[512]; disk_path(rel, p, sizeof p);
    if (!is_dir(p)) return ERROR;
    uint64_t w = 0; DIR *d = opendir(p);
    if (d) { struct dirent *e; while ((e = readdir(d))) { if (e->d_name[0] == '.') continue; char cp[512]; snprintf(cp, sizeof cp, "%s/%s", p, e->d_name); char entry[300]; snprintf(entry, sizeof entry, "%s%s", e->d_name, is_dir(cp) ? "/" : ""); push_str(buf, cap, &w, entry); } closedir(d); }
    return w;
}
static uint64_t fs_stat(const char *path, uint64_t *out) {
    int ri = ram_index(path);
    if (ri >= 0) { out[0] = 0; out[1] = strlen(ram_data[ri]); out[2] = 0; return 0; }
    if (strncmp(path, "/disk/", 6) && strcmp(path, "/disk")) return ERROR;
    const char *rel = !strcmp(path, "/disk") ? "" : path + 6;
    char p[512]; disk_path(rel, p, sizeof p);
    struct stat st; if (stat(p, &st) != 0) return ERROR;
    if (S_ISDIR(st.st_mode)) { out[0] = 2; out[1] = 0; out[2] = 0; }
    else { out[0] = 1; out[1] = st.st_size; out[2] = (st.st_size + 511) / 512; }
    return 0;
}
static uint64_t fs_unlink(const char *path) { if (strncmp(path, "/disk/", 6)) return ERROR; const char *rel = path + 6; if (!safe_rel(rel)) return ERROR; char p[512]; disk_path(rel, p, sizeof p); if (is_dir(p)) return ERROR; return unlink(p) == 0 ? 0 : ERROR; }
static uint64_t fs_mkdir(const char *path) { const char *rel; if (!strcmp(path, "/disk")) rel = ""; else if (!strncmp(path, "/disk/", 6)) rel = path + 6; else return ERROR; char p[512]; disk_path(rel, p, sizeof p); if (is_dir(p)) return 0; return mkdir(p, 0755) == 0 ? 0 : ERROR; }
static uint64_t fs_rmdir(const char *path) { if (strncmp(path, "/disk/", 6)) return ERROR; char p[512]; disk_path(path + 6, p, sizeof p); if (!is_dir(p)) return ERROR; return rmdir(p) == 0 ? 0 : ERROR; }
static uint64_t fs_symlink(const char *target, const char *path) { if (strncmp(path, "/disk/", 6)) return ERROR; const char *rel = path + 6; if (!safe_rel(rel)) return ERROR; char p[512]; disk_path(rel, p, sizeof p); unlink(p); return symlink(target, p) == 0 ? 0 : ERROR; }

// SYS_FETCH_URL: GET url (HTTP/HTTPS) via Java HttpURLConnection, write body to
// the sandbox file dest. Lets kpkg reach HTTPS repos (GitHub raw) with no TLS
// stack in the userland. Runs on the GUI thread (attached to the JVM here).
static uint64_t fs_fetch_url(const char *url, const char *dest) {
    if (strncmp(dest, "/disk/", 6)) return ERROR;
    const char *rel = dest + 6;
    if (!safe_rel(rel)) return ERROR;
    char p[512]; disk_path(rel, p, sizeof p);
    if (!g_vm) return ERROR;
    JNIEnv *env = NULL;
    if ((*g_vm)->GetEnv(g_vm, (void **)&env, JNI_VERSION_1_6) != JNI_OK) {
        if ((*g_vm)->AttachCurrentThread(g_vm, &env, NULL) != JNI_OK) return ERROR;
    }
    uint64_t result = ERROR;
    jclass cls = (*env)->FindClass(env, "com/kimios/libos/KimiBridge");
    if (cls) {
        jmethodID mid = (*env)->GetStaticMethodID(env, cls, "nativeFetchUrl", "(Ljava/lang/String;Ljava/lang/String;)J");
        if (mid) {
            jstring ju = (*env)->NewStringUTF(env, url);
            jstring jd = (*env)->NewStringUTF(env, p);
            jlong n = (*env)->CallStaticLongMethod(env, cls, mid, ju, jd);
            (*env)->DeleteLocalRef(env, ju);
            (*env)->DeleteLocalRef(env, jd);
            if ((*env)->ExceptionCheck(env)) (*env)->ExceptionClear(env);
            else if (n >= 0) result = (uint64_t)n;
        }
    }
    if ((*env)->ExceptionCheck(env)) (*env)->ExceptionClear(env);
    return result;
}

// SYS_LINUX_RUN: run an Alpine sysroot binary natively (kimi_elf.c), capture stdout.
static uint64_t str_arg(uint64_t p, uint64_t len, char *out, size_t cap);
struct linux_run_req { uint64_t bin_ptr, bin_len, arg_ptr, arg_len, out_ptr, out_cap, out_len, code; };
extern int kimi_guest_capture(const char *, const char *, char *const *, char *const *, char *, size_t, size_t *, int *);
static uint64_t fs_linux_run(uint64_t a0) {
    struct linux_run_req *rq = (struct linux_run_req *)a0;
    char bin[160], arg[4096];
    str_arg(rq->bin_ptr, rq->bin_len, bin, sizeof bin);
    str_arg(rq->arg_ptr, rq->arg_len, arg, sizeof arg);
    char sysroot[400], exe[600];
    snprintf(sysroot, sizeof sysroot, "%s/alpine/root", diskRoot);
    if (strchr(bin, '/')) snprintf(exe, sizeof exe, "%s/%s", sysroot, bin);
    else snprintf(exe, sizeof exe, "%s/usr/bin/%s", sysroot, bin);
    const char *base = strrchr(bin, '/'); base = base ? base + 1 : bin;
    char lib[700], ppath[900], phome[500], path[700], home[400], tmp[400], hd[400], td[400];
    snprintf(lib, sizeof lib, "LD_LIBRARY_PATH=%s/usr/lib:%s/lib", sysroot, sysroot);
    snprintf(path, sizeof path, "PATH=%s/usr/bin:%s/bin", sysroot, sysroot);
    snprintf(home, sizeof home, "HOME=%s/home", diskRoot);
    snprintf(tmp, sizeof tmp, "TMPDIR=%s/tmp", diskRoot);
    snprintf(hd, sizeof hd, "%s/home", diskRoot); mkdir(hd, 0755);
    snprintf(td, sizeof td, "%s/tmp", diskRoot); mkdir(td, 0755);
    int is_node = strstr(base, "node") != NULL;
    int is_py = strstr(base, "python") != NULL;
    int is_sh = strcmp(base, "sh") == 0 || strcmp(base, "busybox") == 0;
    int is_npm = strcmp(base, "npm") == 0 || strcmp(base, "npx") == 0;
    static char npmcli[600];
    static char tokbuf[4096];
    char *argv[64]; int n = 0;
    if (is_npm) {
        snprintf(exe, sizeof exe, "%s/usr/bin/node", sysroot);
        snprintf(npmcli, sizeof npmcli, "%s/usr/lib/node_modules/npm/bin/%s-cli.js", sysroot, base);
        argv[n++] = exe; argv[n++] = npmcli;
        strncpy(tokbuf, arg, sizeof tokbuf - 1); tokbuf[sizeof tokbuf - 1] = 0;
        char *save = NULL;
        for (char *t = strtok_r(tokbuf, " \t", &save); t && n < 60; t = strtok_r(NULL, " \t", &save))
            if (*t) argv[n++] = t;
    } else {
        argv[n++] = exe;
        if (arg[0]) {
            if (is_node) { argv[n++] = "-e"; argv[n++] = arg; }
            else if (is_py || is_sh) { argv[n++] = "-c"; argv[n++] = arg; }
            else { argv[n++] = arg; }
        }
    }
    argv[n] = NULL;
    char *envp[16]; int e = 0;
    envp[e++] = lib; envp[e++] = path; envp[e++] = home; envp[e++] = tmp; envp[e++] = "LANG=C.UTF-8";
    char sslfile[700], ssldir[600];
    snprintf(sslfile, sizeof sslfile, "SSL_CERT_FILE=%s/etc/ssl/certs/ca-certificates.crt", sysroot);
    snprintf(ssldir, sizeof ssldir, "SSL_CERT_DIR=%s/etc/ssl/certs", sysroot);
    envp[e++] = sslfile; envp[e++] = ssldir;
    // ICU data (node Intl.Collator etc.): absolute /usr/share/icu path is absent
    // on Android; point ICU_DATA at the sysroot's versioned data dir.
    char icudata[700]; icudata[0] = 0;
    char icudir[600]; snprintf(icudir, sizeof icudir, "%s/usr/share/icu", sysroot);
    DIR *id = opendir(icudir);
    if (id) {
        struct dirent *ie;
        while ((ie = readdir(id))) {
            if (ie->d_name[0] != '.') {
                snprintf(icudata, sizeof icudata, "ICU_DATA=%s/%s", icudir, ie->d_name);
                break;
            }
        }
        closedir(id);
    }
    if (icudata[0]) envp[e++] = icudata;
    if (is_py) {
        snprintf(phome, sizeof phome, "PYTHONHOME=%s/usr", sysroot);
        snprintf(ppath, sizeof ppath, "PYTHONPATH=%s/usr/lib/python3.14:%s/usr/lib/python3.14/lib-dynload", sysroot, sysroot);
        envp[e++] = phome; envp[e++] = ppath; envp[e++] = "PYTHONDONTWRITEBYTECODE=1";
    }
    envp[e] = NULL;
    size_t olen = 0; int code = 0;
    kimi_guest_capture(exe, sysroot, argv, envp, (char *)rq->out_ptr, (size_t)rq->out_cap, &olen, &code);
    rq->out_len = olen; rq->code = (uint64_t)code;
    return (uint64_t)code;
}
static uint64_t fs_rename(const char *o, const char *n) { if (strncmp(o, "/disk/", 6) || strncmp(n, "/disk/", 6)) return ERROR; char op[512], np[512]; disk_path(o + 6, op, sizeof op); disk_path(n + 6, np, sizeof np); if (!file_exists(op) || file_exists(np)) return ERROR; return rename(op, np) == 0 ? 0 : ERROR; }
static uint64_t disk_file_count(void) { uint64_t n = 0; DIR *d = opendir(diskRoot); if (d) { struct dirent *e; while ((e = readdir(d))) if (e->d_name[0] != '.') n++; closedir(d); } return n; }

// ===================== net =====================
static int socks[16]; static int nsock;
static uint64_t net_resolve(const char *name, void *out) {
    struct addrinfo hints, *res; memset(&hints, 0, sizeof hints);
    hints.ai_family = AF_INET; hints.ai_socktype = SOCK_STREAM;
    if (getaddrinfo(name, NULL, &hints, &res) != 0) return ERROR;
    uint64_t r = ERROR;
    for (struct addrinfo *a = res; a; a = a->ai_next)
        if (a->ai_family == AF_INET) { struct sockaddr_in *sin = (struct sockaddr_in *)a->ai_addr; memcpy(out, &sin->sin_addr.s_addr, 4); r = 0; break; }
    freeaddrinfo(res); return r;
}
static uint64_t net_connect(uint64_t ip, uint64_t port, uint64_t timeout) {
    char ipstr[32]; snprintf(ipstr, sizeof ipstr, "%llu.%llu.%llu.%llu",
        (unsigned long long)(ip >> 24) & 0xff, (unsigned long long)(ip >> 16) & 0xff,
        (unsigned long long)(ip >> 8) & 0xff, (unsigned long long)ip & 0xff);
    int fd = socket(AF_INET, SOCK_STREAM, 0); if (fd < 0) return ERROR;
    struct sockaddr_in a; memset(&a, 0, sizeof a);
    a.sin_family = AF_INET; a.sin_port = htons((uint16_t)port); a.sin_addr.s_addr = inet_addr(ipstr);
    (void)timeout;
    if (connect(fd, (struct sockaddr *)&a, sizeof a) != 0) { close(fd); return ERROR; }
    if (nsock >= 16) { close(fd); return ERROR; }
    socks[nsock] = fd; return nsock++;
}
static uint64_t net_send(uint64_t slot, const void *buf, uint64_t len) { if (slot >= (uint64_t)nsock) return 0; ssize_t n = send(socks[slot], buf, len, 0); return n < 0 ? 0 : (uint64_t)n; }
static uint64_t net_recv(uint64_t slot, void *buf, uint64_t len, uint64_t timeout) {
    if (slot >= (uint64_t)nsock) return 0;
    struct timeval tv; tv.tv_sec = timeout / 1000; tv.tv_usec = (timeout % 1000) * 1000;
    setsockopt(socks[slot], SOL_SOCKET, SO_RCVTIMEO, &tv, sizeof tv);
    ssize_t n = recv(socks[slot], buf, len, 0); return n <= 0 ? 0 : (uint64_t)n;
}
static uint64_t net_close(uint64_t slot) { if (slot < (uint64_t)nsock) { close(socks[slot]); socks[slot] = -1; } return 0; }

// ===================== syscall dispatch =====================
static uint64_t str_arg(uint64_t p, uint64_t len, char *out, size_t cap) { if (len >= cap) len = cap - 1; memcpy(out, (void *)p, len); out[len] = 0; return len; }

uint64_t kimi_host_syscall(uint64_t n, uint64_t a0, uint64_t a1, uint64_t a2, uint64_t a3, uint64_t a4) {
    char path[256], path2[256];
    switch (n) {
    case 1: { // SYS_WRITE -> logcat
        char buf[1024]; uint64_t len = a2 < sizeof buf - 1 ? a2 : sizeof buf - 1;
        memcpy(buf, (void *)a1, len); buf[len] = 0; LOG("%s", buf); return a2;
    }
    case 3: return 0;
    case 2: LOG("GUI exit(%llu)", (unsigned long long)a0); pthread_exit(NULL); return 0;
    case 11: return 1;
    case 20: return (now_ns() - start_ns) / 1000000;
    case 21: { struct timespec t = { .tv_sec = a0 / 1000, .tv_nsec = (long)(a0 % 1000) * 1000000 }; nanosleep(&t, NULL); return 0; }
    case 26: { uint64_t *f = (uint64_t *)a0; time_t t = time(NULL); struct tm tm; gmtime_r(&t, &tm); f[0] = t; f[1] = tm.tm_year + 1900; f[2] = tm.tm_mon + 1; f[3] = tm.tm_mday; f[4] = tm.tm_hour; f[5] = tm.tm_min; f[6] = tm.tm_sec; return 0; }
    case 16: { uint64_t *i = (uint64_t *)a0; i[0] = FB_W; i[1] = FB_H; i[2] = FB_W * 4; i[3] = 0; return 0; }
    case 31: return (uint64_t)(uintptr_t)front;
    case 36: return (uint64_t)(uintptr_t)back;
    case 37: fb_present(a0, a1, a2, a3); return 0;
    case 17: { uint64_t x = a0, y = a1, w = a2, h = a3; uint32_t c = (uint32_t)a4; for (uint64_t j = y; j < y + h && j < FB_H; j++) for (uint64_t i = x; i < x + w && i < FB_W; i++) back[j * FB_W + i] = c; return 0; }
    case 32: return input_poll();
    case 33: return ERROR;
    case 39: return 0; // no instr mailbox on Android host
    case 4: str_arg(a0, a1, path, sizeof path); return fs_open(path);
    case 5: return fs_read(a0, a1, (void *)a2, a3);
    case 6: return fs_close(a0);
    case 8: return fs_list((void *)a0, a1);
    case 13: str_arg(a0, a1, path, sizeof path); return fs_create(path);
    case 14: return fs_write(a0, a1, (void *)a2, a3);
    case 15: return (131072ULL << 32) | disk_file_count();
    case 18: str_arg(a0, a1, path, sizeof path); return fs_unlink(path);
    case 19: return 131072 - disk_file_count();
    case 25: str_arg(a0, a1, path, sizeof path); str_arg(a2, a3, path2, sizeof path2); return fs_rename(path, path2);
    case 27: str_arg(a0, a1, path, sizeof path); return fs_stat(path, (uint64_t *)a2);
    case 28: str_arg(a0, a1, path, sizeof path); return fs_listdir(path, (void *)a2, a3);
    case 29: str_arg(a0, a1, path, sizeof path); return fs_mkdir(path);
    case 30: str_arg(a0, a1, path, sizeof path); return fs_rmdir(path);
    case 40: { char target[256]; str_arg(a0, a1, target, sizeof target); str_arg(a2, a3, path, sizeof path); return fs_symlink(target, path); }
    case 41: return fs_linux_run(a0);
    case 42: { char url[512]; str_arg(a0, a1, url, sizeof url); str_arg(a2, a3, path, sizeof path); return fs_fetch_url(url, path); }
    case 23: { if (a1 < 1) return ERROR; uint64_t *o = (uint64_t *)a0; o[0] = 1; o[1] = 0; o[2] = 2; o[3] = 0; return 1; }
    case 12: return 2048;
    case 22: return 0;
    case 34:
        switch (a0) {
        case 0: return 0;
        case 1: str_arg(a1, a2, path, sizeof path); return net_resolve(path, (void *)a3);
        case 2: return net_connect(a1, a2, a3);
        case 3: return net_send(a1, (void *)a2, a3);
        case 4: return net_recv(a1, (void *)a2, a3, a4);
        case 5: return net_close(a1);
        default: return ERROR;
        }
    case 7: case 9: case 35: case 10: case 24: case 38: return ERROR;
    default: LOG("unhandled syscall %llu", (unsigned long long)n); return ERROR;
    }
}

// ===================== JNI =====================
static void *gui_thread(void *arg) { (void)arg; kimi_gui_start(); return NULL; }

JNIEXPORT void JNICALL
Java_com_kimios_libos_KimiBridge_nativeInit(JNIEnv *env, jclass cls, jstring filesDir) {
    (void)cls;
    const char *dir = (*env)->GetStringUTFChars(env, filesDir, NULL);
    snprintf(diskRoot, sizeof diskRoot, "%s/disk", dir);
    (*env)->ReleaseStringUTFChars(env, filesDir, dir);
    mkdir_p(diskRoot);
    if (!front) { front = calloc(FB_W * FB_H, 4); back = calloc(FB_W * FB_H, 4); }
    start_ns = now_ns();
    LOG("native init, disk=%s", diskRoot);
}

JNIEXPORT void JNICALL
Java_com_kimios_libos_KimiBridge_nativeSetSurface(JNIEnv *env, jclass cls, jobject surface) {
    (void)cls;
    if (window) { ANativeWindow_release(window); window = NULL; }
    if (surface) {
        window = ANativeWindow_fromSurface(env, surface);
        if (window) ANativeWindow_setBuffersGeometry(window, FB_W, FB_H, WINDOW_FORMAT_RGBX_8888);
    }
}

JNIEXPORT void JNICALL
Java_com_kimios_libos_KimiBridge_nativeStart(JNIEnv *env, jclass cls) {
    (void)env; (void)cls;
    if (gui_started) return;
    gui_started = 1;
    pthread_t th; pthread_create(&th, NULL, gui_thread, NULL);
    pthread_detach(th);
    LOG("GUI thread started");
}

JNIEXPORT void JNICALL
Java_com_kimios_libos_KimiBridge_nativeTouch(JNIEnv *env, jclass cls, jint x, jint y, jint down) {
    (void)env; (void)cls; touch(x, y, down);
}

JNIEXPORT void JNICALL
Java_com_kimios_libos_KimiBridge_nativeMove(JNIEnv *env, jclass cls, jint x, jint y) {
    (void)env; (void)cls; move(x, y);
}

// Test/bridge entry for SYS_LINUX_RUN (the shell's node/python/alpine path).
JNIEXPORT jint JNICALL
Java_com_kimios_libos_KimiBridge_nativeLinuxRun(JNIEnv *env, jclass cls, jstring jbin, jstring jarg) {
    (void)cls;
    const char *bin = (*env)->GetStringUTFChars(env, jbin, NULL);
    const char *arg = (*env)->GetStringUTFChars(env, jarg, NULL);
    static char out[8192];
    struct linux_run_req rq;
    rq.bin_ptr = (uint64_t)(uintptr_t)bin; rq.bin_len = strlen(bin);
    rq.arg_ptr = (uint64_t)(uintptr_t)arg; rq.arg_len = strlen(arg);
    rq.out_ptr = (uint64_t)(uintptr_t)out; rq.out_cap = sizeof out;
    rq.out_len = 0; rq.code = 0;
    uint64_t code = kimi_host_syscall(41, (uint64_t)(uintptr_t)&rq, 0, 0, 0, 0);
    out[rq.out_len < sizeof out ? rq.out_len : sizeof out - 1] = 0;
    LOG("linux_run bin=%s code=%llu out=%s", bin, (unsigned long long)code, out);
    (*env)->ReleaseStringUTFChars(env, jbin, bin);
    (*env)->ReleaseStringUTFChars(env, jarg, arg);
    return (jint)code;
}