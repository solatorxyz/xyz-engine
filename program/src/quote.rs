//! Maker quotes on the fast path.
//!
//! A maker writes its ladders into its own Slab account and takers write fills into the market's FastBook, so quoting
//! and filling never contend for a write lock. A quote costs the maker no margin check of its own: the maker locks
//! margin in the book once (`slab_fund`), and the book refuses fills beyond what that margin supports
//! (`ladder_room`). Fills are moved into the maker's User account by `slab_settle`.

use super::*;

/// Admin approves a maker for this market; the slab's quoting key co-signs (nobody can bind someone else's quote block).
/// accs: admin(s), market(r), fast(w), slab(w), slab authority(s)
pub(crate) fn fast_join(pid: &Address, a: &mut [AccountView]) -> ProgramResult {
    if a.len() < 4 {
        return Err(err(E::BadData));
    }
    let m: &Market = load(&a[1], pid, DISC_MARKET, MARKET_SIZE)?;
    let f: &mut FastBook = load(&a[2], pid, DISC_FAST, FAST_SIZE)?;
    let s: &mut Slab = load(&a[3], pid, DISC_SLAB, SLAB_SIZE)?;
    if !auth(pid, &m.admin, a) || f.market != key(&a[1]) {
        return Err(err(E::BadAccount));
    }
    if a.len() < 5 || !sg(pid, &a[4]) || key(&a[4]) != s.authority {
        return Err(err(E::BadAccount));
    }
    join_core(f, s, key(&a[3]), key(&a[1]))
}

/// Bind the slab's quote block for this book's idx to the market and list the slab's account as a maker of the book.
pub(crate) fn join_core(f: &mut FastBook, s: &mut Slab, sk: Key, mk: Key) -> ProgramResult {
    let n = f.n as usize;
    let ix = (f.idx as usize).min(SLAB_IDX - 1);
    // the slab's quote block for this idx belongs to this market from now on (another market can't claim it)
    if s.idx_market[ix] != [0; 32] && s.idx_market[ix] != mk {
        return Err(err(E::BadAccount));
    }
    s.idx_market[ix] = mk;
    if f.makers[..n].iter().any(|x| x.slab == sk) {
        return Ok(());
    } // re-join only registers the block (migration)
    if n >= FAST_MAKERS {
        return Err(err(E::Cap));
    }
    if f.makers[..n].iter().any(|x| x.user == s.user) {
        return Err(err(E::BadAccount));
    } // one slab per maker account
    f.makers[n] = FastMaker { slab: sk, user: s.user, margin: 0, budget: 0, base: 0, quote: 0, fund: 0, target: 0 };
    f.n += 1;
    Ok(())
}

/// Maker overwrites its own quotes: no margin read and no market account, so it conflicts with nothing but takers reading this slab.
/// accs: authority(s), slab(w). data: tag, items (idx u16, px u32, qty u32)
pub(crate) fn slab_quote(pid: &Address, a: &mut [AccountView], d: &[u8]) -> ProgramResult {
    if a.len() < 2 || !sg(pid, &a[0]) {
        return Err(err(E::BadAccount));
    }
    let s: &mut Slab = load(&a[1], pid, DISC_SLAB, SLAB_SIZE)?;
    if s.authority != key(&a[0]) {
        return Err(err(E::BadAccount));
    }
    quote_core(s, d)
}

