//! Orders that execute later: take-profit, stop-loss, TWAP slices and next-price orders.
//!
//! A next-price order is committed at slot t and can only be filled at the price the book's signer signed for slot
//! t + NEXT_DELAY_SLOTS: neither the taker nor whoever executes it can pick the price, and a maker's quote cannot be
//! picked off by someone who has already seen the next tick. The signed price arrives as an Ed25519 instruction in the
//! same transaction and is read through the instructions sysvar (`next_px`).

use super::*;

/// Slot at which a trigger was committed for next-price execution: a next-price order at its creation (next_slot), a TWAP slice
/// (trigger_px) or a stop (slices_left) when a keeper armed it.
pub(crate) fn commit_slot(t: &Trigger) -> u64 {
    match t.kind & 0xff {
        spec::TRIGGER_KIND_NEXT => t.next_slot,
        TRIGGER_KIND_TWAP => t.trigger_px,
        _ => t.slices_left,
    }
}

/// A live commitment: it can still be filled at its price, so it can be neither replaced nor starved of margin.
pub(crate) fn committed(t: &Trigger, slot: u64) -> bool {
    t.kind != 0
        && (t.kind & 0xff == spec::TRIGGER_KIND_NEXT || t.kind & spec::TRIGGER_KIND_FLAG_ARMED != 0)
        && slot <= commit_slot(t) + spec::NEXT_DELAY_SLOTS + spec::NEXT_EXPIRY_SLOTS
}

/// The price the book's signer signed for slot `want` of this market, from the Ed25519 instruction right before this one (key,
/// signature and message all inside that instruction). One price a market a slot: whoever executes cannot choose it.
pub(crate) fn next_px(f: &FastBook, mk: &Key, oracle: u64, want: u64, xs: &[AccountView]) -> Result<u64, ProgramError> {
    let no = err(E::NotTriggered);
    if f.px_signer == [0; 32] || oracle == 0 {
        return Err(no);
    }
    let Some(x) = xs.iter().find(|x| x.address() == &INSTRUCTIONS_ID) else { return Err(no) };
    let ins = Instructions::try_from(x)?;
    let cur = ins.load_current_index() as usize;
    if cur == 0 {
        return Err(no);
    }
    let e = ins.load_instruction_at(cur - 1)?;
    if e.get_program_id().to_bytes() != ED25519_ID {
        return Err(no);
    }
    let d = e.get_instruction_data();
    if d.is_empty() {
        return Err(no);
    }
    for i in 0..d[0] as usize {
        let o = 2 + 14 * i;
        if d.len() < o + 14 {
            break;
        }
        let h = |k: usize| u16::from_le_bytes([d[o + 2 * k], d[o + 2 * k + 1]]) as usize;
        if h(1) != 0xffff || h(3) != 0xffff || h(6) != 0xffff || h(5) != spec::FAST_PX_MSG {
            continue;
        }
        let (pk, mo) = (h(2), h(4));
        if d.len() < pk + 32 || d.len() < mo + spec::FAST_PX_MSG {
            continue;
        }
        if d[pk..pk + 32] != f.px_signer || d[mo..mo + 8] != spec::NEXT_PX_MAGIC || d[mo + 8..mo + 40] != *mk {
            continue;
        }
        let (px, sl) = (rd64(d, mo + 40)?, rd64(d, mo + 48)?);
        // a price far from the oracle is not traded on (the same bound as the book's own signed price)
        if sl != want || px == 0 || px >> 32 != 0 || moved(px, oracle, spec::FAST_PX_MAX_DEV_BPS) {
            continue;
        }
        return Ok(px);
    }
    Err(no)
}

