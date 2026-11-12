//! Creating the program's accounts. Every account is allocated by the client (zeroed, owned by the program) and claimed
//! here by writing its discriminator, so an account can be initialised exactly once and never re-typed.

use super::*;

/// accs: admin(s), market(w), insurance(w), vault token acct, oracle
/// data: tag, vault_bump u8, feed_id[32], bucket_width, max_staleness, max_conf_bps, max_age, expo(i64), px_mul, px_div, max_oi
pub(crate) fn init_market(pid: &Address, a: &mut [AccountView], d: &[u8]) -> ProgramResult {
    if a.len() < 5 || !sg(pid, &a[0]) || d.len() < 2 + 32 + 64 {
        return Err(err(E::BadData));
    }
    if !a[1].owned_by(pid) || a[1].data_len() < MARKET_SIZE || disc_of(&a[1]) != 0 {
        return Err(err(E::BadAccount));
    }
    if !a[2].owned_by(pid) || a[2].data_len() < INS_SIZE || disc_of(&a[2]) != 0 {
        return Err(err(E::BadAccount));
    }
    // vault: SPL token account whose owner must be PDA ["vault", seed, bump]; seed = market, or the pool
    // (optional a[5] pool(w) + a[6] pool admin signer: joining a shared vault needs the pool admin -- AUDIT C-2/P-1)
    let mk = key(&a[1]);
    let (seed, bump, pool) = if a.len() >= 7 {
        let p: &mut Pool = load(&a[5], pid, DISC_POOL, POOL_SIZE)?;
        if !auth(pid, &p.admin, &a[6..]) || p.vault != key(&a[3]) {
            return Err(err(E::BadAccount));
        }
        // AUDIT N-6: admission is timelocked (pool_propose) and the market must carry an OI cap within the pool's limit
        let slot = clock()?.slot;
        if p.pending_market != mk || slot < p.pending_eta {
            return Err(err(E::Timelock));
        }
        let max_oi = rd64(d, 2 + 32 + 56)?;
        if max_oi == 0 || (p.max_market_oi != 0 && max_oi > p.max_market_oi) {
            return Err(err(E::Cap));
        }
        p.pending_market = [0; 32];
        p.pending_eta = 0;
        p.n_markets += 1;
        (key(&a[5]), p.vault_bump as u8, key(&a[5]))
    } else {
        (mk, d[1], [0u8; 32])
    };
    let pool_admin = if pool != [0; 32] { Some(load::<Pool>(&a[5], pid, DISC_POOL, POOL_SIZE)?.admin) } else { None };
    let bb = [bump];
    let pda = Address::create_program_address(&[b"vault", &seed, &bb], pid).map_err(|_| err(E::BadAccount))?;
    if !a[3].owned_by(&pinocchio_token::ID) || a[3].data_len() < 165 {
        return Err(err(E::BadAccount));
    }
    let vd = unsafe { a[3].borrow_unchecked() };
    if vd[32..64] != pda.to_bytes() {
        return Err(err(E::BadAccount));
    }
    let m = unsafe { &mut *(a[1].borrow_unchecked().as_ptr() as *mut Market) };
    m.pool = pool;
    m.vault_seed = seed;
    // AUDIT V5-4: a pooled market's admin is pinned to the pool's admin (may be a timelocked m-of-n multisig), so
    // depositors trust one key set, not every admitted market's creator.
    m.disc = DISC_MARKET;
    m.admin = pool_admin.unwrap_or(key(&a[0]));
    m.insurance = key(&a[2]);
    m.vault = key(&a[3]);
    m.oracle = key(&a[4]);
    m.mint.copy_from_slice(&vd[0..32]);
    m.vault_bump = bump as u64;
    m.feed_id.copy_from_slice(&d[2..34]);
    let mut o = 34;
    let mut nx = || {
        let v = rd64(d, o);
        o += 8;
        v
    };
    m.bucket_width = nx()?;
    m.max_staleness_slots = nx()?;
    m.max_conf_bps = nx()?;
    m.max_age_secs = nx()?;
    m.expo = nx()? as i64;
    m.px_mul = nx()? as i64;
    m.px_div = nx()? as i64;
    m.max_oi = nx()?;
    m.imr_bps = IMR_BPS;
    m.mmr_bps = MMR_BPS;
    if m.bucket_width == 0 || m.px_div <= 0 || m.px_mul <= 0 {
        return Err(err(E::BadData));
    }
    let s0 = clock()?.slot;
    m.funding_slot = s0;
    m.period_start = s0;
    m.funding_period = FUNDING_PERIOD_SLOTS;
    m.impact_notional = FUNDING_IMPACT_NOTIONAL;
    let ins = unsafe { &mut *(a[2].borrow_unchecked().as_ptr() as *mut Insurance) };
    ins.disc = DISC_INS;
    ins.market = mk;
    ins.balance = 0;
    Ok(())
}

