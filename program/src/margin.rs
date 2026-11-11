//! Account health: equity, initial and maintenance margin, for cross and isolated positions.
//!
//! `health_g` is the single definition; everything that opens risk checks `health_im`, everything that judges an
//! account for liquidation checks `health`, and withdrawals use the no-profit variants (`*_np`), which count an
//! unrealised gain as zero so that paper profit cannot be withdrawn. A stale price fails closed wherever `need_fresh`
//! is set.

use super::*;

pub(crate) fn find_market(pid: &Address, k: &Key, a: &[AccountView]) -> Result<&'static Market, ProgramError> {
    for x in a {
        if x.address().to_bytes() == *k && disc_of(x) == DISC_MARKET {
            return load(x, pid, DISC_MARKET, MARKET_SIZE).map(|m| &*m);
        }
    }
    Err(err(E::BadAccount))
}

pub(crate) fn settle_funding_all(pid: &Address, u: &mut User, a: &[AccountView]) -> ProgramResult {
    for p in u.pos.iter_mut() {
        if p.market == [0; 32] {
            break;
        } // used positions are packed at the front (gc_pos)
        let m = find_market(pid, &p.market, a)?;
        let idx = m.funding_index;
        let dd = idx as i128 - p.funding_snap as i128;
        let pay = if dd == 0 {
            0
        } else {
            let r = mulf(dd, p.base.unsigned_abs());
            if p.base < 0 {
                -r
            } else {
                r
            }
        };
        if p.flags & POS_ISO != 0 {
            p.iso_margin = i64c(p.iso_margin as i128 - pay)?;
        } else {
            u.cash = i64c(u.cash as i128 - pay)?;
        }
        p.funding_snap = idx;
    }
    Ok(())
}

/// (slot, unix_timestamp)
pub(crate) type Now = (u64, i64);

/// AUDIT L-4: stale if the update slot is too old OR the Pyth publish_time is older than max_age at *use* time.
pub(crate) fn stale(m: &Market, nw: Now) -> bool {
    m.oracle_px == 0
        || nw.0 > m.oracle_slot + m.max_staleness_slots
        || nw.1.saturating_sub(m.oracle_ts) > m.max_age_secs as i64
}

/// (equity, initial req (net-of-resting), maintenance req)
pub(crate) const POS_ISO: u64 = 1;

pub(crate) const POS_PIN: u64 = spec::POS_FLAG_MAKER_PIN;

/// Price window of fast-path fills around the mark (no rounding to zero for low prices).
#[inline(always)]
pub(crate) fn quote_band(px: u64) -> u64 {
    // CU: native u64 when the product can't wrap (every real price); u128 multiply/divide are software routines on SBF
    if px < (1 << 48) {
        px.wrapping_mul(spec::QUOTE_BAND_BPS) / 10000
    } else {
        (px as u128 * spec::QUOTE_BAND_BPS as u128 / 10000) as u64
    }
}

/// Oracle band in bps: the oracle's confidence as a share of its price (an OracleSet reports the publishers' dispersion),
/// capped at spec::BAND_MAX_BPS. Added to the margin requirements, never taken from equity (see spec).
#[inline(always)]
pub(crate) fn band_bps(m: &Market) -> u64 {
    band_of(m.oracle_conf, m.oracle_px)
}

/// CU: this runs for every position of every health pass. Native u64 division when conf x 10000 can't wrap (any real
/// confidence), the u128 software division only beyond that.
#[inline(always)]
pub(crate) fn band_of(conf: u64, px: u64) -> u64 {
    if px == 0 {
        return 0;
    }
    (if conf < (1 << 50) { conf.wrapping_mul(10000) / px } else { (conf as u128 * 10000 / px as u128) as u64 })
        .min(spec::BAND_MAX_BPS)
}

