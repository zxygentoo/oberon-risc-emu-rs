/* Generate differential test vectors for the software FP + idiv routines by
 * calling the C reference directly, so the Rust port (src/fp.rs) can be checked
 * bit-for-bit against known-good output without a C toolchain at test time.
 *
 * Regenerate (run from the repo root):
 *
 *   C=/home/zxy/Projects/oberon-risc-emu/src
 *   gcc -O2 -I "$C" tools/gen_fp_vectors.c "$C/risc-fp.c" -o /tmp/gen_fp \
 *     && /tmp/gen_fp > tests/data/fp_vectors.txt
 *
 * Output format, one record per line (all values hex, no 0x prefix):
 *   A <x> <y> <u> <v> <z>   fp_add(x, y, u, v) = z
 *   M <x> <y> <z>           fp_mul(x, y)       = z
 *   D <x> <y> <z>           fp_div(x, y)       = z
 *   I <x> <y> <s> <q> <r>   idiv(x, y, s)      = {quot=q, rem=r}
 */
#include <stddef.h>
#include <stdint.h>
#include <stdio.h>
#include "risc-fp.h"

/* Deterministic xorshift32 so regeneration is reproducible. */
static uint32_t rng_state = 0x1234567u;
static uint32_t xrand(void) {
  uint32_t x = rng_state;
  x ^= x << 13;
  x ^= x >> 17;
  x ^= x << 5;
  return rng_state = x;
}

/* Boundary bit patterns: signed zeros, +/-1, +/-2, max/min normal, max
 * subnormal, infinities, NaNs, large integers-as-floats, pi, and small
 * integers (exercised by the FLT path). */
static const uint32_t edges[] = {
    0x00000000, 0x80000000, 0x3F800000, 0xBF800000, 0x40000000, 0xC0000000,
    0x7F7FFFFF, 0xFF7FFFFF, 0x00800000, 0x80800000, 0x007FFFFF, 0x807FFFFF,
    0x7F800000, 0xFF800000, 0x7FC00000, 0x00000001, 0x4B000000, 0xCB000000,
    0x40490FDB, 0x3FC00000, 0x12345678, 0xDEADBEEF, 0x000000FF, 0x0000FFFF,
};
#define NE (sizeof(edges) / sizeof(edges[0]))

int main(void) {
  for (int u = 0; u <= 1; u++) {
    for (int v = 0; v <= 1; v++) {
      for (size_t i = 0; i < NE; i++)
        for (size_t j = 0; j < NE; j++)
          printf("A %08X %08X %d %d %08X\n", edges[i], edges[j], u, v,
                 fp_add(edges[i], edges[j], u, v));
      for (int k = 0; k < 1000; k++) {
        uint32_t a = xrand(), b = xrand();
        printf("A %08X %08X %d %d %08X\n", a, b, u, v, fp_add(a, b, u, v));
      }
    }
  }

  for (size_t i = 0; i < NE; i++)
    for (size_t j = 0; j < NE; j++) {
      printf("M %08X %08X %08X\n", edges[i], edges[j], fp_mul(edges[i], edges[j]));
      printf("D %08X %08X %08X\n", edges[i], edges[j], fp_div(edges[i], edges[j]));
    }
  for (int k = 0; k < 2000; k++) {
    uint32_t a = xrand(), b = xrand();
    printf("M %08X %08X %08X\n", a, b, fp_mul(a, b));
    printf("D %08X %08X %08X\n", a, b, fp_div(a, b));
  }

  for (int s = 0; s <= 1; s++) {
    for (size_t i = 0; i < NE; i++)
      for (size_t j = 0; j < NE; j++) {
        struct idiv q = idiv(edges[i], edges[j], s);
        printf("I %08X %08X %d %08X %08X\n", edges[i], edges[j], s, q.quot, q.rem);
      }
    for (int k = 0; k < 1500; k++) {
      uint32_t a = xrand(), b = xrand();
      struct idiv q = idiv(a, b, s);
      printf("I %08X %08X %d %08X %08X\n", a, b, s, q.quot, q.rem);
    }
  }
  return 0;
}
