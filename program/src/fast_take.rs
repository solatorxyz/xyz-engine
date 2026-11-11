//! Taking on the fast path: one taker against the ladders of every maker joined to the book, best price first.
//!
//! `fast_match` is the matching loop; `slab_first` decides which of a maker's quotes may fill right now (pegged price,
//! freshness rule, remaining size, remaining margin). The taker's margin is checked once, on the worst price the
//! order could reach (`worst_ok`), before any maker state is touched.

use super::*;

/// Taker order against maker slabs and, when opposite-side shards are passed, resting book orders. Better price wins.
/// accs: authority(s), user(w), market(r), fast(w), slabs(r) x ns, [shards], [other markets]
/// data: tag, side u8, flags u8, ns u8, limit u64, qty u64
pub(crate) fn take_fast(pid: &Address, a: &mut [AccountView], d: &[u8]) -> ProgramResult {
    cp!(1);
    if a.len() < 4 || d.len() != 20 {
        return Err(err(E::BadData));
    }
    let (bid, fl, ns) = (d[1] as u64 == SIDE_BID, d[2] as u64, d[3] as usize);
    let (limit, mut left) = (rd64(d, 4)?, rd64(d, 12)?);
    if ns > FAST_MAKERS || a.len() < 4 + ns || left == 0 || left > u32::MAX as u64 {
        return Err(err(E::BadData));
    }
    let u: &mut User = load(&a[1], pid, DISC_USER, USER_SIZE)?;
    let m: &Market = load(&a[2], pid, DISC_MARKET, MARKET_SIZE)?;
    let f: &mut FastBook = load(&a[3], pid, DISC_FAST, FAST_SIZE)?;
    let (uk, mk) = (key(&a[1]), key(&a[2]));
    if f.market != mk {
        return Err(err(E::BadAccount));
    }
    cp!(2);
    let clk = clock()?;
    let slot = clk.slot;
    let nw: Now = (slot, clk.unix_timestamp);
    let rest = &a[4 + ns..];
    fresh_px(f, &mk, m.oracle_px, slot, rest)?;
    let dmax = if sg(pid, &a[0]) && u.authority == key(&a[0]) {
        u64::MAX
    } else if sg(pid, &a[0]) {
        delegate_max_qty(pid, rest, &key(&a[0]), &uk, slot).ok_or(err(E::BadAccount))?
    } else {
        return Err(err(E::BadAccount));
    };
    if left > dmax {
        return Err(err(E::Cap));
    }
    cp!(3);
    home(u, m)?;
    cp!(4);
    claim_all(pid, u, &uk, rest)?;
    cp!(5);
    let pi = pos_idx(u, &mk, m.funding_index)?;
    cp!(6);
    settle_funding_all(pid, u, &a[2..])?;
    cp!(7);
    let reduce = fl & FLAG_REDUCE_ONLY != 0;
    if stale(m, nw) {
        return Err(err(E::Stale));
    } // reduce-only too: maker quotes (pegged ones above all) price off the oracle
    if reduce {
        let b = u.pos[pi].base;
        let cap = if bid && b < 0 {
            (-b) as u64
        } else if !bid && b > 0 {
            b as u64
        } else {
            0
        };
        left = left.min(cap);
        if left == 0 {
            gc_pos(u);
            return Ok(());
        }
    } else if m.paused != 0 {
        return Err(err(E::Paused));
    }
    let px0 = mark(m);
    let band = quote_band(px0);
    let (blo, bhi) = bound_range(m);
    let (lo, hi) = (px0.saturating_sub(band).max(blo), (px0 + band).min(bhi));
    let opp = if bid { 1usize } else { 0usize };
    if (f.side != 0 && f.side != opp as u64 + 1) || !book_ok(m, &key(&a[3]), opp) {
        return Err(err(E::BadAccount));
    } // the market's book for this side
    let base_ix = (f.idx as usize) << 4 | opp << 3;
    let signer = key(&a[0]);
    let (sl, cnt, _) = collect_slabs(pid, f, &a[4..4 + ns], &uk, Some(&signer), false, false)?; // no self-trade
    let tier = (u.tier as usize).min(TIERS.len() - 1);
    let taker_bps = TIERS[tier].0;

    cp!(8);
    let mut ev = FastEv::new(&uk, bid);
    fast_match(
        pid,
        f,
        &sl[..cnt],
        base_ix,
        opp,
        bid,
        lo,
        hi,
        rest,
        &mk,
        m,
        u,
        &uk,
        pi,
        limit,
        left,
        slot,
        taker_bps,
        &mut ev,
        0,
        0,
    )?;
    ev.flush();
    cp!(9);
    if reduce {
        // reduce-only never leaves bad debt, whatever it filled against (maker quotes or resting orders from passed shards)
        let (eq, _, _) = phealth(pid, u, pi, &a[2..], nw, false)?;
        if eq < 0 {
            return Err(err(E::Margin));
        }
    } else {
        if m.max_oi != 0 && m.oi_long as i128 + f.oi_long_delta as i128 > m.max_oi as i128 {
            return Err(err(E::Cap));
        }
        let p = &u.pos[pi];
        if m.max_position != 0
            && core::cmp::max((p.base as i128 + p.bid_open as i128).abs(), (p.base as i128 - p.ask_open as i128).abs())
                > m.max_position as i128
        {
            return Err(err(E::Cap));
        }
        cp!(10);
        let (eq, im, _) = health_im(pid, u, &a[2..], nw, true)?;
        if eq < im {
            return Err(err(E::Margin));
        }
        cp!(11);
        if p.flags & POS_ISO != 0 {
            let (e2, i2, _) = iso_health(pid, u, pi, &a[2..], nw, true)?;
            if e2 < i2 {
                return Err(err(E::Margin));
            }
        }
    }
    gc_pos(u);
    cp!(12);
    Ok(())
}