/// The quote write itself, shared with the vault's quoting instruction (inlined: slab_quote compiles to what it was).
#[inline(always)]
pub(crate) fn quote_core(s: &mut Slab, d: &[u8]) -> ProgramResult {
    let mut body = &d[1..];
    if body.len() % spec::QUOTE_ITEM_SIZE != 0 {
        return Err(err(E::BadData));
    }
    // ladder settings items come first (idx high bit set), so the quote loop below stays as tight as before
    while body.len() >= spec::QUOTE_ITEM_SIZE && body[1] & 0x80 != 0 {
        ladder_set(s, u16::from_le_bytes([body[0], body[1]]) as usize, &body[..spec::QUOTE_ITEM_SIZE])?;
        body = &body[spec::QUOTE_ITEM_SIZE..];
    }
    let slot = clock()?.slot as u32;
    s.gen = s.gen.wrapping_add(1);
    let g = s.gen as u32;
    // ladders this touched, in order of first touch (items come ladder by ladder, so this is one compare an item; a ladder
    // touched again later is listed again, which only re-checks it)
    let mut lads: [core::mem::MaybeUninit<u16>; MAX_QUOTE_ITEMS] = [core::mem::MaybeUninit::uninit(); MAX_QUOTE_ITEMS];
    let (mut nl, mut prev) = (0usize, usize::MAX);
    for it in body.chunks_exact(spec::QUOTE_ITEM_SIZE) {
        let i = u16::from_le_bytes([it[0], it[1]]) as usize;
        if i >= SLAB_QUOTES {
            return Err(err(E::BadData));
        }
        let q = unsafe { s.quotes.get_unchecked_mut(i) };
        q.px = u32::from_le_bytes([it[2], it[3], it[4], it[5]]);
        q.qty = u32::from_le_bytes([it[6], it[7], it[8], it[9]]);
        q.gen = g;
        q.slot = slot;
        let lad = i / SLAB_LEVELS;
        if lad != prev {
            if nl == MAX_QUOTE_ITEMS {
                return Err(err(E::BadData));
            }
            lads[nl].write(lad as u16);
            nl += 1;
            prev = lad;
        }
    }
    // Every ladder this touched must stay best-first (asks up, bids down; equal prices allowed), so a taker can stop at
    // the first usable level. The check runs here, on the maker's own account, not on the contended take path.
    for x in &lads[..nl] {
        let lad = unsafe { x.assume_init() } as usize;
        if !ladder_ok(unsafe { s.quotes.as_ptr().add(lad * SLAB_LEVELS) }, lad & 1 == 1) {
            return Err(err(E::BadData));
        }
    }
    s.count = s.count.wrapping_add((body.len() / spec::QUOTE_ITEM_SIZE) as u64);
    Ok(())
}

/// A ladder settings item of slab_quote (spec::QUOTE_ITEM_REF / QUOTE_ITEM_INV).
#[inline(never)]
pub(crate) fn ladder_set(s: &mut Slab, i: usize, it: &[u8]) -> ProgramResult {
    let (lad, x, y) = (
        i & 0x3fff,
        u32::from_le_bytes([it[2], it[3], it[4], it[5]]),
        u32::from_le_bytes([it[6], it[7], it[8], it[9]]),
    );
    if i & 0x8000 == 0 || lad >= SLAB_LADDERS {
        return Err(err(E::BadData));
    }
    let l = &mut s.lad[lad];
    if y > u16::MAX as u32 {
        return Err(err(E::BadData));
    }
    if i & 0x4000 == 0 {
        l.ref_px = x;
        l.tol_bps = y as u16;
    } else {
        l.inv_max = x;
        l.flags = y as u16;
    }
    Ok(())
}

/// Most items one slab_quote can carry (a 1,232-byte transaction holds about 120).
pub(crate) const MAX_QUOTE_ITEMS: usize = 128;

/// One side's ladder is best-first: live levels (qty > 0) never get better further down (asks never lower, bids never
/// higher). Pulled levels (qty 0) can sit anywhere; a live level at price 0 is never usable (takers skip it).
#[inline(always)]
pub(crate) fn ladder_ok(q: *const Quote, ask: bool) -> bool {
    if ask {
        let mut last = 0u32;
        for l in 0..SLAB_LEVELS {
            let x = unsafe { &*q.add(l) };
            if x.qty != 0 {
                if x.px < last {
                    return false;
                }
                last = x.px;
            }
        }
    } else {
        let mut last = u32::MAX;
        for l in 0..SLAB_LEVELS {
            let x = unsafe { &*q.add(l) };
            if x.qty != 0 {
                if x.px > last {
                    return false;
                }
                last = x.px;
            }
        }
    }
    true
}

/// Price of a quote level: px, or in a pegged ladder oracle + px - QUOTE_PEG_BIAS. A pegged price outside (0, 2^32) is unusable
/// (0, which takers skip). Offsets keep their order, so a pegged ladder is best-first exactly when its raw px are.
#[inline(always)]
pub(crate) fn qpx(px: u32, oracle: u64, peg: bool) -> u64 {
    if !peg {
        return px as u64;
    }
    let e = oracle.wrapping_add(px as u64).wrapping_sub(spec::QUOTE_PEG_BIAS as u64);
    if e >> 32 == 0 {
        e
    } else {
        0
    }
}

