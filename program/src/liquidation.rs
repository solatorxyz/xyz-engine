//! Liquidation, in the order it is tried: against the book in chunks inside a price band (`liquidate_book`,
//! `liquidate_fast`), then by a liquidator or the vault taking the position over at the mark (`liquidate`), and only
//! for an account that is bankrupt after that, auto-deleveraging (`adl.rs`). An account is judged only when it has no
//! resting orders left (`no_resting`), so no one can hide a fill to make a solvent account look insolvent.

use super::*;

/// Permissionless (IX_LIQ_CANCEL). accs: user(w), market(r), [that market's shards (w)], [the user's other markets].
/// Only for an account below maintenance on its position in `market` (whole cross account, or that isolated position): its
/// fills in the passed shards are claimed and its resting orders there cancelled. Cancelling only frees initial margin, so
/// doing it for an account that is being liquidated is safe, and it lets the liquidation that follows see a complete account.
pub(crate) fn liq_cancel(pid: &Address, a: &mut [AccountView]) -> ProgramResult {
    if a.len() < 2 {
        return Err(err(E::BadData));
    }
    let t: &mut User = load(&a[0], pid, DISC_USER, USER_SIZE)?;
    let m: &Market = load(&a[1], pid, DISC_MARKET, MARKET_SIZE)?;
    home(t, m)?;
    let (tk, mk) = (key(&a[0]), key(&a[1]));
    let clk = clock()?;
    let nw: Now = (clk.slot, clk.unix_timestamp);
    claim_all(pid, t, &tk, &a[2..])?;
    settle_funding_all(pid, t, &a[1..])?;
    let pi = t.pos.iter().position(|p| p.market == mk).ok_or(err(E::NotLiquidatable))?;
    // the judged account is seen whole: every shard of each market where it has orders (all of its fills claimed), and no maker
    // margin outside its equity; for an isolated position only that market. A hidden fill elsewhere can't make it look weak.
    let iso = t.pos[pi].flags & POS_ISO != 0;
    complete_view(pid, t, &tk, &a[1..], if iso { Some(&mk) } else { None })?;
    if !iso {
        maker_clear(pid, t, &tk, m, &a[1..])?;
    }
    let (eq, _, mm) = phealth(pid, t, pi, &a[1..], nw, true)?;
    if eq >= mm {
        return Err(err(E::NotLiquidatable));
    }
    cancel_all_of(&a[2..], pid, &mk, &tk, t, pi);
    gc_pos(t);
    Ok(())
}

/// Before an account is judged for liquidation / ADL (and before any deficit is socialized): no resting orders, so no fills
/// left unclaimed, on the positions that make up the judged health (every cross position, or the isolated one). Otherwise a
/// liquidator choosing which shards to pass could hide a profitable fill and make a solvent account look bankrupt.
/// The liquidated market's own orders are cancelled by the liquidation itself; others first through IX_LIQ_CANCEL.
pub(crate) fn no_resting(u: &User, pi: usize) -> ProgramResult {
    let iso = u.pos[pi].flags & POS_ISO != 0;
    for (i, p) in u.pos.iter().enumerate() {
        if p.market == [0; 32] {
            break;
        }
        let judged = if iso { i == pi } else { p.flags & POS_ISO == 0 };
        if judged && (p.n_orders != 0 || p.bid_open != 0 || p.ask_open != 0) {
            return Err(err(E::MissingShards));
        }
    }
    Ok(())
}

/// AUDIT M-6: liquidation is a takeover at the protocol price (no book, no shard choice, no price discretion):
/// the liquidator assumes min(|base|, max_qty) of the target's position at the mark price, the target pays
/// LIQ_FEE_BPS to the liquidator and the same to insurance (capped by remaining equity). The liquidator must
/// stay above initial margin. Same terms for every liquidator -> self-dealing gains nothing.
/// accs: liquidator authority(s), liquidator user(w), target user(w), market(r), insurance(w), target-order shards(w)..., [other markets]
/// data: tag, max_qty u64
pub(crate) fn liquidate(pid: &Address, a: &mut [AccountView], d: &[u8]) -> ProgramResult {
    liquidate_core(pid, a, d, false)
}

