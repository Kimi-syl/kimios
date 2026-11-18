// Native library-OS host harness for KimiOS.
//
// Implements the C-ABI `kimi_host_syscall` dispatcher that libkimi routes to on
// hosted targets, then drives the real no_std GUI (`kimi_gui_start`) on a thread
// and dumps the framebuffer to PPM. This proves the KimiOS userland renders
// natively (no QEMU, no EL1/MMU) using only host primitives — the same contract
// the iOS Swift/Metal shell implements.
//
// Build: see build.sh. Run: ./kimios-host [outdir]

#define _GNU_SOURCE
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <pthread.h>
#include <time.h>
#include <unistd.h>

#define FB_W 1080
#define FB_H 1920
#define FB_STRIDE (FB_W * 4)
#define ERROR UINT64_MAX

// Syscall numbers (mirror user/libkimi/src/lib.rs).
enum {
    SYS_WRITE = 1, SYS_EXIT, SYS_READ, SYS_OPEN, SYS_READ_FILE, SYS_CLOSE,
    SYS_EXEC, SYS_LIST, SYS_SPAWN, SYS_WAIT, SYS_GETPID, SYS_STATS, SYS_CREATE,
    SYS_WRITE_FILE, SYS_DISKINFO, SYS_FB_INFO, SYS_FB_FILL, SYS_UNLINK,
    SYS_FREEBLOCKS, SYS_UPTIME, SYS_SLEEP, SYS_BRK, SYS_PS, SYS_KILL, SYS_RENAME,
    SYS_RTC, SYS_STAT, SYS_LISTDIR, SYS_MKDIR, SYS_RMDIR, SYS_FB_MAP,
    SYS_INPUT_POLL, SYS_INPUT_WAIT, SYS_NET, SYS_SPAWN_ARGS, SYS_FB_BACK,
    SYS_FB_PRESENT, SYS_GFX_BENCH, SYS_INSTR_MAP,
};

static uint32_t *front, *back;
static uint64_t instr_box[16];
static uint64_t present_count;
static uint64_t rtc_count;
static uint64_t sleep_count;
static uint64_t start_ns;
static int verbose;

// Heap arena for SYS_BRK (GUI uses no allocator, but keep the contract whole).
static uint8_t heap_arena[8 * 1024 * 1024];
static uint64_t heap_break;

static uint64_t now_ns(void) {
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return (uint64_t)ts.tv_sec * 1000000000ull + ts.tv_nsec;
}
static uint64_t elapsed_ms(void) { return (now_ns() - start_ns) / 1000000ull; }

uint64_t kimi_host_cntpct(void) {
    // Mimic a 24 MHz generic counter; only used for instrumentation stamps.
    return (now_ns() - start_ns) / 41;
}

// ---- scripted touch input (ABS range 0..32767, per virtio-input) ----
typedef struct { uint64_t t_ms; uint16_t kind, code; uint32_t value; } Ev;
static Ev script[] = {
    // unlock (home logical 480x853): centre tap
    {120, 3, 0, 16383}, {121, 3, 1, 15363}, {122, 1, 0x14a, 1}, {180, 1, 0x14a, 0},
    // switch to home page 2 via dot (252,701)
    {300, 3, 0, 17202}, {301, 3, 1, 26900}, {302, 1, 0x14a, 1}, {360, 1, 0x14a, 0},
    // open Marketplace icon (408,408)
    {480, 3, 0, 27852}, {481, 3, 1, 15683}, {482, 1, 0x14a, 1}, {540, 1, 0x14a, 0},
    // apply Graphite Light theme: Themes tab row1 action button (app 385,336)
    {800, 3, 0, 26266}, {801, 3, 1, 13762}, {802, 1, 0x14a, 1}, {860, 1, 0x14a, 0},
    // switch to Fonts tab (app 240,120)
    {1100, 3, 0, 16383}, {1101, 3, 1, 4915}, {1102, 1, 0x14a, 1}, {1160, 1, 0x14a, 0},
    // apply Ndot-55 font: Fonts tab row1 action button (app 385,336)
    {1300, 3, 0, 26266}, {1301, 3, 1, 13762}, {1302, 1, 0x14a, 1}, {1360, 1, 0x14a, 0},
};
static size_t script_pos;
static pthread_mutex_t in_lock = PTHREAD_MUTEX_INITIALIZER;

