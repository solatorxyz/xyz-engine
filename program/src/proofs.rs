//! Kani proofs kept outside lib.rs (compiled only under cfg(kani)). `use super::*` sees the program's private items.
//! Run one at a time inside program/: `cargo kani --harness <name>`. Every harness here verifies (2 shared cores: 2 s to 4 min
//! each). Bounds are stated in each harness; where a bound is narrower than the program's types the comment says why the
//! property carries over. Tried and dropped because the solver did not finish in the time allowed: quote_band == px / 20,
//! band_bps as the exact floor, the exact floor of the flip cost basis, mark_from inside its clamp, the funding rate
//! following the average premium, and slab_first end to end (guard, band, budget, age).
#![allow(unused_imports)]
use super::*;

// ---------- withdrawal lane: np_value ----------
// np_value(base, cost, px) = min(mulf(base, px), cost). The earlier harness (lib.rs, np_value_never_credits_a_gain) asks the
// solver for a 32x48-bit product twice and does not finish. Split here into three facts:
//  1. np_value is the smaller of `mulf(base, px)` and `cost` and one of the two (np_value_is_min_of_value_and_cost): no
//     arithmetic knowledge of the product is needed, so the lots and the price are as wide as the lib.rs harness wanted;
//  2. mulf is the exact product: |a| < 2^32 with any u64 b by `mulf_split_path_exact` (lib.rs); for |a| >= 2^32 the code IS
//     `a * b as i128`, which cannot overflow for an i64 a and a u64 b (|a| <= 2^63, b < 2^64, product < 2^127) -- that path
//     is exercised at a reduced width (mulf_wide_path_reduced);
//  3. the end-to-end statement against the true product at a reduced width (np_value_never_credits_a_gain_reduced).

/// The value is never above the mark value nor above the cost, and it is one of the two: a winning position is worth its
/// cost, a losing one its full mark value. 32-bit lots, 48-bit prices, any i64 cost.
#[kani::proof]
#[kani::solver(cadical)]
fn np_value_is_min_of_value_and_cost() {
    let (base, cost, px): (i64, i64, u64) = (kani::any(), kani::any(), kani::any());
    kani::assume(base.unsigned_abs() < (1u64 << 32) && px < (1u64 << 48));
    let mv = mulf(base as i128, px);
    let v = np_value(base, cost, px);
    assert!(v <= mv && v <= cost as i128);
    if mv > cost as i128 {
        assert_eq!(v, cost as i128);
    } else {
        assert_eq!(v, mv);
    }
}
/// The wide path of mulf (|a| >= 2^32) at a reduced width: exact product, no overflow. At full width the same expression
/// is bounded by 2^63 * 2^64 < 2^127.
#[kani::proof]
#[kani::solver(cadical)]
fn mulf_wide_path_reduced() {
    let (a, b): (i64, u64) = (kani::any(), kani::any());
    kani::assume(a.unsigned_abs() >= (1 << 32) && a.unsigned_abs() < (1 << 40) && b < (1 << 20));
    let r = mulf(a as i128, b);
    assert_eq!(r, a as i128 * b as i128);
    assert!(r.unsigned_abs() < (1u128 << 60));
}
/// End to end against the true product, 16-bit lots x 20-bit prices: a gain is never credited, a loss counts in full, and
/// the part left out is exactly max(value - cost, 0).
#[kani::proof]
#[kani::solver(cadical)]
fn np_value_never_credits_a_gain_reduced() {
    let (base, cost, px): (i64, i64, u64) = (kani::any(), kani::any(), kani::any());
    kani::assume(base.unsigned_abs() < (1u64 << 16) && px < (1u64 << 20));
    let mv = base as i128 * px as i128;
    let v = np_value(base, cost, px);
    assert!(v <= mv && v <= cost as i128);
    if mv <= cost as i128 {
        assert_eq!(v, mv);
    } else {
        assert_eq!(v, cost as i128);
    }
    assert_eq!(mv - v, (mv - cost as i128).max(0));
}