/// Slots a quote of this ladder stays usable after it was written.
#[inline(always)]
pub(crate) fn quote_life(flags: u16) -> u32 {
    if flags & spec::LADDER_NEXT != 0 {
        spec::NEXT_QUOTE_MAX_AGE_SLOTS
    } else {
        spec::QUOTE_MAX_AGE_SLOTS
    }
}

/// Lots maker `mi` may still fill on ladder `lad` (market idx << 1 | side): 0 once the oracle is further than the maker's
/// tolerance from the price it quoted against, else what its inventory cap leaves (u64::MAX without one). Inventory = its
/// position at the last settle + fast fills since. `bid`: the taker buys (hits the ask ladder, the maker sells). A ladder that
/// asks for a fresh price (flags bits 8-15) gives 0 while the book's signed price is older than that many slots.
pub(crate) fn ladder_room(f: &FastBook, l: &Ladder, mi: usize, bid: bool, oracle: u64, slot: u64, nx: bool) -> u64 {
    let fr = (l.flags >> spec::LADDER_FRESH_SHIFT) as u64;
    // (a next-price fill is judged against the price signed for its own slot, not the book's newest)
    if fr != 0 && !nx && (f.fast_px == 0 || slot > f.fast_slot + fr.min(spec::FAST_PX_MAX_SLOTS)) {
        return 0;
    }
    if l.tol_bps != 0 {
        let r = l.ref_px as u64; // < 2^32; the difference is clamped so the product stays native
        if oracle.abs_diff(r).min(1 << 40).wrapping_mul(10000) > r.wrapping_mul(l.tol_bps as u64) {
            return 0;
        }
    }
    if l.inv_max == 0 {
        return u64::MAX;
    }
    let inv = f.ext[mi].inv0 + f.makers[mi].base;
    let room = l.inv_max as i64 + if bid { inv } else { -inv };
    if room > 0 {
        room as u64
    } else {
        0
    }
}

/// Grow a slab or fast book made before the ladder settings / maker extension to the current size. The new bytes are zero
/// (every new setting off). Anyone may call it; the extra rent must already be in the account (transfer it first in the same
/// transaction). accs: target(w)
pub(crate) fn slab_grow(pid: &Address, a: &mut [AccountView]) -> ProgramResult {
    if a.is_empty() || !a[0].owned_by(pid) {
        return Err(err(E::BadAccount));
    }
    let want = match disc_of(&a[0]) {
        DISC_SLAB => SLAB_SIZE,
        DISC_FAST => FAST_SIZE,
        DISC_MARKET => MARKET_SIZE,
        DISC_VAULT => VAULT_SIZE,
        _ => return Err(err(E::BadAccount)),
    };
    if a[0].data_len() >= want {
        return Ok(());
    }
    a[0].resize(want)
}

/// Emergency stop for all quotes of a maker. accs: authority(s), slab(w). data: tag, on u8
pub(crate) fn slab_kill(pid: &Address, a: &mut [AccountView], d: &[u8]) -> ProgramResult {
    if a.len() < 2 || !sg(pid, &a[0]) || d.len() < 2 {
        return Err(err(E::BadData));
    }
    let s: &mut Slab = load(&a[1], pid, DISC_SLAB, SLAB_SIZE)?;
    if s.authority != key(&a[0]) {
        return Err(err(E::BadAccount));
    }
    s.kill = d[1] as u64;
    Ok(())
}

/// Lock (amount > 0) or release (amount < 0) quoting margin. A maker can only fill up to margin / IMR of notional between settles.
/// Optional target turns on auto mode (0 turns it off); see slab_settle.
/// accs: authority(s), user(w), market(r), fast(w), [other markets]. data: tag, amount i64, [target i64]
pub(crate) fn slab_fund(pid: &Address, a: &mut [AccountView], d: &[u8]) -> ProgramResult {
    if a.len() < 4 {
        return Err(err(E::BadData));
    }
    let u: &User = load(&a[1], pid, DISC_USER, USER_SIZE)?;
    if !sg(pid, &a[0]) || u.authority != key(&a[0]) {
        return Err(err(E::BadAccount));
    }
    fund_core(pid, a, d)
}

