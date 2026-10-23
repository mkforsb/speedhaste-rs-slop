//! Fixed-point helpers mirroring the original Watcom inline assembly
//! (wcgsl/h/base.h, sincos.h, atan.c, sqrta.asm). All results wrap like the
//! 32-bit x86 code did; divisions that would fault on x86 (divide by zero)
//! return 0 instead of crashing.

use crate::tables::{ARCTAN, COS};

/// `(a * b) >> 30` — multiply by a 2.30 value (FPMult).
#[inline]
pub fn fp_mult(a: i32, b: i32) -> i32 {
    ((a as i64 * b as i64) >> 30) as i32
}

/// `(a * b) >> n` (FPnMult, FP16Mult = n 16, FP24Mult = n 24).
#[inline]
pub fn fpn_mult(a: i32, b: i32, n: u32) -> i32 {
    ((a as i64 * b as i64) >> n) as i32
}

#[inline]
pub fn fp16_mult(a: i32, b: i32) -> i32 {
    fpn_mult(a, b, 16)
}

#[inline]
pub fn fp24_mult(a: i32, b: i32) -> i32 {
    fpn_mult(a, b, 24)
}

/// `(a << n) / b`, truncating (FPnDiv, FP16Div = n 16, FP24Div = n 24).
#[inline]
pub fn fpn_div(a: i32, b: i32, n: u32) -> i32 {
    if b == 0 {
        return 0;
    }
    (((a as i64) << n) / b as i64) as i32
}

#[inline]
pub fn fp16_div(a: i32, b: i32) -> i32 {
    fpn_div(a, b, 16)
}

#[inline]
pub fn fp24_div(a: i32, b: i32) -> i32 {
    fpn_div(a, b, 24)
}

/// `a * b / c` with a 64-bit intermediate (FPMultDiv).
#[inline]
pub fn fp_mult_div(a: i32, b: i32, c: i32) -> i32 {
    if c == 0 {
        return 0;
    }
    ((a as i64 * b as i64) / c as i64) as i32
}

/// Unsigned `a * b / c` (FPUMultDiv).
#[inline]
pub fn fpu_mult_div(a: u32, b: u32, c: u32) -> u32 {
    if c == 0 {
        return 0;
    }
    ((a as u64 * b as u64) / c as u64) as u32
}

/// `(a * a) >> 16` (FP16Pow2).
#[inline]
pub fn fp16_pow2(a: i32) -> i32 {
    fpn_mult(a, a, 16)
}

/// `a * a`, wrapping (Pow2).
#[inline]
pub fn pow2(a: i32) -> i32 {
    a.wrapping_mul(a)
}

/// Absolute value; `i32::MIN` stays as is (Abs32).
#[inline]
pub fn abs32(a: i32) -> i32 {
    a.wrapping_abs()
}

/// `-1` for negative values, `1` otherwise (including 0) (Sgn).
#[inline]
pub fn sgn(a: i32) -> i32 {
    if a < 0 { -1 } else { 1 }
}

/// Cosine, angle in 65536ths of a circle, result in 2.30 fixed point.
#[inline]
pub fn cos(a: u16) -> i32 {
    COS[(a >> 4) as usize]
}

/// The original's `Sin(a)`, defined as `Cos(a + 16384)` (i.e. it is really
/// minus sine, since the game's 90 degrees points "south").
#[inline]
pub fn sin(a: u16) -> i32 {
    cos(a.wrapping_add(16384))
}