/// Pooled collateral (cross-margin): one SPL vault owned by PDA ["vault", pool, bump] shared by every market
/// the pool admin admits. accs: admin(s), pool(w), vault token acct. data: tag, bump
pub(crate) fn init_pool(pid: &Address, a: &mut [AccountView], d: &[u8]) -> ProgramResult {
    if a.len() < 3 || !sg(pid, &a[0]) || d.len() < 2 {
        return Err(err(E::BadData));
    }
    if !a[1].owned_by(pid) || a[1].data_len() < POOL_SIZE || disc_of(&a[1]) != 0 {
        return Err(err(E::BadAccount));
    }
    let pk = key(&a[1]);
    let bb = [d[1]];
    let pda = Address::create_program_address(&[b"vault", &pk, &bb], pid).map_err(|_| err(E::BadAccount))?;
    if !a[2].owned_by(&pinocchio_token::ID) || a[2].data_len() < 165 {
        return Err(err(E::BadAccount));
    }
    let vd = unsafe { a[2].borrow_unchecked() };
    if vd[32..64] != pda.to_bytes() {
        return Err(err(E::BadAccount));
    }
    let admin = admin_key(pid, a, &key(&a[0]), if d.len() >= 42 { Some(&d[10..42]) } else { None })?;
    let p = unsafe { &mut *(a[1].borrow_unchecked().as_ptr() as *mut Pool) };
    p.disc = DISC_POOL;
    p.admin = admin;
    p.vault = key(&a[2]);
    p.vault_bump = d[1] as u64;
    p.mint.copy_from_slice(&vd[0..32]);
    p.max_market_oi = if d.len() >= 10 { rd64(d, 2)? } else { 0 };
    Ok(())
}

/// AUDIT N-6: pool admin proposes a market for admission; it can join after TIMELOCK_SLOTS.
/// accs: pool(w), [admin signer(s) / multisig + cosigners]. data: tag, market key[32]
pub(crate) fn pool_propose(pid: &Address, a: &mut [AccountView], d: &[u8]) -> ProgramResult {
    if a.is_empty() || d.len() < 33 {
        return Err(err(E::BadData));
    }
    let p: &mut Pool = load(&a[0], pid, DISC_POOL, POOL_SIZE)?;
    if !auth(pid, &p.admin, &a[1..]) {
        return Err(err(E::BadAccount));
    }
    p.pending_market.copy_from_slice(&d[1..33]);
    p.pending_eta = clock()?.slot + spec::TIMELOCK_SLOTS;
    Ok(())
}

/// m-of-n admin account. accs: payer(s), multisig(w, fresh). data: tag, m u8, n u8, keys[n]
pub(crate) fn init_multisig(pid: &Address, a: &mut [AccountView], d: &[u8]) -> ProgramResult {
    if a.len() < 2 || !sg(pid, &a[0]) || d.len() < 3 {
        return Err(err(E::BadData));
    }
    let (mm, n) = (d[1] as usize, d[2] as usize);
    if n == 0 || n > 8 || mm == 0 || mm > n || d.len() < 3 + 32 * n {
        return Err(err(E::BadData));
    }
    if !a[1].owned_by(pid) || a[1].data_len() < MULTISIG_SIZE || disc_of(&a[1]) != 0 {
        return Err(err(E::BadAccount));
    }
    let ms = unsafe { &mut *(a[1].borrow_unchecked().as_ptr() as *mut Multisig) };
    for i in 0..n {
        ms.keys[i].copy_from_slice(&d[3 + 32 * i..35 + 32 * i]);
        for j in 0..i {
            if ms.keys[j] == ms.keys[i] {
                return Err(err(E::BadData));
            }
        }
    }
    ms.disc = DISC_MULTISIG;
    ms.m = mm as u64;
    ms.n = n as u64;
    Ok(())
}