/// slab_fund once the caller's right to move this account's margin is established (its own key, or its vault's rules).
/// accs: -, user(w), market(r), fast(w), [other markets]
pub(crate) fn fund_core(pid: &Address, a: &mut [AccountView], d: &[u8]) -> ProgramResult {
    if a.len() < 4 {
        return Err(err(E::BadData));
    }
    let amt = rd64(d, 1)? as i64;
    let tgt = if d.len() > 9 {
        let t = rd64(d, 9)? as i64;
        if t < 0 {
            return Err(err(E::BadData));
        }
        Some(t)
    } else {
        None
    };
    let u: &mut User = load(&a[1], pid, DISC_USER, USER_SIZE)?;
    let m: &Market = load(&a[2], pid, DISC_MARKET, MARKET_SIZE)?;
    let f: &mut FastBook = load(&a[3], pid, DISC_FAST, FAST_SIZE)?;
    let (uk, mk) = (key(&a[1]), key(&a[2]));
    if f.market != mk {
        return Err(err(E::BadAccount));
    }
    home(u, m)?;
    let clk = clock()?;
    let nw: Now = (clk.slot, clk.unix_timestamp);
    let n = f.n as usize;
    let mi = f.makers[..n].iter().position(|x| x.user == uk).ok_or(err(E::BadAccount))?;
    let vol = f.ext[mi].rebate_bps & !0xffff; // lots filled since the last settle stay counted
    f.ext[mi] = maker_ext(u, &mk);
    f.ext[mi].rebate_bps |= vol;
    // the maker always has a position slot here while quoting, so settles never run out of slots (pinned until released)
    let pi = pos_idx(u, &mk, m.funding_index)?;
    if u.pos[pi].flags & POS_ISO != 0 {
        return Err(err(E::Isolated));
    } // makers are cross only
    let e = &mut f.makers[mi];
    let unsettled = e.base != 0 || e.quote != 0 || e.fund != 0;
    if amt < 0 && unsettled {
        return Err(err(E::Ledger));
    } // settle first
    if let Some(t) = tgt {
        e.target = t;
    }
    let om = e.margin;
    let nm = i64c(om as i128 + amt as i128)?;
    if nm < 0 {
        return Err(err(E::Margin));
    }
    settle_funding_all(pid, u, &a[2..])?;
    u.cash = i64c(u.cash as i128 - amt as i128)?;
    e.margin = nm;
    lock_mirror(u, pi, amt as i128)?;
    // only a settle refills the budget: with fills still unsettled, new margin adds its own budget and nothing more
    // Settled: the budget is what a settle would give -- margin that initial margin leaves free, not the whole margin (a fund of
    // zero used to hand a maker under its initial requirement a full budget again). Stale price: never more than it had.
    e.budget = if unsettled {
        e.budget.saturating_add(maker_budget(m, nm) - maker_budget(m, om))
    } else {
        match health(pid, u, &a[2..], nw, true) {
            Ok((eq, im, _)) => maker_budget(m, (nm as i128 + eq - im).clamp(0, nm as i128) as i64),
            Err(_) => e.budget.min(maker_budget(m, nm)),
        }
    };
    // set once a maker funds here and kept (another book of the market may still hold its fills): a maker account keeps the slot
    if nm > 0 || unsettled || e.target > 0 {
        u.pos[pi].flags |= POS_PIN;
    } // auto mode: a settle may lock margin later
    if key(&a[3]) == m.fast_book[0] || key(&a[3]) == m.fast_book[1] {
        sync_mirror(u, m, &mk, nm);
    }
    if amt > 0 {
        let (eq, im, _) = health_im(pid, u, &a[2..], nw, true)?;
        if u.cash < 0 || eq < im {
            return Err(err(E::Margin));
        }
    }
    Ok(())
}