/// Angle of the vector (x, y) in 65536ths of a circle (GetAngle).
pub fn get_angle(x: i32, y: i32) -> u16 {
    // Index is in 0..=2048 for every branch below; clamp only guards i32::MIN.
    let at = |a: i32, b: i32| ARCTAN[fpn_div(a, b, 11).clamp(0, 2048) as usize];
    let r: u32 = if y >= 0 {
        if x >= 0 {
            if x >= y {
                if x == 0 && y == 0 {
                    return 0;
                }
                at(y, x) as u32
            } else {
                16384 - at(x, y) as u32
            }
        } else if x.wrapping_neg() >= y {
            32768 - at(y, x.wrapping_neg()) as u32
        } else {
            16384 + at(x.wrapping_neg(), y) as u32
        }
    } else if x >= 0 {
        if x >= y.wrapping_neg() {
            65536 - at(y.wrapping_neg(), x) as u32
        } else {
            // Sic: the original has 49158 here instead of 49152.
            49158 + at(x, y.wrapping_neg()) as u32
        }
    } else if x.wrapping_neg() >= y.wrapping_neg() {
        32768 + at(y, x) as u32
    } else {
        49152 - at(x, y) as u32
    };
    r as u16
}

/// Integer square root of an unsigned 32-bit value (SQR_Sqrt), transliterated
/// from the original bit-by-bit assembly.
pub fn sqrt(arg: u32) -> u32 {
    let (mut eax, mut ebx, mut edx, mut ebp) = (0u32, 0u32, arg, 0u32);
    for _ in 0..16 {
        ebp = (ebp << 1) | (eax >> 31);
        eax = (eax << 1).wrapping_add(1);
        ebx = (ebx << 2) | (edx >> 30);
        edx <<= 2;
        let (r, borrow) = ebx.overflowing_sub(eax);
        if borrow {
            eax = eax.wrapping_sub(1);
        } else {
            ebx = r;
            eax = eax.wrapping_add(1);
        }
    }
    (eax >> 1) | (ebp << 31)
}

/// The game's random number generator (RND_Randomize / RND_GetNum).
#[derive(Clone, Debug)]
pub struct Rng {
    s1: u32,
    s2: u32,
    s3: u32,
}

impl Default for Rng {
    fn default() -> Self {
        Rng { s1: 0x7834348B, s2: 0x78ED7F34, s3: 0xa78332bf }
    }
}

impl Rng {
    pub fn randomize(&mut self, seed: u32) {
        self.s1 = seed;
        self.s2 = seed.rotate_right(13);
        self.s3 = self.s2.rotate_right(9);
    }

    /// The generator's state (for network desync checks).
    pub fn state(&self) -> [u32; 3] {
        [self.s1, self.s2, self.s3]
    }

    pub fn next(&mut self) -> u32 {
        let mut eax = self.s1.wrapping_add(0xB35FA137);
        let mut ebx = self.s2.wrapping_add(0x354C63F7);
        let mut edx = self.s3.wrapping_add(0x067B784B);
        eax = eax.rotate_left(2);
        self.s1 = eax;
        ebx = ebx.wrapping_add(eax).rotate_right(1);
        self.s2 = ebx;
        edx = edx.wrapping_sub(ebx);
        eax ^= edx;
        self.s3 = edx;
        eax.wrapping_add(ebx)
    }

    /// `RND_GetNum() % n` as the original computes it (unsigned modulo).
    pub fn below(&mut self, n: u32) -> u32 {
        self.next() % n
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trig_tables_look_right() {
        for i in 0..4096u32 {
            let a = (i * 16) as u16;
            let want = (i as f64 * std::f64::consts::TAU / 4096.0).cos() * (1u64 << 30) as f64;
            assert!((cos(a) as f64 - want).abs() < 2.0, "cos({a}) = {} want {want}", cos(a));
        }
        assert_eq!(get_angle(1, 0), 0);
        assert_eq!(get_angle(0, 1), 16384);
        assert_eq!(get_angle(-1, 0), 32768);
        assert_eq!(get_angle(1, 1), 8192);
        assert_eq!(get_angle(0, -1), 49158); // The original's typo.
    }

    #[test]
    fn sqrt_matches_isqrt() {
        for n in (0..=u32::MAX).step_by(65537).chain([0, 1, 2, 3, 4, 15, 16, 17, u32::MAX]) {
            assert_eq!(sqrt(n), (n as f64).sqrt().floor() as u32, "sqrt({n})");
        }
    }
}
