//! The resting order book: price-time priority across shards.
//!
//! A market's resting orders live in shard accounts (one price range per shard, per side) so that orders far apart in
//! price never contend for the same write lock. A taker passes the shards it wants to cross; `all_shards` /
//! `mark_seen` make sure it cannot skip a shard that holds a better price. Makers are paid through the shard's ledger
//! and collect lazily (`claim`), which is why anything that judges a maker's health first needs `complete_view`.

use super::*;

/// Apply maker fills owed to `uk` from every shard in `a` (lazy maker settlement).
pub(crate) fn claim_all(pid: &Address, u: &mut User, uk: &Key, a: &[AccountView]) -> ProgramResult {
    for x in a {
        if disc_of(x) != DISC_SHARD || !x.owned_by(pid) {
            continue;
        }
        let s: &mut Shard = unsafe { &mut *(x.borrow_unchecked().as_ptr() as *mut Shard) };
        claim(u, uk, s)?;
    }
    Ok(())
}

pub(crate) fn claim(u: &mut User, uk: &Key, s: &mut Shard) -> ProgramResult {
    let mut i = 0;
    while i < s.nled as usize {
        if s.ledger[i].owner == *uk {
            let l = s.ledger[i];
            let pi = u.pos.iter().position(|p| p.market == s.market).ok_or(err(E::Ledger))?;
            let p = &mut u.pos[pi];
            let before = long(p.base);
            let ob = p.base;
            p.base += l.base;
            let closed = ob != 0 && p.base == 0;
            // Funding. The next settle charges every lot of the position from the position's own snapshot, as if the claimed
            // lots had been held since then. They were held since their fills: what the settle will over- or under-charge,
            // lots x (index at fill - snapshot), is handed back here. Without it an idle account's resting order collected
            // (or paid) funding for a period before it was filled -- money that no one on the other side paid.
            let fadj = if l.fund == 0 && p.funding_snap == 0 {
                0
            } else {
                l.fund as i128 - l.base as i128 * p.funding_snap as i128
            };
            let got = l.quote as i128 + fadj;
            if p.flags & POS_ISO != 0 {
                p.iso_margin = i64c(p.iso_margin as i128 + got)?;
            } else {
                u.cash = i64c(u.cash as i128 + got)?;
            }
            p.cost = cost_after(ob, p.base, p.cost, -(l.quote as i128))?;
            if s.side == SIDE_BID {
                p.bid_open -= l.open_reduce
            } else {
                p.ask_open -= l.open_reduce
            }
            p.n_orders -= l.n_closed;
            s.oi_long_delta += long(p.base) - before;
            u.maker_volume += l.base.unsigned_abs();
            s.nled -= 1;
            s.ledger[i] = s.ledger[s.nled as usize];
            if closed {
                let mk = s.market;
                for t in u.triggers.iter_mut() {
                    if t.kind != 0 && t.kind & 0xff < TRIGGER_KIND_TWAP && t.market == mk {
                        t.kind = 0;
                    }
                }
            }
            return Ok(()); // at most one entry per owner per shard
        }
        i += 1;
    }
    Ok(())
}

pub(crate) fn ledger_entry<'s>(s: &'s mut Shard, owner: &Key) -> Option<&'s mut Ledger> {
    let n = s.nled as usize;
    match s.ledger[..n].iter().position(|l| l.owner == *owner) {
        Some(i) => Some(&mut s.ledger[i]),
        None if n < MAX_LEDGER => {
            s.ledger[n] = Ledger { owner: *owner, base: 0, quote: 0, open_reduce: 0, fund: 0, n_closed: 0 };
            s.nled += 1;
            Some(&mut s.ledger[n])
        }
        None => None,
    }
}

pub(crate) fn remove_order(s: &mut Shard, i: usize) {
    let n = s.n as usize;
    s.orders.copy_within(i + 1..n, i);
    s.n -= 1;
    s.orders[n - 1] = unsafe { core::mem::zeroed() };
}

/// Every shard of `side` in market `mk` is among `a`, each once. Used where a third party (keeper, liquidator) chooses the
/// accounts for someone else's fill: it must not be able to leave out the better-priced liquidity.
pub(crate) fn all_shards(pid: &Address, a: &[AccountView], mk: &Key, m: &Market, side: u64) -> ProgramResult {
    let mut seen = [0u64; 4];
    let mut k = 0u64;
    for x in a {
        if disc_of(x) != DISC_SHARD || !x.owned_by(pid) {
            continue;
        }
        let s: &Shard = unsafe { &*(x.borrow_unchecked().as_ptr() as *const Shard) };
        if s.market != *mk || s.side != side {
            continue;
        }
        mark_seen(&mut seen, s.shard_no)?;
        k += 1;
    }
    if k != m.n_side[side as usize] {
        return Err(err(E::MissingShards));
    }
    Ok(())
}