/// Permissionless: move a maker's fills into its User and refill the budget. A maker below maintenance gets its margin back
/// in cash and stops quoting, so normal liquidation sees all of its equity.
/// accs: user(w), market(r), fast(w), [other markets]
pub(crate) fn slab_settle(pid: &Address, a: &mut [AccountView]) -> ProgramResult {
    if a.len() < 3 {
        return Err(err(E::BadData));
    }
    let u: &mut User = load(&a[0], pid, DISC_USER, USER_SIZE)?;
    let m: &Market = load(&a[1], pid, DISC_MARKET, MARKET_SIZE)?;
    let f: &mut FastBook = load(&a[2], pid, DISC_FAST, FAST_SIZE)?;
    let (uk, mk) = (key(&a[0]), key(&a[1]));
    if f.market != mk {
        return Err(err(E::BadAccount));
    }
    home(u, m)?;
    let clk = clock()?;
    let nw: Now = (clk.slot, clk.unix_timestamp);
    let n = f.n as usize;
    let mi = f.makers[..n].iter().position(|x| x.user == uk).ok_or(err(E::BadAccount))?;
    let e = &mut f.makers[mi];
    settle_funding_all(pid, u, &a[1..])?;
    let vol = f.ext[mi].rebate_bps >> 16;
    if e.base != 0 || e.quote != 0 || e.fund != 0 || vol != 0 {
        let pi = pos_idx(u, &mk, m.funding_index)?;
        if u.pos[pi].flags & POS_ISO != 0 {
            return Err(err(E::Isolated));
        }
        let (ob, before) = (u.pos[pi].base, long(u.pos[pi].base));
        u.pos[pi].base = ob.checked_add(e.base).ok_or(err(E::Math))?;
        // funding on the unsettled lots, each from the index it was filled at up to now (the position itself was just settled
        // to the current index): lots x index now - sum(lots x index at fill). The taker side of these fills has been paying
        // or receiving it all along; without this the maker side did neither.
        let fpay = if e.fund == 0 && m.funding_index == 0 {
            0
        } else {
            e.base as i128 * m.funding_index as i128 - e.fund as i128
        };
        credit(u, pi, e.quote as i128 - fpay)?;
        u.pos[pi].cost = cost_after(ob, u.pos[pi].base, u.pos[pi].cost, -(e.quote as i128))?;
        f.oi_long_delta += long(u.pos[pi].base) - before;
        u.maker_volume += vol;
        e.base = 0;
        e.quote = 0;
        e.fund = 0;
    }
    // Auto mode can lock margin below, so the maker gets its slot first and the slot is pinned whenever margin ends up locked.
    // (A maker that turned auto mode on with nothing funded had no slot: the settle locked its margin anyway, unmirrored and
    // unpinned, so a withdrawal no longer had to settle its fills first. Found by the fuzzer.)
    let pm = if e.target > 0 || e.margin > 0 {
        Some(pos_idx(u, &mk, m.funding_index)?)
    } else {
        u.pos.iter().position(|p| p.market == mk)
    };
    let (eq, _, mm) = health(pid, u, &a[1..], nw, false)?;
    if eq < mm {
        u.cash = i64c(u.cash as i128 + e.margin as i128)?;
        if let Some(pi) = pm {
            lock_mirror(u, pi, -(e.margin as i128))?;
        }
        e.margin = 0;
    } else if e.target > 0 {
        // Auto mode works like cross margin through the keeper's settles: idle cash tops the margin up to target,
        // and margin goes back to cash first when the rest of the account falls short of initial margin. Skipped on a stale price.
        if let Ok((eq, im, _)) = health_im(pid, u, &a[1..], nw, true) {
            let mv: i128 = if eq < im {
                -((im - eq).min(e.margin as i128))
            } else if e.target > e.margin {
                ((e.target - e.margin) as i128).min((u.cash as i128).max(0)).min(eq - im)
            } else {
                -((e.margin - e.target) as i128)
            };
            u.cash = i64c(u.cash as i128 - mv)?;
            e.margin = i64c(e.margin as i128 + mv)?;
            if let Some(pi) = pm {
                lock_mirror(u, pi, mv)?;
            }
        }
    }
    // the budget refills only from margin that initial margin leaves free: margin + equity - IM of what is already held
    // (a maker between maintenance and initial margin quotes nothing new). Stale price: no refill.
    let free = match health(pid, u, &a[1..], nw, true) {
        Ok((eq, im, _)) => (e.margin as i128 + eq - im).clamp(0, e.margin.max(0) as i128) as i64,
        Err(_) => 0,
    };
    e.budget = maker_budget(m, free);
    if e.margin > 0 {
        if let Some(pi) = pm {
            u.pos[pi].flags |= POS_PIN;
        }
    }
    if key(&a[2]) == m.fast_book[0] || key(&a[2]) == m.fast_book[1] {
        sync_mirror(u, m, &mk, e.margin);
    }
    f.ext[mi] = maker_ext(u, &mk);
    gc_pos(u);
    Ok(())
}
