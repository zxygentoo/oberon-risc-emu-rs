/* Co-simulation shim for differential testing (compiled only under the `cosim`
 * feature, with risc-fp.c + disk.c). Including risc.c reaches its `static`
 * functions and `struct RISC` fields without modifying the reference, and we
 * re-export a stable C ABI the Rust tests bind to. */
#include <stdint.h>

#include "risc.c"
#include "disk.h"

/* ---- Software FP / idiv (layer 1) ---- */

uint32_t cosim_fp_add(uint32_t x, uint32_t y, int u, int v) {
  return fp_add(x, y, u, v);
}
uint32_t cosim_fp_mul(uint32_t x, uint32_t y) { return fp_mul(x, y); }
uint32_t cosim_fp_div(uint32_t x, uint32_t y) { return fp_div(x, y); }
void cosim_idiv(uint32_t x, uint32_t y, int s, uint32_t *quot, uint32_t *rem) {
  struct idiv d = idiv(x, y, s);
  *quot = d.quot;
  *rem = d.rem;
}

/* ---- CPU (layers 2 & 3) ---- */

struct RISC *cosim_new(void) { return risc_new(); }
void cosim_configure(struct RISC *r, int m, int w, int h) {
  risc_configure_memory(r, m, w, h);
}
void cosim_set_switches(struct RISC *r, int s) { risc_set_switches(r, s); }
void cosim_set_time(struct RISC *r, uint32_t t) { risc_set_time(r, t); }
void cosim_attach_disk(struct RISC *r, const char *path) {
  risc_set_spi(r, 1, disk_new(path));
}
void cosim_single_step(struct RISC *r) { risc_single_step(r); } /* static via #include */
void cosim_run(struct RISC *r, int n) { risc_run(r, n); }

/* State vector layout: [PC, R0..R15, H, flags], flags = Z|N<<1|C<<2|V<<3. */
void cosim_set_state(struct RISC *r, const uint32_t *st) {
  r->PC = st[0];
  for (int i = 0; i < 16; i++) r->R[i] = st[1 + i];
  r->H = st[17];
  uint32_t f = st[18];
  r->Z = (f & 1) != 0;
  r->N = (f & 2) != 0;
  r->C = (f & 4) != 0;
  r->V = (f & 8) != 0;
}
void cosim_dump_state(struct RISC *r, uint32_t *st) {
  st[0] = r->PC;
  for (int i = 0; i < 16; i++) st[1 + i] = r->R[i];
  st[17] = r->H;
  st[18] = (r->Z ? 1u : 0u) | (r->N ? 2u : 0u) | (r->C ? 4u : 0u) | (r->V ? 8u : 0u);
}

uint32_t cosim_ram_read(struct RISC *r, uint32_t word) { return r->RAM[word]; }
void cosim_ram_write(struct RISC *r, uint32_t word, uint32_t value) {
  r->RAM[word] = value;
}

/* Framebuffer access for the lockstep boot check. */
const uint32_t *cosim_framebuffer(struct RISC *r) {
  return risc_get_framebuffer_ptr(r);
}
uint32_t cosim_fb_words(struct RISC *r) {
  return (uint32_t)(r->fb_width * r->fb_height);
}
