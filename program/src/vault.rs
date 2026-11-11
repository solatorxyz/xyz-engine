//! The liquidity vault: one pool that makes markets, backs liquidations and is owned pro rata by its depositors.
//!
//! Its positions sit in program-controlled sub-accounts that no key can sign for. The operator's hot key can write
//! quotes (inside rules the admin sets) and move idle cash between the vault's own sub-accounts; it has no instruction
//! that moves value out. Share price = value / shares, and the value is kept in parts (one per sub-account) so that a
//! vault quoting more markets than one transaction can carry is still priced exactly: see `vault_nav`.

use super::*;

/// accs: admin(s), vault(w, zeroed), market(r). data: tag, lockup_slots, day_cap_bps, perf_bps, dd_bps
pub(crate) fn vault_init(pid: &Address, a: &mut [AccountView], d: &[u8]) -> ProgramResult {
    if a.len() < 3 || !sg(pid, &a[0]) || (d.len() != 33 && d.len() != 65) {
        return Err(err(E::BadData));
    }
    if !a[1].owned_by(pid) || a[1].data_len() < VAULT_SIZE || disc_of(&a[1]) != 0 {
        return Err(err(E::BadAccount));
    }
    let m: &Market = load(&a[2], pid, DISC_MARKET, MARKET_SIZE)?;
    let (cap, perf, dd) = (rd64(d, 9)?, rd64(d, 17)?, rd64(d, 25)?);
    if cap > 10000 || perf > 3000 || dd > 10000 {
        return Err(err(E::BadData));
    }
    let admin = admin_key(pid, a, &key(&a[0]), if d.len() == 65 { Some(&d[33..65]) } else { None })?;
    let v = unsafe { &mut *(a[1].borrow_unchecked().as_ptr() as *mut Vault) };
    v.disc = DISC_VAULT;
    v.admin = admin;
    v.token_vault = m.vault;
    v.lockup_slots = rd64(d, 1)?;
    v.day_cap_bps = cap;
    v.perf_bps = perf;
    v.dd_bps = dd;
    Ok(())
}

/// accs: admin(s), vault(w), user(w, zeroed). The sub-account answers only to the program, on the vault's behalf.
pub(crate) fn vault_add_sub(pid: &Address, a: &mut [AccountView]) -> ProgramResult {
    if a.len() < 3 {
        return Err(err(E::BadData));
    }
    let v: &mut Vault = load(&a[1], pid, DISC_VAULT, VAULT_SIZE)?;
    if !auth(pid, &v.admin, a) || v.n_subs as usize >= VAULT_SUBS {
        return Err(err(E::BadAccount));
    }
    if !a[2].owned_by(pid) || a[2].data_len() < USER_SIZE || disc_of(&a[2]) != 0 {
        return Err(err(E::BadAccount));
    }
    let u = unsafe { &mut *(a[2].borrow_unchecked().as_ptr() as *mut User) };
    u.disc = DISC_USER;
    u.authority = key(&a[1]);
    u.home_vault = v.token_vault;
    v.subs[v.n_subs as usize] = key(&a[2]);
    v.n_subs += 1;
    if v.n_subs == 1 {
        v.active = 1;
    }
    Ok(())
}

/// accs: admin(s), vault(r), market(r), link(w, zeroed or own). data: tag, cap_bps
pub(crate) fn vault_link(pid: &Address, a: &mut [AccountView], d: &[u8]) -> ProgramResult {
    if a.len() < 4 {
        return Err(err(E::BadData));
    }
    let v: &mut Vault = load(&a[1], pid, DISC_VAULT, VAULT_SIZE)?;
    let _m: &Market = load(&a[2], pid, DISC_MARKET, MARKET_SIZE)?;
    if !auth(pid, &v.admin, a) {
        return Err(err(E::BadAccount));
    }
    if !a[3].owned_by(pid) || a[3].data_len() < VLINK_SIZE {
        return Err(err(E::BadAccount));
    }
    let l = unsafe { &mut *(a[3].borrow_unchecked().as_ptr() as *mut VaultLink) };
    if l.disc != 0 && (l.disc != DISC_VLINK || l.vault != key(&a[1]) || l.market != key(&a[2])) {
        return Err(err(E::BadAccount));
    }
    l.disc = DISC_VLINK;
    l.vault = key(&a[1]);
    l.market = key(&a[2]);
    l.cap_bps = rd64(d, 1)?;
    Ok(())
}

