// KimiOS library-OS Android host: JNI bridge + C syscall dispatcher.
//
// The pure-Rust userland (libkimi + kimi-gui) is compiled as a no_std staticlib
// for aarch64-linux-android and linked into this shared library. Every KimiOS
// syscall routes here (kimi_host_syscall) instead of trapping to EL1. Display
// uses ANativeWindow (CPU framebuffer from the SurfaceView), input comes from
// UIKit-style touches via JNI, FS is the app sandbox, net is BSD sockets.
// No QEMU — native ARM64. Mirrors ios/app/Sources/KimiHost.swift + KimiFS/KimiNet.

#include <jni.h>
#include <arm_neon.h>
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
#include <sys/wait.h>
#include <sys/socket.h>
#include <netdb.h>
#include <arpa/inet.h>
#include <signal.h>
#include <sys/ioctl.h>
#include <termios.h>

#define LOG(...) __android_log_print(ANDROID_LOG_INFO, "KimiOS", __VA_ARGS__)
#define ERROR UINT64_MAX
// HAL_PIXEL_FORMAT_BGRX_8888 (not exposed as a WINDOW_FORMAT_ constant in the
// NDK). Matches the GUI's 0x00RRGGBB memory layout (B,G,R,X) for memcpy blits.
#ifndef WINDOW_FORMAT_BGRX_8888
#define WINDOW_FORMAT_BGRX_8888 0x104
#endif
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

// KimiBridge jclass + method IDs cached in nativeInit, which runs on a Java
// thread where the APP classloader is current. FindClass from the GUI pthread
// (attached via AttachCurrentThread) resolves against the SYSTEM classloader
// and cannot see app classes, so every GUI-thread JNI call must use these.
static jclass g_bridge;
static jmethodID g_fetch_async_mid, g_fetch_progress_mid, g_fetch_auth_mid;
static jmethodID g_fetch_mid, g_local_time_mid, g_location_mid, g_open_url_mid,
                 g_audio_mid, g_audio_query_mid, g_import_mid,
                 g_sysinfo_mid, g_proclist_mid;
static jmethodID g_web_init_mid, g_web_buf_mid, g_web_render_mid, g_web_touch_mid,
                 g_web_load_mid, g_web_back_mid, g_web_fwd_mid, g_web_reload_mid,
                 g_web_url_mid, g_web_title_mid, g_web_canback_mid, g_web_canfwd_mid,
                 g_web_viewport_mid, g_web_pause_mid, g_web_resume_mid;
static jmethodID g_plugin_api_mid, g_plugin_manifest_mid, g_plugin_notify_mid;

static JNIEnv *gui_env(int *attached) {
    *attached = 0;
    if (!g_vm) return NULL;
    JNIEnv *env = NULL;
    if ((*g_vm)->GetEnv(g_vm, (void **)&env, JNI_VERSION_1_6) == JNI_OK) return env;
    if ((*g_vm)->AttachCurrentThread(g_vm, &env, NULL) != JNI_OK) return NULL;
    *attached = 1;
    return env;
}
static void gui_done(JNIEnv *env, int attached) {
    if (env && (*env)->ExceptionCheck(env)) (*env)->ExceptionClear(env);
    if (attached) (*g_vm)->DetachCurrentThread(g_vm);
}
// Call a static KimiBridge method returning long[]; copy n elements to out.
static uint64_t call_longs(jmethodID mid, uint64_t *out, int n) {
    if (!g_bridge || !mid || !out) return ERROR;
    int attached = 0; JNIEnv *env = gui_env(&attached);
    if (!env) return ERROR;
    uint64_t r = ERROR;
    jlongArray arr = (jlongArray)(*env)->CallStaticObjectMethod(env, g_bridge, mid);
    if (!(*env)->ExceptionCheck(env) && arr) {
        if ((*env)->GetArrayLength(env, arr) >= n) {
            jlong *e = (*env)->GetLongArrayElements(env, arr, NULL);
            if (e) {
                for (int i = 0; i < n; i++) out[i] = (uint64_t)e[i];
                (*env)->ReleaseLongArrayElements(env, arr, e, JNI_ABORT);
                r = 0;
            }
        }
        (*env)->DeleteLocalRef(env, arr);
    }
    gui_done(env, attached);
    return r;
}
// Call a static KimiBridge method (int) -> long[]; copy up to cap elements to
// out, return the number copied (flat u64 records). ERROR on failure.
static uint64_t call_int_longs(jmethodID mid, jint arg, uint64_t *out, uint64_t cap) {
    if (!g_bridge || !mid || !out || cap == 0) return ERROR;
    int attached = 0; JNIEnv *env = gui_env(&attached);
    if (!env) return ERROR;
    uint64_t r = ERROR;
    jlongArray arr = (jlongArray)(*env)->CallStaticObjectMethod(env, g_bridge, mid, arg);
    if (!(*env)->ExceptionCheck(env) && arr) {
        uint64_t len = (uint64_t)(*env)->GetArrayLength(env, arr);
        uint64_t n = len < cap ? len : cap;
        jlong *e = (*env)->GetLongArrayElements(env, arr, NULL);
        if (e) {
            for (uint64_t i = 0; i < n; i++) out[i] = (uint64_t)e[i];
            (*env)->ReleaseLongArrayElements(env, arr, e, JNI_ABORT);
            r = n;
        }
        (*env)->DeleteLocalRef(env, arr);
    }
    gui_done(env, attached);
    return r;
}
// Call a static KimiBridge method (String) -> long (0 ok, -1 = ERROR).
static uint64_t call_str_long(jmethodID mid, const char *s) {
    if (!g_bridge || !mid) return ERROR;
    int attached = 0; JNIEnv *env = gui_env(&attached);
    if (!env) return ERROR;
    uint64_t r = ERROR;
    jstring js = s ? (*env)->NewStringUTF(env, s) : NULL;
    if (!(*env)->ExceptionCheck(env)) {
        jlong v = (*env)->CallStaticLongMethod(env, g_bridge, mid, js);
        if (!(*env)->ExceptionCheck(env)) r = (uint64_t)v;
    }
    if (js) (*env)->DeleteLocalRef(env, js);
    gui_done(env, attached);
    return r;
}
// Call nativeAudio(String path, int cmd) -> long (0 ok, -1 = ERROR).
static uint64_t audio_cmd(const char *path, int cmd) {
    if (!g_bridge || !g_audio_mid) return ERROR;
    int attached = 0; JNIEnv *env = gui_env(&attached);
    if (!env) return ERROR;
    uint64_t r = ERROR;
    jstring jp = path ? (*env)->NewStringUTF(env, path) : NULL;
    if (!(*env)->ExceptionCheck(env)) {
        jlong v = (*env)->CallStaticLongMethod(env, g_bridge, g_audio_mid, jp, (jint)cmd);
        if (!(*env)->ExceptionCheck(env)) r = (uint64_t)v;
    }
    if (jp) (*env)->DeleteLocalRef(env, jp);
    gui_done(env, attached);
    return r;
}

