//! The market's price, mark and funding.
//!
//! `update_oracle` is permissionless: it reads the primary feed (a Pyth receiver account or an OracleSet quorum),
//! bounds it by confidence, age and a circuit breaker against the last price, then folds the book's impact prices into
//! the mark EMA and accrues funding. Funding is lazy: the market keeps a cumulative index and each position settles
//! against it when it is next touched.

use super::*;

pub const PYTH_RECEIVER: Address = Address::from_str_const("rec5EKMGg6MxZYaMdyBfgwp4d5rB9T1VQH5pJv5LtFJ");

pub const PYTH_PRICE_UPDATE_DISC: [u8; 8] = [34, 241, 35, 99, 157, 126, 244, 205];

pub const IX_OSET_INIT: u64 = 43;

/// accs: market(w), pyth PriceUpdateV2, [best bid shard, best ask shard] (optional, for premium)
/// IX_UPDATE_ORACLE_SOFT: update_oracle whose failure leaves the market untouched instead of failing the transaction, so a
/// publisher can apply its price in the same transaction as its price post (post-and-push). The market is the only account
/// update_oracle writes, so restoring a byte snapshot of it on error is an exact revert (including oracle2_px / paused).
pub(crate) fn update_oracle_soft(pid: &Address, a: &mut [AccountView]) -> ProgramResult {
    if a.is_empty() || !a[0].owned_by(pid) || a[0].data_len() < MARKET_SIZE {
        return Ok(());
    }
    let mut snap = [0u8; MARKET_SIZE];
    snap.copy_from_slice(&unsafe { a[0].borrow_unchecked() }[..MARKET_SIZE]);
    if update_oracle(pid, a).is_err() {
        let d = unsafe { a[0].borrow_unchecked_mut() };
        d[..MARKET_SIZE].copy_from_slice(&snap);
    }
    Ok(())
}