/// What one sub-account is worth: its equity with funding settled, plus the margin it has locked in the books. Its fills
/// must be settled into it (and its resting orders' fills claimed) first: the program takes locked margin out of the
/// account's cash, so equity alone would leave it out and a deposit would buy the vault below its worth.
pub(crate) fn sub_worth(
    pid: &Address,
    u: &mut User,
    uk: &Key,
    a: &[AccountView],
    nw: Now,
) -> Result<i128, ProgramError> {
    settle_funding_all(pid, u, a)?;
    complete_view(pid, u, uk, a, None)?;
    Ok(health(pid, u, a, nw, true)?.0 + maker_mirror(u))
}

/// Vault value: the worth of every active sub-account. One that is passed is valued now (and remembered); one that is not
/// counts at its remembered value, which must be recent. Flat, unused subs need neither.
pub(crate) fn vault_nav(pid: &Address, v: &mut Vault, a: &[AccountView], nw: Now) -> Result<i128, ProgramError> {
    let mut nav = 0i128;
    for i in 0..(v.n_subs as usize).min(VAULT_SUBS) {
        if v.active >> i & 1 == 0 {
            continue;
        }
        match a.iter().find(|x| key(x) == v.subs[i]) {
            Some(x) => {
                let u: &mut User = load(x, pid, DISC_USER, USER_SIZE)?;
                let w = sub_worth(pid, u, &v.subs[i], a, nw)?;
                v.sub_eq[i] = i64c(w)?;
                v.sub_slot[i] = nw.0;
                nav += w;
            }
            None => {
                if v.sub_slot[i] == 0 || nw.0 > v.sub_slot[i].saturating_add(spec::VAULT_VALUE_MAX_AGE_SLOTS) {
                    return Err(err(E::Stale));
                }
                nav += v.sub_eq[i] as i128;
            }
        }
    }
    Ok(nav)
}

/// Permissionless: value one sub-account and remember it, so a vault with more accounts than a transaction can carry can
/// still be valued, a part at a time. accs: vault(w), sub(w), [its markets, the books it quotes on, shards where it rests orders]
pub(crate) fn vault_value(pid: &Address, a: &mut [AccountView]) -> ProgramResult {
    if a.len() < 2 {
        return Err(err(E::BadData));
    }
    let v: &mut Vault = load(&a[0], pid, DISC_VAULT, VAULT_SIZE)?;
    let sk = key(&a[1]);
    let si = vault_sub(v, &sk)?;
    let u: &mut User = load(&a[1], pid, DISC_USER, USER_SIZE)?;
    let clk = clock()?;
    let nw: Now = (clk.slot, clk.unix_timestamp);
    let w = sub_worth(pid, u, &sk, &a[2..], nw)?;
    v.sub_eq[si] = i64c(w)?;
    v.sub_slot[si] = clk.slot;
    // an account with nothing in it is no part of the value (and needs no upkeep) until cash, margin or a position reaches it again
    if si != 0 && u.cash == 0 && u.pos.iter().all(|p| p.market == [0; 32]) {
        v.active &= !(1 << si);
    } else {
        v.active |= 1 << si;
    }
    Ok(())
}