/// Isolated position: equity = iso_margin + base * mark (all of the position's cash flows are booked to iso_margin).
pub(crate) fn iso_health(
    pid: &Address,
    u: &User,
    pi: usize,
    a: &[AccountView],
    nw: Now,
    need_fresh: bool,
) -> Result<(i128, i128, i128), ProgramError> {
    let p = &u.pos[pi];
    let m = find_market(pid, &p.market, a)?;
    if need_fresh && stale(m, nw) && (p.base != 0 || p.bid_open != 0 || p.ask_open != 0) {
        return Err(err(E::Stale));
    }
    let (px, b) = (mark(m), band_bps(m));
    let gross =
        core::cmp::max((p.base as i128 + p.bid_open as i128).abs(), (p.base as i128 - p.ask_open as i128).abs());
    Ok((
        p.iso_margin as i128 + mulf(p.base as i128, px),
        bps_up(mulf(gross, px), m.imr_bps + b),
        bps_up(mulf((p.base as i128).abs(), px), m.mmr_bps + b),
    ))
}

/// Health that governs position `pi`: its own if isolated, else the cross account.
pub(crate) fn phealth(
    pid: &Address,
    u: &User,
    pi: usize,
    a: &[AccountView],
    nw: Now,
    need_fresh: bool,
) -> Result<(i128, i128, i128), ProgramError> {
    if u.pos[pi].flags & POS_ISO != 0 {
        iso_health(pid, u, pi, a, nw, need_fresh)
    } else {
        health(pid, u, a, nw, need_fresh)
    }
}

/// Book a cash flow of position `pi`: to its isolated margin if isolated, else to cross cash.
pub(crate) fn credit(u: &mut User, pi: usize, d: i128) -> ProgramResult {
    if u.pos[pi].flags & POS_ISO != 0 {
        u.pos[pi].iso_margin = i64c(u.pos[pi].iso_margin as i128 + d)?;
    } else {
        u.cash = i64c(u.cash as i128 + d)?;
    }
    Ok(())
}

/// Isolated margin mode for one market. accs: authority(s), user(w), market. data: tag, on u8.
/// Only while the position is flat with no resting orders; switching off returns the isolated margin to cash.
pub(crate) fn set_isolated(pid: &Address, a: &mut [AccountView], d: &[u8]) -> ProgramResult {
    if a.len() < 3 || d.len() < 2 {
        return Err(err(E::BadData));
    }
    let u: &mut User = load(&a[1], pid, DISC_USER, USER_SIZE)?;
    let m: &Market = load(&a[2], pid, DISC_MARKET, MARKET_SIZE)?;
    if !sg(pid, &a[0]) || u.authority != key(&a[0]) {
        return Err(err(E::BadAccount));
    }
    home(u, m)?;
    let mk = key(&a[2]);
    let pi = pos_idx(u, &mk, m.funding_index)?;
    let p = &mut u.pos[pi];
    if p.base != 0 || p.bid_open != 0 || p.ask_open != 0 || p.n_orders != 0 {
        return Err(err(E::Isolated));
    }
    // a maker's market stays cross: its fills are margined by the whole account (maker margin + budget), never by an
    // isolated bucket the settle can't see
    if d[1] != 0 && p.flags & POS_PIN != 0 {
        return Err(err(E::Isolated));
    }
    if d[1] != 0 {
        p.flags |= POS_ISO;
    } else if p.flags & POS_ISO != 0 {
        p.flags &= !POS_ISO;
        u.cash = i64c(u.cash as i128 + p.iso_margin as i128)?;
        p.iso_margin = 0;
    }
    gc_pos(u);
    Ok(())
}