// ---- SYS_WEB helpers: cached-method calls from the GUI pthread ----
// ()J
static uint64_t call_long(jmethodID mid) {
    if (!g_bridge || !mid) return ERROR;
    int attached = 0; JNIEnv *env = gui_env(&attached);
    if (!env) return ERROR;
    uint64_t r = ERROR;
    jlong v = (*env)->CallStaticLongMethod(env, g_bridge, mid);
    if (!(*env)->ExceptionCheck(env)) r = (uint64_t)v;
    gui_done(env, attached);
    return r;
}
// (II)J
static uint64_t call_ii_long(jmethodID mid, jint a, jint b) {
    if (!g_bridge || !mid) return ERROR;
    int attached = 0; JNIEnv *env = gui_env(&attached);
    if (!env) return ERROR;
    uint64_t r = ERROR;
    jlong v = (*env)->CallStaticLongMethod(env, g_bridge, mid, a, b);
    if (!(*env)->ExceptionCheck(env)) r = (uint64_t)v;
    gui_done(env, attached);
    return r;
}
// (III)J
static uint64_t call_iii_long(jmethodID mid, jint a, jint b, jint c) {
    if (!g_bridge || !mid) return ERROR;
    int attached = 0; JNIEnv *env = gui_env(&attached);
    if (!env) return ERROR;
    uint64_t r = ERROR;
    jlong v = (*env)->CallStaticLongMethod(env, g_bridge, mid, a, b, c);
    if (!(*env)->ExceptionCheck(env)) r = (uint64_t)v;
    gui_done(env, attached);
    return r;
}
// (Z)J
static uint64_t call_z_long(jmethodID mid, int v) {
    if (!g_bridge || !mid) return ERROR;
    int attached = 0; JNIEnv *env = gui_env(&attached);
    if (!env) return ERROR;
    uint64_t r = ERROR;
    jlong jv = (*env)->CallStaticLongMethod(env, g_bridge, mid, (jboolean)(v ? JNI_TRUE : JNI_FALSE));
    if (!(*env)->ExceptionCheck(env)) r = (uint64_t)jv;
    gui_done(env, attached);
    return r;
}
// ()Ljava/lang/String; -> copy UTF-8 into out (bounded, NUL-terminated),
// returns copied length (like fs_listdir). ERROR on exception/null.
static uint64_t call_str_out(jmethodID mid, char *out, uint64_t cap) {
    if (!g_bridge || !mid || !out || cap == 0) return ERROR;
    int attached = 0; JNIEnv *env = gui_env(&attached);
    if (!env) return ERROR;
    uint64_t r = ERROR;
    jstring js = (jstring)(*env)->CallStaticObjectMethod(env, g_bridge, mid);
    if (!(*env)->ExceptionCheck(env) && js) {
        const char *s = (*env)->GetStringUTFChars(env, js, NULL);
        if (s) {
            uint64_t len = strlen(s);
            uint64_t n = len < cap - 1 ? len : cap - 1;
            memcpy(out, s, n); out[n] = 0;
            (*env)->ReleaseStringUTFChars(env, js, s);
            r = n;
        }
    }
    if (js) (*env)->DeleteLocalRef(env, js);
    gui_done(env, attached);
    return r;
}
// ()[B -> copy up to cap bytes into out; returns the number copied (0 when
// empty). ERROR on exception/null. Used by SYS_PLUGIN_API notification drain.
static uint64_t call_bytes_out(jmethodID mid, char *out, uint64_t cap) {
    if (!g_bridge || !mid || !out || cap == 0) return ERROR;
    int attached = 0; JNIEnv *env = gui_env(&attached);
    if (!env) return ERROR;
    uint64_t r = ERROR;
    jbyteArray arr = (jbyteArray)(*env)->CallStaticObjectMethod(env, g_bridge, mid);
    if (!(*env)->ExceptionCheck(env) && arr) {
        uint64_t len = (uint64_t)(*env)->GetArrayLength(env, arr);
        uint64_t n = len < cap ? len : cap;
        if (n == 0) r = 0;
        else {
            jbyte *b = (*env)->GetByteArrayElements(env, arr, NULL);
            if (b) {
                memcpy(out, b, n);
                (*env)->ReleaseByteArrayElements(env, arr, b, JNI_ABORT);
                r = n;
            }
        }
        (*env)->DeleteLocalRef(env, arr);
    }
    gui_done(env, attached);
    return r;
}
// nativeWebBuf() -> direct ByteBuffer address (like SYS_FB_BACK). The buffer
// is re-queried every call so a viewport realloc is picked up automatically.
static uint64_t web_buf_addr(void) {
    if (!g_bridge || !g_web_buf_mid) return ERROR;
    int attached = 0; JNIEnv *env = gui_env(&attached);
    if (!env) return ERROR;
    uint64_t r = ERROR;
    jobject buf = (*env)->CallStaticObjectMethod(env, g_bridge, g_web_buf_mid);
    if (!(*env)->ExceptionCheck(env) && buf) {
        void *p = (*env)->GetDirectBufferAddress(env, buf);
        if (p) r = (uint64_t)(uintptr_t)p;
        (*env)->DeleteLocalRef(env, buf);
    }
    gui_done(env, attached);
    return r;
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
    // GUI pixels 0x00RRGGBB -> window RGBX_8888 via bswap32>>8 (R/B swap).
    // NEON processes 4 pixels/iteration (vrev32 = bswap32, then >>8).
    for (int y = 0; y < h; y++) {
        uint32_t *d = dst + (size_t)y * buf.stride;
        const uint32_t *s = front + (size_t)y * FB_W;
        int x = 0;
        for (; x + 4 <= w; x += 4)
            vst1q_u32(d + x, vshrq_n_u32(vrev32q_u8(vld1q_u32(s + x)), 8));
        for (; x < w; x++) d[x] = __builtin_bswap32(s[x]) >> 8;
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
// stack in the userland. Runs on the GUI thread: uses the jclass/methodID
// cached in nativeInit (FindClass here would hit the system classloader).
static uint64_t fs_fetch_url(const char *url, const char *dest) {
    if (strncmp(dest, "/disk/", 6)) return ERROR;
    const char *rel = dest + 6;
    if (!safe_rel(rel)) return ERROR;
    char p[512]; disk_path(rel, p, sizeof p);
    if (!g_bridge || !g_fetch_mid) return ERROR;
    int attached = 0; JNIEnv *env = gui_env(&attached);
    if (!env) return ERROR;
    uint64_t result = ERROR;
    jstring ju = (*env)->NewStringUTF(env, url);
    jstring jd = (*env)->NewStringUTF(env, p);
    if (!(*env)->ExceptionCheck(env)) {
        jlong n = (*env)->CallStaticLongMethod(env, g_bridge, g_fetch_mid, ju, jd);
        if (!(*env)->ExceptionCheck(env) && n >= 0) result = (uint64_t)n;
    }
    if (ju) (*env)->DeleteLocalRef(env, ju);
    if (jd) (*env)->DeleteLocalRef(env, jd);
    gui_done(env, attached);
    return result;
}

// SYS_LINUX_RUN: run an Alpine sysroot binary natively (kimi_elf.c), capture stdout.
static uint64_t str_arg(uint64_t p, uint64_t len, char *out, size_t cap);
struct linux_run_req { uint64_t bin_ptr, bin_len, arg_ptr, arg_len, out_ptr, out_cap, out_len, code; };
extern int kimi_guest_capture(const char *, const char *, char *const *, char *const *, char *, size_t, size_t *, int *);

// Shared guest environment (SYS_LINUX_RUN + SYS_RUN_PATH): point the dynamic
// loader, PATH, HOME/TMPDIR, TLS certs and ICU data at the sysroot/sandbox.
// envp needs room for 16 entries + NULL; buffers live in the caller's GuestEnv.
typedef struct {
    char lib[700], path[700], home[400], tmp[400];
    char sslfile[700], ssldir[600], icudata[700];
    char phome[500], ppath[900];
    char npmprefix[700], nodepath[700];
} GuestEnv;
static int guest_env_build(const char *sysroot, GuestEnv *s, char **envp, int is_py) {
    snprintf(s->lib, sizeof s->lib, "LD_LIBRARY_PATH=%s/usr/lib:%s/lib", sysroot, sysroot);
    snprintf(s->path, sizeof s->path, "PATH=%s/usr/local/bin:%s/usr/bin:%s/bin", sysroot, sysroot, sysroot);
    snprintf(s->home, sizeof s->home, "HOME=%s/home", diskRoot);
    snprintf(s->tmp, sizeof s->tmp, "TMPDIR=%s/tmp", diskRoot);
    char hd[520], td[520];
    snprintf(hd, sizeof hd, "%s/home", diskRoot); mkdir(hd, 0755);
    snprintf(td, sizeof td, "%s/tmp", diskRoot); mkdir(td, 0755);
    int e = 0;
    envp[e++] = s->lib; envp[e++] = s->path; envp[e++] = s->home; envp[e++] = s->tmp; envp[e++] = "LANG=C.UTF-8";
    snprintf(s->sslfile, sizeof s->sslfile, "SSL_CERT_FILE=%s/etc/ssl/certs/ca-certificates.crt", sysroot);
    snprintf(s->ssldir, sizeof s->ssldir, "SSL_CERT_DIR=%s/etc/ssl/certs", sysroot);
    envp[e++] = s->sslfile; envp[e++] = s->ssldir;
    snprintf(s->npmprefix, sizeof s->npmprefix, "NPM_CONFIG_PREFIX=%s/usr/local", sysroot);
    snprintf(s->nodepath, sizeof s->nodepath,
             "NODE_PATH=%s/usr/lib/node_modules:%s/usr/local/lib/node_modules",
             sysroot, sysroot);
    envp[e++] = "COLORTERM=truecolor";
    envp[e++] = s->npmprefix;
    envp[e++] = s->nodepath;
    // ICU data (node Intl.Collator etc.): absolute /usr/share/icu path is absent
    // on Android; point ICU_DATA at the sysroot's versioned data dir.
    s->icudata[0] = 0;
    char icudir[600]; snprintf(icudir, sizeof icudir, "%s/usr/share/icu", sysroot);
    DIR *id = opendir(icudir);
    if (id) {
        struct dirent *ie;
        while ((ie = readdir(id))) {
            if (ie->d_name[0] != '.') {
                snprintf(s->icudata, sizeof s->icudata, "ICU_DATA=%s/%s", icudir, ie->d_name);
                break;
            }
        }
        closedir(id);
    }
    if (s->icudata[0]) envp[e++] = s->icudata;
    if (is_py) {
        snprintf(s->phome, sizeof s->phome, "PYTHONHOME=%s/usr", sysroot);
        snprintf(s->ppath, sizeof s->ppath, "PYTHONPATH=%s/usr/lib/python3.14:%s/usr/lib/python3.14/lib-dynload", sysroot, sysroot);
        envp[e++] = s->phome; envp[e++] = s->ppath; envp[e++] = "PYTHONDONTWRITEBYTECODE=1";
    }
    return e;
}
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
            else {
                strncpy(tokbuf, arg, sizeof tokbuf - 1); tokbuf[sizeof tokbuf - 1] = 0;
                char *save = NULL;
                for (char *t = strtok_r(tokbuf, " \t", &save); t && n < 60; t = strtok_r(NULL, " \t", &save))
                    if (*t) argv[n++] = t;
            }
        }
    }
    argv[n] = NULL;
    GuestEnv ge;
    char *envp[24];
    int e = guest_env_build(sysroot, &ge, envp, is_py);
    envp[e] = NULL;
    size_t olen = 0; int code = 0;
    kimi_guest_capture(exe, sysroot, argv, envp, (char *)rq->out_ptr, (size_t)rq->out_cap, &olen, &code);
    rq->out_len = olen; rq->code = (uint64_t)code;
    return (uint64_t)code;
}
// SYS 57: start async download to /disk/<dest> (progress via SYS 58).
static uint64_t fs_fetch_async(const char *url, const char *dest) {
    if (strncmp(dest, "/disk/", 6)) return ERROR;
    const char *rel = dest + 6;
    if (!safe_rel(rel)) return ERROR;
    char p[512]; disk_path(rel, p, sizeof p);
    if (!g_bridge || !g_fetch_async_mid) return ERROR;
    int attached = 0; JNIEnv *env = gui_env(&attached);
    if (!env) return ERROR;
    jstring ju = (*env)->NewStringUTF(env, url);
    jstring jd = (*env)->NewStringUTF(env, p);
    if (!(*env)->ExceptionCheck(env))
        (*env)->CallStaticLongMethod(env, g_bridge, g_fetch_async_mid, ju, jd);
    if (ju) (*env)->DeleteLocalRef(env, ju);
    if (jd) (*env)->DeleteLocalRef(env, jd);
    gui_done(env, attached);
    return 0;
}
// SYS 58: write [done,total,state] into u64[3] at a0.
static uint64_t fs_fetch_progress(uint64_t a0) {
    if (!g_bridge || !g_fetch_progress_mid) return ERROR;
    int attached = 0; JNIEnv *env = gui_env(&attached);
    if (!env) return ERROR;
    uint64_t *o = (uint64_t *)a0;
    jobject arr = (*env)->CallStaticObjectMethod(env, g_bridge, g_fetch_progress_mid);
    if (arr && !(*env)->ExceptionCheck(env)) {
        jlong *v = (*env)->GetLongArrayElements(env, (jlongArray)arr, NULL);
        if (v) {
            o[0] = (uint64_t)v[0]; o[1] = (uint64_t)v[1]; o[2] = (uint64_t)v[2];
            (*env)->ReleaseLongArrayElements(env, (jlongArray)arr, v, JNI_ABORT);
        }
        (*env)->DeleteLocalRef(env, arr);
    }
    gui_done(env, attached);
    return 0;
}