/// Many takers against one fast book in one instruction. The market, the book, the maker slabs and the clock are read once,
/// and a relayer holding the takers' session delegates signs once (a taker can also sign for itself), so a batch of N takers
/// pays one signature and one book write lock. An order that can't go (no authority, over its delegate cap, other vault, stale
/// or paused market, isolated position, OI cap, margin for its worst case) is skipped and the others still fill. Margin is
/// checked before matching for the most the order can do (worst_ok), which is stricter than take_fast's check after it; no
/// resting-book orders take part (as take_fast without shards). Markets with strict_take are refused.
/// accs: market(r), other markets(r) x nm, fast(w), slabs(r) x ns, n x [user(w), signer(s), delegate record(r) or any account
/// when the signer is the owner], rest (resting-book shards whose maker fills a taker claims first)
/// data: tag, nm u8, ns u8, n u8, n x (side u8, flags u8, limit u64, qty u64)
pub(crate) fn take_fast_batch(pid: &Address, a: &mut [AccountView], d: &[u8]) -> ProgramResult {
    if d.len() < 4 {
        return Err(err(E::BadData));
    }
    let (nm, ns, n) = (d[1] as usize, d[2] as usize, d[3] as usize);
    let (fi, ti) = (1 + nm, 2 + nm + ns);
    if d.len() != 4 + n * 18 || ns > FAST_MAKERS || n > 255 || a.len() < ti + 3 * n {
        return Err(err(E::BadData));
    }
    let m: &Market = load(&a[0], pid, DISC_MARKET, MARKET_SIZE)?;
    let f: &mut FastBook = load(&a[fi], pid, DISC_FAST, FAST_SIZE)?;
    let mk = key(&a[0]);
    let fk = key(&a[fi]);
    if f.market != mk || m.strict_take != 0 || !(book_ok(m, &fk, 0) || book_ok(m, &fk, 1)) {
        return Err(err(E::BadAccount));
    }
    let clk = clock()?;
    let nw: Now = (clk.slot, clk.unix_timestamp);
    fresh_px(f, &mk, m.oracle_px, clk.slot, &a[ti + 3 * n..])?;
    let px0 = mark(m);
    let band = quote_band(px0);
    let (blo, bhi) = bound_range(m);
    let (lo, hi) = (px0.saturating_sub(band).max(blo), (px0 + band).min(bhi));
    let (sl, cnt, _) = collect_slabs(pid, f, &a[fi + 1..fi + 1 + ns], &[0; 32], None, false, false)?;
    let (mkts, rest) = (&a[..1 + nm], &a[ti + 3 * n..]);
    let mut ev = FastEv::with(spec::EVENTS_FAST_BATCH as u8, 0, &key(&a[fi]));
    for k in 0..n {
        let o = 4 + k * 18;
        let o_ = (d[o] as u64 == SIDE_BID, d[o + 1] as u64, rd64(d, o + 2)?, rd64(d, o + 10)?);
        let x = &a[ti + 3 * k..ti + 3 * k + 3];
        batch_take(
            pid,
            f,
            &fk,
            m,
            &mk,
            mkts,
            rest,
            &sl[..cnt],
            &x[0],
            &x[1],
            &x[2],
            nw,
            lo,
            hi,
            o_,
            &mut ev,
            (k as u64) << 40,
        )?;
    }
    ev.flush();
    Ok(())
}