/// Move collateral between cross cash and an isolated position. accs: authority(s), user(w), market, [other
/// markets for cross health]. data: tag, amount i64 (> 0: cash -> isolated, < 0: isolated -> cash).
pub(crate) fn iso_transfer(pid: &Address, a: &mut [AccountView], d: &[u8]) -> ProgramResult {
    if a.len() < 3 {
        return Err(err(E::BadData));
    }
    let amt = rd64(d, 1)? as i64;
    let u: &mut User = load(&a[1], pid, DISC_USER, USER_SIZE)?;
    let m: &Market = load(&a[2], pid, DISC_MARKET, MARKET_SIZE)?;
    if !sg(pid, &a[0]) || u.authority != key(&a[0]) {
        return Err(err(E::BadAccount));
    }
    home(u, m)?;
    let mk = key(&a[2]);
    let pi = u.pos.iter().position(|p| p.market == mk && p.flags & POS_ISO != 0).ok_or(err(E::Isolated))?;
    let clk = clock()?;
    let nw: Now = (clk.slot, clk.unix_timestamp);
    if u.triggers.iter().any(|t| committed(t, clk.slot)) {
        return Err(err(E::Locked));
    } // as in withdraw: no moving margin away from a committed order
    let uk = key(&a[1]);
    claim_all(pid, u, &uk, &a[3..])?;
    complete_view(pid, u, &uk, &a[2..], if amt < 0 { Some(&mk) } else { None })?;
    settle_funding_all(pid, u, &a[2..])?;
    u.cash = i64c(u.cash as i128 - amt as i128)?;
    u.pos[pi].iso_margin = i64c(u.pos[pi].iso_margin as i128 + amt as i128)?;
    // collateral leaving a margin bucket is checked without that bucket's positive PnL (same lane as withdraw)
    if amt > 0 {
        let (e, im) = health_np(pid, u, &a[2..], nw)?;
        if u.cash < 0 || e < im {
            return Err(err(E::Margin));
        }
    } else {
        let (e, im) = iso_health_np(pid, u, pi, &a[2..], nw)?;
        if u.pos[pi].iso_margin < 0 || e < im {
            return Err(err(E::Margin));
        }
    }
    gc_pos(u);
    Ok(())
}

/// Value a position contributes to equity with its positive PnL left out (percolator v16 `Eq_no_positive_credit`):
/// min(base x mark, cost). Losses count in full, gains not at all; `cost` is the net quote paid since the position opened.
#[inline(always)]
pub(crate) fn np_value(base: i64, cost: i64, px: u64) -> i128 {
    core::cmp::min(mulf(base as i128, px), cost as i128)
}

/// Withdrawal lane: (equity without positive unrealized PnL, initial req). Principal can only leave against equity
/// that does not depend on the oracle having moved in the account's favour, so a manipulated or briefly wrong mark
/// can't be turned into a withdrawal of collateral (percolator req 8/9/15: positive PnL is junior, no unbacked use).
pub(crate) fn health_np(pid: &Address, u: &User, a: &[AccountView], nw: Now) -> Result<(i128, i128), ProgramError> {
    let (mut eq, mut im) = (u.cash as i128, 0i128);
    for p in u.pos.iter() {
        if p.market == [0; 32] {
            break;
        }
        if p.flags & POS_ISO != 0 {
            continue;
        }
        let m = find_market(pid, &p.market, a)?;
        if stale(m, nw) && (p.base != 0 || p.bid_open != 0 || p.ask_open != 0) {
            return Err(err(E::Stale));
        }
        let (px, b) = (mark(m), band_bps(m));
        eq += np_value(p.base, p.cost, px);
        let gross =
            core::cmp::max((p.base as i128 + p.bid_open as i128).abs(), (p.base as i128 - p.ask_open as i128).abs());
        im += bps_up(mulf(gross, px), m.imr_bps + b);
    }
    Ok((eq, im))
}

pub(crate) fn iso_health_np(
    pid: &Address,
    u: &User,
    pi: usize,
    a: &[AccountView],
    nw: Now,
) -> Result<(i128, i128), ProgramError> {
    let p = &u.pos[pi];
    let m = find_market(pid, &p.market, a)?;
    if stale(m, nw) && (p.base != 0 || p.bid_open != 0 || p.ask_open != 0) {
        return Err(err(E::Stale));
    }
    let (px, b) = (mark(m), band_bps(m));
    let gross =
        core::cmp::max((p.base as i128 + p.bid_open as i128).abs(), (p.base as i128 - p.ask_open as i128).abs());
    Ok((p.iso_margin as i128 + np_value(p.base, p.cost, px), bps_up(mulf(gross, px), m.imr_bps + b)))
}