/// accs: admin(s), market, shard(w). data: tag, side u8, lo u64
pub(crate) fn init_shard(pid: &Address, a: &mut [AccountView], d: &[u8]) -> ProgramResult {
    if a.len() < 3 || !sg(pid, &a[0]) || d.len() < 10 {
        return Err(err(E::BadData));
    }
    let m: &mut Market = load(&a[1], pid, DISC_MARKET, MARKET_SIZE)?;
    if !auth(pid, &m.admin, a) {
        return Err(err(E::BadAccount));
    }
    if !a[2].owned_by(pid) || a[2].data_len() < SHARD_SIZE || disc_of(&a[2]) != 0 {
        return Err(err(E::BadAccount));
    }
    if m.n_shards >= MAX_SHARD_NO {
        return Err(err(E::Cap));
    } // AUDIT L-3: shard_no fits the id high bits
    let lo = rd64(d, 2)?;
    if lo % m.bucket_width != 0 || d[1] > 1 {
        return Err(err(E::BadData));
    }
    // a side holds at most what one take can walk (keepers and liquidators must pass every opposite shard), 64 in all
    // (complete views of a market)
    if m.n_side[d[1] as usize] >= MAX_TAKE_SHARDS as u64 || m.n_shards >= 64 {
        return Err(err(E::Cap));
    }
    let s = unsafe { &mut *(a[2].borrow_unchecked().as_ptr() as *mut Shard) };
    s.disc = DISC_SHARD;
    s.market = key(&a[1]);
    s.side = d[1] as u64;
    s.lo = lo;
    s.width = m.bucket_width;
    s.shard_no = m.n_shards;
    m.n_shards += 1;
    m.n_side[d[1] as usize] += 1;
    Ok(())
}

/// Move a shard to another price range. Only an empty one: no resting order (n) and no fill waiting to be claimed (nled),
/// so nothing anyone owns is in it and the move can strand nothing. Its side, its number (the high bits of order ids), its
/// order sequence and its accumulators stay, so the market's counts (n_shards, n_side) and every complete view of the market
/// are exactly what they were, and an order id is never issued twice. Why it exists: a shard covers [lo, lo + width), a
/// price that walks away leaves a market's shards behind it, and the only remedy was new shards: rent each time, 64 a
/// market at most, and no instruction that closes one. Same authority as init_shard, and like it immediate: it changes
/// where new orders can rest, never an order that exists.
/// accs: admin(s), market, shard(w). data: tag, lo u64
pub(crate) fn shard_move(pid: &Address, a: &mut [AccountView], d: &[u8]) -> ProgramResult {
    if a.len() < 3 || !sg(pid, &a[0]) || d.len() < 9 {
        return Err(err(E::BadData));
    }
    let m: &mut Market = load(&a[1], pid, DISC_MARKET, MARKET_SIZE)?;
    if !auth(pid, &m.admin, a) {
        return Err(err(E::BadAccount));
    }
    let s: &mut Shard = load(&a[2], pid, DISC_SHARD, SHARD_SIZE)?;
    if s.market != key(&a[1]) {
        return Err(err(E::BadAccount));
    }
    let lo = rd64(d, 1)?;
    if lo % m.bucket_width != 0 {
        return Err(err(E::BadData));
    }
    if s.n != 0 || s.nled != 0 {
        return Err(err(E::Locked));
    }
    s.lo = lo;
    s.width = m.bucket_width;
    Ok(())
}

/// accs: authority(s), user(w)
pub(crate) fn init_user(pid: &Address, a: &mut [AccountView]) -> ProgramResult {
    if a.len() < 2 || !sg(pid, &a[0]) {
        return Err(err(E::BadData));
    }
    if !a[1].owned_by(pid) || a[1].data_len() < USER_SIZE || disc_of(&a[1]) != 0 {
        return Err(err(E::BadAccount));
    }
    let u = unsafe { &mut *(a[1].borrow_unchecked().as_ptr() as *mut User) };
    u.disc = DISC_USER;
    u.authority = key(&a[0]);
    Ok(())
}