pub(crate) fn update_oracle(pid: &Address, a: &mut [AccountView]) -> ProgramResult {
    if a.len() < 2 {
        return Err(err(E::BadData));
    }
    let m: &mut Market = load(&a[0], pid, DISC_MARKET, MARKET_SIZE)?;
    let clk = clock()?;
    let prim = m.oracle == key(&a[1]);
    // Not the market's primary oracle and no fallback configured: nothing to read, so nothing changes (no-op, not an error).
    // Clients built against an earlier single-AGG oracle keep working after a timelocked switch to an OracleSet; the price
    // itself is only ever taken from the primary (every member of a set) or oracle2.
    if !prim && m.oracle2 == [0; 32] {
        return Ok(());
    }
    if m.oracle2 != [0; 32] && (!prim || !a[2..].iter().any(|x| key(x) == m.oracle2)) {
        return Err(err(E::Oracle));
    }
    let r1 = if prim { read_primary(pid, &a[1], &a[2..], &m.feed_id, m, clk.unix_timestamp) } else { Err(E::Oracle) };
    if matches!(r1, Err(E::MissingShards)) {
        return Err(err(E::MissingShards));
    } // every set member must be passed
      // oracle2 configured but not passed counts as unusable (the crank can't dodge the cross-check by omitting it)
      // oracle2 has its own replay floor (oracle2_px keeps its last applied publish time while oracle2 is configured): judged
      // against the primary's stamp it would drop out of the cross-check whenever the primary stamps ahead of it
    let r2 = if m.oracle2 != [0; 32] {
        Some(
            a[2..]
                .iter()
                .find(|x| key(x) == m.oracle2)
                .map_or(Err(E::Oracle), |x| read_pyth(pid, x, &m.feed_id2, m, clk.unix_timestamp, m.oracle2_px as i64)),
        )
    } else {
        None
    };
    if let Some(Ok(p2)) = r2 {
        m.oracle2_px = p2.2 as u64;
    }
    // B-2/B-3: an OracleSet primary (median of independent publishers with a quorum) is its own jump guard; the
    // single-source hold-and-confirm below is for a lone AGG / Pyth primary
    let is_set = prim
        && a[1].owned_by(pid)
        && a[1].data_len() >= OSET_SIZE
        && unsafe { a[1].borrow_unchecked() }[0..8] == OSET_DISC;
    // oracle2: fallback when the primary is unusable; circuit breaker when both are valid but diverge
    // AUDIT V5-5: hysteresis (trip > MAX_DIVERGENCE, auto-resume only <= RESUME) and a move check whenever
    // only one source is usable while oracle2 is configured. Manual resume: admin ADMIN_PAUSE 0 (re-trips if
    // the sources still diverge).
    let single = |m: &mut Market, p: (u64, u64, i64)| -> Option<(u64, u64, i64)> {
        if m.paused == PAUSED_BREAKER {
            return None;
        } // tripped: only both sources agreeing again resume it
        if m.oracle_px != 0 && moved(p.0, m.oracle_px, spec::ORACLE_SINGLE_SOURCE_MAX_MOVE_BPS) {
            if m.paused == 0 {
                m.paused = spec::PAUSED_SINGLE_SOURCE;
            }
            return None;
        }
        Some(p)
    };
    let (px, conf_px, pt) = match (r1, r2) {
        (Ok(p1), Some(Ok(p2))) => {
            let d = (p1.0 as i128 - p2.0 as i128).abs() * 10000;
            let lo = p1.0.min(p2.0) as i128;
            if d > lo * spec::ORACLE_MAX_DIVERGENCE_BPS as i128 {
                if m.paused == 0 {
                    m.paused = PAUSED_BREAKER;
                }
                return Ok(()); // prices not updated; new risk halted until the sources re-converge
            }
            if m.paused == PAUSED_BREAKER || m.paused == spec::PAUSED_SINGLE_SOURCE {
                if d > lo * spec::ORACLE_RESUME_BPS as i128 {
                    return Ok(());
                } // hysteresis band: stay tripped
                m.paused = 0;
            }
            p1
        }
        (Ok(p1), Some(Err(_))) => match single(m, p1) {
            Some(p) => p,
            None => return Ok(()),
        },
        // AUDIT V6-2: no oracle2 -> jump guard with confirmation. A move > SINGLE_SOURCE_MAX_MOVE vs the stored price
        // is held as a candidate (oracle2_px / feed_id2[0..8] are unused without oracle2) and pauses new risk; it is
        // accepted only when a LATER publish (newer publish_time) confirms it within RESUME_BPS.
        (Ok(p1), None) => {
            let jump = !is_set && m.oracle_px != 0 && moved(p1.0, m.oracle_px, spec::ORACLE_SINGLE_SOURCE_MAX_MOVE_BPS);
            // (while an oracle2 change is pending, feed_id2 holds the armed feed id: no candidate, jumps just pause)
            let slot_free = m.pending_op != spec::ADMIN_ORACLE2;
            if jump {
                if !slot_free {
                    if m.paused == 0 {
                        m.paused = spec::PAUSED_SINGLE_SOURCE;
                    }
                    return Ok(());
                }
                let (c, cts) = (m.oracle2_px, i64::from_le_bytes(m.feed_id2[0..8].try_into().unwrap()));
                let same_way = c != 0 && ((p1.0 >= c) == (c >= m.oracle_px));
                let confirmed = c != 0 && p1.2 > cts && (!moved(p1.0, c, spec::ORACLE_RESUME_BPS) || same_way);
                if !confirmed {
                    m.oracle2_px = p1.0;
                    m.feed_id2[0..8].copy_from_slice(&p1.2.to_le_bytes());
                    if m.paused == 0 {
                        m.paused = spec::PAUSED_SINGLE_SOURCE;
                    }
                    return Ok(());
                }
            }
            if slot_free && m.oracle2_px != 0 {
                m.oracle2_px = 0;
                m.feed_id2[0..8].copy_from_slice(&[0; 8]);
            }
            if m.paused == spec::PAUSED_SINGLE_SOURCE {
                m.paused = 0;
            }
            p1
        }
        (Err(_), Some(Ok(p2))) => match single(m, p2) {
            Some(p) => p,
            None => return Ok(()),
        },
        (Err(e), _) => return Err(err(e)),
    };
    let slot = clk.slot;
    let ds = slot.saturating_sub(m.funding_slot);
    // 1) accrue the index (quote per lot) with the current hourly rate at the stored oracle; remainder carried exactly
    if ds > 0 && m.funding_rate != 0 && m.oracle_px != 0 {
        let den = FUNDING_PPB as i128 * m.funding_period as i128;
        // CU: native i64 math when every factor is small enough that the product can't overflow (same result)
        if m.funding_rate.unsigned_abs() < (1 << 26)
            && m.oracle_px < (1 << 28)
            && ds < (1 << 8)
            && m.funding_rem.unsigned_abs() < (1 << 61)
            && den < (1 << 62)
        {
            let num = m.funding_rate * m.oracle_px as i64 * ds as i64 + m.funding_rem;
            let den = den as i64;
            m.funding_index = i64c(m.funding_index as i128 + num.div_euclid(den) as i128)?;
            m.funding_rem = num.rem_euclid(den);
        } else {
            let num = m.funding_rate as i128 * m.oracle_px as i128 * ds as i128 + m.funding_rem as i128;
            m.funding_index = i64c(m.funding_index as i128 + num.div_euclid(den))?;
            m.funding_rem = num.rem_euclid(den) as i64;
        }
    }
    // 2) premium TWAP: previous sample held over the elapsed slots
    m.prem_acc = i64c(m.prem_acc as i128 + m.prem_last as i128 * ds as i128)?;
    m.prem_slots += ds;
    // 3) period roll: F = avgP + clamp(I - avgP, -c, c) (8h terms), paid per period at F/8, capped (HL formula)
    if slot >= m.period_start + m.funding_period {
        m.funding_rate = funding_rate_from(m.prem_acc, m.prem_slots);
        m.prem_acc = 0;
        m.prem_slots = 0;
        m.period_start = slot;
    }
    m.funding_slot = slot;
    // 4) new premium sample from the passed shards (impact prices through the oracle; omitted liquidity can
    //    only pull the sample toward 0, never fabricate one)
    let mk = key(&a[0]);
    let sh = &a[2..];
    // AUDIT N-1: book-derived values (premium sample, mark EMA) only from the COMPLETE shard set of this market
    // (each shard_no exactly once). No shards passed: previous sample and EMA are held (can't be pulled to 0).
    // CU: one pass over the shards does the full-set check, both impact prices and the best bid/ask.
    let book = if sh.is_empty() { None } else { scan_book(sh, pid, &mk, m.n_shards, px, slot, m.impact_notional)? };
    // maker quotes count as book too (registered books, complete approved set, or none)
    let (fib, fia, fb, fa, seen_books) = fast_bbo(sh, pid, m, &mk, slot, px, m.impact_notional)?;
    // a sample is taken only from the whole book: every shard (all or none, scan_book) and every registered FastBook. A caller
    // passing part of it (say the thin shard side without the maker quotes) changes nothing; the previous sample is held.
    let need_books: u8 = (m.fast_book[0] != [0; 32]) as u8
        | (((m.fast_book[1] != [0; 32]) && m.fast_book[1] != m.fast_book[0]) as u8) << 1;
    let complete =
        (m.n_shards == 0 || book.is_some()) && seen_books == need_books && (m.n_shards != 0 || need_books != 0);
    let (ib, ia, sb, sa) = book.unwrap_or((None, None, None, None));
    let best_b = match (sb, fb) {
        (Some(x), Some(y)) => Some(x.max(y)),
        (x, y) => x.or(y),
    };
    let best_a = match (sa, fa) {
        (Some(x), Some(y)) => Some(x.min(y)),
        (x, y) => x.or(y),
    };
    // a crossed book (bid >= ask) is not a price: nothing book-derived is taken from it this update
    let crossed = matches!((best_b, best_a), (Some(b), Some(a)) if b >= a);
    let (ib, ia) = if crossed { (None, None) } else { (ib.or(fib), ia.or(fia)) };
    let (best_b, best_a) = if crossed { (None, None) } else { (best_b, best_a) };
    let ext = if prim { read_ext_primary(&a[1], &a[2..], pid, m, clk.unix_timestamp) } else { None };
    if ext.is_some() {
        m.ext_px = ext.unwrap_or(0);
        m.ext_ts = clk.unix_timestamp;
    }
    if complete && !crossed {
        let cl = (spec::QUOTE_BAND_BPS as i64) * (FUNDING_PPB as i64 / 10000);
        m.prem_last = premium_ppb(ib, ia, px).clamp(-cl, cl);
        let mid = match (best_b, best_a) {
            (Some(b), Some(a)) => Some((b + a) / 2),
            _ => None,
        };
        // 5) mark = clamp(median(oracle, oracle + EMA(mid - oracle), mid), oracle +- MARK_CLAMP_BPS)
        //    with a fresh external perp price: median(oracle + EMA, mid, external)
        // Mid EMA input: the smaller (same-signed) of this sample's mid deviation and the previous sample's, weighted by the
        // slots between the two samples (capped). A skew is credited only for time it was present at both sample ends, so a
        // caller choosing when to sample gains nothing from the gap it waited. event_seq packs the previous sample's slot
        // (40 bits) and deviation in ppm of the oracle (18 bits, offset).
        let (ps, pdev) = (m.event_seq & ((1u64 << 40) - 1), (m.event_seq >> 40) as i64 - (1i64 << 17));
        let ndev = mid.map_or(0i128, |x| x as i128 - px as i128);
        let nppm = (ndev * 1_000_000 / px as i128).clamp(-100_000, 100_000) as i64;
        let used = if ps == 0 || (pdev > 0) != (nppm > 0) || pdev == 0 || nppm == 0 {
            0i128
        } else {
            (if pdev > 0 { pdev.min(nppm) } else { pdev.max(nppm) }) as i128 * px as i128 / 1_000_000
        };
        let gap = if ps == 0 { 0 } else { slot.saturating_sub(ps).min(spec::MARK_SAMPLE_MAX_SLOTS) };
        m.event_seq = (slot & ((1u64 << 40) - 1)) | (((nppm + (1i64 << 17)) as u64) << 40);
        let (e, _) = mark_from(px, m.mark_ema as i128, gap, Some((px as i128 + used).max(1) as u64), ext);
        let (_, mp) = mark_from(px, e, 0, mid, ext);
        m.mark_ema = i64c(e)?;
        m.mark_px = mp;
    } else {
        let (_, mp) = mark_from(px, m.mark_ema as i128, 0, Some((px as i128 + m.mark_ema as i128).max(1) as u64), ext);
        m.mark_px = mp;
    }
    m.oracle_px = px;
    m.oracle_conf = conf_px;
    m.oracle_slot = clk.slot;
    m.oracle_ts = pt;
    apply_bounds(m, px);
    Ok(())
}