pub(crate) fn liquidate_core(pid: &Address, a: &mut [AccountView], d: &[u8], vlt: bool) -> ProgramResult {
    if a.len() < 5 {
        return Err(err(E::BadData));
    }
    let max_qty = rd64(d, 1)?;
    if a[1].address() == a[2].address() {
        return Err(err(E::BadAccount));
    }
    let lq: &mut User = load(&a[1], pid, DISC_USER, USER_SIZE)?;
    let t: &mut User = load(&a[2], pid, DISC_USER, USER_SIZE)?;
    let m: &Market = load(&a[3], pid, DISC_MARKET, MARKET_SIZE)?;
    let ins: &mut Insurance = load(&a[4], pid, DISC_INS, INS_SIZE)?;
    let (tk, mk) = (key(&a[2]), key(&a[3]));
    if (!vlt && !sg(pid, &a[0])) || lq.authority != key(&a[0]) || m.insurance != key(&a[4]) {
        return Err(err(E::BadAccount));
    }
    home(lq, m)?;
    home(t, m)?;
    let clk = clock()?;
    let nw: Now = (clk.slot, clk.unix_timestamp);
    if stale(m, nw) {
        return Err(err(E::Stale));
    }
    let mkts = &a[3..];
    claim_all(pid, t, &tk, &a[5..])?;
    settle_funding_all(pid, t, mkts)?;
    settle_funding_all(pid, lq, mkts)?;
    let pi = t.pos.iter().position(|p| p.market == mk).ok_or(err(E::NotLiquidatable))?;
    if t.pos[pi].flags & POS_ISO == 0 {
        maker_clear(pid, t, &tk, m, mkts)?;
    }
    let (eq, _, mm) = phealth(pid, t, pi, mkts, nw, true)?;
    // the vault steps in only once the book had its chance: equity under 2/3 of maintenance
    if eq >= (if vlt { mm * 2 / 3 } else { mm }) {
        return Err(err(E::NotLiquidatable));
    }
    cancel_all_of(&a[5..], pid, &mk, &tk, t, pi); // free the target's resting margin first
                                                  // AUDIT M-2 + complete view: no resting orders / unclaimed fills left on any judged position (others: IX_LIQ_CANCEL first)
    no_resting(t, pi)?;
    let b = t.pos[pi].base;
    if b == 0 {
        return Err(err(E::NotLiquidatable));
    }
    let px = mark(m);
    // same size limits as a book liquidation: one chunk, never more than restores maintenance (the fee is on what is taken)
    let ab = b.unsigned_abs();
    // a third-party takeover is one bounded chunk, never more than restores maintenance (its fee is on what it takes);
    // the protocol vault (HLP-style backstop, entered only below 2/3 of maintenance) takes the whole position
    let q = if vlt {
        ab.min(max_qty)
    } else {
        ab.min(max_qty).min(liq_chunk(ab)).min(liq_needed(eq, mm, ab, px, m.mmr_bps + band_bps(m)))
    };
    if q == 0 {
        return Err(err(E::BadData));
    }
    let li = pos_idx(lq, &mk, m.funding_index)?;
    if lq.pos[li].flags & POS_ISO != 0 {
        return Err(err(E::Isolated));
    } // takeover lands in the liquidator's cross account
    let s: i64 = if b > 0 { 1 } else { -1 };
    let n = mulf(q as i128, px);
    let l0 = long(t.pos[pi].base) + long(lq.pos[li].base);
    let (otb, olb) = (t.pos[pi].base, lq.pos[li].base);
    t.pos[pi].base -= s * q as i64;
    lq.pos[li].base += s * q as i64;
    credit(t, pi, s as i128 * n)?;
    lq.cash = i64c(lq.cash as i128 - s as i128 * n)?;
    t.pos[pi].cost = cost_after(otb, t.pos[pi].base, t.pos[pi].cost, -(s as i128) * n)?;
    lq.pos[li].cost = cost_after(olb, lq.pos[li].base, lq.pos[li].cost, s as i128 * n)?;
    ins.oi_long_delta += long(t.pos[pi].base) + long(lq.pos[li].base) - l0;
    if m.max_position != 0 && lq.pos[li].base.unsigned_abs() > m.max_position {
        return Err(err(E::Cap));
    }
    // CU: the takeover is at the mark and cancels don't touch cash, so equity is unchanged: eq2 == eq (no 2nd health pass)
    let eq2 = eq;
    let fee = bps_dn(n, LIQ_FEE_BPS).min(if eq2 > 0 { eq2 / 2 } else { 0 });
    credit(t, pi, -2 * fee)?;
    lq.cash = i64c(lq.cash as i128 + fee)?;
    ins.balance = i64c(ins.balance as i128 + fee)?;
    // AUDIT N-3/N-5: insurance never goes negative. A deficit larger than the fund can't be liquidated here:
    // the account goes through ADL (socialized against ranked counterparties, remainder -> Market.bad_debt only).
    if eq2 < 0 {
        // loss order: leftover insurance, then vault capital, then ADL
        let cap = ins.balance.max(0) as i128 + if vlt { lq.cash.max(0) as i128 } else { 0 };
        if -eq2 > cap {
            return Err(err(E::UseAdl));
        }
        let closed = if t.pos[pi].flags & POS_ISO != 0 {
            t.pos[pi].base == 0
        } else {
            t.pos.iter().all(|p| p.base == 0 || p.flags & POS_ISO != 0)
        };
        if closed {
            let from_ins = (-eq2).min(ins.balance.max(0) as i128);
            let from_v = -eq2 - from_ins;
            ins.balance = i64c(ins.balance as i128 - from_ins)?;
            lq.cash = i64c(lq.cash as i128 - from_v)?;
            credit(t, pi, -eq2)?;
        }
    }
    let (le, lim, _) = health_im(pid, lq, mkts, nw, true)?;
    if le < lim {
        return Err(err(E::Margin));
    }
    emit(&[&[EVENTS_LIQUIDATION as u8], &tk, &q.to_le_bytes(), &px.to_le_bytes(), &key(&a[1])]);
    gc_pos(t);
    gc_pos(lq);
    Ok(())
}