/// One order of take_fast_batch. Ok(()) also when the order is skipped; an error (arithmetic) fails the whole batch.
#[inline(always)]
pub(crate) fn batch_take(
    pid: &Address,
    f: &mut FastBook,
    fk: &Key,
    m: &Market,
    mk: &Key,
    mkts: &[AccountView],
    rest: &[AccountView],
    all: &[(*const Slab, usize)],
    ua: &AccountView,
    sa: &AccountView,
    ra: &AccountView,
    nw: Now,
    lo: u64,
    hi: u64,
    (bid, fl, limit, mut left): (bool, u64, u64, u64),
    ev: &mut FastEv,
    idt: u64,
) -> ProgramResult {
    if left == 0 || left > u32::MAX as u64 || !sg(pid, sa) {
        return Ok(());
    }
    let Ok(u) = load::<User>(ua, pid, DISC_USER, USER_SIZE) else { return Ok(()) };
    let uk = key(ua);
    let dmax = if u.authority == key(sa) {
        u64::MAX
    } else {
        match delegate_max_qty(pid, core::slice::from_ref(ra), &key(sa), &uk, nw.0) {
            Some(x) => x,
            None => return Ok(()),
        }
    };
    let opp = if bid { 1usize } else { 0usize };
    if left > dmax || u.home_vault != m.vault || (f.side != 0 && f.side != opp as u64 + 1) || !book_ok(m, fk, opp) {
        return Ok(());
    }
    claim_all(pid, u, &uk, rest)?;
    let Ok(pi) = pos_idx(u, mk, m.funding_index) else { return Ok(()) };
    if settle_funding_all(pid, u, mkts).is_err() {
        gc_pos(u);
        return Ok(());
    }
    let taker_bps = TIERS[(u.tier as usize).min(TIERS.len() - 1)].0;
    if stale(m, nw) {
        left = 0;
    } // reduce-only too (see take_fast)
    if fl & FLAG_REDUCE_ONLY != 0 {
        let b = u.pos[pi].base;
        left = left.min(if bid && b < 0 {
            (-b) as u64
        } else if !bid && b > 0 {
            b as u64
        } else {
            0
        });
        if left > 0 {
            let ok = match phealth(pid, u, pi, mkts, nw, false) {
                Ok((eq0, _, mm0)) => {
                    reduce_ok(eq0, mm0, mark(m), if bid { limit.min(hi) } else { limit.max(lo) }, left, taker_bps)
                }
                Err(_) => false,
            };
            if !ok {
                left = 0;
            }
        }
    } else if m.paused != 0
        || stale(m, nw)
        || u.pos[pi].flags & POS_ISO != 0
        || (m.max_oi != 0 && m.oi_long as i128 + f.oi_long_delta as i128 > m.max_oi as i128)
        || !worst_ok(pid, u, pi, mkts, nw, m, bid, left, if bid { limit.min(hi) } else { limit.max(lo) }, taker_bps)
    {
        left = 0;
    }
    if left > 0 {
        let mut sl: [(*const Slab, usize); FAST_MAKERS] = [(core::ptr::null(), 0); FAST_MAKERS];
        let mut cnt = 0;
        let sk = key(sa);
        for &(sp, mi) in all {
            let (su, sa_) = unsafe { ((*sp).user, (*sp).authority) };
            if su != uk && sa_ != sk {
                sl[cnt] = (sp, mi);
                cnt += 1;
            }
        } // no self-trade
        fast_match(
            pid,
            f,
            &sl[..cnt],
            (f.idx as usize) << 4 | opp << 3,
            opp,
            bid,
            lo,
            hi,
            &[],
            mk,
            m,
            u,
            &uk,
            pi,
            limit,
            left,
            nw.0,
            taker_bps,
            ev,
            idt | (opp as u64 ^ 1) << 39,
            0,
        )?;
    }
    gc_pos(u);
    Ok(())
}