/// accs: admin(s), market(w), rank(w)
pub(crate) fn init_rank(pid: &Address, a: &mut [AccountView]) -> ProgramResult {
    if a.len() < 3 {
        return Err(err(E::BadData));
    }
    let m: &mut Market = load(&a[1], pid, DISC_MARKET, MARKET_SIZE)?;
    if !auth(pid, &m.admin, a) || m.adl_rank != [0; 32] {
        return Err(err(E::BadAccount));
    }
    if !a[2].owned_by(pid) || a[2].data_len() < RANK_SIZE || disc_of(&a[2]) != 0 {
        return Err(err(E::BadAccount));
    }
    let r = unsafe { &mut *(a[2].borrow_unchecked().as_ptr() as *mut Rank) };
    r.disc = DISC_RANK;
    r.market = key(&a[1]);
    m.adl_rank = key(&a[2]);
    Ok(())
}

/// accs: authority(s), user(r), slab(w, zeroed)
pub(crate) fn slab_init(pid: &Address, a: &mut [AccountView]) -> ProgramResult {
    if a.len() < 3 {
        return Err(err(E::BadData));
    }
    let u: &mut User = load(&a[1], pid, DISC_USER, USER_SIZE)?;
    if !sg(pid, &a[0]) || u.authority != key(&a[0]) {
        return Err(err(E::BadAccount));
    }
    if !a[2].owned_by(pid) || a[2].data_len() < SLAB_SIZE || disc_of(&a[2]) != 0 {
        return Err(err(E::BadAccount));
    }
    let s = unsafe { &mut *(a[2].borrow_unchecked().as_ptr() as *mut Slab) };
    s.disc = DISC_SLAB;
    s.authority = key(&a[0]);
    s.user = key(&a[1]);
    Ok(())
}

/// accs: admin(s), market(r), fast(w, zeroed). data: tag, idx u64, [side u8: 0 both, 1 bids, 2 asks]
pub(crate) fn fast_init(pid: &Address, a: &mut [AccountView], d: &[u8]) -> ProgramResult {
    if a.len() < 3 {
        return Err(err(E::BadData));
    }
    let m: &mut Market = load(&a[1], pid, DISC_MARKET, MARKET_SIZE)?;
    if !auth(pid, &m.admin, a) {
        return Err(err(E::BadAccount));
    }
    if !a[2].owned_by(pid) || a[2].data_len() < FAST_SIZE || disc_of(&a[2]) != 0 {
        return Err(err(E::BadAccount));
    }
    let idx = rd64(d, 1)?;
    let side = if d.len() > 9 { d[9] as u64 } else { 0 };
    if idx as usize >= SLAB_QUOTES / (2 * SLAB_LEVELS) || side > 2 {
        return Err(err(E::BadData));
    }
    register_book(m, &key(&a[2]), side)?;
    let f = unsafe { &mut *(a[2].borrow_unchecked().as_ptr() as *mut FastBook) };
    f.disc = DISC_FAST;
    f.market = key(&a[1]);
    f.idx = idx;
    f.side = side;
    Ok(())
}

/// One book per side: a both-sides book takes both slots, a side book its own. A slot already holding another book is refused.
pub(crate) fn register_book(m: &mut Market, fk: &Key, side: u64) -> ProgramResult {
    let slots: &[usize] = match side {
        0 => &[0, 1],
        1 => &[0],
        2 => &[1],
        _ => return Err(err(E::BadData)),
    };
    for &i in slots {
        if m.fast_book[i] != [0; 32] && m.fast_book[i] != *fk {
            return Err(err(E::BadAccount));
        }
    }
    for &i in slots {
        m.fast_book[i] = *fk;
    }
    Ok(())
}

/// Admin registers an existing book (made before the registry) as its market's book. accs: admin(s), market(w), fast(r)
pub(crate) fn fast_register(pid: &Address, a: &mut [AccountView]) -> ProgramResult {
    if a.len() < 3 {
        return Err(err(E::BadData));
    }
    let m: &mut Market = load(&a[1], pid, DISC_MARKET, MARKET_SIZE)?;
    let f: &FastBook = load(&a[2], pid, DISC_FAST, FAST_SIZE)?;
    if !auth(pid, &m.admin, a) || f.market != key(&a[1]) {
        return Err(err(E::BadAccount));
    }
    register_book(m, &key(&a[2]), f.side)
}

/// The book a taker on `opp` (1 = takes asks / buys, 0 = takes bids / sells) must use: the market's registered one.
#[inline(always)]
pub(crate) fn book_ok(m: &Market, fk: &Key, opp: usize) -> bool {
    m.fast_book[opp] != [0; 32] && m.fast_book[opp] == *fk
}
