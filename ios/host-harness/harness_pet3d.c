// Host harness for the runtime 3D path.
//
// Loads a GLB from disk, renders one frame through the real k3d pipeline on the
// host (no QEMU, no device) and reports what it produced. This exercises the
// loader, skinning, morph targets, rasteriser and the framebuffer hash, so a
// regression in any of them fails here instead of only showing up on a phone.
//
// Build (x86_64 Linux host):
//   cargo build --release -p kimi-gui --lib   # from user/
//   cc -O2 -o kimios-pet3d harness_pet3d.c libkimi_gui.a -lpthread -lm
//   ./kimios-pet3d assets/models/testchar.glb

#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>

uint64_t kimi_host_syscall(uint64_t n, uint64_t a0, uint64_t a1, uint64_t a2,
                           uint64_t a3, uint64_t a4) {
    (void)n; (void)a0; (void)a1; (void)a2; (void)a3; (void)a4;
    return 0;
}

// The Rust staticlib emits a debug-info reference to the panic personality even
// with panic=abort; supply a stub so a plain C link succeeds.
void rust_eh_personality(void) {}
uint64_t kimi_host_cntpct(void) { return 0; }

extern uint64_t kimi_test_pet3d(uint64_t ptr, uint64_t len);

static const char *kNames[10] = {
    "loaded",        "vertices",     "triangles",   "skin/joints",
    "morph-targets", "raster-tris",  "drawn",       "pixels-covered",
    "non-blank",     "morph-changes-image",
};

int main(int argc, char **argv) {
    if (argc < 2) {
        fprintf(stderr, "usage: %s <model.glb>\n", argv[0]);
        return 2;
    }
    FILE *f = fopen(argv[1], "rb");
    if (!f) {
        fprintf(stderr, "cannot open %s\n", argv[1]);
        return 2;
    }
    fseek(f, 0, SEEK_END);
    long size = ftell(f);
    fseek(f, 0, SEEK_SET);
    if (size <= 0) {
        fprintf(stderr, "empty file\n");
        fclose(f);
        return 2;
    }
    unsigned char *data = (unsigned char *)malloc((size_t)size);
    if (!data) {
        fclose(f);
        return 2;
    }
    if (fread(data, 1, (size_t)size, f) != (size_t)size) {
        fprintf(stderr, "short read\n");
        free(data);
        fclose(f);
        return 2;
    }
    fclose(f);

    printf("model: %s (%ld bytes)\n", argv[1], size);
    uint64_t r = kimi_test_pet3d((uint64_t)(uintptr_t)data, (uint64_t)size);
    printf("pet3d result = 0x%03llx\n", (unsigned long long)r);
    for (int i = 0; i < 10; i++) {
        printf("  [%-20s] %s\n", kNames[i], (r >> i) & 1 ? "PASS" : "FAIL");
    }
    free(data);

    // All bits except the optional morph one must pass for this asset; morphs are
    // optional so a model without expressions is still a pass.
    uint64_t required = 0x1ff; // bits 0..8
    uint64_t ok = (r & required) == required;
    printf("RESULT: %s\n", ok ? "PASS" : "FAIL");
    return ok ? 0 : 1;
}