/// Initial margin holds for the most an order of `qty` lots can do: every lot filled at `worst` (its limit, kept inside the band)
/// at the taker fee. Equity moves by lots * (mark - price) - fee for a buy (price - mark for a sell), so the worst case bounds
/// any fill from below; initial margin follows the gross exposure, which over a partial fill never exceeds the larger of now
/// and full fill. So when this holds, every possible fill of the order leaves equity >= initial margin. Also the position cap.
pub(crate) fn worst_ok(
    pid: &Address,
    u: &User,
    pi: usize,
    mkts: &[AccountView],
    nw: Now,
    m: &Market,
    bid: bool,
    qty: u64,
    worst: u64,
    taker_bps: u64,
) -> bool {
    let Ok((eq, im, _)) = health_im(pid, u, mkts, nw, true) else { return false };
    let (p, px, q) = (&u.pos[pi], mark(m), qty as i128);
    let nt = mulf(q, worst);
    let slip = if bid { nt - mulf(q, px) } else { mulf(q, px) - nt };
    let eq_w = eq - slip.max(0) - bps_up(nt, taker_bps);
    let gross = |b: i128| core::cmp::max((b + p.bid_open as i128).abs(), (b - p.ask_open as i128).abs());
    let (g0, g1) = (gross(p.base as i128), gross(if bid { p.base as i128 + q } else { p.base as i128 - q }));
    if m.max_position != 0 && g1 > m.max_position as i128 {
        return false;
    }
    let ib = m.imr_bps + band_bps(m);
    let im_w = if g1 > g0 { im - bps_up(mulf(g0, px), ib) + bps_up(mulf(g1, px), ib) } else { im };
    eq_w >= im_w
}

