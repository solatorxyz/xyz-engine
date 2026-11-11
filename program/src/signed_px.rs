//! The book's own signed price. A book may name a key whose Ed25519-signed prices it accepts; quotes pegged to it follow
//! the signer's price between oracle updates. A signed price is bounded by the oracle (FAST_PX_MAX_DEV_BPS), only
//! ever replaced by a newer one, and ignored once it is older than a few slots.

use super::*;

/// Ed25519SigVerify111111111111111111111111111
pub(crate) const ED25519_ID: Key = [
    3, 125, 70, 214, 124, 147, 251, 190, 18, 249, 66, 143, 131, 141, 64, 255, 5, 112, 116, 73, 39, 244, 138, 100, 252,
    202, 112, 68, 128, 0, 0, 0,
];

/// Take the newest signed price for book `f` from the Ed25519 instruction right before this one, when the Instructions
/// sysvar is among `xs`. A signature counts only when its key, signature and message all sit in that instruction's own data
/// (instruction index u16::MAX), the key is the book's signer, the message names this market, its price is within
/// FAST_PX_MAX_DEV_BPS of the oracle and its ms clock is newer than the stored one; others are skipped. The signer's slot is
/// clamped to now, so a signer clock running ahead can't stretch a price's life.
pub(crate) fn fresh_px(f: &mut FastBook, mk: &Key, oracle: u64, slot: u64, xs: &[AccountView]) -> ProgramResult {
    if f.px_signer == [0; 32] || oracle == 0 {
        return Ok(());
    }
    let Some(x) = xs.iter().find(|x| x.address() == &INSTRUCTIONS_ID) else { return Ok(()) };
    let ins = Instructions::try_from(x)?;
    let cur = ins.load_current_index() as usize;
    if cur == 0 {
        return Ok(());
    }
    let e = ins.load_instruction_at(cur - 1)?;
    if e.get_program_id().to_bytes() != ED25519_ID {
        return Ok(());
    }
    let d = e.get_instruction_data();
    if d.is_empty() {
        return Ok(());
    }
    for i in 0..d[0] as usize {
        let o = 2 + 14 * i;
        if d.len() < o + 14 {
            break;
        }
        let h = |k: usize| u16::from_le_bytes([d[o + 2 * k], d[o + 2 * k + 1]]) as usize;
        // signature offset, its ix, key offset, its ix, message offset, size, its ix
        if h(1) != 0xffff || h(3) != 0xffff || h(6) != 0xffff || h(5) != spec::FAST_PX_MSG {
            continue;
        }
        let (pk, mo) = (h(2), h(4));
        if d.len() < pk + 32 || d.len() < mo + spec::FAST_PX_MSG {
            continue;
        }
        if d[pk..pk + 32] != f.px_signer || d[mo..mo + 8] != spec::FAST_PX_MAGIC || d[mo + 8..mo + 40] != *mk {
            continue;
        }
        let (px, sl, ts) = (rd64(d, mo + 40)?, rd64(d, mo + 48)?, rd64(d, mo + 56)?);
        if ts <= f.fast_ts
            || px == 0
            || px >> 32 != 0
            || sl + spec::FAST_PX_TAKE_SLOTS < slot
            || moved(px, oracle, spec::FAST_PX_MAX_DEV_BPS)
        {
            continue;
        }
        f.fast_px = px;
        f.fast_slot = sl.min(slot);
        f.fast_ts = ts;
    }
    Ok(())
}

/// Price pegged ladders and tolerance checks use: the book's signed price while fresh, else the oracle.
#[inline(always)]
pub(crate) fn book_px(f: &FastBook, m: &Market, slot: u64) -> u64 {
    if f.fast_px != 0 && slot <= f.fast_slot + spec::FAST_PX_MAX_SLOTS {
        f.fast_px
    } else {
        m.oracle_px
    }
}

/// Admin sets the key whose signed prices a book takes (zero = none); the stored price is cleared.
/// accs: admin(s), market(r), fast(w). data: tag, key [32]
pub(crate) fn fast_signer(pid: &Address, a: &mut [AccountView], d: &[u8]) -> ProgramResult {
    if a.len() < 3 || d.len() != 33 {
        return Err(err(E::BadData));
    }
    let m: &Market = load(&a[1], pid, DISC_MARKET, MARKET_SIZE)?;
    let f: &mut FastBook = load(&a[2], pid, DISC_FAST, FAST_SIZE)?;
    if !auth(pid, &m.admin, a) || f.market != key(&a[1]) {
        return Err(err(E::BadAccount));
    }
    // Immediate only in the safe direction: clearing the signer (signed prices off), or a book's first signer before the
    // market has a price. Any other change is timelocked (ADMIN_FAST_SIGNER): a new signer can sign within the band.
    if d[1..33] != [0u8; 32] && (f.px_signer != [0; 32] || m.oracle_px != 0) {
        return Err(err(E::Timelock));
    }
    f.px_signer.copy_from_slice(&d[1..33]);
    f.fast_px = 0;
    f.fast_slot = 0;
    f.fast_ts = 0;
    Ok(())
}

/// Anyone may hand a book a newer signed price without trading (a maker or the signer keeping it fresh).
/// accs: fast(w), market(r), Instructions sysvar
pub(crate) fn fast_px_push(pid: &Address, a: &mut [AccountView]) -> ProgramResult {
    if a.len() < 3 {
        return Err(err(E::BadData));
    }
    let f: &mut FastBook = load(&a[0], pid, DISC_FAST, FAST_SIZE)?;
    let m: &Market = load(&a[1], pid, DISC_MARKET, MARKET_SIZE)?;
    let mk = key(&a[1]);
    if f.market != mk {
        return Err(err(E::BadAccount));
    }
    fresh_px(f, &mk, m.oracle_px, clock()?.slot, &a[2..])
}