/// Single pass: full-set check (AUDIT N-1), impact bid/ask (AUDIT N-2 level rule) and mid of the best prices.
/// Ok(None) if no shard of the market was passed. Results identical to full_set + impact_px x2 + best x2.
pub(crate) fn scan_book(
    sh: &[AccountView],
    pid: &Address,
    mk: &Key,
    n: u64,
    oracle: u64,
    slot: u64,
    notional: u64,
) -> Result<Option<(Option<u64>, Option<u64>, Option<u64>, Option<u64>)>, ProgramError> {
    let mut seen = [0u64; 4];
    let mut k = 0u64;
    let mut bufs = [[(0u64, 0u64); IMPACT_MAX]; 2];
    let mut ns = [0usize; 2];
    let mut bb: [Option<u64>; 2] = [None, None];
    for x in sh {
        if disc_of(x) != DISC_SHARD || !x.owned_by(pid) {
            continue;
        }
        let s: &Shard = unsafe { &*(x.borrow_unchecked().as_ptr() as *const Shard) };
        if s.market != *mk {
            continue;
        }
        let no = s.shard_no as usize;
        if no >= 256 {
            return Err(err(E::MissingShards));
        }
        if seen[no / 64] >> (no % 64) & 1 == 1 {
            return Err(err(E::BadAccount));
        }
        seen[no / 64] |= 1 << (no % 64);
        k += 1;
        let bid = s.side == SIDE_BID;
        let sd = if bid { 0 } else { 1 };
        let sorted = s.seq & SPECIAL_BIT == 0;
        let now32 = slot & 0xffff_ffff;
        // book-derived inputs (mid, premium) only from orders that rested at least one slot (one placed and cancelled inside
        // the sampling transaction costs nothing), and only as impact prices (size behind them), never a dust best price
        let (buf, nn) = (&mut bufs[sd], &mut ns[sd]);
        for o in &s.orders[..s.n as usize] {
            if o.meta >> 32 == now32 {
                continue;
            }
            let p = eff(o, bid, oracle);
            if o.expiry_slot != 0 && slot > o.expiry_slot {
                continue;
            }
            if o.band_bps != 0 && off_band(p, oracle, o.band_bps) {
                continue;
            }
            let mut i = *nn;
            while i > 0 && (if bid { p > buf[i - 1].0 } else { p < buf[i - 1].0 }) {
                i -= 1;
            }
            if i > 0 && buf[i - 1].0 == p {
                buf[i - 1].1 = buf[i - 1].1.saturating_add(top(o));
                continue;
            }
            if i >= IMPACT_MAX {
                if sorted {
                    break;
                }
                continue;
            }
            let end = if *nn == IMPACT_MAX { *nn - 1 } else { *nn };
            buf.copy_within(i..end, i + 1);
            buf[i] = (p, top(o));
            *nn = end + 1;
        }
    }
    if k == 0 {
        return Ok(None);
    }
    if k != n {
        return Err(err(E::MissingShards));
    }
    let imp = |buf: &[(u64, u64)]| -> Option<u64> {
        // CU: native u64 walk when the notional is small enough that an + one level can't overflow (same result)
        if notional < (1 << 62) {
            let (mut an, mut aq) = (0u64, 0u64);
            for &(p, q) in buf {
                let need = (notional - an).div_ceil(p);
                let take = if q < need { q } else { need };
                an += take * p;
                aq += take; // take*p <= notional - an + p - 1 < 2^63
                if an >= notional {
                    return Some(an / aq);
                }
            }
            return None;
        }
        let (mut an, mut aq) = (0u128, 0u128);
        for &(p, q) in buf {
            let left = notional as u128 - an;
            let take = (q as u128).min(left.div_ceil(p as u128));
            an += take * p as u128;
            aq += take;
            if an >= notional as u128 {
                return Some((an / aq) as u64);
            }
        }
        None
    };
    let (ib, ia) = (imp(&bufs[0][..ns[0]]), imp(&bufs[1][..ns[1]]));
    let _ = &mut bb;
    Ok(Some((ib, ia, ib, ia)))
}