#[inline(always)]
/// Taker matching shared by take_fast and trigger orders: the better of the best maker quote and the best resting order fills
/// first, until `left` lots are done or nothing is inside `limit`. Books the taker's side, maker fills, fees and OI. Returns lots.
/// Each slab's best usable level is its first usable one (ladders are best-first). A fill changes only the hit maker's budget and
/// used table, so only that slab is looked at again, from its first usable level at the price it hit: a sweep walks each ladder
/// once (a merge of sorted lists).
pub(crate) fn fast_match(
    pid: &Address,
    f: &mut FastBook,
    sl: &[(*const Slab, usize)],
    base_ix: usize,
    opp: usize,
    bid: bool,
    lo: u64,
    hi: u64,
    rest: &[AccountView],
    mk: &Key,
    m: &Market,
    u: &mut User,
    uk: &Key,
    pi: usize,
    limit: u64,
    mut left: u64,
    slot: u64,
    taker_bps: u64,
    ev: &mut FastEv,
    idt: u64,
    nx: u64,
) -> Result<u64, ProgramError> {
    let (want, base0) = (left, long(u.pos[pi].base));
    let mut in_shards = 0i64;
    // only the first sl.len() entries are ever read, and each is written first
    let mut cand: [core::mem::MaybeUninit<Cand>; FAST_MAKERS] = [core::mem::MaybeUninit::uninit(); FAST_MAKERS];
    // nx != 0: a next-price fill. Quotes are priced off the price signed for the order's own slot, and next-price ladders take part.
    let oracle = if nx != 0 { nx } else { book_px(f, m, slot) };
    // maker quotes are judged against the book's price, so they fill only while that price is recent (spec::FAST_GUARD_MAX_AGE_SLOTS)
    let aged = nx == 0
        && !(f.fast_px != 0 && slot <= f.fast_slot + spec::FAST_PX_MAX_SLOTS)
        && slot > m.oracle_slot + spec::FAST_GUARD_MAX_AGE_SLOTS;
    for (k, &(sp, mi)) in sl.iter().enumerate() {
        cand[k].write(if aged {
            (0, 0, 0, 0, 0)
        } else {
            slab_first(f, sp, mi, base_ix, opp, bid, slot, lo, hi, 0, oracle, nx != 0, nx != 0)
        });
    }
    let (mut acc, mut fees) = (Acc { q: 0, nt: 0, fee: 0 }, 0u64);
    while left > 0 {
        let (mut bk, mut best): (usize, Cand) = (usize::MAX, (0, 0, 0, 0, 0));
        for (k, c) in cand[..sl.len()].iter().enumerate() {
            let c = unsafe { c.assume_init_ref() };
            if c.0 != 0
                && (bk == usize::MAX
                    || (bid && c.0 < best.0)
                    || (!bid && c.0 > best.0)
                    || (c.0 == best.0 && c.2 < best.2))
            {
                bk = k;
                best = *c;
            }
        }
        let (bp, ba, _, bl, b0) = best;
        // resting orders at the best quote's price or better go first (or all of them when no quote is usable): at one price a
        // resting order has waited since it was placed, a quote only since its last re-quote -- price, then time, across both books
        if !rest.is_empty() || m.strict_take != 0 {
            acc.book(u, pi, bid)?; // resting-order matching books the taker's position itself
            let lim = if bk == usize::MAX {
                limit
            } else if bid {
                limit.min(bp)
            } else {
                limit.max(bp)
            };
            let lb = long(u.pos[pi].base);
            let tf = take(pid, rest, mk, m, u, uk, pi, bid, lim, left, slot, taker_bps, STP_CANCEL_OLDEST)?;
            in_shards += long(u.pos[pi].base) - lb; // take() books this part of the open interest on the shard
            left -= tf.filled;
            if left == 0 || tf.abort {
                break;
            }
        }
        if bk == usize::MAX || (bid && bp > limit) || (!bid && bp < limit) {
            break;
        }
        let fq = ba.min(left);
        let (sp, mi) = sl[bk];
        let q = unsafe { &(*sp).quotes[base_ix + bl] };
        let nt = fill_maker(
            f,
            mi,
            q,
            opp << 3 | bl,
            bid,
            bp,
            fq,
            (f.ext[mi].rebate_bps & 0xffff).min(taker_bps),
            m.funding_index,
        )?;
        let fee = bps_up(nt as i128, taker_bps) as u64;
        fees += fee;
        acc.add(u, pi, bid, fq, nt, fee)?;
        ev.push(bp, fq, idt | (mi as u64) << 32 | q.gen as u64);
        left -= fq;
        if left > 0 {
            cand[bk].write(slab_first(f, sp, mi, base_ix, opp, bid, slot, lo, hi, b0, oracle, nx != 0, nx != 0));
        }
    }
    acc.book(u, pi, bid)?;
    f.fee_accum = i64c(f.fee_accum as i128 + fees as i128)?;
    f.oi_long_delta += long(u.pos[pi].base) - base0 - in_shards; // only the maker-quote part: the shard part is booked there
    f.fills += 1;
    Ok(want - left)
}

