/* Capture golden framebuffer + CPU-state hashes from a deterministic headless
 * boot of the C reference, so the Rust port (tests/cpu.rs) can prove it
 * reproduces the exact same boot bit-for-bit at fixed checkpoints.
 *
 * Determinism (shared with the Rust side): zero-initialised RAM (calloc), a
 * fresh copy of the disk image, a synthetic 60 Hz clock, and no input. Because
 * risc_run + its progress watchdog are ported verbatim, both sides take the
 * same number of steps per frame and stay in lockstep.
 *
 * Regenerate (run from the repo root); the image is copied first because the
 * boot writes to disk:
 *
 *   C=/home/zxy/Projects/oberon-risc-emu/src
 *   gcc -O2 -I "$C" tools/gen_boot_golden.c "$C/risc-fp.c" "$C/disk.c" -o /tmp/gen_boot
 *   cp <image>.dsk /tmp/golden.dsk && /tmp/gen_boot /tmp/golden.dsk
 *
 * Including risc.c reaches its `static` internals and `struct RISC` fields
 * without modifying the reference.
 */
#include "risc.c"
#include "disk.h"
#include <stddef.h>
#include <stdint.h>
#include <stdio.h>

#define CPU_HZ 25000000
#define FPS 60
#define FRAME_MS (1000 / FPS)

/* Frame counts (1-indexed) at which to emit hashes; must match tests/cpu.rs. */
static const int checkpoints[] = {1, 2, 5, 15, 45, 120, 250};
#define NCP ((int)(sizeof(checkpoints) / sizeof(checkpoints[0])))

static uint64_t fnv1a(const uint8_t *p, size_t n) {
  uint64_t h = 14695981039346656037ULL;
  for (size_t i = 0; i < n; i++) {
    h ^= p[i];
    h *= 1099511628211ULL;
  }
  return h;
}

int main(int argc, char **argv) {
  if (argc != 2) {
    fprintf(stderr, "usage: %s DISK-IMAGE\n", argv[0]);
    return 2;
  }
  struct RISC *r = risc_new();
  risc_set_spi(r, 1, disk_new(argv[1]));

  int total = checkpoints[NCP - 1];
  int ci = 0;
  for (int frame = 0; frame < total; frame++) {
    risc_set_time(r, (uint32_t)frame * FRAME_MS);
    risc_run(r, CPU_HZ / FPS);

    if (ci < NCP && frame + 1 == checkpoints[ci]) {
      uint32_t *fb = risc_get_framebuffer_ptr(r);
      size_t words = (size_t)r->fb_width * (size_t)r->fb_height;
      uint64_t fbh = fnv1a((const uint8_t *)fb, words * 4);

      uint32_t st[19];
      st[0] = r->PC;
      for (int i = 0; i < 16; i++) st[1 + i] = r->R[i];
      st[17] = r->H;
      st[18] = (r->Z ? 1u : 0u) | (r->N ? 2u : 0u) | (r->C ? 4u : 0u) | (r->V ? 8u : 0u);
      uint64_t sh = fnv1a((const uint8_t *)st, sizeof(st));

      printf("%d %016llx %016llx\n", frame + 1, (unsigned long long)fbh,
             (unsigned long long)sh);
      ci++;
    }
  }
  return 0;
}
