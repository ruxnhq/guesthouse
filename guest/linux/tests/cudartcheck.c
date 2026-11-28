// SPDX-License-Identifier: GPL-2.0-or-later
/* The CUDA runtime, which is what a program built with nvcc goes through, as against the driver API of cudavec.
 * It starts the runtime, reads the clock rates (the runtime's own start asks the driver for those), and fills
 * device memory and managed memory, with every value checked. It needs a libcudart, which comes with a CUDA
 * toolkit and not with the driver, so it is opened at run time; with none, it says so and exits 77.
 * Build: gcc -std=gnu17 -O2 -o cudartcheck cudartcheck.c -ldl */
#include <dlfcn.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

#define NO_RUNTIME 77
#define BYTES (4u << 20)
#define SIZE(n) ((size_t)(n)) /* the calls take a size_t, and nothing here has their prototypes */
#define DEV_ATTR_CLOCK_RATE 13
#define DEV_ATTR_MEMORY_CLOCK_RATE 36
#define MEMCPY_DEVICE_TO_HOST 2
#define ATTACH_GLOBAL 1

typedef int (*F)();
static const char *(*error_string)(int);

static const char *const names[] = {
    "libcudart.so", "libcudart.so.13", "libcudart.so.12", "libcudart.so.11.0",
    "/usr/local/cuda/lib64/libcudart.so", "/usr/local/cuda/targets/x86_64-linux/lib/libcudart.so",
};

#define LOAD(n) F n = (F)dlsym(h, #n); if (!n) { printf("%s has no %s\n", name, #n); return 1; }
#define C(x) do { int r_ = (x); if (r_) { printf("%s = %d (%s)\n", #x, r_, error_string ? error_string(r_) : "?"); \
                  return 1; } } while (0)

int main(void)
{
    const char *name = NULL;
    void *h = NULL;
    int version = 0, count = 0, clock = 0, memory_clock = 0;
    unsigned char *device, *managed, *back;
    unsigned seed = (unsigned)time(NULL) % 200 + 1, wrong = 0;

    for (size_t i = 0; i < sizeof(names) / sizeof(names[0]) && !h; i++) {
        h = dlopen(names[i], RTLD_NOW);
        name = names[i];
    }
    if (!h) {
        printf("no CUDA runtime here: a CUDA toolkit brings one (nvidia-cuda-toolkit on Ubuntu)\n");
        return NO_RUNTIME;
    }
    error_string = (const char *(*)(int))dlsym(h, "cudaGetErrorString");
    LOAD(cudaRuntimeGetVersion) LOAD(cudaGetDeviceCount) LOAD(cudaDeviceGetAttribute) LOAD(cudaMalloc)
    LOAD(cudaMemset) LOAD(cudaMemcpy) LOAD(cudaMallocManaged) LOAD(cudaDeviceSynchronize) LOAD(cudaFree)

    C(cudaRuntimeGetVersion(&version));
    C(cudaGetDeviceCount(&count));
    if (count < 1) {
        printf("runtime %d sees no device\n", version);
        return 1;
    }
    C(cudaDeviceGetAttribute(&clock, DEV_ATTR_CLOCK_RATE, 0));
    C(cudaDeviceGetAttribute(&memory_clock, DEV_ATTR_MEMORY_CLOCK_RATE, 0));

    back = malloc(BYTES);
    if (!back) {
        return 1;
    }
    C(cudaMalloc((void **)&device, SIZE(BYTES)));
    C(cudaMemset(device, (int)seed, SIZE(BYTES)));
    C(cudaMemcpy(back, device, SIZE(BYTES), MEMCPY_DEVICE_TO_HOST));
    for (unsigned i = 0; i < BYTES; i++) {
        wrong += back[i] != seed;
    }

    /* Managed memory goes through UVM: the host side writes half, the device the other half. */
    C(cudaMallocManaged((void **)&managed, SIZE(BYTES), ATTACH_GLOBAL));
    memset(managed, seed ^ 0x5a, BYTES / 2);
    C(cudaMemset(managed + BYTES / 2, (int)seed, SIZE(BYTES / 2)));
    C(cudaDeviceSynchronize());
    for (unsigned i = 0; i < BYTES; i++) {
        wrong += managed[i] != (i < BYTES / 2 ? (seed ^ 0x5a) : seed);
    }
    C(cudaFree(managed));
    C(cudaFree(device));
    free(back);
    printf("runtime %d from %s, clocks %d and %d kHz, seed %u: checked %u values, %u wrong\n", version, name, clock,
           memory_clock, seed, 2 * BYTES, wrong);
    return wrong != 0;
}