/// Complete view of an account before its health decides something final (withdraw, isolated margin out, liquidation, ADL):
/// - every market where it has resting orders: all of that market's shards passed (so `claim_all` on the same accounts has
///   booked every fill; a fill left unclaimed would hide its PnL);
/// - every market where it still has maker margin locked (POS_PIN with a non-zero mirror): the market's registered books passed
///   and its fills there settled. With no margin locked a book gives no budget, so it can hold no unsettled fills (margin only
///   leaves a book through a settle, or a release that requires one), and the market needs no book.
/// `a` holds the markets and the shards / books.
pub(crate) fn complete_view(pid: &Address, u: &User, uk: &Key, a: &[AccountView], only: Option<&Key>) -> ProgramResult {
    for p in u.pos.iter() {
        if p.market == [0; 32] {
            break;
        }
        if only.is_some_and(|k| *k != p.market) {
            continue;
        }
        if p.n_orders == 0 && !(p.flags & (POS_PIN | POS_ISO) == POS_PIN && p.iso_margin != 0) {
            continue;
        }
        let m = find_market(pid, &p.market, a)?;
        if p.n_orders != 0 {
            let mut seen = [0u64; 4];
            let mut k = 0u64;
            for x in a {
                if disc_of(x) != DISC_SHARD || !x.owned_by(pid) {
                    continue;
                }
                let s: &Shard = unsafe { &*(x.borrow_unchecked().as_ptr() as *const Shard) };
                if s.market != p.market {
                    continue;
                }
                mark_seen(&mut seen, s.shard_no)?;
                k += 1;
            }
            if k != m.n_shards {
                return Err(err(E::MissingShards));
            }
        }
        if p.flags & (POS_PIN | POS_ISO) == POS_PIN && p.iso_margin != 0 {
            for (i, fk) in m.fast_book.iter().enumerate() {
                if *fk == [0; 32] || (i == 1 && *fk == m.fast_book[0]) {
                    continue;
                }
                let x = a.iter().find(|x| key(x) == *fk).ok_or(err(E::MissingShards))?;
                let f: &FastBook = load(x, pid, DISC_FAST, FAST_SIZE)?;
                if let Some(e) = f.makers[..(f.n as usize).min(FAST_MAKERS)].iter().find(|e| e.user == *uk) {
                    if e.base != 0 || e.quote != 0 {
                        return Err(err(E::Ledger));
                    } // settle first
                }
            }
        }
    }
    Ok(())
}

/// Before an account is liquidated or deleveraged: none of its collateral may still sit locked as maker margin (it would be
/// missing from the equity being judged). A settle hands it back whenever the account is below maintenance without it.
pub(crate) fn maker_clear(pid: &Address, u: &User, uk: &Key, m: &Market, a: &[AccountView]) -> ProgramResult {
    if u.pos.iter().any(|p| p.market != [0; 32] && p.flags & (POS_PIN | POS_ISO) == POS_PIN && p.iso_margin != 0) {
        return Err(err(E::Ledger));
    }
    for fk in m.fast_book.iter() {
        if *fk == [0; 32] {
            continue;
        }
        let Some(x) = a.iter().find(|x| key(x) == *fk) else { continue };
        let f: &FastBook = load(x, pid, DISC_FAST, FAST_SIZE)?;
        if f.makers[..(f.n as usize).min(FAST_MAKERS)]
            .iter()
            .any(|e| e.user == *uk && (e.margin != 0 || e.base != 0 || e.quote != 0))
        {
            return Err(err(E::Ledger));
        }
    }
    Ok(())
}

/// Each shard may appear once in a set the program walks (same shard twice = two cursors on one order list).
#[inline(always)]
pub(crate) fn mark_seen(seen: &mut [u64; 4], no: u64) -> ProgramResult {
    if no >= 256 {
        return Err(err(E::MissingShards));
    }
    let (w, b) = ((no / 64) as usize, no % 64);
    if seen[w] >> b & 1 == 1 {
        return Err(err(E::BadAccount));
    }
    seen[w] |= 1 << b;
    Ok(())
}

#[inline(always)]
pub(crate) fn kind(o: &Order) -> u64 {
    o.meta & 0xff
}

#[inline(always)]
pub(crate) fn level(o: &Order) -> u64 {
    (o.meta >> 24) & 0xff
}

#[inline(always)]
pub(crate) fn rebate_bps(o: &Order) -> u64 {
    (o.meta >> 8) & 0xff
}

pub(crate) fn peg_price(oracle: u64, off: i64, lo: u64, hi: u64) -> u64 {
    let mut p = oracle as i128 + off as i128;
    if lo != 0 && p < lo as i128 {
        p = lo as i128;
    }
    if hi != 0 && p > hi as i128 {
        p = hi as i128;
    }
    if p > MAX_PX as i128 {
        p = MAX_PX as i128;
    } // AUDIT L-2: no u64 truncation
    if p < TICK as i128 {
        TICK
    } else {
        (p as u64) / TICK * TICK
    }
}

/// Effective price, re-derived lazily at match time (no requote tx for pegs / ladder levels).
#[inline(always)]
pub(crate) fn eff(o: &Order, bid_side: bool, oracle: u64) -> u64 {
    match kind(o) {
        KIND_PEG => peg_price(oracle, o.a1, o.a2, o.a3),
        KIND_SPLINE => {
            let d = o.a1.unsigned_abs() * level(o);
            if bid_side {
                o.price.saturating_sub(d).max(TICK)
            } else {
                o.price + d
            }
        }
        _ => o.price,
    }
}

#[inline(always)]
pub(crate) fn top(o: &Order) -> u64 {
    if kind(o) == KIND_SPLINE {
        o.lvl_left
    } else {
        o.qty
    }
}

pub(crate) fn consume(o: &mut Order, f: u64) {
    o.qty -= f;
    if kind(o) == KIND_SPLINE {
        o.lvl_left = o.lvl_left.saturating_sub(f);
        if o.lvl_left == 0 && o.qty > 0 {
            let l = level(o) + 1;
            o.meta = (o.meta & !(0xff << 24)) | (l << 24);
            o.lvl_left = core::cmp::min(o.a2 + o.a3 * l, o.qty);
        }
    }
}