// ---------- cost basis ----------
/// Same-side change (open from flat, add, partial close that keeps the sign): the basis accumulates exactly the quote paid,
/// or the instruction fails when it leaves i64. No rounding, for every input.
#[kani::proof]
fn cost_after_same_side_accumulates_exactly() {
    let (ob, nb, cost, paid): (i64, i64, i64, i64) = (kani::any(), kani::any(), kani::any(), kani::any());
    kani::assume(nb != 0 && (ob == 0 || (ob > 0) == (nb > 0)));
    let want = cost as i128 + paid as i128;
    match cost_after(ob, nb, cost, paid as i128) {
        Ok(c) => assert_eq!(c as i128, want),
        Err(_) => assert!(want > i64::MAX as i128 || want < i64::MIN as i128),
    }
}
/// A flip re-bases to the new position at this trade's average price: the basis takes the sign of the new position (or 0)
/// and is never larger in size than what the trade paid; the old basis is ignored. 8-bit lots and 16-bit notional, built from
/// narrow types so the 128-bit division stays small for the solver (wider versions, and the exact floor, did not finish);
/// nothing in the formula depends on the width (see cost_after_flip_no_overflow for the full-width product).
#[kani::proof]
#[kani::solver(cadical)]
fn cost_after_flip_rebases_with_the_right_sign() {
    let (ob, nb): (i8, i8) = (kani::any(), kani::any());
    let (cost, p, neg): (i64, u16, bool) = (kani::any(), kani::any(), kani::any());
    kani::assume(ob != 0 && nb != 0 && (ob > 0) != (nb > 0));
    let paid: i128 = if neg { -(p as i128) } else { p as i128 };
    let c = cost_after(ob as i64, nb as i64, cost, paid).unwrap() as i128;
    assert!(c == 0 || (c > 0) == (nb > 0));
    assert!(c.abs() <= p as i128);
}
/// The product the flip formula forms, new_base x |paid|, cannot overflow i128 for any i64 position and any quote amount an
/// i64 can hold (and dq = |new - old| >= 2 is never zero on a flip).
#[kani::proof]
#[kani::solver(cadical)]
fn cost_after_flip_no_overflow() {
    let (ob, nb, paid): (i64, i64, i64) = (kani::any(), kani::any(), kani::any());
    kani::assume(ob != 0 && nb != 0 && (ob > 0) != (nb > 0));
    assert!((nb as i128).checked_mul((paid as i128).abs()).is_some());
    assert!((nb as i128 - ob as i128).abs() >= 2);
}

// ---------- liquidation size ----------
/// liq_needed never returns 0 and never panics. Ranges: equity and maintenance up to 2^40 in size (about 1.1e12 quote
/// units), positions up to 2^20 lots, prices up to 2^24, maintenance ratio (band included) up to 100%. Wider inputs only
/// need the products to fit i128 (liq_needed_products_fit).
#[kani::proof]
#[kani::solver(cadical)]
fn liq_needed_at_least_one_lot() {
    let (eq, mm): (i64, i64) = (kani::any(), kani::any());
    let (ab, px, mmr): (u64, u64, u64) = (kani::any(), kani::any(), kani::any());
    kani::assume(eq.unsigned_abs() < (1 << 40) && mm >= 0 && mm < (1 << 40));
    kani::assume(ab >= 1 && ab < (1 << 20) && px >= 1 && px < (1 << 24) && mmr <= 10_000);
    let q = liq_needed(eq as i128, mm as i128, ab, px, mmr);
    assert!(q >= 1);
    let worst = LIQ_FEE_BPS << (LIQ_BOOK_BAND_STEPS - 1);
    // a ratio at or below 4/5 of the worst liquidation fee gives no relief: the whole position (u64::MAX, capped by callers)
    if 5 * mmr <= 4 * worst {
        assert_eq!(q, u64::MAX);
    }
    // an account at or above 1.25 x maintenance needs only the minimum lot
    if 4 * eq as i128 >= 5 * mm as i128 && 5 * mmr > 4 * worst {
        assert_eq!(q, 1);
    }
}
/// The products inside liq_needed at the widths the program can reach: |eq|, mm < 2^62 (an i64 of quote), up to 2^32 lots
/// at prices below 2^32, ratio up to 100%: nothing overflows i128.
#[kani::proof]
#[kani::solver(cadical)]
fn liq_needed_products_fit() {
    let (eq, mm): (i64, i64) = (kani::any(), kani::any());
    let (ab, px, mmr): (u64, u64, u64) = (kani::any(), kani::any(), kani::any());
    kani::assume(eq.unsigned_abs() < (1 << 62) && mm >= 0 && mm < (1 << 62));
    kani::assume(ab >= 1 && ab < (1 << 32) && px >= 1 && px < (1 << 32) && mmr <= 10_000);
    let worst_fee = (LIQ_FEE_BPS << (LIQ_BOOK_BAND_STEPS - 1)) as i128;
    let n = (ab as i128).checked_mul(px as i128).unwrap();
    let relief = n.checked_mul(5 * mmr as i128 - 4 * worst_fee).unwrap();
    let deficit = (5 * mm as i128 - 4 * eq as i128).max(0).checked_mul(10000).unwrap();
    let num = deficit.checked_mul(ab as i128).unwrap();
    if relief > 0 {
        assert!(num.checked_add(relief - 1).is_some());
    }
}