/// Remember the value, track the peak, and stop new takeovers past the drawdown limit.
pub(crate) fn vault_mark(v: &mut Vault, nav: i128, slot: u64) -> ProgramResult {
    v.nav = i64c(nav)?;
    v.nav_slot = slot;
    if v.nav > v.peak_nav {
        v.peak_nav = v.nav;
    }
    if v.dd_bps != 0 && (v.nav as i128) * 10000 < v.peak_nav as i128 * (10000 - v.dd_bps as i128) {
        v.halted = 1;
    }
    Ok(())
}

pub(crate) const VV: i128 = spec::VAULT_VIRTUAL as i128;

/// accs: owner(s), vault(w), depositor(w, zeroed or own), subs[0](w), market(r), token vault(w), src(w), token program, [active subs, markets]
/// data: tag, amount
pub(crate) fn vault_deposit(pid: &Address, a: &mut [AccountView], d: &[u8]) -> ProgramResult {
    if a.len() < 8 || !sg(pid, &a[0]) {
        return Err(err(E::BadData));
    }
    let amt = rd64(d, 1)?;
    if amt == 0 {
        return Err(err(E::BadData));
    }
    let v: &mut Vault = load(&a[1], pid, DISC_VAULT, VAULT_SIZE)?;
    let m: &Market = load(&a[4], pid, DISC_MARKET, MARKET_SIZE)?;
    if v.n_subs == 0
        || key(&a[3]) != v.subs[0]
        || key(&a[5]) != v.token_vault
        || m.vault != v.token_vault
        || v.halted != 0
    {
        return Err(err(E::BadAccount));
    }
    if a[7].address() != &pinocchio_token::ID {
        return Err(err(E::TokenProgram));
    }
    if m.paused != 0 {
        return Err(err(E::Paused));
    }
    if m.tvl_cap != 0 && rd64(unsafe { a[5].borrow_unchecked() }, 64)?.saturating_add(amt) > m.tvl_cap {
        return Err(err(E::Cap));
    }
    if !a[2].owned_by(pid) || a[2].data_len() < VDEP_SIZE {
        return Err(err(E::BadAccount));
    }
    let vd = unsafe { &mut *(a[2].borrow_unchecked().as_ptr() as *mut VaultDeposit) };
    let (vk, ok) = (key(&a[1]), key(&a[0]));
    if vd.disc == 0 {
        vd.disc = DISC_VDEP;
        vd.vault = vk;
        vd.owner = ok;
    } else if vd.disc != DISC_VDEP || vd.vault != vk || vd.owner != ok {
        return Err(err(E::BadAccount));
    }
    let clk = clock()?;
    let nw: Now = (clk.slot, clk.unix_timestamp);
    let nav = vault_nav(pid, v, &a[3..], nw)?;
    if nav + VV <= 0 {
        return Err(err(E::Halted));
    }
    let minted = amt as i128 * (v.shares as i128 + VV) / (nav + VV);
    if minted <= 0 {
        return Err(err(E::BadData));
    }
    pinocchio_token::instructions::Transfer::new(&a[6], &a[5], &a[0], amt).invoke_with_program(a[7].address())?;
    let s0: &mut User = load(&a[3], pid, DISC_USER, USER_SIZE)?;
    s0.cash = i64c(s0.cash as i128 + amt as i128)?;
    v.sub_eq[0] = i64c(v.sub_eq[0] as i128 + amt as i128)?;
    v.shares += u64::try_from(minted).map_err(|_| err(E::Math))?;
    vd.shares += minted as u64;
    vd.basis = i64c(vd.basis as i128 + amt as i128)?;
    vd.unlock_slot = clk.slot + v.lockup_slots;
    vault_mark(v, nav + amt as i128, clk.slot)
}