/// Best order inside one shard: (index, effective price). Pure-limit shards are sorted -> orders[0].
#[inline(always)]
pub(crate) fn shard_best(s: &Shard, oracle: u64) -> Option<(usize, u64)> {
    if s.n == 0 {
        return None;
    }
    let bid = s.side == SIDE_BID;
    if s.seq & SPECIAL_BIT == 0 {
        return Some((0, s.orders[0].price));
    }
    let mut b = (0usize, eff(&s.orders[0], bid, oracle));
    for i in 1..s.n as usize {
        let p = eff(&s.orders[i], bid, oracle);
        if (bid && p > b.1) || (!bid && p < b.1) {
            b = (i, p);
        }
    }
    Some(b)
}

/// Best order on `side` among the passed shards: (shard ptr, index, effective price). Ties: shard order, then FIFO.
pub(crate) fn best(
    a: &[AccountView],
    pid: &Address,
    mk: &Key,
    side: u64,
    oracle: u64,
) -> Option<(*mut Shard, usize, u64)> {
    let bid = side == SIDE_BID;
    let mut b: Option<(*mut Shard, usize, u64)> = None;
    for x in a {
        if disc_of(x) != DISC_SHARD || !x.owned_by(pid) {
            continue;
        }
        let s: &mut Shard = unsafe { &mut *(x.borrow_unchecked().as_ptr() as *mut Shard) };
        if s.market != *mk || s.side != side {
            continue;
        }
        if let Some((i, p)) = shard_best(s, oracle) {
            let better = match b {
                None => true,
                Some((_, _, bp)) => {
                    if bid {
                        p > bp
                    } else {
                        p < bp
                    }
                }
            };
            if better {
                b = Some((s as *mut Shard, i, p));
            }
        }
    }
    b
}

/// shard.seq high bit: set once a peg/spline order is inserted (disables the sorted fast path for that shard).
pub(crate) const SPECIAL_BIT: u64 = 1 << 63;

pub(crate) const RETIRED_BIT: u64 = 1 << 62;

pub(crate) const SEQ_MASK: u64 = (1 << ORDER_SEQ_BITS) - 1;

pub struct Fill {
    pub filled: u64,
    pub notional: i128,
    pub abort: bool,
}

pub(crate) const MAX_TAKE_SHARDS: usize = 16;

#[inline(always)]
pub(crate) fn arrival(c: &(*mut Shard, usize, u64, bool)) -> u64 {
    unsafe { (*c.0).orders[c.1].meta >> 32 }
}

/// Discovery bounds in force for matching: the whole range while the external market is open.
#[inline(always)]
pub(crate) fn bound_range(m: &Market) -> (u64, u64) {
    if m.session == 0 || m.bound_bps == 0 || m.bound_ref == 0 {
        return (0, u64::MAX);
    }
    let d = bps_of(m.bound_ref, m.bound_bps);
    (m.bound_ref.saturating_sub(d), m.bound_ref.saturating_add(d))
}

/// Re-anchor while closed (price within thresh of the bound moves the reference to the bound, until bound_max), then clamp mark.
pub(crate) fn apply_bounds(m: &mut Market, px: u64) {
    if m.bound_bps == 0 {
        return;
    }
    if m.session == 0 || m.bound_ref == 0 {
        m.bound_ref = px;
        m.bound_up = 0;
        m.bound_dn = 0;
        return;
    }
    let d = bps_of(m.bound_ref, m.bound_bps);
    let t = bps_of(d, m.bound_thresh_bps);
    if px >= m.bound_ref + t && m.bound_up < m.bound_max {
        m.bound_ref += d;
        m.bound_up += 1;
    } else if px + t <= m.bound_ref && m.bound_dn < m.bound_max {
        m.bound_ref -= d;
        m.bound_dn += 1;
    }
    let (lo, hi) = bound_range(m);
    m.mark_px = m.mark_px.clamp(lo, hi);
}

