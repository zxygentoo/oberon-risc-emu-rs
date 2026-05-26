//! FFI bindings to the C reference for differential testing (only built under
//! the `cosim` feature; the C is compiled and linked by `build.rs`).
//!
//! [`CRisc`] wraps an opaque `struct RISC *`. The C `risc_new` never frees, so
//! these intentionally leak — fine for a test process.

use std::ffi::{c_void, CString};
use std::path::Path;

#[allow(non_camel_case_types)]
type c_risc = c_void;

extern "C" {
    fn cosim_fp_add(x: u32, y: u32, u: i32, v: i32) -> u32;
    fn cosim_fp_mul(x: u32, y: u32) -> u32;
    fn cosim_fp_div(x: u32, y: u32) -> u32;
    fn cosim_idiv(x: u32, y: u32, s: i32, quot: *mut u32, rem: *mut u32);

    fn cosim_new() -> *mut c_risc;
    fn cosim_set_switches(r: *mut c_risc, s: i32);
    fn cosim_set_time(r: *mut c_risc, t: u32);
    fn cosim_attach_disk(r: *mut c_risc, path: *const std::os::raw::c_char);
    fn cosim_single_step(r: *mut c_risc);
    fn cosim_run(r: *mut c_risc, n: i32);
    fn cosim_set_state(r: *mut c_risc, st: *const u32);
    fn cosim_dump_state(r: *mut c_risc, st: *mut u32);
    fn cosim_ram_read(r: *mut c_risc, word: u32) -> u32;
    fn cosim_ram_write(r: *mut c_risc, word: u32, value: u32);
    fn cosim_framebuffer(r: *mut c_risc) -> *const u32;
    fn cosim_fb_words(r: *mut c_risc) -> u32;
}

/// `fp_add` in the C reference.
pub fn fp_add(x: u32, y: u32, u: bool, v: bool) -> u32 {
    unsafe { cosim_fp_add(x, y, u as i32, v as i32) }
}
pub fn fp_mul(x: u32, y: u32) -> u32 {
    unsafe { cosim_fp_mul(x, y) }
}
pub fn fp_div(x: u32, y: u32) -> u32 {
    unsafe { cosim_fp_div(x, y) }
}
/// `idiv` in the C reference, as `(quot, rem)`.
pub fn idiv(x: u32, y: u32, signed_div: bool) -> (u32, u32) {
    let (mut q, mut r) = (0u32, 0u32);
    unsafe { cosim_idiv(x, y, signed_div as i32, &mut q, &mut r) };
    (q, r)
}

/// A C-reference `struct RISC` instance (opaque, leaked on drop).
pub struct CRisc(*mut c_risc);

impl CRisc {
    pub fn new() -> Self {
        CRisc(unsafe { cosim_new() })
    }

    pub fn set_switches(&mut self, switches: u32) {
        unsafe { cosim_set_switches(self.0, switches as i32) }
    }
    pub fn set_time(&mut self, tick: u32) {
        unsafe { cosim_set_time(self.0, tick) }
    }
    pub fn attach_disk(&mut self, path: &Path) {
        let s = CString::new(path.to_str().expect("utf-8 disk path")).unwrap();
        unsafe { cosim_attach_disk(self.0, s.as_ptr()) }
    }
    pub fn single_step(&mut self) {
        unsafe { cosim_single_step(self.0) }
    }
    pub fn run(&mut self, cycles: u32) {
        unsafe { cosim_run(self.0, cycles as i32) }
    }

    /// State vector: `[PC, R0..R15, H, flags]`, flags = `Z|N<<1|C<<2|V<<3`.
    pub fn set_state(&mut self, st: &[u32; 19]) {
        unsafe { cosim_set_state(self.0, st.as_ptr()) }
    }
    pub fn dump_state(&self) -> [u32; 19] {
        let mut st = [0u32; 19];
        unsafe { cosim_dump_state(self.0, st.as_mut_ptr()) };
        st
    }

    pub fn ram_read(&self, word: usize) -> u32 {
        unsafe { cosim_ram_read(self.0, word as u32) }
    }
    pub fn ram_write(&mut self, word: usize, value: u32) {
        unsafe { cosim_ram_write(self.0, word as u32, value) }
    }

    pub fn framebuffer(&self) -> &[u32] {
        unsafe {
            let ptr = cosim_framebuffer(self.0);
            let len = cosim_fb_words(self.0) as usize;
            std::slice::from_raw_parts(ptr, len)
        }
    }
}

impl Default for CRisc {
    fn default() -> Self {
        Self::new()
    }
}