/// accs: owner(s), vault(w), depositor(w), subs[0](w), market(w when a performance fee is due, else r), token vault(w), dst(w), token program, vault pda, [active subs, markets, books]
/// data: tag, shares
pub(crate) fn vault_withdraw(pid: &Address, a: &mut [AccountView], d: &[u8]) -> ProgramResult {
    if a.len() < 9 || !sg(pid, &a[0]) {
        return Err(err(E::BadData));
    }
    let sh = rd64(d, 1)?;
    let v: &mut Vault = load(&a[1], pid, DISC_VAULT, VAULT_SIZE)?;
    let vd: &mut VaultDeposit = load(&a[2], pid, DISC_VDEP, VDEP_SIZE)?;
    // held shared and read once: the valuation below reads this market again through the account list. The fee is booked through
    // a borrow of its own further down.
    let m: &Market = load(&a[4], pid, DISC_MARKET, MARKET_SIZE)?;
    let (m_vault, m_bump, m_seed) = (m.vault, m.vault_bump, m.vault_seed);
    if vd.vault != key(&a[1])
        || vd.owner != key(&a[0])
        || key(&a[3]) != v.subs[0]
        || key(&a[5]) != v.token_vault
        || m_vault != v.token_vault
    {
        return Err(err(E::BadAccount));
    }
    if a[7].address() != &pinocchio_token::ID {
        return Err(err(E::TokenProgram));
    }
    if sh == 0 || sh > vd.shares {
        return Err(err(E::BadData));
    }
    let clk = clock()?;
    let nw: Now = (clk.slot, clk.unix_timestamp);
    if clk.slot < vd.unlock_slot {
        return Err(err(E::Locked));
    }
    let nav = vault_nav(pid, v, &a[3..], nw)?;
    let gross = sh as i128 * (nav + VV) / (v.shares as i128 + VV);
    let cost = vd.basis as i128 * sh as i128 / vd.shares as i128;
    // The performance fee is the protocol's: it leaves the vault for the market's fee ledger (so the market must be passed
    // writable whenever there is a gain to charge). It used to stay in the vault, a gift to the holders who remained.
    let fee = if gross > cost { (gross - cost) * v.perf_bps as i128 / 10000 } else { 0 };
    let out = gross - fee;
    if out <= 0 {
        return Err(err(E::BadData));
    }
    if v.day_cap_bps != 0 {
        if clk.slot >= v.day_start + spec::SLOTS_PER_DAY {
            v.day_start = clk.slot;
            v.day_out = 0;
        }
        if (v.day_out as i128 + out) * 10000 > nav * v.day_cap_bps as i128 {
            return Err(err(E::Cap));
        }
        v.day_out = i64c(v.day_out as i128 + out)?;
    }
    let s0: &mut User = load(&a[3], pid, DISC_USER, USER_SIZE)?;
    if out + fee > s0.cash as i128 {
        return Err(err(E::Margin));
    }
    s0.cash = i64c(s0.cash as i128 - out - fee)?;
    v.sub_eq[0] = i64c(v.sub_eq[0] as i128 - out - fee)?;
    if fee > 0 {
        let mw: &mut Market = load(&a[4], pid, DISC_MARKET, MARKET_SIZE)?;
        mw.fees = i64c(mw.fees as i128 + fee)?;
    }
    let (eq, im, _) = health_im(pid, s0, &a[3..], nw, true)?;
    if eq < im {
        return Err(err(E::Margin));
    }
    v.shares -= sh;
    vd.shares -= sh;
    vd.basis = i64c(vd.basis as i128 - cost)?;
    vault_mark(v, nav - out - fee, clk.slot)?;
    let bump = [m_bump as u8];
    let seeds = [Seed::from(b"vault".as_slice()), Seed::from(&m_seed), Seed::from(&bump)];
    pinocchio_token::instructions::Transfer::new(&a[5], &a[6], &a[8], out as u64)
        .invoke_signed_with_program(&[Signer::from(&seeds)], a[7].address())
}