#[inline(always)]
pub(crate) fn health_g<const MM: bool>(
    pid: &Address,
    u: &User,
    a: &[AccountView],
    nw: Now,
    need_fresh: bool,
) -> Result<(i128, i128, i128), ProgramError> {
    let (mut eq, mut im, mut mm) = (u.cash as i128, 0i128, 0i128);
    for p in u.pos.iter() {
        if p.market == [0; 32] {
            break;
        }
        if p.flags & POS_ISO != 0 {
            continue;
        } // isolated positions are margined on their own
        let m = find_market(pid, &p.market, a)?;
        if need_fresh && stale(m, nw) && (p.base != 0 || p.bid_open != 0 || p.ask_open != 0) {
            return Err(err(E::Stale));
        }
        let (px, b) = (mark(m), band_bps(m));
        eq += mulf(p.base as i128, px);
        let gross =
            core::cmp::max((p.base as i128 + p.bid_open as i128).abs(), (p.base as i128 - p.ask_open as i128).abs());
        im += bps_up(mulf(gross, px), m.imr_bps + b);
        if MM {
            mm += bps_up(mulf((p.base as i128).abs(), px), m.mmr_bps + b);
        }
    }
    Ok((eq, im, mm))
}

/// (equity, initial requirement, maintenance requirement) of the cross account.
pub(crate) fn health(
    pid: &Address,
    u: &User,
    a: &[AccountView],
    nw: Now,
    need_fresh: bool,
) -> Result<(i128, i128, i128), ProgramError> {
    health_g::<true>(pid, u, a, nw, need_fresh)
}

/// The same pass without the maintenance requirement (third value 0), for the checks that only compare equity with the
/// initial requirement: one product and one bps rounding less per position.
pub(crate) fn health_im(
    pid: &Address,
    u: &User,
    a: &[AccountView],
    nw: Now,
    need_fresh: bool,
) -> Result<(i128, i128, i128), ProgramError> {
    health_g::<false>(pid, u, a, nw, need_fresh)
}

pub(crate) fn pos_idx(u: &mut User, mk: &Key, funding_index: i64) -> Result<usize, ProgramError> {
    let mut free = usize::MAX;
    for (i, p) in u.pos.iter().enumerate() {
        if p.market == *mk {
            return Ok(i);
        }
        if p.market == [0; 32] {
            free = i;
            break;
        }
    }
    if free == usize::MAX {
        return Err(err(E::Cap));
    }
    u.pos[free] = Pos {
        market: *mk,
        base: 0,
        funding_snap: funding_index,
        bid_open: 0,
        ask_open: 0,
        n_orders: 0,
        cost: 0,
        iso_margin: 0,
        flags: 0,
    };
    Ok(free)
}

/// Free flat, idle positions and keep the used ones packed at the front in their order, so every loop over
/// positions stops at the first empty slot. Runs last in each instruction (indices held earlier are stale after it).
#[cold]
#[inline(never)]
pub(crate) fn trig_sync(u: &mut User) {
    for i in 0..MAX_TRIGGERS {
        let t = &u.triggers[i];
        if t.kind == 0 || t.kind & 0xff >= TRIGGER_KIND_TWAP {
            continue;
        } // stops only: a TWAP or a next-price order is not tied to a position
        let mk = t.market;
        let open = u.pos.iter().any(|p| p.market == mk && p.base != 0);
        let t = &mut u.triggers[i];
        if open {
            t.next_slot = 1;
        } else if t.next_slot != 0 {
            t.kind = 0;
        }
    }
}

pub(crate) fn gc_pos(u: &mut User) {
    // a TP/SL dies with the position it protected: once it has seen one (next_slot = 1) and the account is flat in that market,
    // it is cleared, so it can't fire on the next, unrelated position. One set ahead of an entry order waits for the position.
    if u.triggers[0].kind | u.triggers[1].kind | u.triggers[2].kind | u.triggers[3].kind != 0 {
        trig_sync(u);
    }
    let mut j = 0;
    for i in 0..MAX_POS {
        let p = &mut u.pos[i];
        if p.market == [0; 32] {
            break;
        }
        if p.base == 0 {
            p.cost = 0;
            p.flags &= POS_ISO | POS_PIN;
            if p.bid_open == 0 && p.ask_open == 0 && p.iso_margin == 0 && p.flags == 0 {
                p.market = [0; 32];
                continue;
            }
        }
        if i != j {
            u.pos.swap(i, j);
        }
        j += 1;
    }
}

#[inline(always)]
/// Maker margin locked in this market's books, mirrored on the (cross) position in `iso_margin` (unused for cross positions):
/// liquidation and ADL refuse while any is locked (maker_clear). Saturates at 0 for margin locked before the mirror existed.
pub(crate) fn lock_mirror(u: &mut User, pi: usize, d: i128) -> ProgramResult {
    let p = &mut u.pos[pi];
    if p.flags & POS_ISO != 0 {
        return Err(err(E::Isolated));
    }
    p.iso_margin = i64c((p.iso_margin as i128 + d).max(0))?;
    Ok(())
}