/// accs: authority(s), user(w), market(r). data: tag, slot u8, kind u8, side u8, trigger_px, limit_px, qty, tip, interval, slices (u64s)
pub(crate) fn set_trigger(pid: &Address, a: &mut [AccountView], d: &[u8]) -> ProgramResult {
    if a.len() < 3 || d.len() < 4 + 6 * 8 {
        return Err(err(E::BadData));
    }
    let u: &mut User = load(&a[1], pid, DISC_USER, USER_SIZE)?;
    let m: &Market = load(&a[2], pid, DISC_MARKET, MARKET_SIZE)?;
    if !sg(pid, &a[0]) || u.authority != key(&a[0]) {
        return Err(err(E::BadAccount));
    }
    home(u, m)?;
    let (s, k, side) = (d[1] as usize, d[2] as u64, d[3] as u64);
    let v = |i: usize| rd64(d, 4 + 8 * i);
    // kind byte: low 6 bits = TP/SL/TWAP, bit 6 = FLAG_MARK (>>2 of 256), bit 7 = FLAG_CONFIRM_ORACLE (>>2 of 512)
    let k = (k & 0x3f) | ((k >> 6 & 1) << 8) | ((k >> 7 & 1) << 9);
    if s >= MAX_TRIGGERS || k & 0xff > spec::TRIGGER_KIND_NEXT || side > 1 {
        return Err(err(E::BadData));
    }
    let now = clock()?.slot;
    // a commitment stands until it is filled or lapses: overwriting the slot would be a way to take it back
    if committed(&u.triggers[s], now) {
        return Err(err(E::Locked));
    }
    let next = k & 0xff == spec::TRIGGER_KIND_NEXT;
    let twap = k & 0xff == TRIGGER_KIND_TWAP || next;
    // TP/SL: next_slot is the "has protected a position" mark (see gc_pos); a TWAP's is its next slice
    let armed = u.pos.iter().any(|p| p.market == key(&a[2]) && p.base != 0) as u64;
    let mut t = Trigger {
        market: key(&a[2]),
        kind: k,
        side,
        trigger_px: v(0)?,
        limit_px: v(1)?,
        qty: v(2)?,
        tip: v(3)?,
        interval: v(4)?,
        next_slot: if twap { now } else { armed },
        slices_left: v(5)?,
    };
    // next price: one shot, committed at this slot (next_slot), no trigger level
    if next {
        if t.qty == 0 {
            return Err(err(E::BadData));
        }
        t.trigger_px = 0;
        t.interval = 1;
        t.slices_left = 1;
    } else if twap {
        t.trigger_px = 0;
    } else {
        t.slices_left = 0;
    } // these fields hold the commitment slot once a keeper arms them
    if twap && (t.slices_left == 0 || t.interval == 0) {
        return Err(err(E::BadData));
    }
    u.triggers[s] = t;
    Ok(())
}