/// Permissionless: the vault takes over a position the book could not close, at the mark.
/// accs: vault(w), sub(w), target(w), market(r), insurance(w), link(r), target shards(w)..., [other markets]. data: tag, max_qty
pub(crate) fn vault_takeover(pid: &Address, a: &mut [AccountView], d: &[u8]) -> ProgramResult {
    if a.len() < 6 {
        return Err(err(E::BadData));
    }
    let v: &mut Vault = load(&a[0], pid, DISC_VAULT, VAULT_SIZE)?;
    let l: &VaultLink = load(&a[5], pid, DISC_VLINK, VLINK_SIZE)?;
    let m: &Market = load(&a[3], pid, DISC_MARKET, MARKET_SIZE)?;
    let (vk, mk, sk) = (key(&a[0]), key(&a[3]), key(&a[1]));
    if v.halted != 0 {
        return Err(err(E::Halted));
    }
    if l.vault != vk || l.market != mk {
        return Err(err(E::BadAccount));
    }
    let si = v.subs[..v.n_subs as usize].iter().position(|k| *k == sk).ok_or(err(E::BadAccount))?;
    liquidate_core(pid, a, d, true)?;
    v.active |= 1 << si;
    let u: &User = load(&a[1], pid, DISC_USER, USER_SIZE)?;
    if let Some(p) = u.pos.iter().find(|p| p.market == mk) {
        // the inventory cap is judged on the live NAV (every active sub passed with its markets), never a stored one
        let clk = clock()?;
        let nw: Now = (clk.slot, clk.unix_timestamp);
        let nav = vault_nav(pid, v, &a[1..], nw)?;
        if mulf((p.base as i128).abs(), mark(m)) * 10000 > nav.max(0) * l.cap_bps as i128 {
            return Err(err(E::Cap));
        }
        vault_mark(v, nav, clk.slot)?;
        if v.halted != 0 {
            return Err(err(E::Halted));
        }
    }
    Ok(())
}

/// Permissionless: refresh value, peak and the drawdown stop. accs: vault(w), [active subs with their markets and books; a
/// sub left out counts at its remembered value while that is recent]
pub(crate) fn vault_crank(pid: &Address, a: &mut [AccountView]) -> ProgramResult {
    if a.len() < 2 {
        return Err(err(E::BadData));
    }
    let v: &mut Vault = load(&a[0], pid, DISC_VAULT, VAULT_SIZE)?;
    let clk = clock()?;
    let nw: Now = (clk.slot, clk.unix_timestamp);
    let nav = vault_nav(pid, v, &a[1..], nw)?;
    vault_mark(v, nav, clk.slot)
}

/// accs: admin(s), vault(w). data: tag, lockup_slots, day_cap_bps, dd_bps, halted
pub(crate) fn vault_set(pid: &Address, a: &mut [AccountView], d: &[u8]) -> ProgramResult {
    // data: tag, lockup, daily cap, drawdown, halted, [performance fee bps]
    if a.len() < 2 || (d.len() != 33 && d.len() != 41) {
        return Err(err(E::BadData));
    }
    let v: &mut Vault = load(&a[1], pid, DISC_VAULT, VAULT_SIZE)?;
    if !auth(pid, &v.admin, a) {
        return Err(err(E::BadAccount));
    }
    v.lockup_slots = rd64(d, 1)?;
    v.day_cap_bps = rd64(d, 9)?;
    v.dd_bps = rd64(d, 17)?;
    v.halted = rd64(d, 25)?;
    if d.len() == 41 {
        v.perf_bps = rd64(d, 33)?;
    }
    if v.day_cap_bps > 10000 || v.dd_bps > 10000 || v.perf_bps > 5000 {
        return Err(err(E::BadData));
    }
    Ok(())
}