/// Taker matches against the best opposite order across the passed shards (effective prices).
/// Per-shard best is computed once and recomputed only for the shard just modified (no full rescan per fill).
pub(crate) fn take(
    pid: &Address,
    a: &[AccountView],
    mk: &Key,
    m: &Market,
    u: &mut User,
    uk: &Key,
    pi: usize,
    taker_bid: bool,
    limit: u64,
    mut qty: u64,
    slot: u64,
    taker_bps: u64,
    stp: u64,
) -> Result<Fill, ProgramError> {
    let opp = if taker_bid { SIDE_ASK } else { SIDE_BID };
    let mut f = Fill { filled: 0, notional: 0, abort: false };
    let mut first: *mut Shard = core::ptr::null_mut();
    let base0 = long(u.pos[pi].base);
    let oracle = m.oracle_px;
    let mut cs: [(*mut Shard, usize, u64, bool); MAX_TAKE_SHARDS] =
        [(core::ptr::null_mut(), 0, 0, false); MAX_TAKE_SHARDS];
    let mut ns = 0;
    let mut ro = [false; MAX_TAKE_SHARDS];
    let mut seen = [0u64; 4];
    for x in a {
        if disc_of(x) != DISC_SHARD || !x.owned_by(pid) {
            continue;
        }
        let s: &mut Shard = unsafe { &mut *(x.borrow_unchecked().as_ptr() as *mut Shard) };
        if s.market != *mk || s.side != opp {
            continue;
        }
        mark_seen(&mut seen, s.shard_no)?;
        if ns == MAX_TAKE_SHARDS {
            return Err(err(E::BadData));
        }
        let b = shard_best(s, oracle);
        ro[ns] = !x.is_writable();
        cs[ns] = (s as *mut Shard, b.map_or(0, |v| v.0), b.map_or(0, |v| v.1), b.is_some());
        ns += 1;
    }
    // P-3: every opposite shard must be visible; shards the order won't touch may be read-only, so takers only serialize where they trade
    if m.strict_take != 0 && ns as u64 != m.n_side[opp as usize] {
        return Err(err(E::MissingShards));
    }
    let (blo, bhi) = bound_range(m);
    let mut ev = EvBuf::new(uk, taker_bid);
    while qty > 0 {
        let mut k = usize::MAX;
        for j in 0..ns {
            if cs[j].3
                && (k == usize::MAX
                    || (taker_bid && cs[j].2 < cs[k].2)
                    || (!taker_bid && cs[j].2 > cs[k].2)
                    || (cs[j].2 == cs[k].2 && arrival(&cs[j]) < arrival(&cs[k])))
            {
                k = j;
            }
        }
        if k == usize::MAX {
            break;
        }
        let (sp, i, px, _) = cs[k];
        if (taker_bid && px > limit) || (!taker_bid && px < limit) {
            break;
        }
        if px < blo || px > bhi {
            break;
        } // closed market: nothing trades outside the discovery bounds
        if ro[k] {
            return Err(err(E::BadAccount));
        }
        let s = unsafe { &mut *sp };
        if i >= s.n as usize {
            return Err(err(E::BadAccount));
        }
        if first.is_null() {
            first = sp;
        }
        let mut o = s.orders[i];
        if eff(&o, s.side == SIDE_BID, oracle) != px {
            return Err(err(E::BadAccount));
        }
        let expired = o.expiry_slot != 0 && slot > o.expiry_slot;
        let off_band = o.band_bps != 0 && off_band(px, oracle, o.band_bps);
        let mut stop = false;
        if o.owner == *uk {
            let live = !expired && !off_band;
            if stp == STP_CANCEL_NEWEST && live {
                f.abort = true;
                break;
            }
            let d = if stp > STP_CANCEL_NEWEST && live { core::cmp::min(qty, top(&o)) } else { o.qty };
            if stp > STP_CANCEL_NEWEST && live {
                qty -= d;
            }
            if s.side == SIDE_BID {
                u.pos[pi].bid_open -= d
            } else {
                u.pos[pi].ask_open -= d
            }
            consume(&mut o, d);
            if o.qty == 0 {
                remove_order(s, i);
                u.pos[pi].n_orders -= 1;
            } else {
                s.orders[i] = o
            }
        } else if expired || off_band {
            match ledger_entry(s, &o.owner) {
                Some(l) => {
                    l.open_reduce += o.qty;
                    l.n_closed += 1;
                    remove_order(s, i);
                }
                None => stop = true,
            }
        } else {
            let q = core::cmp::min(qty, top(&o));
            let notional = match px.checked_mul(q) {
                Some(n) => n as i128,
                None => px as i128 * q as i128,
            }; // OPT: native mul
            let rebate = bps_dn(notional, rebate_bps(&o));
            let fee = bps_up(notional, taker_bps);
            match ledger_entry(s, &o.owner) {
                None => stop = true, // ledger full: stop (crank_claim drains it)
                Some(l) => {
                    if taker_bid {
                        l.base -= q as i64;
                        l.quote = i64c(l.quote as i128 + notional + rebate)?;
                    } else {
                        l.base += q as i64;
                        l.quote = i64c(l.quote as i128 - notional + rebate)?;
                    }
                    // these lots carry funding from this index on, however long they wait for the claim (see claim)
                    l.fund = fund_acc(l.fund, if taker_bid { -(q as i64) } else { q as i64 }, m.funding_index)?;
                    l.open_reduce += q;
                    consume(&mut o, q);
                    if o.qty == 0 {
                        l.n_closed += 1;
                    }
                    s.fee_accum = i64c(s.fee_accum as i128 + fee - rebate)?;
                    credit(u, pi, -fee + if taker_bid { -notional } else { notional })?;
                    let ob = u.pos[pi].base;
                    u.pos[pi].base += if taker_bid { q as i64 } else { -(q as i64) };
                    u.pos[pi].cost =
                        cost_after(ob, u.pos[pi].base, u.pos[pi].cost, if taker_bid { notional } else { -notional })?;
                    f.filled += q;
                    f.notional += notional;
                    qty -= q;
                    ev.push(&o.owner, px, q, o.id);
                    if o.qty == 0 {
                        remove_order(s, i)
                    } else {
                        s.orders[i] = o
                    }
                }
            }
        }
        if stop {
            break;
        }
        let b = shard_best(s, oracle);
        cs[k] = (sp, b.map_or(0, |v| v.0), b.map_or(0, |v| v.1), b.is_some());
    }
    ev.flush();
    if !first.is_null() {
        unsafe {
            (*first).oi_long_delta += long(u.pos[pi].base) - base0;
        }
    }
    Ok(f)
}

/// |px - oracle| / oracle > band_bps, in native u64 when it fits (OPT; same result as the u128 form).
#[inline(always)]
pub(crate) fn off_band(px: u64, oracle: u64, band: u64) -> bool {
    let d = px.abs_diff(oracle);
    exceeds(d, oracle, band)
}

/// price x qty < MIN_ORDER_NOTIONAL, without a wide multiply for ordinary sizes.
#[inline(always)]
pub(crate) fn below_min_notional(price: u64, qty: u64) -> bool {
    if price < (1 << 32) && qty < (1 << 32) {
        price.wrapping_mul(qty) < MIN_ORDER_NOTIONAL
    } else {
        (price as u128) * (qty as u128) < MIN_ORDER_NOTIONAL as u128
    }
}