/// External perp consensus from our own oracle account, in market price units. None when old, absent or from Pyth.
/// External perp consensus of the primary oracle: the AGG's own value, or for an OracleSet the median of its members' fresh values
/// (members are already required among `rest` by read_primary).
pub(crate) fn read_ext_primary(
    x: &AccountView,
    rest: &[AccountView],
    pid: &Address,
    m: &Market,
    now: i64,
) -> Option<u64> {
    if !(x.owned_by(pid) && x.data_len() >= OSET_SIZE && unsafe { x.borrow_unchecked() }[0..8] == OSET_DISC) {
        return read_ext(x, pid, m, now);
    }
    let s = unsafe { x.borrow_unchecked() };
    let n = (s[8] as usize).min(OSET_MAX);
    let mut v = [0u64; OSET_MAX];
    let mut k = 0;
    for i in 0..n {
        let mk = &s[16 + 32 * i..48 + 32 * i];
        if let Some(acc) = rest.iter().find(|r| r.address().as_ref() == mk) {
            if let Some(q) = read_ext(acc, pid, m, now) {
                v[k] = q;
                k += 1;
            }
        }
    }
    if k == 0 || k < s[9] as usize {
        return None;
    } // like the price: a quorum of publishers, never one alone
    for i in 1..k {
        let y = v[i];
        let mut j = i;
        while j > 0 && v[j - 1] > y {
            v[j] = v[j - 1];
            j -= 1;
        }
        v[j] = y;
    }
    Some(if k % 2 == 1 { v[k / 2] } else { v[k / 2 - 1] / 2 + v[k / 2] / 2 + (v[k / 2 - 1] % 2 + v[k / 2] % 2) / 2 })
}

