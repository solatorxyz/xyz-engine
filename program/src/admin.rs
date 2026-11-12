//! Parameters and who may change them. Changes that can only reduce risk (pause, lower a cap, clear a signer) are
//! immediate; everything else is proposed, waits out `TIMELOCK_SLOTS` in the open, and is executed by anyone.

use super::*;

/// accs: admin(s), market(w). data: tag, op u8, value u64
pub(crate) fn admin(pid: &Address, a: &mut [AccountView], d: &[u8]) -> ProgramResult {
    if a.len() < 2 || d.len() < 10 {
        return Err(err(E::BadData));
    }
    let m: &mut Market = load(&a[1], pid, DISC_MARKET, MARKET_SIZE)?;
    let v = rd64(d, 2)?;
    // Emergency stop: any ONE member of the market's multisig can pause (no new risk or deposits; cancels, reduce-only,
    // withdrawals and liquidations keep working). Un-pausing and everything else needs the full m of n.
    if d[1] as u64 == ADMIN_PAUSE && v == 1 && any_member(pid, &m.admin, a) {
        if m.paused == 0 {
            m.paused = 1;
        }
        return Ok(());
    } // never downgrades a tripped breaker
    if !auth(pid, &m.admin, a) {
        return Err(err(E::BadAccount));
    }
    // immediate: pause and new-risk caps (cannot move funds). Everything else goes through the timelock.
    // AUDIT V5-4: on a pooled market the OI cap can only be lowered (never above the value admitted under the pool cap)
    if m.pool != [0; 32] && d[1] as u64 == ADMIN_MAX_OI && (v == 0 || v > m.max_oi) {
        return Err(err(E::Cap));
    }
    match d[1] as u64 {
        ADMIN_PAUSE => m.paused = v,
        ADMIN_TVL_CAP => m.tvl_cap = v,
        ADMIN_MAX_OI => m.max_oi = v,
        spec::ADMIN_MAX_POSITION => m.max_position = v,
        _ => return Err(err(E::Timelock)),
    }
    Ok(())
}

/// Timelocked admin change. accs: admin(s), market(w). data: tag, op u8 (0 = cancel), val u64, [key 32]
pub(crate) fn admin_propose(pid: &Address, a: &mut [AccountView], d: &[u8]) -> ProgramResult {
    if a.len() < 2 || d.len() < 10 {
        return Err(err(E::BadData));
    }
    let m: &mut Market = load(&a[1], pid, DISC_MARKET, MARKET_SIZE)?;
    if !auth(pid, &m.admin, a) {
        return Err(err(E::BadAccount));
    }
    let op = d[1] as u64;
    if !(op == 0 || (spec::ADMIN_IMR..=spec::ADMIN_MARKET_CFG).contains(&op)) || op == spec::ADMIN_MAX_POSITION {
        return Err(err(E::BadData));
    }
    if op == spec::ADMIN_MARKET_CFG {
        let v = rd64(d, 2)?;
        if v & 0xffff > 10000 || (v >> 16) & 0xffff > 10000 || v >> 48 > 1 {
            return Err(err(E::BadData));
        }
    }
    let pv = rd64(d, 2)?;
    // Margin has a floor in every market, pooled or on its own: MMR_BPS is the largest move one post of a single
    // feed can make without being held (ORACLE_SINGLE_SOURCE_MAX_MOVE_BPS), so a position at maintenance still has
    // equity after it, and IMR_BPS is twice that. A pooled market also cannot hand itself to another admin.
    if match op {
        spec::ADMIN_IMR => pv < IMR_BPS,
        spec::ADMIN_MMR => pv < MMR_BPS,
        spec::ADMIN_SET_ADMIN => m.pool != [0; 32],
        _ => false,
    } {
        return Err(err(E::Cap)); // AUDIT V5-4 (also re-checked at execute)
    }
    m.pending_op = op;
    m.pending_val = pv;
    m.pending_key = [0; 32];
    if d.len() >= 42 {
        m.pending_key.copy_from_slice(&d[10..42]);
    }
    if op == spec::ADMIN_ORACLE2 {
        // fallback disabled immediately (safe direction); new feed id armed, key set at execute
        m.oracle2 = [0; 32];
        if d.len() >= 74 {
            m.feed_id2.copy_from_slice(&d[42..74]);
        }
    }
    m.pending_eta = clock()?.slot + spec::TIMELOCK_SLOTS;
    Ok(())
}