/// FOK pre-check (AUDIT M-3): can `need` be filled at `limit` from other owners' live liquidity in the passed shards?
/// Best price on `side` among the passed shards counting only orders that can still trade (not expired, inside their own band).
pub(crate) fn best_live(a: &[AccountView], pid: &Address, mk: &Key, side: u64, oracle: u64, slot: u64) -> Option<u64> {
    let bid = side == SIDE_BID;
    let (mut b, mut any) = (0u64, false);
    for x in a {
        if disc_of(x) != DISC_SHARD || !x.owned_by(pid) {
            continue;
        }
        let s: &Shard = unsafe { &*(x.borrow_unchecked().as_ptr() as *const Shard) };
        if s.market != *mk || s.side != side || s.n == 0 {
            continue;
        }
        let sorted = s.seq & SPECIAL_BIT == 0;
        let o0 = &s.orders[0];
        // the usual case: a pure-limit shard whose top order has no expiry and no band -- its price, nothing else to look at
        let p = if sorted && o0.expiry_slot | o0.band_bps == 0 {
            o0.price
        } else {
            let mut f: Option<u64> = None;
            for o in &s.orders[..(s.n as usize).min(MAX_ORDERS)] {
                let p = if sorted { o.price } else { eff(o, bid, oracle) };
                if (o.expiry_slot != 0 && slot > o.expiry_slot) || (o.band_bps != 0 && off_band(p, oracle, o.band_bps))
                {
                    continue;
                }
                if f.is_none_or(|bp| if bid { p > bp } else { p < bp }) {
                    f = Some(p);
                }
                if sorted {
                    break;
                } // best-first: the first live order is the shard's best
            }
            match f {
                Some(p) => p,
                None => continue,
            }
        };
        if !any || (bid && p > b) || (!bid && p < b) {
            b = p;
            any = true;
        }
    }
    if any {
        Some(b)
    } else {
        None
    }
}

pub(crate) fn fok_avail(
    a: &[AccountView],
    pid: &Address,
    mk: &Key,
    opp: u64,
    oracle: u64,
    limit: u64,
    slot: u64,
    uk: &Key,
    need: u64,
) -> bool {
    let bid_side = opp == SIDE_BID;
    let ok_px = |p: u64| if bid_side { p >= limit } else { p <= limit };
    let mut have = 0u64;
    let mut seen = [0u64; 4];
    for x in a {
        if disc_of(x) != DISC_SHARD || !x.owned_by(pid) {
            continue;
        }
        let s: &Shard = unsafe { &*(x.borrow_unchecked().as_ptr() as *const Shard) };
        if s.market != *mk || s.side != opp {
            continue;
        }
        if mark_seen(&mut seen, s.shard_no).is_err() {
            return false;
        }
        for o in &s.orders[..s.n as usize] {
            if o.owner == *uk || (o.expiry_slot != 0 && slot > o.expiry_slot) {
                continue;
            }
            let p0 = eff(o, bid_side, oracle);
            if o.band_bps != 0 && off_band(p0, oracle, o.band_bps) {
                continue;
            }
            if kind(o) == KIND_SPLINE {
                let (mut lv, mut left, mut avail) = (level(o), o.qty, o.lvl_left);
                while left > 0 {
                    let d = o.a1.unsigned_abs() * lv;
                    let p = if bid_side { o.price.saturating_sub(d).max(TICK) } else { o.price + d };
                    if !ok_px(p) {
                        break;
                    }
                    have += avail;
                    left -= avail;
                    lv += 1;
                    avail = core::cmp::min(o.a2 + o.a3 * lv, left);
                }
            } else if ok_px(p0) {
                have += o.qty;
            }
            if have >= need {
                return true;
            }
        }
    }
    false
}

pub(crate) fn insert(
    a: &[AccountView],
    pid: &Address,
    mk: &Key,
    uk: &Key,
    bid: bool,
    mut o: Order,
    slot: u64,
) -> Result<u64, ProgramError> {
    let side = if bid { SIDE_BID } else { SIDE_ASK };
    let price = o.price;
    let mut full = false;
    for x in a {
        if disc_of(x) != DISC_SHARD || !x.owned_by(pid) {
            continue;
        }
        let s: &mut Shard = unsafe { &mut *(x.borrow_unchecked().as_ptr() as *mut Shard) };
        if s.market != *mk || s.side != side || price < s.lo || price >= s.lo + s.width {
            continue;
        }
        let n = s.n as usize;
        // P-1: a shard that overflowed stays closed until it empties, so later orders can't jump ahead of the overflow shard
        if s.seq & RETIRED_BIT != 0 {
            if n == 0 && s.seq & SEQ_MASK < SEQ_MASK {
                s.seq &= !RETIRED_BIT;
            } else {
                full = true;
                continue;
            }
        }
        if n >= MAX_ORDERS {
            s.seq |= RETIRED_BIT;
            full = true;
            continue;
        }
        if s.seq & SEQ_MASK >= SEQ_MASK {
            s.seq |= RETIRED_BIT;
            full = true;
            continue;
        } // AUDIT L-3: never wrap; retire the shard // AUDIT H-3: overflow into another shard for the same bucket
        let mut i = n;
        // best-first by anchor price: bids desc, asks asc; FIFO within price
        for j in 0..n {
            let p = s.orders[j].price;
            if (bid && p < price) || (!bid && p > price) {
                i = j;
                break;
            }
        }
        s.orders.copy_within(i..n, i + 1);
        s.seq += 1;
        if kind(&o) != KIND_LIMIT {
            s.seq |= SPECIAL_BIT;
        }
        o.id = (s.shard_no << ORDER_SEQ_BITS) | (s.seq & SEQ_MASK); // AUDIT L-3: unique per market, no wrap
        o.meta = (o.meta & 0xffff_ffff) | ((slot & 0xffff_ffff) << 32); // P-2: arrival slot, compared across shards at equal price
        o.owner = *uk;
        s.orders[i] = o;
        s.n += 1;
        return Ok(o.id);
    }
    Err(err(if full { E::ShardFull } else { E::NoShard }))
}