/// A market with one book (both registered slots the same account): the mirror is exactly that book's margin.
pub(crate) fn sync_mirror(u: &mut User, m: &Market, mk: &Key, margin: i64) {
    if m.fast_book[0] != m.fast_book[1] && m.fast_book[1] != [0; 32] {
        return;
    }
    if let Some(p) = u.pos.iter_mut().find(|p| p.market == *mk && p.flags & POS_ISO == 0) {
        p.iso_margin = margin.max(0);
    }
}

pub(crate) fn maker_budget(m: &Market, margin: i64) -> i64 {
    i64::try_from(margin.max(0) as i128 * 10000 / m.imr_bps as i128).unwrap_or(i64::MAX)
}

/// What takes see of a maker until its next settle: its position in this market and its rebate tier.
#[inline(always)]
pub(crate) fn maker_ext(u: &User, mk: &Key) -> MakerExt {
    MakerExt {
        inv0: u.pos.iter().find(|p| p.market == *mk).map_or(0, |p| p.base),
        rebate_bps: TIERS[(u.tier as usize).min(TIERS.len() - 1)].1,
    }
}

/// Atomic guard: fails the whole transaction unless the account ends where the caller expects. Put it last in a multi-leg
/// transaction (pair trade, multi-market rebalance, perp + spot hedge): a leg that fills short reverts every leg.
/// Checks position bounds per market, token account balances (the spot leg), and a minimum cross equity at the stored mark.
/// accs: user(r), markets(r) x np, token accounts(r) x nt, [other markets the user holds, for the equity check]
/// data: tag, np u8, nt u8, min_equity i64 (i64::MIN = unchecked), np x (min_base i64, max_base i64), nt x min_amount u64
pub(crate) fn assert_state(pid: &Address, a: &mut [AccountView], d: &[u8]) -> ProgramResult {
    if d.len() < 11 {
        return Err(err(E::BadData));
    }
    let (np, nt) = (d[1] as usize, d[2] as usize);
    if d.len() != 11 + np * 16 + nt * 8 || a.len() < 1 + np + nt {
        return Err(err(E::BadData));
    }
    let u: &User = load(&a[0], pid, DISC_USER, USER_SIZE)?;
    for i in 0..np {
        let _m: &Market = load(&a[1 + i], pid, DISC_MARKET, MARKET_SIZE)?;
        let mk = key(&a[1 + i]);
        let (lo, hi) = (rd64(d, 11 + i * 16)? as i64, rd64(d, 19 + i * 16)? as i64);
        let mut b = 0i64;
        for p in u.pos.iter() {
            if p.market == [0; 32] {
                break;
            }
            if p.market == mk {
                b = p.base;
                break;
            }
        }
        if b < lo || b > hi {
            return Err(err(E::Assert));
        }
    }
    for j in 0..nt {
        let t = &a[1 + np + j];
        if !t.owned_by(&pinocchio_token::ID) || t.data_len() < 165 {
            return Err(err(E::BadAccount));
        }
        let amt = u64::from_le_bytes(unsafe { t.borrow_unchecked() }[64..72].try_into().unwrap());
        if amt < rd64(d, 11 + np * 16 + j * 8)? {
            return Err(err(E::Assert));
        }
    }
    let min_eq = rd64(d, 3)? as i64;
    if min_eq != i64::MIN {
        let clk = clock()?;
        let (eq, _, _) = health_im(pid, u, &a[1..], (clk.slot, clk.unix_timestamp), false)?;
        if eq < min_eq as i128 {
            return Err(err(E::Assert));
        }
    }
    Ok(())
}

/// Margin an account has locked for quoting, as the program mirrors it on its pinned cross positions.
pub(crate) fn maker_mirror(u: &User) -> i128 {
    u.pos
        .iter()
        .filter(|p| p.market != [0; 32] && p.flags & (POS_PIN | POS_ISO) == POS_PIN)
        .map(|p| p.iso_margin as i128)
        .sum()
}