// ---------- oracle band, funding clamp ----------
/// The oracle band is at most BAND_MAX_BPS for every price and confidence (0 without a price or a confidence); no panic.
#[kani::proof]
#[kani::solver(cadical)]
fn band_bps_capped() {
    let mut m: Market = unsafe { core::mem::zeroed() };
    m.oracle_px = kani::any();
    m.oracle_conf = kani::any();
    let b = band_bps(&m);
    assert!(b <= spec::BAND_MAX_BPS);
    if m.oracle_px == 0 || m.oracle_conf == 0 {
        assert_eq!(b, 0);
    }
}
/// The funding rate is clamped to +-FUNDING_MAX_RATE_HOURLY_PPB for every accumulator and sample count, never panics, and
/// with no samples (or a zero premium) is the interest term alone.
#[kani::proof]
#[kani::solver(cadical)]
fn funding_rate_clamped() {
    let (acc, n): (i64, u64) = (kani::any(), kani::any());
    let r = funding_rate_from(acc, n);
    assert!(r >= -(FUNDING_MAX_RATE_HOURLY_PPB as i64) && r <= FUNDING_MAX_RATE_HOURLY_PPB as i64);
    if n == 0 || acc == 0 {
        assert_eq!(r, (FUNDING_INTEREST_8H_PPB / FUNDING_HOURS_PER_8H) as i64);
    }
}