/// The taker side of a take's maker fills (lots, notional, fees), booked to the User once instead of per fill. Exact: the
/// cost basis only accumulates while the position keeps its sign, so a fill that brings it to zero or across is booked at that
/// boundary on its own terms (see cost_after), same as booking every fill. A take moves at most 2^32 lots at prices < 2^32,
/// so notional and fees fit u64; the sign is a branch, never an i128 multiply (a software routine on SBF).
pub(crate) struct Acc {
    q: u64,
    nt: u64,
    fee: u64,
}

impl Acc {
    #[inline(always)]
    pub(crate) fn add(&mut self, u: &mut User, pi: usize, bid: bool, fq: u64, nt: u64, fee: u64) -> ProgramResult {
        let (b0, q, d) = (u.pos[pi].base, self.q as i64, fq as i64);
        let (b, nb) = if bid { (b0 + q, b0 + q + d) } else { (b0 - q, b0 - q - d) };
        let cross = b != 0 && (nb == 0 || (b > 0) != (nb > 0));
        if cross && nb != 0 {
            self.book(u, pi, bid)?;
        }
        self.q += fq;
        self.nt += nt;
        self.fee += fee;
        if cross {
            self.book(u, pi, bid)?;
        }
        Ok(())
    }
    #[inline(always)]
    pub(crate) fn book(&mut self, u: &mut User, pi: usize, bid: bool) -> ProgramResult {
        if self.q == 0 {
            return Ok(());
        }
        let paid = if bid { self.nt as i128 } else { -(self.nt as i128) };
        credit(u, pi, -(self.fee as i128) - paid)?;
        let ob = u.pos[pi].base;
        u.pos[pi].base =
            if bid { ob.checked_add(self.q as i64) } else { ob.checked_sub(self.q as i64) }.ok_or(err(E::Math))?;
        u.pos[pi].cost = cost_after(ob, u.pos[pi].base, u.pos[pi].cost, paid)?;
        self.q = 0;
        self.nt = 0;
        self.fee = 0;
        Ok(())
    }
}

/// Approved slabs of book `f` among `xs` (only slab accounts are looked at when `filter`; otherwise every account must be one).
/// Each maker may appear once: a repeat is refused (a duplicated slab would fill the same quote again, past its size, budget and
/// inventory cap). The slab's quote block must be registered for this market (fast_join). Killed slabs and the taker's own
/// (`skip_user`, or quotes signed by `skip_auth`) are left out. With `all`, every approved maker must be present, so a
/// liquidator or trigger keeper can't choose whose quotes fill. Returns the slabs, their count and the makers whose quote block is
/// not registered for this market (skipped).
pub(crate) fn collect_slabs(
    pid: &Address,
    f: &FastBook,
    xs: &[AccountView],
    skip_user: &Key,
    skip_auth: Option<&Key>,
    all: bool,
    filter: bool,
) -> Result<([(*const Slab, usize); FAST_MAKERS], usize, u64), ProgramError> {
    let mut sl: [(*const Slab, usize); FAST_MAKERS] = [(core::ptr::null(), 0); FAST_MAKERS];
    let (mut cnt, mut seen, mut unreg) = (0usize, 0u64, 0u64);
    let n = (f.n as usize).min(FAST_MAKERS);
    for (j, x) in xs.iter().enumerate() {
        if filter && disc_of(x) != DISC_SLAB {
            continue;
        }
        let s: &Slab = load(x, pid, DISC_SLAB, SLAB_SIZE)?;
        let k = key(x);
        let mi = if j < n && f.makers[j].slab == k {
            j
        } else {
            f.makers[..n].iter().position(|e| e.slab == k).ok_or(err(E::BadAccount))?
        };
        if seen >> mi & 1 != 0 {
            return Err(err(E::BadAccount));
        }
        seen |= 1 << mi;
        // a block not registered for this market never fills here (and makes the book unprovable for ADL)
        if s.idx_market[(f.idx as usize).min(SLAB_IDX - 1)] != f.market {
            unreg |= 1 << mi;
            continue;
        }
        if s.kill != 0 || s.user == *skip_user || skip_auth.is_some_and(|a| s.authority == *a) {
            continue;
        }
        sl[cnt] = (s as *const Slab, mi);
        cnt += 1;
    }
    if all && seen != (1u64 << n) - 1 {
        return Err(err(E::MissingShards));
    }
    Ok((sl, cnt, unreg))
}

