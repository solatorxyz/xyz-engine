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
    agg_store(p, d, 0)
}

/// The post itself, shared by agg_post (the publisher signs the transaction) and agg_post_signed (the publisher signed the
/// values, anyone brings them). `d`: one byte that is not read, then price, conf, publish_time, n_sources, [ext_px, ext_ts].
/// `max_age`: a publish_time more than this many seconds behind the clock is refused; 0 = no bound, which is agg_post
/// (staleness there is update_oracle's to judge), and with the constant 0 the check is not compiled into it.
#[inline(always)]
fn agg_store(p: &mut [u8], d: &[u8], max_age: i64) -> ProgramResult {
    let r = |b: &[u8], o: usize| i64::from_le_bytes(b[o..o + 8].try_into().unwrap());
    let (px, conf, pt, ns) = (r(d, 1), r(d, 9), r(d, 17), r(d, 25));
    let (last, last_pt, jump) = (r(p, 73), r(p, 93), r(p, 109) as u64);
    let now = clock()?.unix_timestamp;
    if px <= 0 || conf < 0 || ns <= 0 || pt > now + 5 || (max_age != 0 && pt < now - max_age) {
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

/// A publisher's price, brought by anyone. The publisher signs the values off chain (Ed25519 over
/// AGG_SIG_MAGIC, the feed's address, and the same bytes agg_post takes); whoever needs the price puts that signature in an
/// Ed25519 instruction right before this one and the feed is written exactly as agg_post would write it. So the
/// transaction that uses a price can carry it, and a publisher no longer has to pay to keep every feed fresh on chain in
/// case somebody trades.
/// What is the same as agg_post: the key (the feed's stored authority), price > 0, a publish_time not ahead of the clock,
/// the jump limit, and that a value no newer than the stored one changes nothing. What is added, because the bringer and
/// not the publisher now chooses the moment: the value must be at most AGG_SIGNED_MAX_AGE_SECS old, so nobody can hold a
/// signed price back and bring it once it has become favourable; and once a newer one is on the feed an older one is dead.
/// A signature counts only when key, signature and message all sit in the Ed25519 instruction's own data. Every feed
/// passed must find its signature there: one that does not fails the transaction, it is not skipped.
/// accs: Instructions sysvar, feed(w), [more feeds (w)]. data: tag
pub(crate) fn agg_post_signed(pid: &Address, a: &mut [AccountView]) -> ProgramResult {
    if a.len() < 2 || a[0].address() != &INSTRUCTIONS_ID {
        return Err(err(E::BadData));
    }
    let (sysvar, feeds) = a.split_at_mut(1);
    let ins = Instructions::try_from(&sysvar[0])?;
    let cur = ins.load_current_index() as usize;
    if cur == 0 {
        return Err(err(E::BadData));
    }
    let e = ins.load_instruction_at(cur - 1)?;
    if e.get_program_id().to_bytes() != ED25519_ID {
        return Err(err(E::BadData));
    }
    let d = e.get_instruction_data();
    for x in feeds.iter_mut() {
        if !x.owned_by(pid) || x.data_len() < AGG_SIZE {
            return Err(err(E::BadAccount));
        }
        let fk = key(x);
        let p = unsafe { x.borrow_unchecked_mut() };
        if p[0..8] != AGG_DISC {
            return Err(err(E::BadAccount));
        }
        let mut found = false;
        for i in 0..d.first().copied().unwrap_or(0) as usize {
            let o = 2 + 14 * i;
            if d.len() < o + 14 {
                break;
            }
            let h = |k: usize| u16::from_le_bytes([d[o + 2 * k], d[o + 2 * k + 1]]) as usize;
            // signature offset, its ix, key offset, its ix, message offset, size, its ix
            let (pk, mo, ml) = (h(2), h(4), h(5));
            if h(1) != 0xffff || h(3) != 0xffff || h(6) != 0xffff || (ml != 72 && ml != 88) {
                continue;
            }
            if d.len() < pk + 32 || d.len() < mo + ml {
                continue;
            }
            if d[pk..pk + 32] != p[8..40] || d[mo..mo + 8] != spec::AGG_SIG_MAGIC || d[mo + 8..mo + 40] != fk {
                continue;
            }
            // the message's values start at mo + 40; agg_store reads them from offset 1, as in agg_post's data
            agg_store(p, &d[mo + 39..mo + ml], spec::AGG_SIGNED_MAX_AGE_SECS)?;
            found = true;
            break;
        }
        if !found {
            return Err(err(E::BadData));
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