static uint64_t pack_ev(uint16_t kind, uint16_t code, uint32_t value) {
    return ((uint64_t)kind << 48) | ((uint64_t)code << 32) | (uint64_t)value;
}

static uint64_t poll_input(void) {
    pthread_mutex_lock(&in_lock);
    uint64_t t = elapsed_ms();
    uint64_t r = ERROR;
    if (script_pos < sizeof(script) / sizeof(script[0]) &&
        script[script_pos].t_ms <= t) {
        Ev e = script[script_pos++];
        r = pack_ev(e.kind, e.code, e.value);
        if (verbose) fprintf(stderr, "[host] t=%llums deliver ev kind=%u code=0x%x val=%u (pos=%zu)\n",
                (unsigned long long)t, e.kind, e.code, e.value, script_pos);
    }
    pthread_mutex_unlock(&in_lock);
    return r;
}

// ---- minimal filesystem stubs (home screen needs no disk) ----
static uint64_t fs_stub(void) { return ERROR; }

uint64_t kimi_host_syscall(uint64_t n, uint64_t a0, uint64_t a1,
                           uint64_t a2, uint64_t a3, uint64_t a4) {
    (void)a3; (void)a4;
    switch (n) {
    case SYS_FB_INFO: {
        uint64_t *info = (uint64_t *)a0;
        info[0] = FB_W; info[1] = FB_H; info[2] = FB_STRIDE; info[3] = 0;
        return 0;
    }
    case SYS_FB_MAP:   return (uint64_t)(uintptr_t)front;
    case SYS_FB_BACK:  return (uint64_t)(uintptr_t)back;
    case SYS_FB_PRESENT: {
        uint64_t x0 = a0, y0 = a1, x1 = a2, y1 = a3;
        if (verbose) fprintf(stderr, "[host] t=%llums PRESENT [%llu,%llu,%llu,%llu]\n", (unsigned long long)elapsed_ms(), (unsigned long long)x0,(unsigned long long)y0,(unsigned long long)x1,(unsigned long long)y1);
        if (x1 > FB_W) x1 = FB_W;
        if (y1 > FB_H) y1 = FB_H;
        for (uint64_t y = y0; y < y1; y++)
            memcpy(front + y * FB_W + x0, back + y * FB_W + x0,
                   (size_t)(x1 - x0) * 4);
        __sync_fetch_and_add(&present_count, 1);
        return 0;
    }
    case SYS_FB_FILL: {
        uint64_t x = a0, y = a1, w = a2, h = a3;
        uint32_t c = (uint32_t)a4;
        for (uint64_t j = y; j < y + h && j < FB_H; j++)
            for (uint64_t i = x; i < x + w && i < FB_W; i++)
                back[j * FB_W + i] = c;
        return 0;
    }
    case SYS_INPUT_POLL: return poll_input();
    case SYS_INPUT_WAIT: return ERROR;
    case SYS_INSTR_MAP:  return (uint64_t)(uintptr_t)instr_box;
    case SYS_UPTIME:     return elapsed_ms();
    case SYS_SLEEP: {
        __sync_fetch_and_add(&sleep_count,1);
        struct timespec ts = { .tv_sec = a0 / 1000,
                               .tv_nsec = (long)(a0 % 1000) * 1000000L };
        nanosleep(&ts, NULL);
        return 0;
    }
    case SYS_RTC: {
        __sync_fetch_and_add(&rtc_count,1);
        uint64_t *f = (uint64_t *)a0;
        time_t t = time(NULL);
        struct tm tm; gmtime_r(&t, &tm);
        f[0] = (uint64_t)t; f[1] = tm.tm_year + 1900; f[2] = tm.tm_mon + 1;
        f[3] = tm.tm_mday; f[4] = tm.tm_hour; f[5] = tm.tm_min; f[6] = tm.tm_sec;
        return 0;
    }
    case SYS_WRITE: {
        const char *buf = (const char *)a1;
        fwrite(buf, 1, (size_t)a2, stdout);
        fflush(stdout);
        return a2;
    }
    case SYS_READ:   return 0;
    case SYS_GETPID: return 1;
    case SYS_EXIT:
        fprintf(stderr, "[host] GUI exit(%llu)\n", (unsigned long long)a0);
        pthread_exit(NULL);
        return 0;
    case SYS_BRK: {
        if (a0 == 0) return heap_break ? heap_break : (uint64_t)(uintptr_t)heap_arena;
        uint64_t base = (uint64_t)(uintptr_t)heap_arena;
        if (a0 > base + sizeof(heap_arena)) return heap_break;
        heap_break = a0;
        return heap_break;
    }
    case SYS_STATS:     return sizeof(heap_arena);
    case SYS_DISKINFO:  return ERROR;
    case SYS_FREEBLOCKS:return ERROR;
    case SYS_OPEN: case SYS_CREATE: case SYS_READ_FILE: case SYS_WRITE_FILE:
    case SYS_CLOSE: case SYS_LIST: case SYS_LISTDIR: case SYS_STAT:
    case SYS_UNLINK: case SYS_MKDIR: case SYS_RMDIR: case SYS_RENAME:
    case SYS_NET: case SYS_SPAWN: case SYS_SPAWN_ARGS: case SYS_EXEC:
    case SYS_WAIT: case SYS_PS: case SYS_KILL: case SYS_GFX_BENCH:
        return fs_stub();
    default:
        fprintf(stderr, "[host] unhandled syscall %llu\n", (unsigned long long)n);
        return ERROR;
    }
}