// ---------- fast path: ladder tolerance, default maker guard, pegged price, maker budget ----------
/// ladder_room's tolerance test `|oracle - ref|.min(2^40) * 10000 > ref * tol` is evaluated with wrapping native multiplies.
/// For every oracle (u64), ref (u32) and tolerance (u16) neither product wraps, and the result is the exact statement
/// |oracle - ref| x 10000 > ref x tol in wide arithmetic (the clamp to 2^40 never changes the answer).
#[kani::proof]
#[kani::solver(cadical)]
fn ladder_room_tolerance_cannot_wrap() {
    let f: FastBook = unsafe { core::mem::zeroed() };
    let l = Ladder { ref_px: kani::any(), tol_bps: kani::any(), flags: 0, inv_max: 0, _r1: 0 };
    let (oracle, slot, bid): (u64, u64, bool) = (kani::any(), kani::any(), kani::any());
    let room = ladder_room(&f, &l, 0, bid, oracle, slot);
    let (r, d) = (l.ref_px as u64, oracle.abs_diff(l.ref_px as u64));
    assert!(d.min(1 << 40).checked_mul(10000).is_some() && r.checked_mul(l.tol_bps as u64).is_some());
    let beyond = d as u128 * 10000 > r as u128 * l.tol_bps as u128;
    if l.tol_bps != 0 && beyond {
        assert_eq!(room, 0);
    } else {
        assert_eq!(room, u64::MAX);
    }
}
/// With an inventory cap the room is exactly what the cap leaves on that side (never negative), for inventories whose sum
/// cannot overflow (|position|, |unsettled| < 2^61).
#[kani::proof]
#[kani::solver(cadical)]
fn ladder_room_inventory_cap() {
    let mut f: FastBook = unsafe { core::mem::zeroed() };
    let mi: usize = kani::any();
    kani::assume(mi < FAST_MAKERS);
    f.ext[mi].inv0 = kani::any();
    f.makers[mi].base = kani::any();
    kani::assume(f.ext[mi].inv0.unsigned_abs() < (1 << 61) && f.makers[mi].base.unsigned_abs() < (1 << 61));
    let l = Ladder { ref_px: 0, tol_bps: 0, flags: 0, inv_max: kani::any(), _r1: 0 };
    kani::assume(l.inv_max != 0);
    let bid: bool = kani::any();
    let room = ladder_room(&f, &l, mi, bid, kani::any(), kani::any()) as i128;
    let inv = f.ext[mi].inv0 as i128 + f.makers[mi].base as i128;
    // taker buys (bid): the maker sells, its short may reach inv_max; taker sells: its long may reach inv_max
    let left = l.inv_max as i128 + if bid { inv } else { -inv };
    assert_eq!(room, left.max(0));
}
/// A fresh-only ladder (flags bits 8-15 = N) gives no room without a signed price at most min(N, FAST_PX_MAX_SLOTS) slots old.
#[kani::proof]
#[kani::solver(cadical)]
fn ladder_room_fresh_only() {
    let mut f: FastBook = unsafe { core::mem::zeroed() };
    f.fast_px = kani::any();
    f.fast_slot = kani::any();
    kani::assume(f.fast_slot < (1 << 62));
    let l = Ladder { ref_px: 0, tol_bps: 0, flags: kani::any(), inv_max: 0, _r1: 0 };
    let slot: u64 = kani::any();
    let fr = (l.flags >> spec::LADDER_FRESH_SHIFT) as u64;
    let room = ladder_room(&f, &l, 0, kani::any(), kani::any(), slot);
    if fr != 0 && (f.fast_px == 0 || slot > f.fast_slot + fr.min(spec::FAST_PX_MAX_SLOTS)) {
        assert_eq!(room, 0);
    }
    if fr != 0 && room != 0 {
        assert!(f.fast_px != 0 && slot <= f.fast_slot + spec::FAST_PX_MAX_SLOTS);
    }
}
/// The default maker guard, as written in slab_first and fast_bbo: g = oracle.min(2^40).wrapping_mul(FAST_GUARD_BPS) / 10000.
/// The multiply never wraps, g is at most FAST_GUARD_BPS of the price (exactly its floor up to 2^40), and
/// glo <= oracle <= ghi, for every u64 price.
#[kani::proof]
#[kani::solver(cadical)]
fn fast_guard_window_cannot_wrap() {
    let oracle: u64 = kani::any();
    assert!(oracle.min(1 << 40).checked_mul(spec::FAST_GUARD_BPS).is_some());
    let g = oracle.min(1 << 40).wrapping_mul(spec::FAST_GUARD_BPS) / 10000;
    let (glo, ghi) = (oracle.saturating_sub(g), oracle.saturating_add(g));
    assert!(glo <= oracle && oracle <= ghi);
    assert!(g as u128 * 10000 <= oracle as u128 * spec::FAST_GUARD_BPS as u128);
    if oracle <= (1 << 40) {
        assert_eq!(g as u128, oracle as u128 * spec::FAST_GUARD_BPS as u128 / 10000);
        assert_eq!(glo, oracle - g);
    }
}
/// A pegged level prices at book price + offset - QUOTE_PEG_BIAS when that lies in (0, 2^32), else it is unusable (0);
/// a fixed level is its own px. Book prices below 2^62.
#[kani::proof]
fn qpx_pegged_price_exact_or_unusable() {
    let (px, oracle, peg): (u32, u64, bool) = (kani::any(), kani::any(), kani::any());
    let p = qpx(px, oracle, peg);
    assert!(p < (1 << 32));
    if !peg {
        assert_eq!(p, px as u64);
        return;
    }
    let want = oracle as i128 + px as i128 - spec::QUOTE_PEG_BIAS as i128;
    if oracle < (1 << 62) {
        if want > 0 && want < (1i128 << 32) {
            assert_eq!(p as i128, want);
        } else {
            assert_eq!(p, 0);
        }
    }
}
/// The quoting budget is margin x 10000 / IMR: never negative, 0 for no (or negative) margin, at least the margin.
/// 16-bit margin, IMR from 1 bps to 100% (the any-i64 version did not finish; the i64 conversion saturates by construction).
#[kani::proof]
#[kani::solver(cadical)]
fn maker_budget_never_negative() {
    let mut m: Market = unsafe { core::mem::zeroed() };
    let (imr, margin): (u16, i16) = (kani::any(), kani::any());
    kani::assume(imr >= 1 && imr <= 10_000);
    m.imr_bps = imr as u64;
    let b = maker_budget(&m, margin as i64);
    assert!(b >= 0);
    if margin <= 0 {
        assert_eq!(b, 0);
    } else {
        assert!(b >= margin as i64);
    }
}
/// reduce_ok (batch reduce-only pre-check): when it passes, equity after the worst fill (slip at the worst price plus the
/// taker fee rounded up) is not negative; no panic (12-bit lots, 20-bit prices, fee up to 1%).
#[kani::proof]
#[kani::solver(cadical)]
fn reduce_ok_bounds_the_worst_fill_reduced() {
    let eq: i64 = kani::any();
    let (px0, worst, qty, bps): (u64, u64, u64, u64) = (kani::any(), kani::any(), kani::any(), kani::any());
    kani::assume(
        px0 < (1 << 20) && worst < (1 << 20) && qty < (1 << 12) && bps <= 100 && eq.unsigned_abs() < (1 << 40),
    );
    if reduce_ok(eq as i128, 0, px0, worst, qty, bps) {
        let slip = qty as i128 * (px0 as i128 - worst as i128).abs();
        let fee_up = (qty as i128 * worst as i128 * bps as i128 + 9999) / 10000;
        assert!(eq as i128 - slip - fee_up >= 0);
    }
}
/// off_band (|px - oracle| / oracle > band) agrees with the wide formula on both of its paths (native when the products fit,
/// u128 otherwise) for 40-bit prices and every 16-bit band (the order field is a u16 on the wire).
#[kani::proof]
#[kani::solver(cadical)]
fn off_band_matches_wide_formula() {
    let (px, oracle, band): (u64, u64, u64) = (kani::any(), kani::any(), kani::any());
    kani::assume(px < (1 << 40) && oracle < (1 << 40) && band < (1 << 16));
    assert_eq!(off_band(px, oracle, band), px.abs_diff(oracle) as u128 * 10000 > oracle as u128 * band as u128);
}