/// Batch reduce-only orders (maker quotes only, no shards): the worst case of `qty` lots at `worst` (taker fee included) must keep
/// equity at or above zero, so a reduce-only fill can never leave bad debt. take_fast and triggers check equity after matching.
pub(crate) fn reduce_ok(eq: i128, _mm: i128, px0: u64, worst: u64, qty: u64, taker_bps: u64) -> bool {
    let slip = qty as i128 * (px0 as i128 - worst as i128).abs();
    let fee = qty as i128 * worst as i128 * taker_bps as i128 / 10000 + 1;
    eq - slip - fee >= 0
}

/// One level a taker could hit: price (0 = none), lots, quote slot, level, and the first usable level at that price (a fill
/// leaves every level above that one unusable, so the slab is looked at again from there).
pub(crate) type Cand = (u64, u64, u32, usize, usize);

/// Best usable level of one maker's slab on the side a taker hits, inside [lo, hi], from level `from` on. Ladders are best-first
/// (slab_quote), so that is the first usable level, except that a later level at the same price with an older quote goes first.
/// Lots are capped by the maker's remaining budget.
#[inline(always)]
pub(crate) fn slab_first(
    f: &FastBook,
    sp: *const Slab,
    mi: usize,
    base_ix: usize,
    opp: usize,
    bid: bool,
    slot: u64,
    lo: u64,
    hi: u64,
    from: usize,
    oracle: u64,
    nx: bool,
    signed: bool,
) -> Cand {
    let s = unsafe { &*sp };
    let e = &f.makers[mi];
    if e.budget <= 0 {
        return (0, 0, 0, 0, 0);
    }
    let lad = unsafe { s.lad.get_unchecked(base_ix >> 3) };
    if lad.flags & spec::LADDER_NEXT != 0 && !nx {
        return (0, 0, 0, 0, 0);
    } // next-price only: no immediate take
      // `nx`: next-price ladders may be used (a committed order's fill, or a liquidation). `signed`: the fill carries the price
      // signed for its own slot, which is what a next-price ladder's freshness is judged by. Any other ladder that asks for a fresh
      // price keeps asking for the book's own, in every path: a liquidation does not waive it (found by the `lanes` fuzzer).
    let (room, peg) = (
        ladder_room(f, lad, mi, bid, oracle, slot, signed && lad.flags & spec::LADDER_NEXT != 0),
        lad.flags & spec::LADDER_PEG != 0,
    );
    if room == 0 {
        return (0, 0, 0, 0, 0);
    }
    // default maker guard (spec::FAST_GUARD_BPS): fixed-price levels through the book's price are skipped
    let guard = !peg && lad.tol_bps == 0;
    let g = if guard { oracle.min(1 << 40).wrapping_mul(spec::FAST_GUARD_BPS) / 10000 } else { 0 };
    let (glo, ghi) = (oracle.saturating_sub(g), oracle.saturating_add(g));
    let mut best: Cand = (0, 0, 0, 0, 0);
    for l in from..SLAB_LEVELS {
        let q = unsafe { s.quotes.get_unchecked(base_ix + l) };
        let p = qpx(q.px, oracle, peg);
        if q.qty == 0 || p == 0 {
            continue;
        }
        if best.0 != 0 && (p != best.0 || q.slot >= best.2) {
            if p != best.0 {
                break;
            }
            continue;
        }
        // beyond the band on the far side every later level is too
        if bid {
            if p > hi {
                break;
            }
            if p < lo {
                continue;
            }
        } else {
            if p < lo {
                break;
            }
            if p > hi {
                continue;
            }
        }
        if slot as u32 - q.slot.min(slot as u32) > quote_life(lad.flags) {
            continue;
        }
        if guard && (if bid { p < glo } else { p > ghi }) {
            continue;
        } // taker buys an ask under the price / sells into a bid over it
        let u_ = unsafe { f.used.get_unchecked(mi).get_unchecked(opp << 3 | l) };
        let av = (if u_.gen == q.gen { q.qty.saturating_sub(u_.qty) } else { q.qty }) as u64;
        let av = av.min(e.budget as u64 / p).min(room);
        if av == 0 {
            continue;
        }
        best = (p, av, q.slot, l, if best.0 == 0 { l } else { best.4 });
    }
    best
}