// ---------- the vault as a maker ----------
// Depositors' money quotes the markets: a sub-account of the vault is the maker. Its slab's quoting authority is the vault
// account itself, which can sign nothing, so slab_quote / slab_kill / fast_join never apply and these instructions are the
// only way in. Who may do what:
//   vault admin   makes the slab, approves it on a book (with the market admin), locks margin, names the operator and the rules
//   operator      writes quotes inside the rules, stops them, releases margin, moves idle cash between the vault's own accounts
//   anyone        settles fills (slab_settle), stops the quotes of a halted vault
// Nothing here moves value out of the vault; the quote rules bound what a quote can give away.
pub(crate) fn vault_sub(v: &Vault, k: &Key) -> Result<usize, ProgramError> {
    v.subs[..(v.n_subs as usize).min(VAULT_SUBS)].iter().position(|x| x == k).ok_or(err(E::BadAccount))
}

pub(crate) fn vault_op(pid: &Address, v: &Vault, x: &AccountView) -> bool {
    v.operator != [0; 32] && sg(pid, x) && key(x) == v.operator
}

/// accs: admin(s), vault(w), sub(r), slab(w, zeroed)
pub(crate) fn vault_slab_init(pid: &Address, a: &mut [AccountView]) -> ProgramResult {
    if a.len() < 4 {
        return Err(err(E::BadData));
    }
    let v: &mut Vault = load(&a[1], pid, DISC_VAULT, VAULT_SIZE)?;
    if !auth(pid, &v.admin, a) {
        return Err(err(E::BadAccount));
    }
    let si = vault_sub(v, &key(&a[2]))?;
    let _u: &User = load(&a[2], pid, DISC_USER, USER_SIZE)?;
    if !a[3].owned_by(pid) || a[3].data_len() < SLAB_SIZE || disc_of(&a[3]) != 0 {
        return Err(err(E::BadAccount));
    }
    let s = unsafe { &mut *(a[3].borrow_unchecked().as_ptr() as *mut Slab) };
    s.disc = DISC_SLAB;
    s.authority = key(&a[1]);
    s.user = key(&a[2]);
    v.active |= 1 << si; // a quoting account is part of the vault's value from now on
    Ok(())
}

/// accs: admin(s), market(r), fast(w), slab(w), vault(r), [more signers / the multisig accounts]
pub(crate) fn vault_join(pid: &Address, a: &mut [AccountView]) -> ProgramResult {
    if a.len() < 5 {
        return Err(err(E::BadData));
    }
    let m: &Market = load(&a[1], pid, DISC_MARKET, MARKET_SIZE)?;
    let f: &mut FastBook = load(&a[2], pid, DISC_FAST, FAST_SIZE)?;
    let s: &mut Slab = load(&a[3], pid, DISC_SLAB, SLAB_SIZE)?;
    let v: &Vault = load(&a[4], pid, DISC_VAULT, VAULT_SIZE)?;
    if !auth(pid, &m.admin, a) || !auth(pid, &v.admin, a) || f.market != key(&a[1]) {
        return Err(err(E::BadAccount));
    }
    if s.authority != key(&a[4]) || m.vault != v.token_vault {
        return Err(err(E::BadAccount));
    }
    vault_sub(v, &s.user)?;
    join_core(f, s, key(&a[3]), key(&a[1]))
}

/// One ladder of a vault slab is inside the vault's rules. A ladder offering nothing passes.
pub(crate) fn vault_ladder_ok(v: &Vault, s: &Slab, lad: usize) -> bool {
    let (l, ask) = (&s.lad[lad], lad & 1 == 1);
    let q = &s.quotes[lad * SLAB_LEVELS..(lad + 1) * SLAB_LEVELS];
    if q.iter().all(|x| x.qty == 0) {
        return true;
    }
    if l.flags & spec::LADDER_PEG == 0 {
        return false;
    }
    if v.q_fresh & 1 != 0 && l.flags >> spec::LADDER_FRESH_SHIFT == 0 {
        return false;
    }
    if v.q_fresh & 2 != 0 && l.flags & spec::LADDER_NEXT == 0 {
        return false;
    }
    let mut edge = 0i64;
    if v.q_min_edge_bps != 0 {
        if l.ref_px == 0 || l.tol_bps == 0 || (v.q_max_tol_bps != 0 && l.tol_bps as u64 > v.q_max_tol_bps) {
            return false;
        }
        edge = ((l.ref_px as u64 * v.q_min_edge_bps + 9999) / 10000) as i64;
    }
    q.iter().all(|x| {
        let off = x.px as i64 - spec::QUOTE_PEG_BIAS as i64;
        x.qty == 0 || if ask { off >= edge } else { off <= -edge }
    })
}

