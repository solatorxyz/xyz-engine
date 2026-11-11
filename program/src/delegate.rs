//! Keys that may act for an account without owning it: an aggregator's posting key, a per-market trading delegate with a
//! size limit and an expiry, and the market's session key. None of them can withdraw.

use super::*;

pub const IX_AGG_INIT: u64 = 40;

pub const IX_AGG_POST: u64 = 41;

pub const IX_SET_DELEGATE: u64 = 42;

/// accs: authority(s), agg(w, program-owned, AGG_SIZE, zeroed). data: tag, feed_id[32], expo i32, max_jump_bps u64
pub(crate) fn agg_init(pid: &Address, a: &mut [AccountView], d: &[u8]) -> ProgramResult {
    if a.len() < 2 || !sg(pid, &a[0]) || d.len() != 45 {
        return Err(err(E::BadData));
    }
    if !a[1].owned_by(pid) || a[1].data_len() < AGG_SIZE {
        return Err(err(E::BadAccount));
    }
    let ak = key(&a[0]);
    let p = unsafe { a[1].borrow_unchecked_mut() };
    if p[0..8] != [0u8; 8] {
        return Err(err(E::BadAccount));
    }
    p[0..8].copy_from_slice(&AGG_DISC);
    p[8..40].copy_from_slice(&ak);
    p[40] = 1;
    p[41..73].copy_from_slice(&d[1..33]);
    p[89..93].copy_from_slice(&d[33..37]);
    p[109..117].copy_from_slice(&d[37..45]);
    Ok(())
}

/// accs: authority(s), agg(w). data: tag, price i64, conf u64, publish_time i64, n_sources u64, [ext_px i64, ext_ts i64].
/// Checks: signer == stored authority; price > 0; publish_time strictly newer and not in the future (+5 s);
/// |price - last| <= max_jump_bps of last per post (0 = off). Staleness/confidence/divergence are enforced by update_oracle.
pub(crate) fn agg_post(pid: &Address, a: &mut [AccountView], d: &[u8]) -> ProgramResult {
    if a.len() < 2 || !sg(pid, &a[0]) || (d.len() != 33 && d.len() != 49) {
        return Err(err(E::BadData));
    }
    if !a[1].owned_by(pid) || a[1].data_len() < AGG_SIZE {
        return Err(err(E::BadAccount));
    }
    let ak = key(&a[0]);
    let p = unsafe { a[1].borrow_unchecked_mut() };
    if p[0..8] != AGG_DISC || p[8..40] != ak {
        return Err(err(E::BadAccount));
    }
    let r = |b: &[u8], o: usize| i64::from_le_bytes(b[o..o + 8].try_into().unwrap());
    let (px, conf, pt, ns) = (r(d, 1), r(d, 9), r(d, 17), r(d, 25));
    let (last, last_pt, jump) = (r(p, 73), r(p, 93), r(p, 109) as u64);
    let now = clock()?.unix_timestamp;
    if px <= 0 || conf < 0 || ns <= 0 || pt > now + 5 {
        return Err(err(E::Oracle));
    }
    // A post no newer than the stored one is a no-op, not an error: publishers send ~1 tx/s per feed and transactions land
    // out of order across leaders, so an overtaken post must not sink the other posts (and pushes) bundled in its tx.
    // Nothing is applied, so this is the same replay protection with a different return code.
    if pt <= last_pt {
        return Ok(());
    }
    if jump != 0 && last > 0 && ((px - last).unsigned_abs() as u128) * 10000 > (last as u128) * jump as u128 {
        return Err(err(E::Breaker));
    }
    p[101..109].copy_from_slice(&last_pt.to_le_bytes());
    p[73..81].copy_from_slice(&px.to_le_bytes());
    p[81..89].copy_from_slice(&conf.to_le_bytes());
    p[93..101].copy_from_slice(&pt.to_le_bytes());
    p[117..125].copy_from_slice(&ns.to_le_bytes());
    let n = r(p, 125) + 1;
    p[125..133].copy_from_slice(&n.to_le_bytes());
    if d.len() == 49 {
        // external perp consensus: price, publish time (0, 0 = none). One with a bad time is stored as none.
        let (xv, xt) = (r(d, 33), r(d, 41));
        if xv > 0 && xt <= now + 5 && xt >= now - spec::EXT_MAX_AGE_SECS {
            p[133..149].copy_from_slice(&d[33..49]);
        } else {
            p[133..149].fill(0);
        }
    }
    Ok(())
}

/// Owner approves / updates / revokes (expiry 0) a session-key delegate. accs: authority(s), user(r), record(w, program-owned).
/// data: tag, delegate[32], expiry_slot u64, max_qty u64. A record is bound to one user forever.
pub(crate) fn set_delegate(pid: &Address, a: &mut [AccountView], d: &[u8]) -> ProgramResult {
    if a.len() < 3 || d.len() != 49 {
        return Err(err(E::BadData));
    }
    let u: &mut User = load(&a[1], pid, DISC_USER, USER_SIZE)?;
    if !sg(pid, &a[0]) || u.authority != key(&a[0]) {
        return Err(err(E::BadAccount));
    }
    if !a[2].owned_by(pid) || a[2].data_len() < DELEGATE_SIZE {
        return Err(err(E::BadAccount));
    }
    let r = unsafe { &mut *(a[2].borrow_unchecked().as_ptr() as *mut Delegate) };
    if r.disc == 0 {
        r.disc = DISC_DELEGATE;
        r.user = key(&a[1]);
    } else if r.disc != DISC_DELEGATE || r.user != key(&a[1]) {
        return Err(err(E::BadAccount));
    }
    r.delegate.copy_from_slice(&d[1..33]);
    r.expiry_slot = rd64(d, 33)?;
    r.max_qty = rd64(d, 41)?;
    Ok(())
}

/// Delegate authorisation for batch/place/cancel: a matching, unexpired Delegate record among the trailing accounts.
pub(crate) fn delegate_max_qty(pid: &Address, a: &[AccountView], signer: &Key, uk: &Key, slot: u64) -> Option<u64> {
    for x in a {
        if !x.owned_by(pid) || x.data_len() < DELEGATE_SIZE || disc_of(x) != DISC_DELEGATE {
            continue;
        }
        let r = unsafe { &*(x.borrow_unchecked().as_ptr() as *const Delegate) };
        if r.user == *uk && r.delegate == *signer && slot <= r.expiry_slot {
            return Some(r.max_qty);
        }
    }
    None
}

/// accs: signer(s) (admin or session_auth), market(w). data: tag, session u8
pub(crate) fn set_session(pid: &Address, a: &mut [AccountView], d: &[u8]) -> ProgramResult {
    if a.len() < 2 || d.len() < 2 || d[1] > 1 {
        return Err(err(E::BadData));
    }
    let m: &mut Market = load(&a[1], pid, DISC_MARKET, MARKET_SIZE)?;
    let by_key = sg(pid, &a[0]) && m.session_auth != [0; 32] && key(&a[0]) == m.session_auth;
    if !by_key && !auth(pid, &m.admin, a) {
        return Err(err(E::BadAccount));
    }
    if m.session != d[1] as u64 {
        m.session = d[1] as u64;
        m.bound_up = 0;
        m.bound_dn = 0;
        if m.session == 1 {
            m.bound_ref = m.oracle_px;
        }
    }
    Ok(())
}