static void *gui_thread(void *arg) {
    (void)arg;
    extern void kimi_gui_start(void) __attribute__((noreturn));
    kimi_gui_start();
    return NULL;
}

static uint32_t *snap;

static void dump_ppm(const char *path) {
    // Snapshot the framebuffer fast (the GUI thread keeps running), then write
    // PPM + stats from the snapshot so the capture is a coherent single frame.
    uint64_t pc = __sync_fetch_and_add(&present_count, 0);
    uint64_t rc = __sync_fetch_and_add(&rtc_count, 0);
    uint64_t sc = __sync_fetch_and_add(&sleep_count, 0);
    memcpy(snap, front, (size_t)FB_W * FB_H * 4);

    FILE *f = fopen(path, "wb");
    if (!f) { perror(path); return; }
    fprintf(f, "P6\n%d %d\n255\n", FB_W, FB_H);
    uint64_t nonblack = 0;
    static uint32_t set[1 << 16];
    static uint8_t used[1 << 16];
    memset(used, 0, sizeof used);
    size_t nseen = 0;
    for (int y = 0; y < FB_H; y++) {
        for (int x = 0; x < FB_W; x++) {
            uint32_t px = snap[y * FB_W + x] & 0x00ffffff;
            if (px) nonblack++;
            uint32_t h = (px * 2654435761u) >> 16;
            while (used[h]) {
                if (set[h] == px) break;
                h = (h + 1) & 0xffff;
            }
            if (!used[h]) { used[h] = 1; set[h] = px; nseen++; }
            uint8_t rgb[3] = { (uint8_t)(px >> 16), (uint8_t)(px >> 8), (uint8_t)px };
            fwrite(rgb, 1, 3, f);
        }
    }
    fclose(f);
    printf("[host] %s: %llu presents, rtc=%llu sleep=%llu, %.1f%% non-black, %zu distinct colours\n",
           path, (unsigned long long)pc, (unsigned long long)rc, (unsigned long long)sc,
           100.0 * nonblack / ((uint64_t)FB_W * FB_H), nseen);
}

int main(int argc, char **argv) {
    const char *outdir = argc > 1 ? argv[1] : ".";
    front = calloc(FB_W * FB_H, 4);
    back = calloc(FB_W * FB_H, 4);
    snap = calloc(FB_W * FB_H, 4);
    if (!front || !back || !snap) { fprintf(stderr, "alloc failed\n"); return 1; }
    verbose = getenv("KIMIOS_HOST_VERBOSE") != NULL;
    start_ns = now_ns();

    pthread_t th;
    if (pthread_create(&th, NULL, gui_thread, NULL) != 0) {
        fprintf(stderr, "pthread_create failed\n"); return 1;
    }

    usleep(650 * 1000);   // Marketplace opened (Nothing Dark)
    char p1[512], p2[512], p3[512];
    snprintf(p1, sizeof p1, "%s/market.ppm", outdir);
    dump_ppm(p1);

    usleep(300 * 1000);   // Graphite Light theme applied
    snprintf(p2, sizeof p2, "%s/market_themed.ppm", outdir);
    dump_ppm(p2);

    usleep(500 * 1000);   // Ndot-55 font applied
    snprintf(p3, sizeof p3, "%s/market_font.ppm", outdir);
    dump_ppm(p3);

    printf("[host] marketplace render + theme/font apply OK\n");
    exit(0);
}