// SYS 59: authenticated GET (Bearer) to /disk/<dest>; returns bytes or ERROR.
static uint64_t fs_fetch_auth(const char *url, const char *dest, const char *bearer) {
    if (strncmp(dest, "/disk/", 6)) return ERROR;
    const char *rel = dest + 6;
    if (!safe_rel(rel)) return ERROR;
    char p[512]; disk_path(rel, p, sizeof p);
    if (!g_bridge || !g_fetch_auth_mid) return ERROR;
    int attached = 0; JNIEnv *env = gui_env(&attached);
    if (!env) return ERROR;
    uint64_t result = ERROR;
    jstring ju = (*env)->NewStringUTF(env, url);
    jstring jd = (*env)->NewStringUTF(env, p);
    jstring jb = (*env)->NewStringUTF(env, bearer && bearer[0] ? bearer : "");
    if (!(*env)->ExceptionCheck(env)) {
        jlong n = (*env)->CallStaticLongMethod(env, g_bridge, g_fetch_auth_mid, ju, jd, jb);
        if (!(*env)->ExceptionCheck(env) && n >= 0) result = (uint64_t)n;
    }
    if (ju) (*env)->DeleteLocalRef(env, ju);
    if (jd) (*env)->DeleteLocalRef(env, jd);
    if (jb) (*env)->DeleteLocalRef(env, jb);
    gui_done(env, attached);
    return result;
}

// SYS_RUN_PATH: execute an absolute /disk/... program via the guest loader.
// argv = [abspath, arg] (arg omitted when empty); same sysroot env as
// SYS_LINUX_RUN. Returns the guest exit code, ERROR when spawn fails.
static uint64_t fs_run_path(const char *path, const char *arg) {
    if (strncmp(path, "/disk/", 6)) return ERROR;
    const char *rel = path + 6;
    if (!*rel || !safe_rel(rel)) return ERROR;
    char exe[512]; disk_path(rel, exe, sizeof exe);
    struct stat st;
    if (stat(exe, &st) != 0 || !S_ISREG(st.st_mode)) return ERROR;
    char sysroot[512]; snprintf(sysroot, sizeof sysroot, "%s/alpine/root", diskRoot);
    const char *base = strrchr(rel, '/'); base = base ? base + 1 : rel;
    GuestEnv ge;
    char *envp[24];
    int e = guest_env_build(sysroot, &ge, envp, strstr(base, "python") != NULL);
    envp[e] = NULL;
    char *argv[3]; int n = 0;
    argv[n++] = exe;
    if (arg && arg[0]) argv[n++] = (char *)arg;
    argv[n] = NULL;
    static char out[8192];
    size_t olen = 0; int code = 0;
    if (kimi_guest_capture(exe, sysroot, argv, envp, out, sizeof out, &olen, &code) < 0) return ERROR;
    LOG("run_path %s code=%d out=%s", exe, code, out);
    return (uint64_t)code;
}
// ===================== plugins (SYS_PLUGIN_API=53 / SYS_PLUGIN_RUN=54) =====================
extern pid_t kimi_guest_spawn(const char *, const char *, char *const *, char *const *, int *, int *);

// Start/query the localhost Plugin API server (Java KimiApiServer, lazily
// bound to 127.0.0.1:0 on daemon threads). Returns the port, 0 if unavailable.
static uint64_t plugin_api_port(void) {
    uint64_t p = call_long(g_plugin_api_mid);
    if (p == ERROR || p == 0 || p > 65535) return 0;
    return p;
}

