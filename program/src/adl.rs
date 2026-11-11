//! Auto-deleveraging: a bankrupt position is closed against the most profitable, most leveraged opposite positions at the
//! bankruptcy price. The ranking is kept on chain (`Rank`), maintained permissionlessly, and `adl` refuses a
//! counterparty that is not at the head of it.

use super::*;

/// ADL score of position `pi` at price `px`: (PnL / equity) x (|notional| / equity), both x1e4.
/// None = not eligible (flat, not profitable, or equity <= 0).
pub(crate) fn adl_score(u: &User, pi: usize, px: u64, eq: i128) -> Option<i64> {
    let p = &u.pos[pi];
    if p.base == 0 || eq <= 0 {
        return None;
    }
    let notional = mulf(p.base as i128, px);
    let pnl = notional - p.cost as i128;
    if pnl <= 0 {
        return None;
    }
    let a = pnl.checked_mul(10000).map(|x| x / eq);
    let b = notional.abs().checked_mul(10000).map(|x| x / eq);
    let s = match (a, b) {
        (Some(a), Some(b)) => a.checked_mul(b).map(|x| x / 10000).unwrap_or(i128::MAX),
        _ => i128::MAX,
    };
    Some(s.min(i64::MAX as i128) as i64)
}

pub(crate) fn rank_n(r: &Rank, side: usize) -> usize {
    (if side == 0 { r.n_long } else { r.n_short }) as usize
}

pub(crate) fn rank_set_n(r: &mut Rank, side: usize, n: usize) {
    if side == 0 {
        r.n_long = n as u64
    } else {
        r.n_short = n as u64
    }
}

pub(crate) fn rank_remove(r: &mut Rank, side: usize, user: &Key) {
    let (o, n) = (side * RANK_N, rank_n(r, side));
    if let Some(i) = r.entries[o..o + n].iter().position(|e| e.user == *user) {
        r.entries.copy_within(o + i + 1..o + n, o + i);
        rank_set_n(r, side, n - 1);
    }
}

/// insert sorted by score desc (ties: earlier entry first); full list keeps the top RANK_N.
pub(crate) fn rank_insert(r: &mut Rank, side: usize, user: &Key, score: i64) {
    let (o, n) = (side * RANK_N, rank_n(r, side));
    let i = r.entries[o..o + n].iter().position(|e| e.score < score).unwrap_or(n);
    if i >= RANK_N {
        return;
    }
    let end = if n == RANK_N { n - 1 } else { n };
    r.entries.copy_within(o + i..o + end, o + i + 1);
    r.entries[o + i] = RankEntry { user: *user, score };
    rank_set_n(r, side, end + 1);
}

/// Permissionless: (re)score one account in this market's ADL queue at the current price.
/// accs: market(r), rank(w), user(w), [other markets of the user]
pub(crate) fn adl_rank(pid: &Address, a: &mut [AccountView]) -> ProgramResult {
    if a.len() < 3 {
        return Err(err(E::BadData));
    }
    let m: &Market = load(&a[0], pid, DISC_MARKET, MARKET_SIZE)?;
    let r: &mut Rank = load(&a[1], pid, DISC_RANK, RANK_SIZE)?;
    let u: &mut User = load(&a[2], pid, DISC_USER, USER_SIZE)?;
    if m.adl_rank != key(&a[1]) {
        return Err(err(E::BadAccount));
    }
    home(u, m)?;
    let clk = clock()?;
    let nw: Now = (clk.slot, clk.unix_timestamp);
    if stale(m, nw) {
        return Err(err(E::Stale));
    }
    let (uk, mk) = (key(&a[2]), key(&a[0]));
    settle_funding_all(pid, u, a)?;
    rank_remove(r, 0, &uk);
    rank_remove(r, 1, &uk);
    if let Some(pi) = u.pos.iter().position(|p| p.market == mk) {
        // AUDIT V5-1: isolated positions are ranked too (own equity)
        let (eq, _, _) = phealth(pid, u, pi, a, nw, false)?;
        if let Some(s) = adl_score(u, pi, mark(m), eq) {
            rank_insert(r, if u.pos[pi].base > 0 { 0 } else { 1 }, &uk, s);
        }
    }
    Ok(())
}