/// accs: operator(s), vault(r), slab(w). data: tag, items (as slab_quote)
pub(crate) fn vault_quote(pid: &Address, a: &mut [AccountView], d: &[u8]) -> ProgramResult {
    if a.len() < 3 {
        return Err(err(E::BadAccount));
    }
    let v: &Vault = load(&a[1], pid, DISC_VAULT, VAULT_SIZE)?;
    let s: &mut Slab = load(&a[2], pid, DISC_SLAB, SLAB_SIZE)?;
    if !vault_op(pid, v, &a[0]) || s.authority != key(&a[1]) {
        return Err(err(E::BadAccount));
    }
    if v.halted != 0 {
        return Err(err(E::Halted));
    }
    quote_core(s, d)?;
    // every ladder an item named (its settings or one of its levels) is judged as it stands after the write
    let mut prev = usize::MAX;
    for it in d[1..].chunks_exact(spec::QUOTE_ITEM_SIZE) {
        let i = u16::from_le_bytes([it[0], it[1]]) as usize;
        let lad = if i & 0x8000 != 0 { i & 0x3fff } else { i / SLAB_LEVELS };
        if lad == prev {
            continue;
        }
        if lad >= SLAB_LADDERS || !vault_ladder_ok(v, s, lad) {
            return Err(err(E::BadData));
        }
        prev = lad;
    }
    Ok(())
}

/// accs: signer(s), sub(w), market(r), fast(w), vault(w), [other markets]. data: tag, amount i64, [target i64]
pub(crate) fn vault_fund(pid: &Address, a: &mut [AccountView], d: &[u8]) -> ProgramResult {
    if a.len() < 5 {
        return Err(err(E::BadData));
    }
    let v: &mut Vault = load(&a[4], pid, DISC_VAULT, VAULT_SIZE)?;
    let u: &User = load(&a[1], pid, DISC_USER, USER_SIZE)?;
    let f: &FastBook = load(&a[3], pid, DISC_FAST, FAST_SIZE)?;
    let (vk, uk) = (key(&a[4]), key(&a[1]));
    if u.authority != vk {
        return Err(err(E::BadAccount));
    }
    let si = vault_sub(v, &uk)?;
    let amt = rd64(d, 1)? as i64;
    let tgt = if d.len() > 9 { Some(rd64(d, 9)? as i64) } else { None };
    let cur =
        f.makers[..(f.n as usize).min(FAST_MAKERS)].iter().find(|x| x.user == uk).ok_or(err(E::BadAccount))?.target;
    let more = amt > 0 || tgt.is_some_and(|t| t > cur);
    // more quoting margin is the admin's call, and never while the vault is halted; less is also the operator's
    if !auth(pid, &v.admin, a) && (more || !vault_op(pid, v, &a[0])) {
        return Err(err(E::BadAccount));
    }
    if more && v.halted != 0 {
        return Err(err(E::Halted));
    }
    v.active |= 1 << si;
    fund_core(pid, a, d)
}

/// accs: signer, vault(r), slab(w). data: tag, on u8
pub(crate) fn vault_kill(pid: &Address, a: &mut [AccountView], d: &[u8]) -> ProgramResult {
    if a.len() < 3 || d.len() < 2 {
        return Err(err(E::BadData));
    }
    let v: &Vault = load(&a[1], pid, DISC_VAULT, VAULT_SIZE)?;
    let s: &mut Slab = load(&a[2], pid, DISC_SLAB, SLAB_SIZE)?;
    if s.authority != key(&a[1]) {
        return Err(err(E::BadAccount));
    }
    let trusted = auth(pid, &v.admin, a) || vault_op(pid, v, &a[0]);
    if !trusted && !(v.halted != 0 && d[1] != 0) {
        return Err(err(E::BadAccount));
    }
    s.kill = (d[1] != 0) as u64;
    Ok(())
}