// nativePluginManifest(id) -> String[]{runtime, entry, argsJoined}; bounded
// copies into the caller's buffers. 0 ok, ERROR when missing/invalid.
static uint64_t plugin_manifest(const char *id, char *runtime, size_t rcap,
                                char *entry, size_t ecap, char *args, size_t acap) {
    if (!g_bridge || !g_plugin_manifest_mid) return ERROR;
    int attached = 0; JNIEnv *env = gui_env(&attached);
    if (!env) return ERROR;
    uint64_t r = ERROR;
    jstring js = (*env)->NewStringUTF(env, id);
    if (js && !(*env)->ExceptionCheck(env)) {
        jobjectArray arr = (jobjectArray)(*env)->CallStaticObjectMethod(env, g_bridge, g_plugin_manifest_mid, js);
        if (!(*env)->ExceptionCheck(env) && arr && (*env)->GetArrayLength(env, arr) >= 3) {
            char *dst[3] = { runtime, entry, args };
            size_t caps[3] = { rcap, ecap, acap };
            int ok = 1;
            for (int i = 0; i < 3 && ok; i++) {
                jstring s = (jstring)(*env)->GetObjectArrayElement(env, arr, i);
                if (!s) { ok = 0; break; }
                const char *u = (*env)->GetStringUTFChars(env, s, NULL);
                if (!u) { (*env)->DeleteLocalRef(env, s); ok = 0; break; }
                size_t len = strlen(u); if (len >= caps[i]) len = caps[i] - 1;
                memcpy(dst[i], u, len); dst[i][len] = 0;
                (*env)->ReleaseStringUTFChars(env, s, u);
                (*env)->DeleteLocalRef(env, s);
            }
            if (ok) r = 0;
        }
        if (arr) (*env)->DeleteLocalRef(env, arr);
    }
    if (js) (*env)->DeleteLocalRef(env, js);
    gui_done(env, attached);
    return r;
}

// Background plugin drainer: log guest stdout to logcat until EOF, then reap
// the child (wait4) so services never accumulate zombies. Detached pthread;
// the guest keeps running independently of the GUI thread.
typedef struct { int fd; pid_t pid; char id[64]; } plugin_bg_arg;
static void *plugin_bg_drain(void *arg) {
    plugin_bg_arg *a = (plugin_bg_arg *)arg;
    char buf[4096];
    ssize_t r;
    while ((r = read(a->fd, buf, sizeof buf - 1)) > 0) {
        buf[r] = 0;
        __android_log_print(ANDROID_LOG_INFO, "KimiOS-plugin", "%s", buf);
    }
    close(a->fd);
    int st = 0;
    wait4(a->pid, &st, 0, NULL);
    int code = WIFEXITED(st) ? WEXITSTATUS(st) : (128 + WTERMSIG(st));
    LOG("plugin %s background exit=%d", a->id, code);
    free(a);
    return NULL;
}

// SYS_PLUGIN_RUN: run diskRoot/plugins/<id>/ per its plugin.json manifest.
// argv: native -> [entry, args...]; python/node -> [sysroot interpreter,
// entry, args...] (args tokenized by spaces). env = guest_env_build plus
// KIMI_API_PORT/KIMI_PLUGIN_ID/KIMI_PLUGIN_DIR. background=1 forks without
// waiting (returns 0); foreground captures output and returns the exit code.
static uint64_t fs_plugin_run(const char *id, int background, char *outbuf, size_t outcap, size_t *olen_out) {
    if (!*id || !safe_rel(id) || strchr(id, '/') || strchr(id, '\\')) return ERROR;
    char mpath[512];
    snprintf(mpath, sizeof mpath, "%s/plugins/%s/plugin.json", diskRoot, id);
    if (!file_exists(mpath)) return ERROR;
    char runtime[32], entry[256], args[2048];
    if (plugin_manifest(id, runtime, sizeof runtime, entry, sizeof entry, args, sizeof args) != 0) return ERROR;
    if (!entry[0]) return ERROR;
    char sysroot[512]; snprintf(sysroot, sizeof sysroot, "%s/alpine/root", diskRoot);
    char pdir[512], exe[600];
    snprintf(pdir, sizeof pdir, "%s/plugins/%s", diskRoot, id);
    if (strncmp(entry, "sysroot:", 8) == 0) {
        // Converter-generated Alpine plugin: run the ELF in-place in the
        // sysroot so FHS data paths (/usr/share, ...) resolve. No pdir join.
        const char *rel = entry + 8;
        if (!*rel || strstr(rel, "..")) return ERROR;
        snprintf(exe, sizeof exe, "%s/%s", sysroot, rel);
    } else {
        if (entry[0] == '/' || !safe_rel(entry)) return ERROR;
        snprintf(exe, sizeof exe, "%s/%s", pdir, entry);
    }
    if (!file_exists(exe)) return ERROR;
    int is_py = strcmp(runtime, "python") == 0;
    int is_node = strcmp(runtime, "node") == 0;
    char interp[600];
    if (is_py || is_node) {
        snprintf(interp, sizeof interp, "%s/usr/bin/%s", sysroot, is_py ? "python3" : "node");
        if (!file_exists(interp)) return ERROR;
    }
    static char tokbuf[2048];
    char *argv[64]; int n = 0;
    if (is_py || is_node) { argv[n++] = interp; argv[n++] = exe; }
    else argv[n++] = exe;
    if (args[0]) {
        strncpy(tokbuf, args, sizeof tokbuf - 1); tokbuf[sizeof tokbuf - 1] = 0;
        char *save = NULL;
        for (char *t = strtok_r(tokbuf, " \t", &save); t && n < 60; t = strtok_r(NULL, " \t", &save))
            if (*t) argv[n++] = t;
    }
    argv[n] = NULL;
    // The ELF to load is the interpreter for python/node plugins (exe is the
    // script argument); the native entry itself for native plugins.
    const char *load_exe = (is_py || is_node) ? interp : exe;
    GuestEnv ge;
    char kport[48], kpid[300], kdir[600];
    char *envp[24];
    int e = guest_env_build(sysroot, &ge, envp, is_py);
    uint64_t port = plugin_api_port();
    snprintf(kport, sizeof kport, "KIMI_API_PORT=%llu", (unsigned long long)port);
    snprintf(kpid, sizeof kpid, "KIMI_PLUGIN_ID=%s", id);
    snprintf(kdir, sizeof kdir, "KIMI_PLUGIN_DIR=%s", pdir);
    envp[e++] = kport; envp[e++] = kpid; envp[e++] = kdir;
    envp[e] = NULL;
    if (background) {
        int in = -1, of = -1;
        pid_t pid = kimi_guest_spawn(load_exe, sysroot, argv, envp, &in, &of);
        if (pid < 0) return ERROR;
        if (in >= 0) close(in);
        plugin_bg_arg *a = malloc(sizeof(plugin_bg_arg));
        if (a) {
            a->fd = of; a->pid = pid;
            snprintf(a->id, sizeof a->id, "%s", id);
            pthread_t th;
            if (pthread_create(&th, NULL, plugin_bg_drain, a) == 0) pthread_detach(th);
            else { close(of); free(a); }
        } else close(of);
        LOG("plugin %s background pid=%d", id, (int)pid);
        return 0;
    }
    static char sbuf[8192];
    char *out = outbuf ? outbuf : sbuf;
    size_t cap = outbuf ? outcap : sizeof sbuf;
    size_t olen = 0; int code = 0;
    if (kimi_guest_capture(load_exe, sysroot, argv, envp, out, cap, &olen, &code) < 0) return ERROR;
    LOG("plugin %s code=%d out=%.400s", id, code, out);
    if (olen_out) *olen_out = olen;
    return (uint64_t)code;
}