/// ADL / backstop for a bankrupt account (equity < 0). The counterparty is NOT chosen by the caller: the
/// program walks the on-chain ranking (opposite side, best score first), re-scores each entry, evicts
/// ineligible ones, and uses the first eligible. The caller must pass the user accounts of every entry it walks.
/// Closes at the mark; deficit -> insurance, then the counterparty up to its own equity (it can never be made
/// bankrupt), remainder recorded as market bad debt.
/// accs: target user(w), market(w), insurance(w), rank(w), ranked users(w)..., [other markets]
pub(crate) fn adl(pid: &Address, a: &mut [AccountView]) -> ProgramResult {
    if a.len() < 5 {
        return Err(err(E::BadAccount));
    }
    let t: &mut User = load(&a[0], pid, DISC_USER, USER_SIZE)?;
    // held shared: the health passes below read this market through the account list. Its two writes go through a borrow of
    // their own, made where they happen, and this one is not read again after them.
    let m: &Market = load(&a[1], pid, DISC_MARKET, MARKET_SIZE)?;
    let ins: &mut Insurance = load(&a[2], pid, DISC_INS, INS_SIZE)?;
    let r: &mut Rank = load(&a[3], pid, DISC_RANK, RANK_SIZE)?;
    if m.insurance != key(&a[2]) || m.adl_rank != key(&a[3]) {
        return Err(err(E::BadAccount));
    }
    home(t, m)?;
    let clk = clock()?;
    let nw: Now = (clk.slot, clk.unix_timestamp);
    if stale(m, nw) {
        return Err(err(E::Stale));
    }
    let (tk, mk) = (key(&a[0]), key(&a[1]));
    let mkts = &a[1..];
    // fills sitting in the passed shards count, and the target's resting orders here are cancelled (a bankrupt account
    // can't keep a live order as a free option); the shards holding them must be among the accounts
    claim_all(pid, t, &tk, &a[4..])?;
    settle_funding_all(pid, t, mkts)?;
    let ti = t.pos.iter().position(|p| p.market == mk).ok_or(err(E::NotBankrupt))?;
    if t.pos[ti].flags & POS_ISO == 0 {
        maker_clear(pid, t, &tk, m, mkts)?;
    }
    cancel_all_of(&a[4..], pid, &mk, &tk, t, ti);
    no_resting(t, ti)?;
    let (eq, _, tmm) = phealth(pid, t, ti, mkts, nw, true)?;
    // AUDIT V6-1: a position marked ADL-eligible by liquidate_book (book empty within the widest band) may be
    // deleveraged while below maintenance, inside the window; otherwise ADL needs bankruptcy (equity < 0).
    let fl = t.pos[ti].flags;
    let eligible = fl & spec::POS_FLAG_ADL_ELIGIBLE != 0 && clk.slot <= (fl >> 8) + LIQ_ADL_WINDOW_SLOTS && eq < tmm;
    if eq >= 0 && !eligible {
        return Err(err(E::NotBankrupt));
    }
    let tb = t.pos[ti].base;
    if tb == 0 {
        return Err(err(E::NotBankrupt));
    }
    let side = if tb > 0 { 1 } else { 0 }; // counterparties hold the opposite position
    let px = mark(m);
    // AUDIT N-4: every candidate passed by the caller is re-scored now (fresh ranking at use time)
    // Entries that can't be used (the target itself, non-user / foreign accounts) are evicted here too, so after
    // this loop every passed account is freshly scored and the walk needs no second health pass (CU).
    for ca in a[4..].iter() {
        let ck = key(ca);
        if ck == tk {
            rank_remove(r, side, &ck);
            continue;
        }
        let Ok(c) = load::<User>(ca, pid, DISC_USER, USER_SIZE) else { continue };
        if home(c, m).is_err() {
            rank_remove(r, side, &ck);
            continue;
        }
        // its other markets not passed: it can't be re-scored here, so it keeps its place (refused below if it is the top)
        if settle_funding_all(pid, c, mkts).is_err() {
            continue;
        }
        rank_remove(r, side, &ck);
        if let Some(ci) = c.pos.iter().position(|p| p.market == mk && ((p.base > 0) == (side == 0)) && p.base != 0) {
            let (ce, _, _) = phealth(pid, c, ci, mkts, nw, false)?;
            if let Some(s) = adl_score(c, ci, px, ce) {
                rank_insert(r, side, &ck, s);
            }
        }
    }
    if rank_n(r, side) == 0 {
        return Err(err(E::Rank));
    }
    let ck = r.entries[side * RANK_N].user;

    let ca = a[4..].iter().find(|x| key(x) == ck).ok_or(err(E::Rank))?; // top entry must be passed (and was just re-scored)
    let c: &mut User = load(ca, pid, DISC_USER, USER_SIZE)?;
    // the top entry must have been re-scored in this call (its markets passed; settling again is a no-op then) and must hold
    // the opposite side now: an entry that couldn't be evaluated keeps its rank and blocks, it is never skipped or trusted
    if home(c, m).is_err() || settle_funding_all(pid, c, mkts).is_err() {
        return Err(err(E::Rank));
    }
    let ci = c.pos.iter().position(|p| p.market == mk).ok_or(err(E::Rank))?;
    let cb = c.pos[ci].base;
    if cb == 0 || (cb > 0) != (side == 0) {
        return Err(err(E::Rank));
    }
    let q = tb.unsigned_abs().min(cb.unsigned_abs()) as i64;
    // the target's deficit belongs to its whole position: this step carries only the closed lots' share of it
    let share = |d: i128| -> i128 { (d * q as i128 + tb.unsigned_abs() as i128 - 1) / tb.unsigned_abs() as i128 };
    let dq = if tb > 0 { -q } else { q }; // target reduces toward 0
    let n = mulf(dq as i128, px);
    let l0 = long(tb) + long(cb);
    let (otb, ocb) = (t.pos[ti].base, c.pos[ci].base);
    // the counterparty's unrealized profit on the lots closed here: what deleveraging at the bankruptcy price would take,
    // and the most it pays toward the target's deficit (never its collateral, never more than its equity)
    let c_gain = ((mulf(ocb as i128, px) - c.pos[ci].cost as i128) * q as i128 / ocb.unsigned_abs() as i128).max(0);
    t.pos[ti].base += dq;
    c.pos[ci].base -= dq;
    {
        let mw: &mut Market = load(&a[1], pid, DISC_MARKET, MARKET_SIZE)?;
        mw.oi_long = mw.oi_long.wrapping_add_signed(long(t.pos[ti].base) + long(c.pos[ci].base) - l0);
    } // partial sums may dip below zero; the total is exact (as in the crank)
    credit(t, ti, -n)?;
    credit(c, ci, n)?;
    t.pos[ti].cost = cost_after(otb, t.pos[ti].base, t.pos[ti].cost, n)?;
    c.pos[ci].cost = cost_after(ocb, c.pos[ci].base, c.pos[ci].cost, -n)?;
    let (mut eq2, _, _) = phealth(pid, t, ti, mkts, nw, false)?;
    if eligible && eq2 > 0 {
        // non-bankrupt fallback ADL pays the max-step liquidation fee to insurance
        let fee = bps_dn(n.abs(), LIQ_FEE_BPS << (LIQ_BOOK_BAND_STEPS - 1)).min(eq2);
        credit(t, ti, -fee)?;
        ins.balance = i64c(ins.balance as i128 + fee)?;
        eq2 -= fee;
    }
    let (mut ce, _, _) = phealth(pid, c, ci, mkts, nw, false)?; // one health pass; equity is linear in cash / iso_margin
    if eq2 < 0 {
        let deficit = share(-eq2); // the rest stays with the target's remaining lots, for the next step
        let from_ins = deficit.min(ins.balance.max(0) as i128);
        let from_c = (deficit - from_ins).min(ce.max(0)).min(c_gain); // AUDIT M-5: never below zero, never beyond its gain on these lots
        ins.balance = i64c(ins.balance as i128 - from_ins)?;
        credit(c, ci, -from_c)?;
        ce -= from_c;
        credit(t, ti, deficit)?; // VERIFY-1: the unrecovered part is carried in bad_debt, not also in the target's cash
        let mw: &mut Market = load(&a[1], pid, DISC_MARKET, MARKET_SIZE)?;
        mw.bad_debt = i64c(mw.bad_debt as i128 + deficit - from_ins - from_c)?;
    }
    // re-rank the counterparty at its new size
    rank_remove(r, side, &ck);
    if let Some(s) = adl_score(c, ci, px, ce) {
        rank_insert(r, side, &ck, s);
    }
    emit(&[&[EVENTS_ADL as u8], &tk, &ck, &q.to_le_bytes()]);
    gc_pos(t);
    gc_pos(c);
    Ok(())
}