/// side & CANCEL_BY_CID_BIT: `id` is a client order id, searched across all passed shards of that side.
pub(crate) fn cancel(
    a: &[AccountView],
    pid: &Address,
    mk: &Key,
    uk: &Key,
    u: &mut User,
    pi: usize,
    id: u64,
    side: u8,
    price: u64,
) -> ProgramResult {
    if side as u64 & spec::CANCEL_ALL_BIT != 0 {
        cancel_all_of(a, pid, mk, uk, u, pi);
        return Ok(());
    }
    let by_cid = side as u64 & CANCEL_BY_CID_BIT != 0;
    let side = (side as u64) & 1;
    for x in a {
        if disc_of(x) != DISC_SHARD || !x.owned_by(pid) {
            continue;
        }
        let s: &mut Shard = unsafe { &mut *(x.borrow_unchecked().as_ptr() as *mut Shard) };
        if s.market != *mk || s.side != side {
            continue;
        }
        if !by_cid && s.shard_no != id >> ORDER_SEQ_BITS {
            continue;
        }
        let _ = price;
        for i in 0..s.n as usize {
            let o = &s.orders[i];
            if o.owner == *uk && ((by_cid && o.cid == id && id != 0) || (!by_cid && o.id == id)) {
                let q = o.qty;
                if side == SIDE_BID {
                    u.pos[pi].bid_open -= q
                } else {
                    u.pos[pi].ask_open -= q
                }
                u.pos[pi].n_orders -= 1;
                remove_order(s, i);
                return Ok(());
            }
        }
        if !by_cid {
            return Ok(());
        } // already filled/removed: idempotent cancel (MM friendly)
    }
    if by_cid {
        Ok(())
    } else {
        Err(err(E::NoShard))
    }
}

/// Remove every resting order of `uk` in the passed shards (liquidation). Returns count.
pub(crate) fn cancel_all_of(a: &[AccountView], pid: &Address, mk: &Key, uk: &Key, u: &mut User, pi: usize) -> u64 {
    let mut c = 0;
    for x in a {
        if disc_of(x) != DISC_SHARD || !x.owned_by(pid) {
            continue;
        }
        let s: &mut Shard = unsafe { &mut *(x.borrow_unchecked().as_ptr() as *mut Shard) };
        if s.market != *mk {
            continue;
        }
        let mut i = 0;
        while i < s.n as usize {
            if s.orders[i].owner == *uk {
                let q = s.orders[i].qty;
                if s.side == SIDE_BID {
                    u.pos[pi].bid_open -= q
                } else {
                    u.pos[pi].ask_open -= q
                }
                u.pos[pi].n_orders -= 1;
                remove_order(s, i);
                c += 1;
            } else {
                i += 1;
            }
        }
    }
    c
}

