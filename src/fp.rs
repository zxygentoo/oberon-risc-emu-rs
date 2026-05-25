//! Software floating-point and integer division (port of `risc-fp.c`).
//!
//! These are bit-exact software models of the RISC5 FPGA arithmetic units
//! (`FPAdder.v`, `FPMultiplier.v`, `FPDivider.v`, `Divider.v`). They use
//! signed-magnitude mantissa math on `u32`/`i32`, so every operation is ported
//! verbatim from the C: unsigned wraparound becomes `wrapping_*`, C's
//! arithmetic right shift becomes `(x as i32) >> n`, and the C idiom
//! `n > 31 ? sign : x >> n` guards against Rust's shift-amount-overflow panic.
//!
//! There is no separate FLT/FLOOR opcode: they are `FAD` (op 12) with modifier
//! bits, so [`fp_add`]'s `u` selects FLT (integer -> float) and `v` selects
//! FLOOR (float -> integer); plain FAD passes `u = v = false`. `FSB` is `FAD`
//! with operand 2's sign flipped by the caller.

/// Result of an integer division: quotient and remainder (mirrors the C
/// `struct idiv`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IDiv {
    pub quot: u32,
    pub rem: u32,
}

/// Floating-point add (`FAD`/`FSB`/`FLT`/`FLOOR`). `u` = FLT, `v` = FLOOR.
///
/// Port of `fp_add` (`risc-fp.c:3`).
pub fn fp_add(x: u32, y: u32, u: bool, v: bool) -> u32 {
    let xs = (x & 0x8000_0000) != 0;
    let xe: u32;
    let x0: i32;
    if !u {
        xe = (x >> 23) & 0xFF;
        let xm = ((x & 0x7F_FFFF) << 1) | 0x100_0000;
        x0 = (if xs { xm.wrapping_neg() } else { xm }) as i32;
    } else {
        xe = 150;
        x0 = (((x & 0x00FF_FFFF) as i32) << 8) >> 7;
    }

    let ys = (y & 0x8000_0000) != 0;
    let ye = (y >> 23) & 0xFF;
    let mut ym = (y & 0x7F_FFFF) << 1;
    if !u && !v {
        ym |= 0x100_0000;
    }
    let y0 = (if ys { ym.wrapping_neg() } else { ym }) as i32;

    let e0: u32;
    let x3: i32;
    let y3: i32;
    if ye > xe {
        let shift = ye - xe;
        e0 = ye;
        x3 = if shift > 31 { x0 >> 31 } else { x0 >> shift };
        y3 = y0;
    } else {
        let shift = xe - ye;
        e0 = xe;
        x3 = x0;
        y3 = if shift > 31 { y0 >> 31 } else { y0 >> shift };
    }

    let sum = (((xs as u32) << 26) | ((xs as u32) << 25) | ((x3 as u32) & 0x01FF_FFFF))
        .wrapping_add(((ys as u32) << 26) | ((ys as u32) << 25) | ((y3 as u32) & 0x01FF_FFFF));

    let s = (if sum & (1 << 26) != 0 { sum.wrapping_neg() } else { sum }).wrapping_add(1)
        & 0x07FF_FFFF;

    let mut e1 = e0.wrapping_add(1);
    let mut t3 = s >> 1;
    if (s & 0x3FF_FFFC) != 0 {
        while (t3 & (1 << 24)) == 0 {
            t3 <<= 1;
            e1 = e1.wrapping_sub(1);
        }
    } else {
        t3 <<= 24;
        e1 = e1.wrapping_sub(24);
    }

    let xn = (x & 0x7FFF_FFFF) == 0;
    let yn = (y & 0x7FFF_FFFF) == 0;

    if v {
        (((sum << 5) as i32) >> 6) as u32
    } else if xn {
        if u || yn {
            0
        } else {
            y
        }
    } else if yn {
        x
    } else if (t3 & 0x01FF_FFFF) == 0 || (e1 & 0x100) != 0 {
        0
    } else {
        ((sum & 0x0400_0000) << 5) | (e1 << 23) | ((t3 >> 1) & 0x7F_FFFF)
    }
}