/// AUDIT V5-3: book liquidation, no liquidator capital. Permissionless. Cancels the target's orders in this
/// market, then sends ONE reduce-only IOC chunk (<= LIQ_BOOK_CHUNK_BPS of |base|, <= max_qty, >= 1 lot) into the
/// book, limited to mark -/+ LIQ_BOOK_BAND_BPS. Fee (LIQ_FEE_BPS of filled x mark, capped at remaining equity)
/// -> insurance. A deficit above the insurance balance reverts with UseAdl (ADL path); insurance never < 0.
/// Same rule as perp_core::Exchange::liquidate_book (conformance-tested against core + C++).
/// accs: target user(w), market(r), insurance(w), shards(w)..., [other markets]. data: tag, max_qty u64
pub(crate) fn liquidate_book(pid: &Address, a: &mut [AccountView], d: &[u8]) -> ProgramResult {
    if a.len() < 4 {
        return Err(err(E::BadData));
    }
    let max_qty = rd64(d, 1)?;
    let t: &mut User = load(&a[0], pid, DISC_USER, USER_SIZE)?;
    let m: &Market = load(&a[1], pid, DISC_MARKET, MARKET_SIZE)?;
    let ins: &mut Insurance = load(&a[2], pid, DISC_INS, INS_SIZE)?;
    if m.insurance != key(&a[2]) {
        return Err(err(E::BadAccount));
    }
    home(t, m)?;
    let (tk, mk) = (key(&a[0]), key(&a[1]));
    let clk = clock()?;
    let nw: Now = (clk.slot, clk.unix_timestamp);
    if stale(m, nw) {
        return Err(err(E::Stale));
    }
    let mkts = &a[1..];
    claim_all(pid, t, &tk, &a[3..])?;
    settle_funding_all(pid, t, mkts)?;
    let pi = t.pos.iter().position(|p| p.market == mk).ok_or(err(E::NotLiquidatable))?;
    if t.pos[pi].flags & POS_ISO == 0 {
        maker_clear(pid, t, &tk, m, mkts)?;
    }
    let (eq, _, mm) = phealth(pid, t, pi, mkts, nw, true)?;
    if eq >= mm {
        return Err(err(E::NotLiquidatable));
    }
    cancel_all_of(&a[3..], pid, &mk, &tk, t, pi);
    no_resting(t, pi)?;
    let b = t.pos[pi].base;
    if b == 0 {
        return Err(err(E::NotLiquidatable));
    }
    let ab = b.unsigned_abs();
    let px = mark(m);
    let bid = b < 0;
    let q = ab.min(max_qty).min(liq_chunk(ab)).min(liq_needed(eq, mm, ab, px, m.mmr_bps + band_bps(m)));
    if q == 0 {
        return Err(err(E::BadData));
    }
    let taker_bps = TIERS[(t.tier as usize).min(TIERS.len() - 1)].0;
    // maker quotes on that side are book too: such a market is liquidated by liquidate_fast, which takes both
    if m.fast_book[if bid { 1 } else { 0 }] != [0; 32] {
        return Err(err(E::BadAccount));
    }
    // the permissionless caller can't hide the better bids: every opposite shard is passed, each once
    all_shards(pid, &a[3..], &mk, m, if bid { SIDE_ASK } else { SIDE_BID })?;
    // AUDIT V6-1: escalate the band (5% -> 10% -> 20%); each wider step doubles the liquidation fee (penalty to insurance)
    let (mut filled, mut step) = (0u64, 0u64);
    while step < LIQ_BOOK_BAND_STEPS {
        let band = px * (LIQ_BOOK_BAND_BPS << step) / 10000;
        let limit = if bid { px + band } else { px.saturating_sub(band).max(1) };
        filled = take(pid, &a[3..], &mk, m, t, &tk, pi, bid, limit, q, clk.slot, taker_bps, STP_CANCEL_OLDEST)?.filled;
        if filled != 0 {
            break;
        }
        step += 1;
    }
    if filled == 0 {
        // No liquidity within the widest band. With EVERY shard of the market passed (proof the book is empty there),
        // the position becomes ADL-eligible for LIQ_ADL_WINDOW_SLOTS (ADL then accepts it while below MM, not only
        // when bankrupt). Without the full shard set: NoFill.
        if m.n_shards == 0 || m.n_shards > 64 {
            return Err(err(E::NoFill));
        }
        // maker quotes are book too: with a registered FastBook for this side, emptiness is proven by liquidate_fast
        if m.fast_book[if bid { 1 } else { 0 }] != [0; 32] {
            return Err(err(E::NoFill));
        }
        let mut seen = 0u64;
        for x in a[3..].iter() {
            if let Ok(sh) = load::<Shard>(x, pid, DISC_SHARD, SHARD_SIZE) {
                if sh.market == mk && sh.shard_no < 64 {
                    seen |= 1 << sh.shard_no;
                }
            }
        }
        let all = if m.n_shards == 64 { u64::MAX } else { (1u64 << m.n_shards) - 1 };
        if seen & all != all {
            return Err(err(E::NoFill));
        }
        t.pos[pi].flags = (t.pos[pi].flags & (POS_ISO | POS_PIN)) | spec::POS_FLAG_ADL_ELIGIBLE | (clk.slot << 8);
        emit(&[&[spec::EVENTS_LIQ_NOFILL as u8], &tk, &clk.slot.to_le_bytes()]);
        return Ok(());
    }
    let f = Fill1 { filled };
    let (eq2, _, _) = phealth(pid, t, pi, mkts, nw, false)?;
    let n = mulf(f.filled as i128, px);
    let fee = bps_dn(n, LIQ_FEE_BPS << step).min(eq2.max(0));
    credit(t, pi, -fee)?;
    pay_liq_fee(pid, &a[3..], &tk, m, ins, fee)?;
    if eq2 < 0 {
        if -eq2 > ins.balance.max(0) as i128 {
            return Err(err(E::UseAdl));
        }
        let closed = if t.pos[pi].flags & POS_ISO != 0 {
            t.pos[pi].base == 0
        } else {
            t.pos.iter().all(|p| p.base == 0 || p.flags & POS_ISO != 0)
        };
        if closed {
            ins.balance = i64c(ins.balance as i128 + eq2)?;
            credit(t, pi, -eq2)?;
        }
    }
    emit(&[&[EVENTS_LIQUIDATION as u8], &tk, &f.filled.to_le_bytes(), &px.to_le_bytes(), &[0u8; 32]]);
    gc_pos(t);
    Ok(())
}