/// Permissionless once the timelock expired. accs: market(w), [the market's registered FastBooks (w), for ADMIN_FAST_SIGNER]
pub(crate) fn admin_execute(pid: &Address, a: &mut [AccountView]) -> ProgramResult {
    if a.is_empty() {
        return Err(err(E::BadData));
    }
    let m: &mut Market = load(&a[0], pid, DISC_MARKET, MARKET_SIZE)?;
    if m.pending_op == 0 || clock()?.slot < m.pending_eta {
        return Err(err(E::Timelock));
    }
    let v = m.pending_val;
    // AUDIT V5-4: pooled markets: margin can only be tightened (never below the spec defaults) and the admin is not transferable
    if match m.pending_op {
        spec::ADMIN_IMR => v < IMR_BPS,
        spec::ADMIN_MMR => v < MMR_BPS,
        spec::ADMIN_SET_ADMIN => m.pool != [0; 32],
        _ => false,
    } {
        return Err(err(E::Cap));
    }
    match m.pending_op {
        spec::ADMIN_IMR => {
            if v <= m.mmr_bps || v > 10000 {
                return Err(err(E::BadData));
            }
            m.imr_bps = v
        }
        spec::ADMIN_MMR => {
            if v == 0 || v >= m.imr_bps {
                return Err(err(E::BadData));
            }
            m.mmr_bps = v
        }
        spec::ADMIN_IMPACT_NOTIONAL => {
            if v == 0 {
                return Err(err(E::BadData));
            }
            m.impact_notional = v
        }
        spec::ADMIN_SET_ADMIN => {
            if m.pending_key == [0; 32] {
                return Err(err(E::BadData));
            }
            m.admin = m.pending_key
        }
        spec::ADMIN_FUNDING_PERIOD => {
            if v == 0 {
                return Err(err(E::BadData));
            }
            m.funding_period = v
        }
        spec::ADMIN_ORACLE2 => m.oracle2 = m.pending_key,
        spec::ADMIN_ORACLE => {
            if m.pending_key == [0; 32] {
                return Err(err(E::BadData));
            }
            m.oracle = m.pending_key
        }
        spec::ADMIN_MARKET_CFG => {
            m.bound_bps = v & 0xffff;
            m.bound_thresh_bps = (v >> 16) & 0xffff;
            m.bound_max = (v >> 32) & 0xffff;
            m.strict_take = v >> 48;
            m.session_auth = m.pending_key;
        }
        spec::ADMIN_FAST_SIGNER => {
            // every registered book of the market is passed and gets the signer (a book left out would keep the old one)
            let mk = key(&a[0]);
            for (i, fk) in m.fast_book.iter().enumerate() {
                if *fk == [0; 32] || (i == 1 && *fk == m.fast_book[0]) {
                    continue;
                }
                let x = a[1..].iter().find(|x| key(x) == *fk).ok_or(err(E::MissingShards))?;
                let f: &mut FastBook = load(x, pid, DISC_FAST, FAST_SIZE)?;
                if f.market != mk {
                    return Err(err(E::BadAccount));
                }
                f.px_signer = m.pending_key;
                f.fast_px = 0;
                f.fast_slot = 0;
                f.fast_ts = 0;
            }
        }
        _ => return Err(err(E::BadData)),
    }
    m.pending_op = 0;
    m.pending_eta = 0;
    Ok(())
}

/// MM priority tier (fee tier / rebate). accs: admin(s), market(r), user(w). data: tag, tier u8
pub(crate) fn set_tier(pid: &Address, a: &mut [AccountView], d: &[u8]) -> ProgramResult {
    if a.len() < 3 || d.len() < 2 {
        return Err(err(E::BadData));
    }
    let m: &Market = load(&a[1], pid, DISC_MARKET, MARKET_SIZE)?;
    let u: &mut User = load(&a[2], pid, DISC_USER, USER_SIZE)?;
    if !auth(pid, &m.admin, a) || d[1] as usize >= TIERS.len() {
        return Err(err(E::BadAccount));
    }
    home(u, m)?; // AUDIT M-4: a market admin can only tier accounts bound to its own vault
    u.tier = d[1] as u64;
    Ok(())
}

/// accs: admin(s), market(w). data: tag, bound_bps, thresh_bps, max_resets, strict_take (u64s), session_auth[32]
pub(crate) fn market_cfg(pid: &Address, a: &mut [AccountView], d: &[u8]) -> ProgramResult {
    if a.len() < 2 || d.len() != 65 {
        return Err(err(E::BadData));
    }
    let m: &mut Market = load(&a[1], pid, DISC_MARKET, MARKET_SIZE)?;
    if !auth(pid, &m.admin, a) {
        return Err(err(E::BadAccount));
    }
    if m.oracle_px != 0 {
        return Err(err(E::Timelock));
    } // live market: ADMIN_MARKET_CFG through the timelock
    let (b, t) = (rd64(d, 1)?, rd64(d, 9)?);
    if b > 10000 || t > 10000 {
        return Err(err(E::BadData));
    }
    m.bound_bps = b;
    m.bound_thresh_bps = t;
    m.bound_max = rd64(d, 17)?;
    m.strict_take = rd64(d, 25)?;
    m.session_auth.copy_from_slice(&d[33..65]);
    Ok(())
}