/// Floating-point multiply (`FML`). Port of `fp_mul` (`risc-fp.c:69`).
pub fn fp_mul(x: u32, y: u32) -> u32 {
    let sign = (x ^ y) & 0x8000_0000;
    let xe = (x >> 23) & 0xFF;
    let ye = (y >> 23) & 0xFF;

    let xm = (x & 0x7F_FFFF) | 0x80_0000;
    let ym = (y & 0x7F_FFFF) | 0x80_0000;
    let m = (xm as u64) * (ym as u64);

    let mut e1 = (xe + ye).wrapping_sub(127);
    let z0: u32;
    if (m & (1u64 << 47)) != 0 {
        e1 = e1.wrapping_add(1);
        z0 = (((m >> 23) + 1) & 0xFF_FFFF) as u32;
    } else {
        z0 = (((m >> 22) + 1) & 0xFF_FFFF) as u32;
    }

    if xe == 0 || ye == 0 {
        0
    } else if (e1 & 0x100) == 0 {
        sign | ((e1 & 0xFF) << 23) | (z0 >> 1)
    } else if (e1 & 0x80) == 0 {
        sign | (0xFF << 23) | (z0 >> 1)
    } else {
        0
    }
}

/// Floating-point divide (`FDV`). Port of `fp_div` (`risc-fp.c:98`).
pub fn fp_div(x: u32, y: u32) -> u32 {
    let sign = (x ^ y) & 0x8000_0000;
    let xe = (x >> 23) & 0xFF;
    let ye = (y >> 23) & 0xFF;

    let xm = (x & 0x7F_FFFF) | 0x80_0000;
    let ym = (y & 0x7F_FFFF) | 0x80_0000;
    let q1 = ((xm as u64) * (1u64 << 25) / (ym as u64)) as u32;

    let mut e1 = (xe.wrapping_sub(ye)).wrapping_add(126);
    let q2: u32;
    if (q1 & (1 << 25)) != 0 {
        e1 = e1.wrapping_add(1);
        q2 = (q1 >> 1) & 0xFF_FFFF;
    } else {
        q2 = q1 & 0xFF_FFFF;
    }
    let q3 = q2.wrapping_add(1);

    if xe == 0 {
        0
    } else if ye == 0 {
        sign | (0xFF << 23)
    } else if (e1 & 0x100) == 0 {
        sign | ((e1 & 0xFF) << 23) | (q3 >> 1)
    } else if (e1 & 0x80) == 0 {
        sign | (0xFF << 23) | (q2 >> 1)
    } else {
        0
    }
}

/// 32-iteration restoring integer division on a 64-bit RQ register, with the
/// signed fixup. Port of `idiv` (`risc-fp.c:130`, modelling `Divider.v`).
pub fn idiv(x: u32, y: u32, signed_div: bool) -> IDiv {
    let sign = ((x as i32) < 0) && signed_div;
    let x0 = if sign { x.wrapping_neg() } else { x };

    let mut rq = x0 as u64;
    for _ in 0..32 {
        let w0 = (rq >> 31) as u32;
        let w1 = w0.wrapping_sub(y);
        if (w1 as i32) < 0 {
            rq = ((w0 as u64) << 32) | ((rq & 0x7FFF_FFFF) << 1);
        } else {
            rq = ((w1 as u64) << 32) | ((rq & 0x7FFF_FFFF) << 1) | 1;
        }
    }

    let mut d = IDiv {
        quot: rq as u32,
        rem: (rq >> 32) as u32,
    };
    if sign {
        d.quot = d.quot.wrapping_neg();
        if d.rem != 0 {
            d.quot = d.quot.wrapping_sub(1);
            d.rem = y.wrapping_sub(d.rem);
        }
    }
    d
}