/// Liquidation fee: LIQ_KEEPER_SHARE_BPS goes to the keeper that sent the liquidation (the first other User account passed;
/// it must hold cash against the same vault), the rest to insurance. Without a keeper account all of it goes to insurance.
/// Paying whoever does the work lets anyone run a liquidator, so liquidations don't hang on one operator.
pub(crate) fn pay_liq_fee(
    pid: &Address,
    rest: &[AccountView],
    tk: &Key,
    m: &Market,
    ins: &mut Insurance,
    fee: i128,
) -> ProgramResult {
    let mut to_ins = fee;
    if let Some(x) = rest.iter().find(|x| x.owned_by(pid) && disc_of(x) == DISC_USER && key(x) != *tk) {
        let k: &mut User = load(x, pid, DISC_USER, USER_SIZE)?;
        if k.home_vault != m.vault {
            return Err(err(E::Vault));
        }
        let share = fee * spec::LIQ_KEEPER_SHARE_BPS as i128 / 10000;
        k.cash = i64c(k.cash as i128 + share)?;
        to_ins -= share;
    }
    ins.balance = i64c(ins.balance as i128 + to_ins)?;
    Ok(())
}

pub(crate) struct Fill1 {
    filled: u64,
}

/// Lots to close so that afterwards equity >= 1.25 x maintenance. Closing q of |base| lots at the mark frees q/|base| of
/// the maintenance requirement and costs at most the widest-band fee (LIQ_FEE_BPS << 2), so
/// q >= (1.25 mm - eq) / (notional x (1.25 mmr - worst fee) / |base|). Partial liquidation like Hyperliquid's: an account just
/// under maintenance loses a sliver instead of a fixed 25% chunk (the chunk stays as the cap per transaction). At least 1 lot.
pub(crate) fn liq_needed(eq: i128, mm: i128, ab: u64, px: u64, mmr_bps: u64) -> u64 {
    let worst_fee = (LIQ_FEE_BPS << (LIQ_BOOK_BAND_STEPS - 1)) as i128;
    let relief = mulf(ab as i128, px) * (5 * mmr_bps as i128 - 4 * worst_fee); // x 40000 per |base| lots
    let deficit = (5 * mm - 4 * eq).max(0) * 10000; // (1.25 mm - eq) x 40000
    if relief <= 0 {
        return u64::MAX;
    }
    u64::try_from((deficit * ab as i128 + relief - 1) / relief).unwrap_or(u64::MAX).max(1)
}