/// Best usable maker quote across slabs: (slab slot, level, price, lots available). Better price first, then the older
/// quote, then the earlier slab.
#[inline(always)]
pub(crate) fn best_quote(
    f: &FastBook,
    sl: &[(*const Slab, usize)],
    base_ix: usize,
    opp: usize,
    bid: bool,
    slot: u64,
    lo: u64,
    hi: u64,
    oracle: u64,
    nx: bool,
) -> Option<(usize, usize, u64, u64)> {
    let (mut bk, mut best): (usize, Cand) = (usize::MAX, (0, 0, 0, 0, 0));
    for (k, &(sp, mi)) in sl.iter().enumerate() {
        let c = slab_first(f, sp, mi, base_ix, opp, bid, slot, lo, hi, 0, oracle, nx, false);
        if c.0 != 0
            && (bk == usize::MAX || (bid && c.0 < best.0) || (!bid && c.0 > best.0) || (c.0 == best.0 && c.2 < best.2))
        {
            bk = k;
            best = c;
        }
    }
    if bk == usize::MAX {
        None
    } else {
        Some((bk, best.3, best.0, best.1))
    }
}

/// Book `fq` lots at `p` against maker `mi` (quote level `ul` in its used table), paying the maker `rb` bps of the notional out
/// of the book's fees (rb never above the taker fee). Returns the notional.
#[inline(always)]
pub(crate) fn fill_maker(
    f: &mut FastBook,
    mi: usize,
    q: &Quote,
    ul: usize,
    bid: bool,
    p: u64,
    fq: u64,
    rb: u64,
    fidx: i64,
) -> Result<u64, ProgramError> {
    let nt = p.wrapping_mul(fq); // px < 2^32, qty < 2^32: cannot wrap (a checked multiply is a software routine)
    let r = if rb != 0 { bps_dn(nt as i128, rb) } else { 0 };
    if r != 0 {
        f.fee_accum = i64c(f.fee_accum as i128 - r)?;
    }
    let e = &mut f.makers[mi];
    e.budget -= nt as i64;
    // the sign is a branch: a checked i64 / i128 multiply by +-1 is a software routine
    e.base = if bid { e.base.checked_sub(fq as i64) } else { e.base.checked_add(fq as i64) }.ok_or(err(E::Math))?;
    e.quote = i64c(e.quote as i128 + if bid { nt as i128 } else { -(nt as i128) } + r)?;
    e.fund = fund_acc(e.fund, if bid { -(fq as i64) } else { fq as i64 }, fidx)?; // funding runs from this index (slab_settle)
    f.ext[mi].rebate_bps = f.ext[mi].rebate_bps.wrapping_add(fq << 16);
    let us = &mut f.used[mi][ul];
    if us.gen == q.gen {
        us.qty += fq as u32;
    } else {
        us.gen = q.gen;
        us.qty = fq as u32;
    }
    Ok(nt)
}
