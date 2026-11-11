//! Fixed-point arithmetic shared by every path. Prices are integers in quote units per lot, sizes are lots, money is
//! i128 while it is being computed and i64 where it is stored (`i64c` is the only way back, and it fails instead of
//! wrapping). The `mulf` / `bps_*` fast paths use a native 64-bit multiply when the operands allow it and fall back to
//! i128 otherwise; `arith_tests` in `lib.rs` checks them against the i128 answer across the edges.

use super::*;

pub(crate) fn i64c(x: i128) -> Result<i64, ProgramError> {
    i64::try_from(x).map_err(|_| err(E::Math))
}

// i128 division is a software routine on SBF (hundreds of CU); use native u64 math when the value fits (OPT).
pub(crate) const FAST: i128 = 1 << 48;

/// CU: signed x unsigned product. With overflow-checks on, an i128 multiply is a checked software routine, and so is a checked
/// u64 multiply (SBF has no high-word multiply). When |a| < 2^32 the exact product comes from native wrapping u64 multiplies that
/// cannot wrap: one when b < 2^32 too, else b's two 32-bit halves (prices in 1e-8 units are above 2^32). Same result.
#[inline(always)]
pub(crate) fn mulf(a: i128, b: u64) -> i128 {
    let ua = a.unsigned_abs();
    if ua < (1 << 32) {
        let x = ua as u64;
        let r = if b < (1 << 32) {
            x.wrapping_mul(b) as i128
        } else {
            ((x.wrapping_mul(b >> 32) as i128) << 32) + x.wrapping_mul(b & 0xFFFF_FFFF) as i128
        };
        if a < 0 {
            -r
        } else {
            r
        }
    } else {
        a * b as i128
    }
}

#[inline(always)]
pub(crate) fn bps_up(x: i128, b: u64) -> i128 {
    if x >= 0 && x < FAST && b <= 65_535 {
        (((x as u64).wrapping_mul(b) + 9999) / 10000) as i128
    } else {
        (x * b as i128 + 9999) / 10000
    }
}

#[inline(always)]
pub(crate) fn bps_dn(x: i128, b: u64) -> i128 {
    if x >= 0 && x < FAST && b <= 65_535 {
        ((x as u64).wrapping_mul(b) / 10000) as i128
    } else {
        x * b as i128 / 10000
    }
}

#[inline(always)]
/// AUDIT N-7: cost basis after a base change with signed quote `paid` (> 0 = paid). Flat -> 0; sign flip ->
/// new base valued at this trade's average price; otherwise accumulate.
pub(crate) fn cost_after(old_b: i64, new_b: i64, cost: i64, paid: i128) -> Result<i64, ProgramError> {
    if new_b == 0 {
        return Ok(0);
    }
    if old_b != 0 && (old_b > 0) != (new_b > 0) {
        let dq = (new_b as i128 - old_b as i128).abs();
        return i64c(new_b as i128 * paid.abs() / dq);
    }
    i64c(cost as i128 + paid)
}

pub(crate) fn long(b: i64) -> i64 {
    if b > 0 {
        b
    } else {
        0
    }
}

/// acc + lots x index, for Ledger.fund / FastMaker.fund. Native multiply where it cannot wrap (lots < 2^32, |index| < 2^31).
#[inline(always)]
pub(crate) fn fund_acc(acc: i64, lots: i64, idx: i64) -> Result<i64, ProgramError> {
    if idx == 0 {
        return Ok(acc);
    }
    if idx.unsigned_abs() < (1 << 31) && lots.unsigned_abs() < (1 << 32) {
        acc.checked_add(lots.wrapping_mul(idx)).ok_or(err(E::Math))
    } else {
        i64c(acc as i128 + lots as i128 * idx as i128)
    }
}

/// a x 10000 > b x bps. CU: native u64 products when they can't wrap; checked and u128 multiplies are software routines on SBF.
#[inline(always)]
pub(crate) fn exceeds(a: u64, b: u64, bps: u64) -> bool {
    if a < (1 << 48) && b < (1 << 48) && bps <= 65_535 {
        a.wrapping_mul(10000) > b.wrapping_mul(bps)
    } else {
        a as u128 * 10000 > b as u128 * bps as u128
    }
}

/// x x bps / 10000, rounded down, same fast path.
#[inline(always)]
pub(crate) fn bps_of(x: u64, bps: u64) -> u64 {
    if x < (1 << 48) && bps <= 65_535 {
        x.wrapping_mul(bps) / 10000
    } else {
        (x as u128 * bps as u128 / 10000) as u64
    }
}

/// |a - b| > b * bps / 1e4, in native u64 (saturating: a product that would overflow counts as a move / huge bound)
#[inline(always)]
pub(crate) fn moved(a: u64, b: u64, bps: u64) -> bool {
    a.abs_diff(b).saturating_mul(10000) > b.saturating_mul(bps)
}