pub(crate) fn read_ext(x: &AccountView, pid: &Address, m: &Market, now: i64) -> Option<u64> {
    if !x.owned_by(pid) || x.data_len() < AGG_SIZE {
        return None;
    }
    let p = unsafe { x.borrow_unchecked() };
    if p[0..8] != AGG_DISC {
        return None;
    }
    let r = |o: usize| i64::from_le_bytes(p[o..o + 8].try_into().unwrap());
    let (v, ts) = (r(133), r(141));
    if v <= 0 || now.checked_sub(ts).is_none_or(|d| d.unsigned_abs() > spec::EXT_MAX_AGE_SECS as u64) {
        return None;
    }
    let q = (v as i128 * m.px_mul as i128 / m.px_div as i128) as u64;
    if q == 0 {
        None
    } else {
        Some(q)
    }
}

/// Maker quotes as book: from the market's registered books only, every approved maker's slab passed (all or none, like the
/// shard set). Quotes posted in this slot don't count; each counts with its unused size, capped by its maker's budget.
/// Returns (impact bid, impact ask, best bid, best ask) over IMPACT_MAX levels per side, like scan_book.
pub(crate) fn fast_bbo(
    sh: &[AccountView],
    pid: &Address,
    m: &Market,
    mk: &Key,
    slot: u64,
    px: u64,
    notional: u64,
) -> Result<(Option<u64>, Option<u64>, Option<u64>, Option<u64>, u8), ProgramError> {
    let band = quote_band(px);
    let (lo, hi) = (px.saturating_sub(band), px + band);
    let mut bufs = [[(0u64, 0u64); IMPACT_MAX]; 2];
    let mut ns = [0usize; 2];
    let mut books = 0;
    for fx in sh.iter().filter(|x| disc_of(x) == DISC_FAST) {
        let fk = key(fx);
        if fk != m.fast_book[0] && fk != m.fast_book[1] {
            return Err(err(E::BadAccount));
        }
        let bit = if fk == m.fast_book[0] { 1 } else { 2 };
        if books & bit != 0 || (m.fast_book[0] == m.fast_book[1] && books != 0) {
            return Err(err(E::BadAccount));
        } // each book once
        books |= bit;
        let f: &FastBook = load(fx, pid, DISC_FAST, FAST_SIZE)?;
        if f.market != *mk {
            return Err(err(E::BadAccount));
        }
        let mut seen = 0u64;
        for x in sh {
            if disc_of(x) != DISC_SLAB {
                continue;
            }
            let s: &Slab = load(x, pid, DISC_SLAB, SLAB_SIZE)?;
            let k = key(x);
            let Some(mi) = f.makers[..f.n as usize].iter().position(|e| e.slab == k) else { continue };
            if seen >> mi & 1 == 1 {
                return Err(err(E::BadAccount));
            }
            seen |= 1 << mi;
            if s.kill != 0 || f.makers[mi].budget <= 0 {
                continue;
            }
            let mut budget = f.makers[mi].budget as u128;
            for sd in 0..2usize {
                if f.side != 0 && f.side != sd as u64 + 1 {
                    continue;
                }
                let base = (f.idx as usize) << 4 | sd << 3;
                let lad = &s.lad[base >> 3];
                let mut room = ladder_room(f, lad, mi, sd == 1, px, slot, false);
                // same default maker guard as the matcher (slab_first): a level no taker can fill doesn't count for the mark
                let guard = lad.flags & spec::LADDER_PEG == 0 && lad.tol_bps == 0;
                let g = if guard { px.min(1 << 40).wrapping_mul(spec::FAST_GUARD_BPS) / 10000 } else { 0 };
                for l in 0..SLAB_LEVELS {
                    if room == 0 || budget == 0 {
                        break;
                    }
                    let q = &s.quotes[base + l];
                    let p = qpx(q.px, px, lad.flags & spec::LADDER_PEG != 0);
                    if q.qty == 0
                        || p < lo
                        || p > hi
                        || q.slot as u64 >= slot & 0xffff_ffff
                        || slot as u32 - q.slot.min(slot as u32) > quote_life(lad.flags)
                    {
                        continue;
                    }
                    if guard && (if sd == 1 { p < px.saturating_sub(g) } else { p > px.saturating_add(g) }) {
                        continue;
                    }
                    let u_ = &f.used[mi][sd << 3 | l];
                    let left =
                        if u_.gen == q.gen { (q.qty as u64).saturating_sub(u_.qty as u64) } else { q.qty as u64 };
                    let qty = left.min(room).min((budget / p as u128) as u64);
                    if qty == 0 {
                        continue;
                    }
                    room -= qty;
                    budget -= qty as u128 * p as u128;
                    let bid = sd == 0;
                    let (buf, nn) = (&mut bufs[sd], &mut ns[sd]);
                    let mut i = *nn;
                    while i > 0 && (if bid { p > buf[i - 1].0 } else { p < buf[i - 1].0 }) {
                        i -= 1;
                    }
                    if i > 0 && buf[i - 1].0 == p {
                        buf[i - 1].1 = buf[i - 1].1.saturating_add(qty);
                        continue;
                    }
                    if i >= IMPACT_MAX {
                        continue;
                    }
                    let end = if *nn == IMPACT_MAX { *nn - 1 } else { *nn };
                    buf.copy_within(i..end, i + 1);
                    buf[i] = (p, qty);
                    *nn = end + 1;
                }
            }
        }
        if seen != (1u64 << f.n) - 1 {
            return Err(err(E::MissingShards));
        }
    }
    let imp = |buf: &[(u64, u64)]| -> Option<u64> {
        let (mut an, mut aq) = (0u128, 0u128);
        for &(p, q) in buf {
            let left = (notional as u128).saturating_sub(an);
            let take = (q as u128).min(left.div_ceil(p as u128));
            an += take * p as u128;
            aq += take;
            if an >= notional as u128 {
                return Some((an / aq) as u64);
            }
        }
        None
    };
    let (ib, ia) = (imp(&bufs[0][..ns[0]]), imp(&bufs[1][..ns[1]]));
    Ok((ib, ia, ib, ia, books as u8)) // mid from impact prices too: one lot at the edge of the band moves nothing
}