/// place (tag 1: tag + place_item) or batch (tag 3: spec batch layout).
/// accs: authority(s), user(w), market(r), shards(w)..., [other markets(r) for cross-margin]
pub(crate) fn batch(pid: &Address, a: &mut [AccountView], d: &[u8], single: bool) -> ProgramResult {
    if a.len() < 4 {
        return Err(err(E::BadData));
    }
    let u: &mut User = load(&a[1], pid, DISC_USER, USER_SIZE)?;
    let m: &Market = load(&a[2], pid, DISC_MARKET, MARKET_SIZE)?;
    let (uk, mk) = (key(&a[1]), key(&a[2]));
    let clk = clock()?;
    let slot = clk.slot;
    let nw: Now = (slot, clk.unix_timestamp);
    // owner, or an approved session-key delegate (place/cancel only, per-order qty cap)
    let dmax = if sg(pid, &a[0]) && u.authority == key(&a[0]) {
        u64::MAX
    } else if sg(pid, &a[0]) {
        delegate_max_qty(pid, &a[3..], &key(&a[0]), &uk, slot).ok_or(err(E::BadAccount))?
    } else {
        return Err(err(E::BadAccount));
    };
    home(u, m)?;
    let rest = &a[3..];
    let (nc, np, mut o) = if single {
        (0usize, 1usize, 1usize)
    } else {
        if d.len() < BATCH_HEADER_SIZE as usize {
            return Err(err(E::BadData));
        }
        (
            d[BATCH_HEADER_N_CANCEL_OFF as usize] as usize,
            d[BATCH_HEADER_N_PLACE_OFF as usize] as usize,
            BATCH_HEADER_SIZE as usize,
        )
    };
    if d.len() != o + nc * CANCEL_ITEM_SIZE as usize + np * PLACE_ITEM_SIZE as usize {
        return Err(err(E::BadData));
    }
    claim_all(pid, u, &uk, rest)?;
    let pi = pos_idx(u, &mk, m.funding_index)?;
    settle_funding_all(pid, u, &a[2..])?;
    // a resting bid above the mark (ask below it) loses (price - mark) x qty the moment it fills: that loss is margined now,
    // so an order alone can't make the account bankrupt when it fills (initial margin is only charged at the mark)
    let mut adverse: i128 = 0;
    let mpx = mark(m);
    // 1) cancels first (cancel priority within the instruction)
    // FLAG_IF_CANCELLED on any place: note which of the first 64 cancel items removed a live order, read off the account's own
    // order count. A batch that doesn't use the flag pays one pass over its flag bytes and nothing per cancel.
    let pb = o + nc * CANCEL_ITEM_SIZE as usize + PLACE_ITEM_FLAGS_OFF as usize;
    let mut fls = 0u8;
    if nc != 0 {
        for j in 0..np {
            fls |= unsafe { *d.get_unchecked(pb + j * PLACE_ITEM_SIZE as usize) };
        }
    } // in bounds: the length was checked above
    let mut hit = 0u64;
    if fls as u64 & spec::FLAG_IF_CANCELLED == 0 {
        for _ in 0..nc {
            cancel(rest, pid, &mk, &uk, u, pi, rd64(d, o)?, d[o + 8], rd64(d, o + 9)?)?;
            o += CANCEL_ITEM_SIZE as usize;
        }
    } else {
        let (mut bit, mut nprev) = (1u64, u.pos[pi].n_orders);
        for _ in 0..nc {
            cancel(rest, pid, &mk, &uk, u, pi, rd64(d, o)?, d[o + 8], rd64(d, o + 9)?)?;
            let n = u.pos[pi].n_orders;
            if n != nprev {
                hit |= bit;
                nprev = n;
            }
            bit = bit.wrapping_shl(1);
            o += CANCEL_ITEM_SIZE as usize;
        }
    }
    let is_stale = stale(m, nw);
    let mut risk_up = false;
    let mut reduced = false;
    let tier = (u.tier as usize).min(TIERS.len() - 1);
    let (taker_bps, maker_rebate) = TIERS[tier];
    for ip in 0..np {
        let it = &d[o..o + PLACE_ITEM_SIZE as usize];
        o += PLACE_ITEM_SIZE as usize;
        let bid = it[0] as u64 == SIDE_BID;
        let fl = it[PLACE_ITEM_FLAGS_OFF as usize] as u64;
        let k = it[PLACE_ITEM_KIND_OFF as usize] as u64;
        let mut price = rd64(it, PLACE_ITEM_PRICE_OFF as usize)?;
        let mut qty = rd64(it, PLACE_ITEM_QTY_OFF as usize)?;
        if qty > dmax {
            return Err(err(E::Cap));
        }
        let expiry = rd64(it, PLACE_ITEM_EXPIRY_SLOT_OFF as usize)?;
        let bo = PLACE_ITEM_ORACLE_BAND_BPS_OFF as usize;
        let band = u16::from_le_bytes([it[bo], it[bo + 1]]) as u64;
        let cid = rd64(it, PLACE_ITEM_CID_OFF as usize)?;
        let (a1, a2, a3) = (
            rd64(it, PLACE_ITEM_A1_OFF as usize)? as i64,
            rd64(it, PLACE_ITEM_A2_OFF as usize)?,
            rd64(it, PLACE_ITEM_A3_OFF as usize)?,
        );
        let stp = (fl & FLAG_STP_MASK) >> FLAG_STP_SHIFT;
        let reduce = fl & FLAG_REDUCE_ONLY != 0;
        let fok = fl & FLAG_FOK != 0;
        let mut ioc = fl & FLAG_IOC != 0 || fok;
        let mut post = fl & FLAG_POST_ONLY != 0;
        if k > KIND_SPLINE || qty == 0 || (k != KIND_PEG && (price == 0 || !price.is_multiple_of(TICK))) {
            return Err(err(E::BadData));
        }
        if k != KIND_LIMIT {
            if reduce || ioc {
                return Err(err(E::BadData));
            }
            post = true;
        }
        // replace: only where the paired cancel pulled a live order (one that filled or expired meanwhile is not quoted again)
        if fl & spec::FLAG_IF_CANCELLED != 0 && (ip >= 64 || hit >> ip & 1 == 0) {
            continue;
        }
        if k == KIND_SPLINE {
            if a2 == 0 || a2 > MAX_SPLINE_LEVELS || a1 <= 0 || price > MAX_PX || qty > MAX_PX || a3 > MAX_PX {
                return Err(err(E::BadData));
            }
            // AUDIT H-1: every level price must be representable and >= 1 tick (no overflow at match time)
            let span = (a1 as u64).checked_mul(a2 - 1).ok_or(err(E::BadData))?;
            if (bid && span >= price) || (!bid && price.checked_add(span).is_none_or(|p| p > MAX_PX)) {
                return Err(err(E::BadData));
            }
        }
        if k == KIND_PEG && (a1.unsigned_abs() > MAX_PX || a2 > MAX_PX || a3 > MAX_PX) {
            return Err(err(E::BadData));
        }
        if k == KIND_LIMIT && price > MAX_PX {
            return Err(err(E::BadData));
        }
        if expiry != 0 && expiry <= slot {
            continue;
        } // already expired: skip, don't fail the batch
        if reduce {
            if post {
                return Err(err(E::BadData));
            } // reduce-only never rests, post-only never takes: nothing could happen
            let b = u.pos[pi].base;
            let cap = if bid && b < 0 {
                (-b) as u64
            } else if !bid && b > 0 {
                b as u64
            } else {
                0
            };
            qty = qty.min(cap);
            ioc = true;
            if qty == 0 {
                continue;
            }
        } else {
            // in a batch a paused market or a stale oracle skips the place (like the ALO / FOK / full-bucket skips): the
            // cancels in the same instruction must land exactly then -- a maker pulling quotes can't be held by its own re-quote
            if m.paused != 0 {
                if single {
                    return Err(err(E::Paused));
                }
                continue;
            }
            if is_stale {
                if single {
                    return Err(err(E::Stale));
                }
                continue;
            }
            risk_up = true;
        }
        if k == KIND_PEG {
            price = peg_price(m.oracle_px, a1, a2, a3);
        }
        // AUDIT H-3: minimum notional for anything that can rest (anti book-stuffing)
        if !ioc && below_min_notional(price, qty) {
            return Err(err(E::BadData));
        }
        let opp = if bid { SIDE_ASK } else { SIDE_BID };
        let mut bo = best(rest, pid, &mk, opp, m.oracle_px).map(|b| b.2);
        let mut crosses = bo.is_some_and(|p| if bid { p <= price } else { p >= price });
        // post-only is judged against the best order that could still trade: an expired or off-band order at the top is not a
        // cross. Looked at only when the top does cross, so a quote that doesn't pays nothing for it.
        if post && crosses {
            bo = best_live(rest, pid, &mk, opp, m.oracle_px, slot);
            crosses = bo.is_some_and(|p| if bid { p <= price } else { p >= price });
        }
        if post && crosses {
            if fl & FLAG_ALO_SLIDE == 0 || k == KIND_PEG {
                continue;
            } // ALO: skip, don't fail the batch
            let p = bo.unwrap();
            price = if bid { p.saturating_sub(TICK) } else { p + TICK };
            if price == 0 {
                continue;
            }
        }
        if fok && !fok_avail(rest, pid, &mk, if bid { SIDE_ASK } else { SIDE_BID }, m.oracle_px, price, slot, &uk, qty)
        {
            continue;
        } // AUDIT M-3: skip only this order
        let (filled, abort) = if crosses && !post {
            let f = take(pid, rest, &mk, m, u, &uk, pi, bid, price, qty, slot, taker_bps, stp)?;
            (f.filled, f.abort)
        } else {
            (0, false)
        };
        if fok && filled < qty {
            return Err(err(E::FokUnfilled));
        }
        if reduce && filled != 0 {
            reduced = true;
        }
        let left = qty - filled;
        if left > 0 && !ioc && !abort {
            let (total, lvl, a2s) = if k == KIND_SPLINE {
                (a2 * left + a3 * (a2 * (a2 - 1) / 2), left, left) // a2 = n levels in the wire item; stored a2 = base qty
            } else {
                (left, 0, a2)
            };
            if total > dmax {
                return Err(err(E::Cap));
            } // a session key's cap is on what rests, spline levels included
            let n_lv = if k == KIND_SPLINE { a2 } else { 0 };
            let ord = Order {
                id: 0,
                owner: [0; 32],
                price,
                qty: total,
                expiry_slot: expiry,
                band_bps: band,
                cid,
                meta: k | (maker_rebate << 8) | (n_lv << 16),
                a1,
                a2: a2s,
                a3,
                lvl_left: lvl,
            };
            if u.pos[pi].n_orders >= MAX_OPEN_ORDERS_PER_MARKET {
                return Err(err(E::Cap));
            } // AUDIT M-2
            let p0 = if k == KIND_PEG { peg_price(m.oracle_px, a1, a2, a3) } else { price };
            if bid && p0 > mpx {
                adverse += mulf(total as i128, p0 - mpx);
            }
            if !bid && p0 < mpx {
                adverse += mulf(total as i128, mpx - p0);
            } // a spline's first level is its anchor
              // LOAD-1: in a batch, a full bucket (64 orders) skips only this item (like ALO/FOK skips) instead of
              // failing the whole replace -- otherwise one crowded price bucket wipes every other quote and cancel in the tx.
            match insert(rest, pid, &mk, &uk, bid, ord, slot) {
                Ok(_) => {}
                Err(ProgramError::Custom(c)) if !single && c == E::ShardFull as u32 => continue,
                Err(e) => return Err(e),
            }
            if bid {
                u.pos[pi].bid_open += total
            } else {
                u.pos[pi].ask_open += total
            }
            u.pos[pi].n_orders += 1;
        }
    }
    if risk_up {
        if m.max_oi != 0 && m.oi_long > m.max_oi {
            return Err(err(E::Cap));
        }
        let p = &u.pos[pi]; // per-account position limit (gross incl. resting orders)
        if m.max_position != 0
            && core::cmp::max((p.base as i128 + p.bid_open as i128).abs(), (p.base as i128 - p.ask_open as i128).abs())
                > m.max_position as i128
        {
            return Err(err(E::Cap));
        }
        let (eq, im, _) = health_im(pid, u, &a[2..], nw, true)?;
        let iso = u.pos[pi].flags & POS_ISO != 0;
        if eq - if iso { 0 } else { adverse } < im {
            return Err(err(E::Margin));
        }
        if iso {
            let (e2, i2, _) = iso_health(pid, u, pi, &a[2..], nw, true)?;
            if e2 - adverse < i2 {
                return Err(err(E::Margin));
            }
        }
    } else if reduced {
        // reduce-only never leaves bad debt, whatever price it was sent at (same rule as take_fast and TP/SL): without this a
        // reduce-only sell far under the mark into a chosen bid hands the account's whole equity -- and more -- to that bid
        let (eq, _, _) = phealth(pid, u, pi, &a[2..], nw, false)?;
        if eq < 0 {
            return Err(err(E::Margin));
        }
    }
    gc_pos(u);
    Ok(())
}

/// Permissionless: push owed maker fills from one shard ledger into the passed user accounts (drains a full ledger).
/// accs: shard(w), users(w)...
pub(crate) fn crank_claim(pid: &Address, a: &mut [AccountView]) -> ProgramResult {
    if a.len() < 2 {
        return Err(err(E::BadData));
    }
    let s: &mut Shard = load(&a[0], pid, DISC_SHARD, SHARD_SIZE)?;
    for x in a[1..].iter() {
        let u: &mut User = load(x, pid, DISC_USER, USER_SIZE)?;
        claim(u, &key(x), s)?;
    }
    Ok(())
}