#[inline(always)]
pub(crate) fn liq_chunk(ab: u64) -> u64 {
    ((ab as u128 * LIQ_BOOK_CHUNK_BPS as u128 + 9999) / 10000).max(1) as u64
}

/// Liquidation against maker quotes, for markets (or moments) where the fast book holds the liquidity. Same rules as
/// liquidate_book: below maintenance, 25% chunks, fee to insurance, bankrupt remainder to insurance or UseAdl.
/// When nothing fills, the market has no shards and every approved maker's slab was passed, the position becomes
/// ADL-eligible (the fast book is provably empty within the band). Accounts with resting book orders go through liquidate_book.
/// accs: user(w), market(r), insurance(w), fast(w), slabs(r) x ns, [other markets]. data: tag, ns u8, max_qty u64
pub(crate) fn liquidate_fast(pid: &Address, a: &mut [AccountView], d: &[u8]) -> ProgramResult {
    if a.len() < 4 || d.len() != 10 {
        return Err(err(E::BadData));
    }
    let (ns, max_qty) = (d[1] as usize, rd64(d, 2)?);
    if ns > FAST_MAKERS || a.len() < 4 + ns {
        return Err(err(E::BadData));
    }
    let t: &mut User = load(&a[0], pid, DISC_USER, USER_SIZE)?;
    let m: &Market = load(&a[1], pid, DISC_MARKET, MARKET_SIZE)?;
    let ins: &mut Insurance = load(&a[2], pid, DISC_INS, INS_SIZE)?;
    let f: &mut FastBook = load(&a[3], pid, DISC_FAST, FAST_SIZE)?;
    let (tk, mk) = (key(&a[0]), key(&a[1]));
    if m.insurance != key(&a[2]) || f.market != mk {
        return Err(err(E::BadAccount));
    }
    home(t, m)?;
    let clk = clock()?;
    let nw: Now = (clk.slot, clk.unix_timestamp);
    let slot = clk.slot;
    if stale(m, nw) {
        return Err(err(E::Stale));
    }
    let mkts = &a[1..];
    claim_all(pid, t, &tk, &a[4 + ns..])?;
    settle_funding_all(pid, t, mkts)?;
    let pi = t.pos.iter().position(|p| p.market == mk).ok_or(err(E::NotLiquidatable))?;
    if t.pos[pi].flags & POS_ISO == 0 {
        maker_clear(pid, t, &tk, m, mkts)?;
    }
    let (eq, _, mm) = phealth(pid, t, pi, mkts, nw, true)?;
    if eq >= mm {
        return Err(err(E::NotLiquidatable));
    }
    cancel_all_of(&a[4 + ns..], pid, &mk, &tk, t, pi); // the target's resting orders here go first, as in liquidate_book
    no_resting(t, pi)?;
    let b = t.pos[pi].base;
    if b == 0 {
        return Err(err(E::NotLiquidatable));
    }
    let ab = b.unsigned_abs();
    let px = mark(m);
    let bid = b < 0;
    let mut left = ab.min(max_qty).min(liq_chunk(ab)).min(liq_needed(eq, mm, ab, px, m.mmr_bps + band_bps(m)));
    if left == 0 {
        return Err(err(E::BadData));
    }
    // same price window as an ordinary take: a liquidation never reaches quotes a taker couldn't
    let band = quote_band(px);
    let (blo, bhi) = bound_range(m);
    let (lo, hi) = (px.saturating_sub(band).max(blo).max(1), (px + band).min(bhi));
    let opp = if bid { 1usize } else { 0usize };
    if (f.side != 0 && f.side != opp as u64 + 1) || !book_ok(m, &key(&a[3]), opp) {
        return Err(err(E::BadAccount));
    }
    let base_ix = (f.idx as usize) << 4 | opp << 3;
    // every approved maker must be passed: the liquidator can't steer the fill to its own quotes
    let (sl, cnt, unreg) = collect_slabs(pid, f, &a[4..4 + ns], &tk, None, true, false)?;
    let unregistered = unreg != 0;
    let base0 = long(t.pos[pi].base);
    let (mut filled, mut notional) = (0u64, 0i128);
    // a market with shards too: every opposite shard is passed (after the slabs) and its resting orders inside the band fill
    // first, so the liquidator can't route around better resting liquidity to maker quotes it prefers
    let sside = if bid { SIDE_ASK } else { SIDE_BID };
    let hybrid = m.n_side[sside as usize] != 0;
    if hybrid {
        all_shards(pid, &a[4 + ns..], &mk, m, sside)?;
    }
    let taker_bps = TIERS[(t.tier as usize).min(TIERS.len() - 1)].0;
    let mut in_shards = 0i64;
    let mut shard_fill = 0u64;
    let mut step = 0u64;
    // resting orders and maker quotes fill best price first: before each maker level, the shards' orders strictly better than it
    // the shards' best price is read once and refreshed only after a shard fill (not before every maker level)
    let mut sbest: Option<u64> =
        if hybrid { best(&a[4 + ns..], pid, &mk, sside, m.oracle_px).map(|b| b.2) } else { None };
    let mut shard_take =
        |t: &mut User, lim: u64, left: u64, sbest: &mut Option<u64>| -> Result<(u64, i128), ProgramError> {
            let reach = match *sbest {
                Some(p) => {
                    if bid {
                        p <= lim
                    } else {
                        p >= lim
                    }
                }
                None => false,
            };
            if !reach {
                return Ok((0, 0));
            }
            let lb = long(t.pos[pi].base);
            let tf = take(pid, &a[4 + ns..], &mk, m, t, &tk, pi, bid, lim, left, slot, taker_bps, STP_CANCEL_OLDEST)?;
            in_shards += long(t.pos[pi].base) - lb;
            shard_fill += tf.filled;
            *sbest = best(&a[4 + ns..], pid, &mk, sside, m.oracle_px).map(|b| b.2);
            Ok((tf.filled, tf.notional))
        };
    let mut ev = FastEv::new(&tk, bid);
    // same rule as a take: maker quotes fill only while the price they are judged against is recent (resting orders still do)
    let aged = !(f.fast_px != 0 && slot <= f.fast_slot + spec::FAST_PX_MAX_SLOTS)
        && slot > m.oracle_slot + spec::FAST_GUARD_MAX_AGE_SLOTS;
    let mut levels = 0u32;
    while left > 0 && levels < spec::LIQ_FAST_MAX_LEVELS {
        let best = if aged {
            None
        } else {
            best_quote(f, &sl[..cnt], base_ix, opp, bid, slot, lo, hi, book_px(f, m, slot), true)
        }; // a liquidation fills next-price ladders too, at the book's price
        if hybrid {
            let lim = match best {
                Some((_, _, bp, _)) => {
                    if bid {
                        hi.min(bp)
                    } else {
                        lo.max(bp)
                    }
                }
                None => {
                    if bid {
                        hi
                    } else {
                        lo
                    }
                }
            };
            let (sf, sn) = shard_take(t, lim, left, &mut sbest)?;
            left -= sf;
            filled += sf;
            notional += sn;
            if left == 0 {
                break;
            }
        }
        let Some((bk, bl, bp, ba)) = best else { break };
        levels += 1;
        let (sp, mi) = sl[bk];
        let q = unsafe { &(*sp).quotes[base_ix + bl] };
        let fq = ba.min(left);
        let nt = fill_maker(f, mi, q, opp << 3 | bl, bid, bp, fq, 0, m.funding_index)?;
        let sg: i128 = if bid { 1 } else { -1 };
        credit(t, pi, -sg * nt as i128)?;
        let ob = t.pos[pi].base;
        t.pos[pi].base = ob.checked_add(sg as i64 * fq as i64).ok_or(err(E::Math))?;
        t.pos[pi].cost = cost_after(ob, t.pos[pi].base, t.pos[pi].cost, sg * nt as i128)?;
        ev.push(bp, fq, (mi as u64) << 32 | q.gen as u64);
        filled += fq;
        notional += nt as i128;
        left -= fq;
    }
    ev.flush();
    // nothing within the quote band: the shards' orders further out, as a book liquidation would (10%, then 20%; each wider step
    // doubles the fee, a penalty to insurance)
    let lad = ((f.idx as usize) << 1 | opp).min(SLAB_LADDERS - 1);
    let ladders_off = f.px_signer != [0; 32]
        && sl[..cnt].iter().any(|&(sp, _)| {
            let fr = (unsafe { (*sp).lad[lad].flags } >> spec::LADDER_FRESH_SHIFT) as u64;
            fr != 0 && (f.fast_px == 0 || slot > f.fast_slot + fr.min(spec::FAST_PX_MAX_SLOTS))
        });
    while filled == 0 && hybrid && !ladders_off && step + 1 < LIQ_BOOK_BAND_STEPS {
        step += 1;
        let band = (px as u128 * (LIQ_BOOK_BAND_BPS << step) as u128 / 10000) as u64;
        let lim = if bid { (px + band).min(bhi) } else { px.saturating_sub(band).max(blo).max(1) };
        let (sf, sn) = shard_take(t, lim, left, &mut sbest)?;
        left -= sf;
        filled += sf;
        notional += sn;
    }
    if filled == 0 {
        // no fill opens ADL only when the book really is empty: it has makers (a spare empty book proves nothing), all were
        // passed (checked above), and fresh-only ladders are not merely switched off for want of a current signed price
        let px_fresh = f.fast_px != 0 && slot <= f.fast_slot + spec::FAST_PX_MAX_SLOTS;
        if f.n == 0 || unregistered || (f.px_signer != [0; 32] && !px_fresh) {
            return Err(err(E::NoFill));
        }
        // a market with shards too (all opposite shards were passed above): none may hold a live order a book liquidation could
        // still reach (its widest band, any better price; expired / off-band orders can never fill and don't count)
        if m.n_side[sside as usize] != 0 {
            let wide = (px as u128 * (LIQ_BOOK_BAND_BPS << (LIQ_BOOK_BAND_STEPS - 1)) as u128 / 10000) as u64;
            for x in a[4 + ns..].iter() {
                if disc_of(x) != DISC_SHARD || !x.owned_by(pid) {
                    continue;
                }
                let s: &Shard = unsafe { &*(x.borrow_unchecked().as_ptr() as *const Shard) };
                if s.market != mk || s.side != sside {
                    continue;
                }
                let live = s.orders[..s.n as usize].iter().any(|o| {
                    let p = eff(o, sside == SIDE_BID, m.oracle_px);
                    let gone = (o.expiry_slot != 0 && slot > o.expiry_slot)
                        || (o.band_bps != 0 && off_band(p, m.oracle_px, o.band_bps))
                        || p < blo
                        || p > bhi;
                    !gone
                        && o.owner != tk
                        && if sside == SIDE_BID { p >= px.saturating_sub(wide) } else { p <= px + wide }
                });
                if live {
                    return Err(err(E::NoFill));
                }
            }
        }
        // a fresh-only ladder switched off only because the stored signed price is older than its own window is not "empty"
        if ladders_off {
            return Err(err(E::NoFill));
        }
        t.pos[pi].flags = (t.pos[pi].flags & (POS_ISO | POS_PIN)) | spec::POS_FLAG_ADL_ELIGIBLE | (slot << 8);
        emit(&[&[spec::EVENTS_LIQ_NOFILL as u8], &tk, &slot.to_le_bytes()]);
        return Ok(());
    }
    f.oi_long_delta += long(t.pos[pi].base) - base0 - in_shards; // the shard part is booked on its shard by take()
    if filled > shard_fill {
        f.fills += 1;
    }
    let (eq2, _, _) = phealth(pid, t, pi, mkts, nw, false)?;
    let fee = bps_dn(notional, LIQ_FEE_BPS << step).min(eq2.max(0));
    credit(t, pi, -fee)?;
    pay_liq_fee(pid, &a[4 + ns..], &tk, m, ins, fee)?;
    if eq2 < 0 {
        if -eq2 > ins.balance.max(0) as i128 {
            return Err(err(E::UseAdl));
        }
        let closed = if t.pos[pi].flags & POS_ISO != 0 {
            t.pos[pi].base == 0
        } else {
            t.pos.iter().all(|p| p.base == 0 || p.flags & POS_ISO != 0)
        };
        if closed {
            ins.balance = i64c(ins.balance as i128 + eq2)?;
            credit(t, pi, -eq2)?;
        }
    }
    emit(&[&[EVENTS_LIQUIDATION as u8], &tk, &filled.to_le_bytes(), &px.to_le_bytes(), &[0u8; 32]]);
    gc_pos(t);
    Ok(())
}