/// PriceUpdateV2: disc8 | write_authority32 | verification_level (1 = Full) | feed_id32 | price i64 | conf u64 | expo i32 | publish_time i64 | ...
/// accs: creator(s), set(w, program-owned, OSET_SIZE, zeroed). data: tag, quorum u8, n u8, n x member key[32].
/// Members must be distinct; quorum > n/2 so two disjoint halves can never both price. The set is only used once the market
/// admin points the market's oracle at it (timelocked oracle change), so its creator gains nothing.
pub(crate) fn oset_init(pid: &Address, a: &mut [AccountView], d: &[u8]) -> ProgramResult {
    if a.len() < 2 || !sg(pid, &a[0]) || d.len() < 3 {
        return Err(err(E::BadData));
    }
    let (q, n) = (d[1] as usize, d[2] as usize);
    if n == 0 || n > OSET_MAX || q == 0 || q > n || q * 2 <= n || d.len() != 3 + 32 * n {
        return Err(err(E::BadData));
    }
    if !a[1].owned_by(pid) || !a[1].is_writable() || a[1].data_len() < OSET_SIZE {
        return Err(err(E::BadAccount));
    }
    let p = unsafe { a[1].borrow_unchecked_mut() };
    if p[0..8] != [0u8; 8] {
        return Err(err(E::BadAccount));
    }
    for i in 0..n {
        for j in 0..i {
            if d[3 + 32 * i..35 + 32 * i] == d[3 + 32 * j..35 + 32 * j] {
                return Err(err(E::BadData));
            }
        }
    }
    p[0..8].copy_from_slice(&OSET_DISC);
    p[8] = n as u8;
    p[9] = q as u8;
    p[16..16 + 32 * n].copy_from_slice(&d[3..]);
    Ok(())
}

