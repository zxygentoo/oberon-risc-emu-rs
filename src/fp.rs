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
    // Unpack x into sign, biased exponent, and a signed-magnitude mantissa. The
    // 25-bit mantissa carries the hidden leading 1 (bit 24) and a low guard bit
    // (the `<< 1`). For FLT (`u`) x is instead a 24-bit signed integer at the
    // fixed exponent 150 (= 127 + 23), so its value sits in the mantissa field.
    let x_sign = (x & 0x8000_0000) != 0;
    let x_exp: u32;
    let x_signed: i32;
    if !u {
        x_exp = (x >> 23) & 0xFF;
        let x_mant = ((x & 0x7F_FFFF) << 1) | 0x100_0000;
        x_signed = (if x_sign {
            x_mant.wrapping_neg()
        } else {
            x_mant
        }) as i32;
    } else {
        x_exp = 150;
        x_signed = (((x & 0x00FF_FFFF) as i32) << 8) >> 7;
    }

    // Unpack y the same way; the hidden bit is suppressed for FLT/FLOOR, where y
    // carries no float mantissa.
    let y_sign = (y & 0x8000_0000) != 0;
    let y_exp = (y >> 23) & 0xFF;
    let mut y_mant = (y & 0x7F_FFFF) << 1;
    if !u && !v {
        y_mant |= 0x100_0000;
    }
    let y_signed = (if y_sign {
        y_mant.wrapping_neg()
    } else {
        y_mant
    }) as i32;

    // Align to the larger exponent by arithmetic-right-shifting the smaller
    // operand's mantissa (sign-preserving). Shifts of 32+ clamp to a full sign
    // fill, replicating the C's `n > 31 ? sign : x >> n` (Rust would panic).
    let exp: u32;
    let x_aligned: i32;
    let y_aligned: i32;
    if y_exp > x_exp {
        let shift = y_exp - x_exp;
        exp = y_exp;
        x_aligned = if shift > 31 {
            x_signed >> 31
        } else {
            x_signed >> shift
        };
        y_aligned = y_signed;
    } else {
        let shift = x_exp - y_exp;
        exp = x_exp;
        x_aligned = x_signed;
        y_aligned = if shift > 31 {
            y_signed >> 31
        } else {
            y_signed >> shift
        };
    }

    // Add the aligned mantissas in a 27-bit field, each sign-extended into the
    // two guard bits (26, 25) so a carry out of bit 24 keeps its sign.
    let sum = (((x_sign as u32) << 26)
        | ((x_sign as u32) << 25)
        | ((x_aligned as u32) & 0x01FF_FFFF))
        .wrapping_add(
            ((y_sign as u32) << 26) | ((y_sign as u32) << 25) | ((y_aligned as u32) & 0x01FF_FFFF),
        );

    // Magnitude of the signed sum, plus 1 as the rounding bias.
    let mag = (if sum & (1 << 26) != 0 {
        sum.wrapping_neg()
    } else {
        sum
    })
    .wrapping_add(1)
        & 0x07FF_FFFF;

    // Post-normalize: shift the mantissa left until its leading 1 reaches bit 24,
    // decrementing the exponent each step. A sum with nothing above the guard
    // region takes the hardware's fixed 24-place shift instead.
    let mut out_exp = exp.wrapping_add(1);
    let mut out_mant = mag >> 1;
    if (mag & 0x3FF_FFFC) != 0 {
        while (out_mant & (1 << 24)) == 0 {
            out_mant <<= 1;
            out_exp = out_exp.wrapping_sub(1);
        }
    } else {
        out_mant <<= 24;
        out_exp = out_exp.wrapping_sub(24);
    }

    let x_is_zero = (x & 0x7FFF_FFFF) == 0;
    let y_is_zero = (y & 0x7FFF_FFFF) == 0;

    if v {
        // FLOOR: reinterpret the raw sum as the signed integer result.
        (((sum << 5) as i32) >> 6) as u32
    } else if x_is_zero {
        // x == 0: result is y, but FLT(0) and 0 + 0 give +0.
        if u || y_is_zero {
            0
        } else {
            y
        }
    } else if y_is_zero {
        x
    } else if (out_mant & 0x01FF_FFFF) == 0 || (out_exp & 0x100) != 0 {
        // Mantissa cancelled to zero, or the exponent ran out of range.
        0
    } else {
        // Reassemble: sign (from the sum's guard bit), exponent, 23-bit mantissa.
        ((sum & 0x0400_0000) << 5) | (out_exp << 23) | ((out_mant >> 1) & 0x7F_FFFF)
    }
}

/// Floating-point multiply (`FML`). Port of `fp_mul` (`risc-fp.c:69`).
pub fn fp_mul(x: u32, y: u32) -> u32 {
    let sign = (x ^ y) & 0x8000_0000;
    let xe = (x >> 23) & 0xFF;
    let ye = (y >> 23) & 0xFF;

    // 24-bit mantissas with the hidden leading 1; their product is up to 48 bits.
    let xm = (x & 0x7F_FFFF) | 0x80_0000;
    let ym = (y & 0x7F_FFFF) | 0x80_0000;
    let m = (xm as u64) * (ym as u64);

    // Add the exponents (removing one bias). A product that reached bit 47 is
    // >= 2.0: bump the exponent and round from bit 23, otherwise round from 22.
    let mut e1 = (xe + ye).wrapping_sub(127);
    let z0: u32;
    if (m & (1u64 << 47)) != 0 {
        e1 = e1.wrapping_add(1);
        z0 = (((m >> 23) + 1) & 0xFF_FFFF) as u32;
    } else {
        z0 = (((m >> 22) + 1) & 0xFF_FFFF) as u32;
    }

    // Zero operand -> 0; in-range exponent -> assemble; overflow (bit 8 set,
    // bit 7 clear) -> infinity; underflow -> 0.
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

    // Divide the 24-bit mantissas, pre-scaling the dividend by 2^25 for precision.
    let xm = (x & 0x7F_FFFF) | 0x80_0000;
    let ym = (y & 0x7F_FFFF) | 0x80_0000;
    let q1 = ((xm as u64) * (1u64 << 25) / (ym as u64)) as u32;

    // Subtract the exponents (re-adding the bias). A quotient that reached bit 25
    // needs the exponent bumped and a bit dropped; q3 is the rounded mantissa.
    let mut e1 = (xe.wrapping_sub(ye)).wrapping_add(126);
    let q2: u32;
    if (q1 & (1 << 25)) != 0 {
        e1 = e1.wrapping_add(1);
        q2 = (q1 >> 1) & 0xFF_FFFF;
    } else {
        q2 = q1 & 0xFF_FFFF;
    }
    let q3 = q2.wrapping_add(1);

    // x == 0 -> 0; y == 0 -> infinity; in range -> assemble (rounded q3);
    // overflow -> infinity (unrounded q2); underflow -> 0.
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