/// Permissionless keeper crank. accs: keeper(s), keeper user(w), target user(w), market(r), [fast book(w), slabs(r)...], shards(w)..., [other markets]
/// data: tag, slot u8
pub(crate) fn exec_trigger(pid: &Address, a: &mut [AccountView], d: &[u8]) -> ProgramResult {
    if a.len() < 5 || d.len() < 2 || a[1].address() == a[2].address() {
        return Err(err(E::BadData));
    }
    let kp: &mut User = load(&a[1], pid, DISC_USER, USER_SIZE)?;
    let t: &mut User = load(&a[2], pid, DISC_USER, USER_SIZE)?;
    let m: &Market = load(&a[3], pid, DISC_MARKET, MARKET_SIZE)?;
    if !sg(pid, &a[0]) || kp.authority != key(&a[0]) {
        return Err(err(E::BadAccount));
    }
    home(kp, m)?;
    home(t, m)?;
    let (tk, mk) = (key(&a[2]), key(&a[3]));
    let si = d[1] as usize;
    if si >= MAX_TRIGGERS {
        return Err(err(E::BadData));
    }
    let tr = t.triggers[si];
    if tr.kind == 0 || tr.market != mk {
        return Err(err(E::NotTriggered));
    }
    let clk = clock()?;
    let slot = clk.slot;
    let nw: Now = (slot, clk.unix_timestamp);
    if stale(m, nw) {
        return Err(err(E::Stale));
    }
    let bid = tr.side == SIDE_BID;
    let k8 = tr.kind & 0xff;
    let thr = |px: u64| match k8 {
        TRIGGER_KIND_TP => {
            if bid {
                px <= tr.trigger_px
            } else {
                px >= tr.trigger_px
            }
        }
        TRIGGER_KIND_SL => {
            if bid {
                px >= tr.trigger_px
            } else {
                px <= tr.trigger_px
            }
        }
        _ => false,
    };
    // same rule as perp_core::trigger_fires: oracle by default; FLAG_MARK = mark, + FLAG_CONFIRM_ORACLE = both
    // Next-price execution. A next-price order is committed from its creation; a stop or a TWAP slice once a keeper arms it
    // (arm = 1 in the data, allowed when it would fire). A commitment fills only at the price signed for its own slot +
    // NEXT_DELAY_SLOTS, and lapses NEXT_EXPIRY_SLOTS later.
    let next = k8 == spec::TRIGGER_KIND_NEXT;
    let armed = next || tr.kind & spec::TRIGGER_KIND_FLAG_ARMED != 0;
    let at = commit_slot(&tr) + spec::NEXT_DELAY_SLOTS;
    if armed {
        if slot > at + spec::NEXT_EXPIRY_SLOTS {
            let w = &mut t.triggers[si];
            if next {
                w.kind = 0;
            } else {
                w.kind &= !spec::TRIGGER_KIND_FLAG_ARMED;
                if k8 == TRIGGER_KIND_TWAP {
                    w.trigger_px = 0;
                } else {
                    w.slices_left = 0;
                }
            }
            gc_pos(t);
            return Ok(());
        }
        if slot < at {
            return Err(err(E::NotTriggered));
        }
    }
    let fire = if armed {
        true
    } else if k8 == TRIGGER_KIND_TWAP {
        slot >= tr.next_slot
    } else if tr.kind & spec::TRIGGER_KIND_FLAG_MARK != 0 {
        thr(mark(m)) && (tr.kind & spec::TRIGGER_KIND_FLAG_CONFIRM_ORACLE == 0 || thr(m.oracle_px))
    } else {
        thr(m.oracle_px)
    };
    if !fire {
        return Err(err(E::NotTriggered));
    }
    if !armed && d.len() > 2 && d[2] == 1 {
        let w = &mut t.triggers[si];
        w.kind |= spec::TRIGGER_KIND_FLAG_ARMED;
        if k8 == TRIGGER_KIND_TWAP {
            w.trigger_px = slot;
        } else {
            w.slices_left = slot;
        }
        return Ok(());
    }
    let rest = &a[4..];
    claim_all(pid, t, &tk, rest)?;
    settle_funding_all(pid, t, &a[3..])?;
    let pi = pos_idx(t, &mk, m.funding_index)?;
    let twap = k8 == TRIGGER_KIND_TWAP || next; // a next-price order is one slice
                                                // a slice is what is left over the slices left, so a short fill is caught up by the later ones (and qty < slices still trades)
    let mut qty = if twap { (tr.qty / tr.slices_left.max(1)).max(1).min(tr.qty) } else { tr.qty };
    if twap && qty == 0 {
        t.triggers[si].kind = 0;
        gc_pos(t);
        return Ok(());
    } // nothing left: clear, no tip
    if !twap {
        // the reduce cap is read off the position: with resting orders, the keeper must pass the account's own side too, or a
        // fill it left unclaimed would let the stop sell a position that is already gone (and flip the account)
        if t.pos[pi].n_orders != 0 {
            all_shards(pid, rest, &mk, m, tr.side)?;
        }
        let b = t.pos[pi].base;
        let cap = if bid && b < 0 {
            (-b) as u64
        } else if !bid && b > 0 {
            b as u64
        } else {
            0
        };
        qty = qty.min(cap);
    } else if m.paused != 0 {
        return Err(err(E::Paused));
    }
    let taker_bps = TIERS[(t.tier as usize).min(TIERS.len() - 1)].0;
    if qty == 0 {
        t.triggers[si].kind = 0;
        gc_pos(t);
        return Ok(());
    } // nothing to reduce: clear, no tip
      // no limit = market: the mark -/+ 10%, so a triggered stop is never sold to a stub bid (and a buy stop can fill at all)
    let limit = if tr.limit_px != 0 {
        tr.limit_px
    } else {
        let px = mark(m);
        let s = bps_of(px, spec::TRIGGER_MARKET_SLIP_BPS);
        if bid {
            px + s
        } else {
            px.saturating_sub(s).max(TICK)
        }
    };
    // the keeper picks the accounts, the user owns the fill: every opposite shard must be passed (no routing to a worse bid)
    all_shards(pid, rest, &mk, m, if bid { SIDE_ASK } else { SIDE_BID })?;
    // the tip comes out before the account is judged, so it can never leave it under water (checked after the fill)
    let tip = core::cmp::min(tr.tip, MAX_KEEPER_TIP) as i64;
    // a FastBook among the accounts: fill against maker quotes and resting orders (best price first), same rules as take_fast
    let opp = if bid { 1usize } else { 0usize };
    let fast_acc = rest.iter().find(|x| disc_of(x) == DISC_FAST);
    // a market with a registered book for this side always fills through it (the keeper can't route around the makers)
    if m.fast_book[opp] != [0; 32] && fast_acc.is_none_or(|x| key(x) != m.fast_book[opp]) {
        return Err(err(E::MissingShards));
    }
    let filled = if let Some(fx) = fast_acc {
        let fb: &mut FastBook = load(fx, pid, DISC_FAST, FAST_SIZE)?;
        if fb.market != mk || (fb.side != 0 && fb.side != opp as u64 + 1) || !book_ok(m, &key(fx), opp) {
            return Err(err(E::BadAccount));
        }
        let px0 = mark(m);
        let band = quote_band(px0);
        let (blo, bhi) = bound_range(m);
        let (lo, hi) = (px0.saturating_sub(band).max(blo), (px0 + band).min(bhi));
        let (sl, cnt, _) = collect_slabs(pid, fb, rest, &tk, None, true, true)?;
        let base_ix = (fb.idx as usize) << 4 | opp << 3;
        // committed: the price signed for its slot. Not committed: an immediate fill, refused while a maker here quotes for
        // next-price only (the keeper must arm it first, or it would be routed around those quotes to a worse price).
        let nx = if armed {
            next_px(fb, &mk, m.oracle_px, at, rest)?
        } else {
            if sl[..cnt]
                .iter()
                .any(|&(sp, _)| unsafe { (*sp).lad.get_unchecked(base_ix >> 3).flags } & spec::LADDER_NEXT != 0)
            {
                return Err(err(E::NotTriggered));
            }
            0
        };
        {
            let mut ev = FastEv::new(&tk, bid);
            let r = fast_match(
                pid,
                fb,
                &sl[..cnt],
                base_ix,
                opp,
                bid,
                lo,
                hi,
                rest,
                &mk,
                m,
                t,
                &tk,
                pi,
                limit,
                qty,
                slot,
                taker_bps,
                &mut ev,
                0,
                nx,
            )?;
            ev.flush();
            r
        }
    } else {
        if armed {
            return Err(err(E::NotTriggered));
        }
        take(pid, rest, &mk, m, t, &tk, pi, bid, limit, qty, slot, taker_bps, STP_CANCEL_OLDEST)?.filled
    };
    if filled == 0 {
        return Err(err(E::NotTriggered));
    } // AUDIT M-1: no tip / no slice burn without a fill
      // TP/SL: the tip is paid in proportion to the fill, out of what is left of it -- however many partial fills it takes, the
      // keepers together earn the tip once (a keeper feeding it one lot at a time gains nothing)
    let full_tip = tip;
    let tip = if twap { tip } else { (tip as i128 * filled as i128 / qty as i128) as i64 };
    let w = &mut t.triggers[si];
    if twap {
        w.qty -= filled;
        w.slices_left -= 1;
        w.next_slot = slot + tr.interval; // what a slice missed stays in the order
        w.kind &= !spec::TRIGGER_KIND_FLAG_ARMED;
        if !next {
            w.trigger_px = 0;
        } // the next slice is committed afresh
        if w.slices_left == 0 || w.qty == 0 {
            w.kind = 0;
        }
        credit(t, pi, -(tip as i128))?;
        let (eq, im, _) = phealth(pid, t, pi, &a[3..], nw, true)?;
        if eq < im {
            return Err(err(E::Margin));
        }
        // a TWAP slice adds risk like any order: same position and open-interest caps
        let p = &t.pos[pi];
        if m.max_position != 0
            && core::cmp::max((p.base as i128 + p.bid_open as i128).abs(), (p.base as i128 - p.ask_open as i128).abs())
                > m.max_position as i128
        {
            return Err(err(E::Cap));
        }
        if m.max_oi != 0 && m.oi_long as i128 + if bid { filled as i128 } else { 0 } > m.max_oi as i128 {
            return Err(err(E::Cap));
        }
    } else {
        // a partial fill leaves the rest of the stop armed (it used to be spent by its first lot, leaving the position bare)
        if filled == qty {
            w.kind = 0;
        } else {
            w.qty = w.qty.saturating_sub(filled);
            w.tip = (full_tip - tip) as u64;
            w.kind &= !spec::TRIGGER_KIND_FLAG_ARMED;
            w.slices_left = 0;
        }
        // TP/SL reduce: never leave bad debt (fills may include resting orders from shards the keeper passed)
        credit(t, pi, -(tip as i128))?;
        let (eq, _, _) = phealth(pid, t, pi, &a[3..], nw, false)?;
        if eq < 0 {
            return Err(err(E::Margin));
        }
        if t.pos[pi].flags & POS_ISO == 0 {
            let (ce, _, _) = health_im(pid, t, &a[3..], nw, false)?;
            if ce < 0 {
                return Err(err(E::Margin));
            }
        }
    }
    kp.cash = i64c(kp.cash as i128 + tip as i128)?;
    emit(&[&[EVENTS_TRIGGER as u8, si as u8, tr.kind as u8], &tk, &filled.to_le_bytes()]);
    gc_pos(t);
    Ok(())
}