/// Primary oracle: a single AGG / Pyth account, or an OracleSet whose members must all be passed among `rest`. Each member is read
/// like a single AGG (fresh, same feed, conf within limit; no per-member replay rule), at least `quorum` usable; price = median
/// (mean of the middle pair for an even count), conf = max(median conf, median |p - median|), publish time = median, which must
/// not be older than the last applied one. Members that disagree widen conf past max_conf_bps -> unusable.
pub(crate) fn read_primary(
    pid: &Address,
    x: &AccountView,
    rest: &[AccountView],
    feed: &Key,
    m: &Market,
    now: i64,
) -> Result<(u64, u64, i64), E> {
    if !(x.owned_by(pid) && x.data_len() >= OSET_SIZE && unsafe { x.borrow_unchecked() }[0..8] == OSET_DISC) {
        return read_pyth(pid, x, feed, m, now, m.oracle_ts);
    }
    let s = unsafe { x.borrow_unchecked() };
    let (n, q) = ((s[8] as usize).min(OSET_MAX), s[9] as usize);
    let (mut px, mut cf, mut pt) = ([0u64; OSET_MAX], [0u64; OSET_MAX], [0i64; OSET_MAX]);
    let mut k = 0;
    for i in 0..n {
        let mk = &s[16 + 32 * i..48 + 32 * i];
        let Some(acc) = rest.iter().find(|r| r.address().as_ref() == mk) else { return Err(E::MissingShards) };
        if let Ok((p, c, t)) = read_pyth(pid, acc, feed, m, now, 0) {
            px[k] = p;
            cf[k] = c;
            pt[k] = t;
            k += 1;
        }
    }
    if k < q.max(1) {
        return Err(E::Oracle);
    }
    fn isort<T: Copy + PartialOrd>(v: &mut [T]) {
        for i in 1..v.len() {
            let x = v[i];
            let mut j = i;
            while j > 0 && v[j - 1] > x {
                v[j] = v[j - 1];
                j -= 1;
            }
            v[j] = x;
        }
    }
    let med_u = |v: &mut [u64]| {
        isort(v);
        let n = v.len();
        if n % 2 == 1 {
            v[n / 2]
        } else {
            v[n / 2 - 1] / 2 + v[n / 2] / 2 + (v[n / 2 - 1] % 2 + v[n / 2] % 2) / 2
        }
    };
    let mut ps = px;
    let med = med_u(&mut ps[..k]);
    let mut dev = [0u64; OSET_MAX];
    for i in 0..k {
        dev[i] = px[i].abs_diff(med);
    }
    let conf = med_u(&mut cf[..k]).max(med_u(&mut dev[..k]));
    let mut ts = pt;
    isort(&mut ts[..k]);
    let t = ts[(k - 1) / 2];
    if t < m.oracle_ts {
        return Err(E::Stale);
    }
    if exceeds(conf, med, m.max_conf_bps) {
        return Err(E::Oracle);
    }
    Ok((med, conf, t))
}

pub(crate) fn read_pyth(
    pid: &Address,
    x: &AccountView,
    feed: &Key,
    m: &Market,
    now: i64,
    floor: i64,
) -> Result<(u64, u64, i64), E> {
    if x.data_len() < 133 {
        return Err(E::Oracle);
    }
    let p = unsafe { x.borrow_unchecked() };
    let ok =
        (x.owned_by(&PYTH_RECEIVER) && p[0..8] == PYTH_PRICE_UPDATE_DISC) || (x.owned_by(pid) && p[0..8] == AGG_DISC);
    if !ok || p[40] != 1 || p[41..73] != *feed {
        return Err(E::Oracle);
    }
    let r = |o: usize| u64::from_le_bytes(p[o..o + 8].try_into().unwrap());
    let (price, conf) = (r(73) as i64, r(81));
    let expo = i32::from_le_bytes(p[89..93].try_into().unwrap()) as i64;
    let pt = r(93) as i64;
    if price <= 0 || expo != m.expo {
        return Err(E::Oracle);
    }
    if now - pt > m.max_age_secs as i64 {
        return Err(E::Stale);
    }
    if pt < floor {
        return Err(E::Stale);
    } // AUDIT L-4: no replay of an older update (floor = last applied publish time)
    if exceeds(conf, price as u64, m.max_conf_bps) {
        return Err(E::Oracle);
    } // price > 0 checked above
      // CU: native u64 division for the common px_mul == 1 config (same result: both operands positive)
    let (px, conf_px) = if m.px_mul == 1 && m.px_div > 0 {
        (price as u64 / m.px_div as u64, conf / m.px_div as u64)
    } else {
        (
            (price as i128 * m.px_mul as i128 / m.px_div as i128) as u64,
            (conf as i128 * m.px_mul as i128 / m.px_div as i128) as u64,
        )
    };
    if px == 0 {
        return Err(E::Oracle);
    }
    Ok((px, conf_px, pt))
}

pub(crate) fn mark(m: &Market) -> u64 {
    if m.mark_px != 0 {
        m.mark_px
    } else {
        m.oracle_px
    }
}