/// accs: admin(s), vault(w). data: tag, operator [32], min_edge_bps, max_tol_bps, fresh
pub(crate) fn vault_operator(pid: &Address, a: &mut [AccountView], d: &[u8]) -> ProgramResult {
    if a.len() < 2 || d.len() != 57 {
        return Err(err(E::BadData));
    }
    let v: &mut Vault = load(&a[1], pid, DISC_VAULT, VAULT_SIZE)?;
    if !auth(pid, &v.admin, a) {
        return Err(err(E::BadAccount));
    }
    let (edge, tol) = (rd64(d, 33)?, rd64(d, 41)?);
    // An edge is measured on the ladder's reference price, so it is only worth what the tolerance lets the reference stray:
    // with an edge the tolerance cap is required and bounded.
    if edge > spec::VAULT_MAX_EDGE_BPS || tol > spec::VAULT_MAX_TOL_BPS || (edge != 0 && tol == 0) {
        return Err(err(E::BadData));
    }
    v.operator.copy_from_slice(&d[1..33]);
    v.q_min_edge_bps = edge;
    v.q_max_tol_bps = tol;
    v.q_fresh = rd64(d, 49)? & 3; // bit 0: fresh signed price, bit 1: next-price ladders only
    Ok(())
}

/// accs: signer(s), vault(w), from sub(w), to sub(w), [from's markets]. data: tag, amount u64
pub(crate) fn vault_move(pid: &Address, a: &mut [AccountView], d: &[u8]) -> ProgramResult {
    if a.len() < 4 {
        return Err(err(E::BadData));
    }
    let amt = i64c(rd64(d, 1)? as i128)?;
    let v: &mut Vault = load(&a[1], pid, DISC_VAULT, VAULT_SIZE)?;
    if !auth(pid, &v.admin, a) && !vault_op(pid, v, &a[0]) {
        return Err(err(E::BadAccount));
    }
    let (fk, tk) = (key(&a[2]), key(&a[3]));
    let (fi, ti) = (vault_sub(v, &fk)?, vault_sub(v, &tk)?);
    if fi == ti || amt <= 0 {
        return Err(err(E::BadData));
    }
    let from: &mut User = load(&a[2], pid, DISC_USER, USER_SIZE)?;
    let to: &mut User = load(&a[3], pid, DISC_USER, USER_SIZE)?;
    if from.home_vault != to.home_vault {
        return Err(err(E::Vault));
    }
    let clk = clock()?;
    let nw: Now = (clk.slot, clk.unix_timestamp);
    settle_funding_all(pid, from, &a[4..])?;
    // judged on everything the giving account owes: fills not settled or claimed yet could hide a loss its cash still has to carry
    complete_view(pid, from, &fk, &a[4..], None)?;
    from.cash = i64c(from.cash as i128 - amt as i128)?;
    to.cash = i64c(to.cash as i128 + amt as i128)?;
    // the giving account keeps what its own positions need; the receiving one is counted in the vault's value from now on
    let (eq, im, _) = health_im(pid, from, &a[4..], nw, true)?;
    if from.cash < 0 || eq < im {
        return Err(err(E::Margin));
    }
    v.active |= (1 << fi) | (1 << ti);
    // the remembered values move with the cash: a valuation that takes one account live and the other from memory must
    // neither lose the money nor count it twice
    v.sub_eq[fi] = i64c(v.sub_eq[fi] as i128 - amt as i128)?;
    v.sub_eq[ti] = i64c(v.sub_eq[ti] as i128 + amt as i128)?;
    Ok(())
}