// SYS_COPY: byte-copy src -> dst, both /disk/... paths. Creates the dst parent
// directory. Returns 0 ok, ERROR on failure (partial dst removed).
static uint64_t fs_copy(const char *src, const char *dst) {
    if (strncmp(src, "/disk/", 6) || strncmp(dst, "/disk/", 6)) return ERROR;
    const char *sr = src + 6, *dr = dst + 6;
    if (!*sr || !*dr || !safe_rel(sr) || !safe_rel(dr)) return ERROR;
    char sp[512], dp[512];
    disk_path(sr, sp, sizeof sp); disk_path(dr, dp, sizeof dp);
    if (!strcmp(sp, dp)) return ERROR;
    if (is_dir(sp) || is_dir(dp)) return ERROR;
    char dir[512]; snprintf(dir, sizeof dir, "%s", dp);
    char *slash = strrchr(dir, '/'); if (slash) { *slash = 0; mkdir_p(dir); }
    FILE *in = fopen(sp, "rb"); if (!in) return ERROR;
    FILE *out = fopen(dp, "wb"); if (!out) { fclose(in); return ERROR; }
    char buf[8192]; size_t n; uint64_t r = 0;
    while ((n = fread(buf, 1, sizeof buf, in)) > 0)
        if (fwrite(buf, 1, n, out) != n) { r = ERROR; break; }
    if (ferror(in)) r = ERROR;
    fclose(in);
    if (fclose(out) != 0) r = ERROR;
    if (r == ERROR) unlink(dp);
    return r;
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

// ===================== interactive process sessions (SYS_PROC=56) =========
// PTY-backed guest sessions for the TUI terminal. Sub-ops:
//  0 spawn(req{bin_ptr,bin_len,arg_ptr,arg_len,rows,cols}) -> slot | ERROR
//  1 read(slot,buf,cap)   -> bytes (non-blocking; 0 = none/EOF)
//  2 write(slot,buf,len)  -> bytes written to child stdin
//  3 kill(slot)           -> SIGHUP the child
//  4 resize(slot,rows,cols)
//  5 status(slot)         -> 0 running, else exit_code+1
//  6 close(slot)
extern pid_t kimi_guest_spawn_pty(const char *, const char *, char *const *, char *const *, int, int, int *);
#define PROC_MAX 8
static struct { int master; int infd; int outfd; pid_t pid; int used; int exited; int code; } procs[PROC_MAX];
static pthread_mutex_t proc_lock = PTHREAD_MUTEX_INITIALIZER;

static uint64_t proc_spawn(uint64_t a1) {
    struct proc_req { uint64_t bin_ptr, bin_len, arg_ptr, arg_len, rows, cols; } *rq = (void *)a1;
    char bin[160], arg[2048];
    str_arg(rq->bin_ptr, rq->bin_len, bin, sizeof bin);
    str_arg(rq->arg_ptr, rq->arg_len, arg, sizeof arg);
    char sysroot[400], exe[600];
    snprintf(sysroot, sizeof sysroot, "%s/alpine/root", diskRoot);
    const char *b0 = strrchr(bin, '/'); b0 = b0 ? b0 + 1 : bin;
    const char *base = b0;
    int is_shell = (strcmp(b0, "sh") == 0 || strcmp(b0, "ash") == 0 || bin[0] == 0);
    static char tokbuf[2048];
    char *argv[64]; int n = 0;
    const char *applet = NULL;
    char bb[600];
    snprintf(bb, sizeof bb, "%s/bin/busybox", sysroot);
    if (!file_exists(bb)) snprintf(bb, sizeof bb, "%s/usr/bin/busybox", sysroot);
    if (is_shell) {
        // /bin/sh is an ABSOLUTE symlink to /bin/busybox which the guest cannot
        // resolve (absolute paths hit the Android host root). Launch busybox's
        // sh applet directly with the real binary.
        if (!file_exists(bb)) return ERROR;
        snprintf(exe, sizeof exe, "%s", bb);
        argv[n++] = exe; argv[n++] = (char *)"sh";
    } else {
        if (strchr(bin, '/')) snprintf(exe, sizeof exe, "%s/%s", sysroot, bin);
        else snprintf(exe, sizeof exe, "%s/usr/bin/%s", sysroot, bin);
        if (!file_exists(exe)) snprintf(exe, sizeof exe, "%s/bin/%s", sysroot, b0);
        if (!file_exists(exe)) snprintf(exe, sizeof exe, "%s/usr/local/bin/%s", sysroot, b0);
        if (!file_exists(exe)) {
            // Bare name that is a busybox applet (top/vi/ls/…): run it via the
            // real busybox binary, avoiding the broken absolute symlinks.
            if (!file_exists(bb)) return ERROR;
            snprintf(exe, sizeof exe, "%s", bb);
            applet = b0;
        }
        argv[n++] = exe;
        if (applet) argv[n++] = (char *)applet;
    }
    if (arg[0]) {
        strncpy(tokbuf, arg, sizeof tokbuf - 1); tokbuf[sizeof tokbuf - 1] = 0;
        char *save = NULL;
        for (char *t = strtok_r(tokbuf, " \t", &save); t && n < 60; t = strtok_r(NULL, " \t", &save))
            if (*t) argv[n++] = t;
    }
    argv[n] = NULL;
    GuestEnv ge;
    char *envp[24];
    int e = guest_env_build(sysroot, &ge, envp, strstr(base, "python") != NULL);
    envp[e++] = "TERM=xterm-256color";
    envp[e++] = "PS1=kimios$ ";
    envp[e] = NULL;
    // rows>0 => interactive: give the guest a real PTY (isatty, prompt, echo,
    // job control, full-screen TUI). rows==0 => headless programs (servers):
    // use plain pipes (the PTY path tripped the emulator's seccomp for some
    // guests; pipes are the proven path).
    int rows = (int)rq->rows;
    int cols = (int)rq->cols;
    int want_pty = rows > 0;
    if (cols <= 0) cols = 100;
    pthread_mutex_lock(&proc_lock);
    int slot = -1;
    for (int i = 0; i < PROC_MAX; i++) if (!procs[i].used) { slot = i; break; }
    if (slot < 0) { pthread_mutex_unlock(&proc_lock); return ERROR; }
    int master = -1, in = -1, of = -1, is_pty = 0;
    pid_t pid = -1;
    if (want_pty) {
        pid = kimi_guest_spawn_pty(exe, sysroot, argv, envp, rows, cols, &master);
        if (pid >= 0 && master >= 0) {
            is_pty = 1;
            int fl = fcntl(master, F_GETFL, 0);
            if (fl >= 0) fcntl(master, F_SETFL, fl | O_NONBLOCK);
        } else {
            pid = -1;
        }
    }
    if (!is_pty) {
        pid = kimi_guest_spawn(exe, sysroot, argv, envp, &in, &of);
        if (pid < 0 || in < 0 || of < 0) { pthread_mutex_unlock(&proc_lock); return ERROR; }
        int fl = fcntl(of, F_GETFL, 0);
        if (fl >= 0) fcntl(of, F_SETFL, fl | O_NONBLOCK);
    }
    procs[slot].master = is_pty ? master : -1;
    procs[slot].infd = is_pty ? -1 : in;
    procs[slot].outfd = is_pty ? -1 : of;
    procs[slot].pid = pid;
    procs[slot].used = 1; procs[slot].exited = 0; procs[slot].code = 0;
    pthread_mutex_unlock(&proc_lock);
    LOG("proc spawn slot=%d pid=%d exe=%s (%s)", slot, (int)pid, exe, is_pty ? "pty" : "pipes");
    return (uint64_t)slot;
}
static uint64_t proc_read(uint64_t slot, void *buf, uint64_t cap) {
    if (slot >= PROC_MAX) return 0;
    pthread_mutex_lock(&proc_lock);
    uint64_t r = 0;
    if (procs[slot].used) {
        int fd = procs[slot].master >= 0 ? procs[slot].master : procs[slot].outfd;
        if (fd >= 0) { ssize_t n = read(fd, buf, cap); if (n > 0) r = (uint64_t)n; }
    }
    pthread_mutex_unlock(&proc_lock);
    return r;
}
static uint64_t proc_write(uint64_t slot, const void *buf, uint64_t len) {
    if (slot >= PROC_MAX) return 0;
    pthread_mutex_lock(&proc_lock);
    uint64_t r = 0;
    if (procs[slot].used) {
        int fd = procs[slot].master >= 0 ? procs[slot].master : procs[slot].infd;
        if (fd >= 0) { ssize_t n = write(fd, buf, len); if (n > 0) r = (uint64_t)n; }
    }
    pthread_mutex_unlock(&proc_lock);
    return r;
}
static uint64_t proc_kill(uint64_t slot) {
    if (slot >= PROC_MAX) return ERROR;
    pthread_mutex_lock(&proc_lock);
    if (procs[slot].used && procs[slot].pid > 0) kill(procs[slot].pid, SIGHUP);
    pthread_mutex_unlock(&proc_lock);
    return 0;
}
static uint64_t proc_resize(uint64_t slot, uint64_t rows, uint64_t cols) {
    if (slot >= PROC_MAX) return ERROR;
    pthread_mutex_lock(&proc_lock);
    if (procs[slot].used && procs[slot].master >= 0) {
        struct winsize ws; memset(&ws, 0, sizeof ws);
        ws.ws_row = (unsigned short)rows; ws.ws_col = (unsigned short)cols;
        ioctl(procs[slot].master, TIOCSWINSZ, &ws);
        if (procs[slot].pid > 0) kill(procs[slot].pid, SIGWINCH);
    }
    pthread_mutex_unlock(&proc_lock);
    return 0;
}
static uint64_t proc_status(uint64_t slot) {
    if (slot >= PROC_MAX) return ERROR;
    pthread_mutex_lock(&proc_lock);
    uint64_t r = ERROR;
    if (procs[slot].used) {
        if (procs[slot].exited) {
            r = (uint64_t)procs[slot].code + 1;
        } else {
            int st = 0;
            pid_t w = waitpid(procs[slot].pid, &st, WNOHANG);
            if (w == procs[slot].pid) {
                procs[slot].exited = 1;
                procs[slot].code = WIFEXITED(st) ? WEXITSTATUS(st) : (256 + WTERMSIG(st));
                r = (uint64_t)procs[slot].code + 1;
            } else if (w == 0) {
                r = 0; // still running
            }
        }
    }
    pthread_mutex_unlock(&proc_lock);
    return r;
}
static uint64_t proc_close(uint64_t slot) {
    if (slot >= PROC_MAX) return ERROR;
    pthread_mutex_lock(&proc_lock);
    if (procs[slot].used) {
        if (procs[slot].pid > 0 && !procs[slot].exited) kill(procs[slot].pid, SIGHUP);
        if (procs[slot].master >= 0) close(procs[slot].master);
        if (procs[slot].infd >= 0) close(procs[slot].infd);
        if (procs[slot].outfd >= 0) close(procs[slot].outfd);
        if (procs[slot].pid > 0) { int st; waitpid(procs[slot].pid, &st, WNOHANG); }
        procs[slot].used = 0; procs[slot].master = -1; procs[slot].infd = -1; procs[slot].outfd = -1; procs[slot].pid = -1;
    }
    pthread_mutex_unlock(&proc_lock);
    return 0;
}

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
    case 57: { char url[512]; str_arg(a0, a1, url, sizeof url); str_arg(a2, a3, path, sizeof path); return fs_fetch_async(url, path); }
    case 58: return fs_fetch_progress(a0);
    case 59: { struct fetch_auth_req { uint64_t up, ul, dp, dl, kp, kl; } *rq = (void *)a0;
        char url[512], key[256]; str_arg(rq->up, rq->ul, url, sizeof url); str_arg(rq->kp, rq->kl, key, sizeof key);
        str_arg(rq->dp, rq->dl, path, sizeof path); return fs_fetch_auth(url, path, key); }
    case 43: return call_longs(g_local_time_mid, (uint64_t *)a0, 9); // SYS_LOCAL_TIME
    case 44: return call_longs(g_location_mid, (uint64_t *)a0, 4);   // SYS_LOCATION
    case 45: { char url[512]; str_arg(a0, a1, url, sizeof url); return call_str_long(g_open_url_mid, url); } // SYS_OPEN_URL
    case 46: { // SYS_AUDIO: a2 = cmd (0 play, 1 pause, 2 stop, 3 resume, 4 query)
        if (a2 == 4) return call_longs(g_audio_query_mid, (uint64_t *)a0, 3);
        if (a2 == 0) {
            str_arg(a0, a1, path, sizeof path);
            if (strncmp(path, "/disk/", 6) || !safe_rel(path + 6)) return ERROR;
            char abs[512]; disk_path(path + 6, abs, sizeof abs);
            return audio_cmd(abs, 0);
        }
        if (a2 <= 3) return audio_cmd(NULL, (int)a2);
        return ERROR;
    }
    case 47: { // SYS_IMPORT_FILE: launch SAF picker into /disk dir (async copy)
        str_arg(a0, a1, path, sizeof path);
        const char *rel;
        if (!strcmp(path, "/disk")) rel = "";
        else if (!strncmp(path, "/disk/", 6)) rel = path + 6;
        else return ERROR;
        if (!safe_rel(rel)) return ERROR;
        char abs[512]; disk_path(rel, abs, sizeof abs);
        return call_str_long(g_import_mid, abs);
    }
    case 48: { // SYS_WEB: embedded browser, sub-op in a0 (libkimi web_* wrappers)
        switch (a0) {
        case 0: { // init(w=a1, h=a2) device px
            if (a1 == 0 || a2 == 0 || a1 > 4096 || a2 > 4096) return ERROR;
            uint64_t r = call_ii_long(g_web_init_mid, (jint)a1, (jint)a2);
            return r == 0 ? 0 : ERROR;
        }
        case 1: return web_buf_addr(); // buf(): RGBA pixel buffer address
        case 2: return call_z_long(g_web_render_mid, (int)(a1 != 0)); // render(force): seq
        case 3: { // touch(x=a1, y=a2, action=a3: 0 down, 1 up, 2 move, 3 cancel)
            uint64_t r = call_iii_long(g_web_touch_mid, (jint)a1, (jint)a2, (jint)a3);
            return r == 0 ? 0 : ERROR;
        }
        case 4: { // load(url_ptr=a1, url_len=a2)
            char url[2048];
            str_arg(a1, a2, url, sizeof url);
            if (!url[0]) return ERROR;
            uint64_t r = call_str_long(g_web_load_mid, url);
            return r == 0 ? 0 : ERROR;
        }
        case 5: { uint64_t r = call_long(g_web_back_mid);    return r == 0 ? 0 : ERROR; }
        case 6: { uint64_t r = call_long(g_web_fwd_mid);     return r == 0 ? 0 : ERROR; }
        case 7: { uint64_t r = call_long(g_web_reload_mid);  return r == 0 ? 0 : ERROR; }
        case 8: return call_str_out(g_web_url_mid, (char *)a1, a2);   // url(buf, cap) -> len
        case 9: return call_str_out(g_web_title_mid, (char *)a1, a2); // title(buf, cap) -> len
        case 10: return call_long(g_web_canback_mid); // 0/1
        case 11: return call_long(g_web_canfwd_mid);  // 0/1
        case 12: { // viewport(w=a1, h=a2): re-layout + realloc pixel buffer
            if (a1 == 0 || a2 == 0 || a1 > 4096 || a2 > 4096) return ERROR;
            uint64_t r = call_ii_long(g_web_viewport_mid, (jint)a1, (jint)a2);
            return r == 0 ? 0 : ERROR;
        }
        case 13: { uint64_t r = call_long(g_web_pause_mid);  return r == 0 ? 0 : ERROR; }
        case 14: { uint64_t r = call_long(g_web_resume_mid); return r == 0 ? 0 : ERROR; }
        default: return ERROR;
        }
    }
    case 49: return call_longs(g_sysinfo_mid, (uint64_t *)a0, 10); // SYS_SYSINFO
    case 50: { // SYS_PROCLIST: flat 8-u64 records, a1 = max records
        if (a1 == 0 || a1 > 4096) return ERROR;
        uint64_t n = call_int_longs(g_proclist_mid, (jint)a1, (uint64_t *)a0, a1 * 8);
        return n == ERROR ? ERROR : n / 8;
    }
    case 51: { // SYS_RUN_PATH: execute a /disk/... binary via the guest loader
        char rp[512], rarg[4096];
        str_arg(a0, a1, rp, sizeof rp);
        str_arg(a2, a3, rarg, sizeof rarg);
        return fs_run_path(rp, rarg);
    }
    case 52: { // SYS_COPY: copy a file within /disk
        char src[512], dst[512];
        str_arg(a0, a1, src, sizeof src);
        str_arg(a2, a3, dst, sizeof dst);
        return fs_copy(src, dst);
    }
    case 53: { // SYS_PLUGIN_API: a0=0 start/query API server -> port;
               // a0=1 drain notifications into buf=a1 (cap=a2) as
               // NUL-terminated title\0body\0 strings -> bytes written.
        if (a0 == 0) {
            uint64_t p = call_long(g_plugin_api_mid);
            if (p == ERROR || p == 0 || p > 65535) return ERROR;
            return p;
        }
        if (a0 == 1) return call_bytes_out(g_plugin_notify_mid, (char *)a1, a2);
        return ERROR;
    }
    case 54: { // SYS_PLUGIN_RUN: a0=id ptr, a1=len, a2=background (0 wait, 1 detach)
        char id[128];
        str_arg(a0, a1, id, sizeof id);
        return fs_plugin_run(id, a2 != 0, NULL, 0, NULL);
    }
    case 55: { // SYS_PLUGIN_RUN_OUT: a0=req{id_ptr,id_len,out_ptr,out_cap,out_len,code}
        struct plugin_run_req { uint64_t id_ptr, id_len, out_ptr, out_cap, out_len, code; } *rq = (void *)a0;
        char id[128];
        str_arg(rq->id_ptr, rq->id_len, id, sizeof id);
        size_t olen = 0;
        uint64_t code = fs_plugin_run(id, 0, (char *)rq->out_ptr, (size_t)rq->out_cap, &olen);
        if (code == ERROR) return ERROR;
        rq->out_len = olen; rq->code = code;
        return code;
    }
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
    case 56: // SYS_PROC: interactive PTY sessions
        switch (a0) {
        case 0: return proc_spawn(a1);
        case 1: return proc_read(a1, (void *)a2, a3);
        case 2: return proc_write(a1, (const void *)a2, a3);
        case 3: return proc_kill(a1);
        case 4: return proc_resize(a1, a2, a3);
        case 5: return proc_status(a1);
        case 6: return proc_close(a1);
        case 7: return 1; // PTY capability
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
    // Runs on a Java thread (app classloader): cache the bridge class + method
    // IDs once so GUI-pthread syscalls never need FindClass.
    if (!g_bridge) {
        jclass local = (*env)->FindClass(env, "com/kimios/libos/KimiBridge");
        if (!local) { if ((*env)->ExceptionCheck(env)) (*env)->ExceptionClear(env); local = cls; }
        g_bridge = (jclass)(*env)->NewGlobalRef(env, local);
        g_fetch_mid       = (*env)->GetStaticMethodID(env, g_bridge, "nativeFetchUrl",  "(Ljava/lang/String;Ljava/lang/String;)J");
        g_fetch_async_mid = (*env)->GetStaticMethodID(env, g_bridge, "nativeFetchAsync","(Ljava/lang/String;Ljava/lang/String;)J");
        g_fetch_auth_mid = (*env)->GetStaticMethodID(env, g_bridge, "nativeFetchAuth","(Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;)J");
        g_fetch_progress_mid = (*env)->GetStaticMethodID(env, g_bridge, "fetchProgress", "()[J");
        g_local_time_mid  = (*env)->GetStaticMethodID(env, g_bridge, "nativeLocalTime", "()[J");
        g_location_mid    = (*env)->GetStaticMethodID(env, g_bridge, "nativeLocation",   "()[J");
        g_open_url_mid    = (*env)->GetStaticMethodID(env, g_bridge, "nativeOpenUrl",    "(Ljava/lang/String;)J");
        g_audio_mid       = (*env)->GetStaticMethodID(env, g_bridge, "nativeAudio",      "(Ljava/lang/String;I)J");
        g_audio_query_mid = (*env)->GetStaticMethodID(env, g_bridge, "nativeAudioQuery", "()[J");
        g_import_mid      = (*env)->GetStaticMethodID(env, g_bridge, "nativeImportFile", "(Ljava/lang/String;)J");
        g_sysinfo_mid     = (*env)->GetStaticMethodID(env, g_bridge, "nativeSysInfo",  "()[J");
        g_proclist_mid    = (*env)->GetStaticMethodID(env, g_bridge, "nativeProcList", "(I)[J");
        g_web_init_mid     = (*env)->GetStaticMethodID(env, g_bridge, "nativeWebInit",        "(II)J");
        g_web_buf_mid      = (*env)->GetStaticMethodID(env, g_bridge, "nativeWebBuf",         "()Ljava/nio/ByteBuffer;");
        g_web_render_mid   = (*env)->GetStaticMethodID(env, g_bridge, "nativeWebRender",      "(Z)J");
        g_web_touch_mid    = (*env)->GetStaticMethodID(env, g_bridge, "nativeWebTouch",       "(III)J");
        g_web_load_mid     = (*env)->GetStaticMethodID(env, g_bridge, "nativeWebLoad",        "(Ljava/lang/String;)J");
        g_web_back_mid     = (*env)->GetStaticMethodID(env, g_bridge, "nativeWebBack",        "()J");
        g_web_fwd_mid      = (*env)->GetStaticMethodID(env, g_bridge, "nativeWebForward",     "()J");
        g_web_reload_mid   = (*env)->GetStaticMethodID(env, g_bridge, "nativeWebReload",      "()J");
        g_web_url_mid      = (*env)->GetStaticMethodID(env, g_bridge, "nativeWebUrl",         "()Ljava/lang/String;");
        g_web_title_mid    = (*env)->GetStaticMethodID(env, g_bridge, "nativeWebTitle",       "()Ljava/lang/String;");
        g_web_canback_mid  = (*env)->GetStaticMethodID(env, g_bridge, "nativeWebCanGoBack",   "()J");
        g_web_canfwd_mid   = (*env)->GetStaticMethodID(env, g_bridge, "nativeWebCanGoForward","()J");
        g_web_viewport_mid = (*env)->GetStaticMethodID(env, g_bridge, "nativeWebViewport",    "(II)J");
        g_web_pause_mid    = (*env)->GetStaticMethodID(env, g_bridge, "nativeWebPause",       "()J");
        g_web_resume_mid   = (*env)->GetStaticMethodID(env, g_bridge, "nativeWebResume",      "()J");
        g_plugin_api_mid      = (*env)->GetStaticMethodID(env, g_bridge, "nativePluginApi",           "()J");
        g_plugin_manifest_mid = (*env)->GetStaticMethodID(env, g_bridge, "nativePluginManifest",    "(Ljava/lang/String;)[Ljava/lang/String;");
        g_plugin_notify_mid   = (*env)->GetStaticMethodID(env, g_bridge, "nativePluginNotifications","()[B");
        jmethodID setroot = (*env)->GetStaticMethodID(env, g_bridge, "setDiskRoot", "(Ljava/lang/String;)V");
        if (setroot) {
            jstring jr = (*env)->NewStringUTF(env, diskRoot);
            if (jr) {
                (*env)->CallStaticVoidMethod(env, g_bridge, setroot, jr);
                (*env)->DeleteLocalRef(env, jr);
            }
        }
        if ((*env)->ExceptionCheck(env)) (*env)->ExceptionClear(env);
        LOG("bridge class + method IDs cached");
    }
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

extern uint64_t kimi_test_kpkg(void);
JNIEXPORT jint JNICALL
Java_com_kimios_libos_KimiBridge_nativeKpkgUpdate(JNIEnv *env, jclass cls) {
    (void)env; (void)cls;
    uint64_t r = kimi_test_kpkg();
    LOG("kpkg update result=%llu", (unsigned long long)r);
    return (jint)r;
}

extern uint64_t kimi_test_install(uint64_t, uint64_t);
JNIEXPORT jint JNICALL
Java_com_kimios_libos_KimiBridge_nativeInstall(JNIEnv *env, jclass cls, jstring jname, jstring jdest) {
    (void)cls;
    const char *name = (*env)->GetStringUTFChars(env, jname, NULL);
    const char *dest = (*env)->GetStringUTFChars(env, jdest, NULL);
    uint64_t r = kimi_test_install((uint64_t)(uintptr_t)name, (uint64_t)(uintptr_t)dest);
    LOG("kpkg install %s -> %s result=%llu", name, dest, (unsigned long long)r);
    (*env)->ReleaseStringUTFChars(env, jname, name);
    (*env)->ReleaseStringUTFChars(env, jdest, dest);
    return (jint)r;
}
JNIEXPORT jint JNICALL
Java_com_kimios_libos_KimiBridge_nativeSysTest(JNIEnv *env, jclass cls) {
    (void)env; (void)cls;
    uint64_t si[10] = {0};
    uint64_t r = kimi_host_syscall(49, (uint64_t)(uintptr_t)si, 0, 0, 0, 0);
    LOG("SYSINFO r=%lld cpu_x10=%llu ram_tot_kb=%llu ram_used_kb=%llu stor_tot_kb=%llu stor_used_kb=%llu uptime=%llu procs=%llu batt=%llu",
        (long long)r, (unsigned long long)si[0], (unsigned long long)si[1], (unsigned long long)si[2],
        (unsigned long long)si[3], (unsigned long long)si[4], (unsigned long long)si[5],
        (unsigned long long)si[6], (unsigned long long)si[7]);
    static uint64_t pl[8 * 32];
    uint64_t n = kimi_host_syscall(50, (uint64_t)(uintptr_t)pl, 32, 0, 0, 0);
    LOG("PROCLIST n=%llu", (unsigned long long)n);
    for (uint64_t i = 0; i < 3 && i < n; i++) {
        uint64_t *rec = pl + i * 8; char nm[25]; memcpy(nm, &rec[5], 24); nm[24] = 0;
        LOG("  proc pid=%llu ram_kb=%llu cpu_x10=%llu name=%s",
            (unsigned long long)rec[0], (unsigned long long)rec[2], (unsigned long long)rec[3], nm);
    }
    // copy test: create source, copy, stat dest
    const char *src = "/disk/systest.txt"; const char *dst = "/disk/systest_copy.txt";
    uint64_t h = kimi_host_syscall(13, (uint64_t)(uintptr_t)src, strlen(src), 0, 0, 0);
    if (h != ERROR) { kimi_host_syscall(14, h, 0, (uint64_t)(uintptr_t)"hello", 5, 0); kimi_host_syscall(6, h, 0, 0, 0, 0); }
    uint64_t cr = kimi_host_syscall(52, (uint64_t)(uintptr_t)src, strlen(src), (uint64_t)(uintptr_t)dst, strlen(dst), 0);
    uint64_t st[3] = {0};
    uint64_t sr = kimi_host_syscall(27, (uint64_t)(uintptr_t)dst, strlen(dst), (uint64_t)(uintptr_t)st, 0, 0);
    LOG("COPY r=%llu stat_r=%llu dst_size=%llu", (unsigned long long)cr, (unsigned long long)sr, (unsigned long long)st[1]);
    return 0;
}

extern uint64_t kimi_request_app(uint64_t);
JNIEXPORT jint JNICALL
Java_com_kimios_libos_KimiBridge_nativeOpenApp(JNIEnv *env, jclass cls, jint kind) {
    (void)env; (void)cls;
    kimi_request_app((uint64_t)kind);
    return 0;
}

// Test/bridge entry for SYS_PLUGIN_API/RUN (intent extras "api"/"plugin").
JNIEXPORT jlong JNICALL
Java_com_kimios_libos_KimiBridge_nativePluginTest(JNIEnv *env, jclass cls, jstring jid, jint background) {
    (void)cls;
    if (!jid) {
        uint64_t r = kimi_host_syscall(53, 0, 0, 0, 0, 0);
        LOG("plugin api port=%llu", (unsigned long long)r);
        return (jlong)r;
    }
    const char *id = (*env)->GetStringUTFChars(env, jid, NULL);
    uint64_t r = kimi_host_syscall(54, (uint64_t)(uintptr_t)id, strlen(id), background ? 1 : 0, 0, 0);
    LOG("plugin run %s bg=%d result=%llu", id, (int)background, (unsigned long long)r);
    (*env)->ReleaseStringUTFChars(env, jid, id);
    return (jlong)r;
}

extern uint64_t kimi_test_import_alpine(uint64_t);
JNIEXPORT jint JNICALL
Java_com_kimios_libos_KimiBridge_nativeImportAlpine(JNIEnv *env, jclass cls, jstring jpkg) {
    (void)cls;
    const char *pkg = (*env)->GetStringUTFChars(env, jpkg, NULL);
    uint64_t r = kimi_test_import_alpine((uint64_t)(uintptr_t)pkg);
    LOG("import-alpine %s result=%llu", pkg, (unsigned long long)r);
    (*env)->ReleaseStringUTFChars(env, jpkg, pkg);
    return (jint)r;
}

extern uint64_t kimi_test_store_count(uint64_t, uint64_t);
extern uint64_t kimi_test_store_names(uint64_t, uint64_t, uint64_t, uint64_t);
extern uint64_t kimi_test_store_run(uint64_t, uint64_t, uint64_t, uint64_t);
extern uint64_t kimi_test_term(uint64_t, uint64_t);
JNIEXPORT jlong JNICALL
Java_com_kimios_libos_KimiBridge_nativeTermTest(JNIEnv *env, jclass cls) {
    (void)env; (void)cls;
    static char out[4096];
    uint64_t n = kimi_test_term((uint64_t)(uintptr_t)out, sizeof out - 1);
    out[n] = 0;
    LOG("term-test bytes=%llu stream=[%.*s]", (unsigned long long)n, (int)n, out);
    return (jlong)n;
}
JNIEXPORT jlong JNICALL
Java_com_kimios_libos_KimiBridge_nativeStoreCount(JNIEnv *env, jclass cls, jint filter, jstring jq) {
    (void)cls;
    const char *q = jq ? (*env)->GetStringUTFChars(env, jq, NULL) : "";
    uint64_t n = kimi_test_store_count((uint64_t)filter, (uint64_t)(uintptr_t)q);
    char names[512];
    uint64_t len = kimi_test_store_names((uint64_t)filter, (uint64_t)(uintptr_t)q,
                                         (uint64_t)(uintptr_t)names, sizeof names - 1);
    names[len] = 0;
    LOG("mstore filter=%d q='%s' count=%llu names=[%s]", (int)filter, q,
        (unsigned long long)n, names);
    if (jq) (*env)->ReleaseStringUTFChars(env, jq, q);
    return (jlong)n;
}

JNIEXPORT jlong JNICALL
Java_com_kimios_libos_KimiBridge_nativeStoreRun(JNIEnv *env, jclass cls, jstring jname, jint src) {
    (void)cls;
    const char *nm = (*env)->GetStringUTFChars(env, jname, NULL);
    static char out[2048];
    uint64_t r = kimi_test_store_run((uint64_t)(uintptr_t)nm, (uint64_t)src,
                                     (uint64_t)(uintptr_t)out, sizeof out - 1);
    // `r` is the full output length + 1, not the number of bytes copied into
    // `out` (the Rust side clamps before copying). Clamp here too, otherwise a
    // store item whose output exceeds the buffer (e.g. busybox usage text)
    // writes the NUL terminator far past the end of `out`.
    size_t n = (r > 0) ? (size_t)(r - 1) : 0;
    if (n > sizeof out - 1) n = sizeof out - 1;
    out[n] = 0;
    LOG("mstore run src=%d name='%s' result=%llu out=[%.*s]", (int)src, nm,
        (unsigned long long)r, (int)n, out);
    (*env)->ReleaseStringUTFChars(env, jname, nm);
    return (jlong)r;
}