/// Same as perp_core::mark_from.
pub(crate) fn mark_from(px: u64, ema: i128, ds: u64, mid: Option<u64>, ext: Option<u64>) -> (i128, u64) {
    let s = match mid {
        Some(x) => x as i128 - px as i128,
        None => 0,
    };
    let w = core::cmp::min(ds, spec::MARK_EMA_SLOTS) as i128;
    let d = s - ema;
    // CU: native i64 when |d| * w can't overflow (same truncating division)
    let e = if d.unsigned_abs() < (1 << 40) && w < (1 << 20) {
        ema + ((d as i64 * w as i64) / spec::MARK_EMA_SLOTS as i64) as i128
    } else {
        ema + d * w / spec::MARK_EMA_SLOTS as i128
    };
    let adj = px as i128 + e;
    let (a, b, c) = match ext {
        Some(x) => (adj, mid.map_or(adj, |v| v as i128), x as i128),
        None => (px as i128, adj, mid.unwrap_or(px) as i128),
    };
    let med = a.min(b).max(a.max(b).min(c));
    let band = if px < (1 << 48) {
        (px * spec::MARK_CLAMP_BPS / 10000) as i128
    } else {
        px as i128 * spec::MARK_CLAMP_BPS as i128 / 10000
    };
    (e, med.clamp((px as i128 - band).max(1), px as i128 + band) as u64)
}

/// Hourly funding rate (ppb of oracle per period) from the period's premium TWAP (ppb).
pub(crate) fn funding_rate_from(acc: i64, n: u64) -> i64 {
    let avg = if n > 0 { acc as i128 / n as i128 } else { 0 };
    let (i8h, c8h, cap) =
        (FUNDING_INTEREST_8H_PPB as i128, FUNDING_PREMIUM_CLAMP_8H_PPB as i128, FUNDING_MAX_RATE_HOURLY_PPB as i128);
    let f8 = avg + (i8h - avg).clamp(-c8h, c8h);
    (f8 / FUNDING_HOURS_PER_8H as i128).clamp(-cap, cap) as i64
}

/// premium = (max(impact_bid - oracle, 0) - max(oracle - impact_ask, 0)) / oracle, in ppb.
pub(crate) fn premium_ppb(ib: Option<u64>, ia: Option<u64>, o: u64) -> i64 {
    let up = ib.map_or(0, |b| b.saturating_sub(o)) as i128;
    let dn = ia.map_or(0, |x| o.saturating_sub(x)) as i128;
    let x = up - dn;
    if x.unsigned_abs() < (1 << 33) && o < (1 << 62) {
        (x as i64 * FUNDING_PPB as i64) / o as i64
    } else {
        (x * FUNDING_PPB as i128 / o as i128) as i64
    }
}

pub(crate) const PAUSED_BREAKER: u64 = spec::PAUSED_BREAKER;

pub(crate) const IMPACT_MAX: usize = spec::IMPACT_MAX_LEVELS as usize;

/// accs: user(w), markets... (permissionless lazy funding settlement)
pub(crate) fn settle(pid: &Address, a: &mut [AccountView]) -> ProgramResult {
    let u: &mut User = load(&a[0], pid, DISC_USER, USER_SIZE)?;
    settle_funding_all(pid, u, &a[1..])
}

/// accs: market(w), shards(w)... : fold per-shard OI / fee counters into Market (crank, off the trade path)
pub(crate) fn aggregate(pid: &Address, a: &mut [AccountView]) -> ProgramResult {
    let m: &mut Market = load(&a[0], pid, DISC_MARKET, MARKET_SIZE)?;
    let mk = key(&a[0]);
    for x in a[1..].iter() {
        if disc_of(x) == DISC_FAST {
            let f: &mut FastBook = load(x, pid, DISC_FAST, FAST_SIZE)?;
            if f.market != mk {
                return Err(err(E::BadAccount));
            }
            m.oi_long = m.oi_long.wrapping_add_signed(f.oi_long_delta);
            f.oi_long_delta = 0; // partial sums may dip below zero; the total is exact
            m.fees += f.fee_accum;
            f.fee_accum = 0;
            continue;
        }
        if disc_of(x) == DISC_INS {
            let ins: &mut Insurance = load(x, pid, DISC_INS, INS_SIZE)?;
            if ins.market != mk {
                return Err(err(E::BadAccount));
            }
            m.oi_long = m.oi_long.wrapping_add_signed(ins.oi_long_delta);
            ins.oi_long_delta = 0;
            continue; // partial sums may dip below zero; the total is exact
        }
        let s: &mut Shard = load(x, pid, DISC_SHARD, SHARD_SIZE)?;
        if s.market != mk {
            return Err(err(E::BadAccount));
        }
        m.oi_long = m.oi_long.wrapping_add_signed(s.oi_long_delta);
        s.oi_long_delta = 0; // partial sums may dip below zero; the total is exact
        m.fees += s.fee_accum;
        s.fee_accum = 0;
    }
    Ok(())
}
