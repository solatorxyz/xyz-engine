//! Full-onchain perp program (pinocchio, zero-copy). Matching, cross-margin, liquidation,
//! lazy funding, ADL. UNAUDITED. See ../README.md.
#![cfg_attr(target_os = "solana", no_std)]
#![allow(clippy::too_many_arguments)]
pub mod state;
#[path = "../../perp-core/src/spec.rs"]
#[allow(dead_code)]
pub mod spec;

use pinocchio::{error::ProgramError, sysvars::clock::Clock, AccountView, Address, ProgramResult};
#[cfg(target_os = "solana")]
use pinocchio::sysvars::Sysvar;
use pinocchio::cpi::{Seed, Signer};
use spec::{IX_PLACE, IX_BATCH, IX_CANCEL, IX_SET_ORACLE, IX_LIQUIDATE, IX_BACKSTOP, IMR_BPS, MMR_BPS, SIDE_BID, SIDE_ASK, TIERS,
    BATCH_HEADER_SIZE, BATCH_HEADER_N_CANCEL_OFF, BATCH_HEADER_N_PLACE_OFF, CANCEL_ITEM_SIZE, PLACE_ITEM_SIZE,
    FLAG_REDUCE_ONLY, FLAG_IOC, FLAG_POST_ONLY, LIQ_FEE_BPS, LIQ_BOOK_CHUNK_BPS, LIQ_BOOK_BAND_BPS, LIQ_BOOK_BAND_STEPS, LIQ_ADL_WINDOW_SLOTS,
    FLAG_FOK, FLAG_ALO_SLIDE, FLAG_STP_MASK, FLAG_STP_SHIFT, STP_CANCEL_OLDEST, STP_CANCEL_NEWEST, KIND_LIMIT, KIND_PEG, KIND_SPLINE,
    MAX_SPLINE_LEVELS, CANCEL_BY_CID_BIT, MAX_KEEPER_TIP, TRIGGER_KIND_TP, TRIGGER_KIND_SL, TRIGGER_KIND_TWAP,
    ADMIN_PAUSE, ADMIN_TVL_CAP, ADMIN_MAX_OI, EVENTS_FILL, EVENTS_LIQUIDATION, EVENTS_TRIGGER, EVENTS_ADL,
    PLACE_ITEM_KIND_OFF, PLACE_ITEM_PRICE_OFF, PLACE_ITEM_QTY_OFF, PLACE_ITEM_EXPIRY_SLOT_OFF, PLACE_ITEM_ORACLE_BAND_BPS_OFF,
    PLACE_ITEM_CID_OFF, PLACE_ITEM_A1_OFF, PLACE_ITEM_A2_OFF, PLACE_ITEM_A3_OFF, PLACE_ITEM_FLAGS_OFF, TICK_SIZE as TICK, MAX_OPEN_ORDERS_PER_MARKET, MIN_ORDER_NOTIONAL, MAX_PX, FUNDING_PPB, FUNDING_PERIOD_SLOTS, FUNDING_INTEREST_8H_PPB, FUNDING_PREMIUM_CLAMP_8H_PPB, FUNDING_MAX_RATE_HOURLY_PPB, FUNDING_IMPACT_NOTIONAL, FUNDING_HOURS_PER_8H, MAX_SHARD_NO, ORDER_SEQ_BITS};
// Compile-time conformance: zero-copy structs == shared spec layout (also used by the C++ decoder).
const _: () = {
    use core::mem::offset_of as off;
    assert!(MARKET_SIZE as u64 == spec::MARKET_SIZE && SHARD_SIZE as u64 == spec::SHARD_SIZE && USER_SIZE as u64 == spec::USER_SIZE);
    assert!(ORDER_SIZE as u64 == spec::ORDER_SIZE && off!(Shard, orders) as u64 == spec::SHARD_ORDERS_OFF && off!(Shard, ledger) as u64 == spec::SHARD_LEDGER_OFF);
    assert!(off!(Shard, n) as u64 == spec::SHARD_N_OFF && off!(Market, oracle_px) as u64 == spec::MARKET_ORACLE_PX_OFF && off!(Market, oi_long) as u64 == spec::MARKET_OI_LONG_OFF);
    assert!(off!(User, pos) as u64 == spec::USER_POS_OFF && off!(Order, owner) as u64 == spec::ORDER_OWNER_OFF && off!(Order, qty) as u64 == spec::ORDER_QTY_OFF);
    assert!(DISC_SHARD == spec::DISC_SHARD && DISC_MARKET == spec::DISC_MARKET && DISC_USER == spec::DISC_USER && DISC_INS == spec::DISC_INSURANCE);
    assert!(off!(User, triggers) as u64 == spec::USER_TRIGGERS_OFF && off!(Market, paused) as u64 == spec::MARKET_PAUSED_OFF && off!(Order, lvl_left) as u64 == spec::ORDER_LVL_LEFT_OFF);
    assert!(core::mem::size_of::<Trigger>() as u64 == spec::TRIGGER_SIZE && MAX_LEDGER as u64 * spec::LEDGER_SIZE + spec::SHARD_LEDGER_OFF == spec::SHARD_SIZE);
    assert!(INS_SIZE as u64 == spec::INSURANCE_SIZE && RANK_SIZE as u64 == spec::RANK_SIZE && POOL_SIZE as u64 == spec::POOL_SIZE && MARKET_SIZE as u64 == spec::MARKET_SIZE);
    assert!(off!(Market, pending_key) as u64 == spec::MARKET_PENDING_KEY_OFF && off!(Shard, shard_no) as u64 == spec::SHARD_SHARD_NO_OFF && off!(Pos, flags) as u64 == spec::POS_FLAGS_OFF);
    assert!(DISC_RANK == spec::DISC_RANK && DISC_POOL == spec::DISC_POOL && RANK_N as u64 == spec::ADL_RANK_SIZE && E::Isolated as u64 == spec::ERR_ISOLATED && E::Breaker as u64 == spec::ERR_BREAKER);
    assert!(spec::IX_INIT_MARKET == IX_INIT_MARKET as u64 && spec::IX_AGGREGATE == IX_AGGREGATE as u64 && spec::IX_WITHDRAW == IX_WITHDRAW as u64);
};
use state::*;

#[cfg(target_os = "solana")]
mod ep {
    use super::*;
    pinocchio::program_entrypoint!(process_instruction);
    pinocchio::no_allocator!();
    pinocchio::nostd_panic_handler!();
}

/// Clock sysvar. On SBF this is exactly `Clock::get()`; on a host build (native coverage fuzzing of this same
/// processor code, program/fuzz) the harness supplies the clock through `HOST_CLOCK`.
#[cfg(target_os = "solana")]
#[inline(always)]
fn clock() -> Result<Clock, ProgramError> { Clock::get() }
#[cfg(not(target_os = "solana"))]
pub static HOST_CLOCK: std::sync::Mutex<(u64, i64)> = std::sync::Mutex::new((0, 0));
#[cfg(not(target_os = "solana"))]
fn clock() -> Result<Clock, ProgramError> {
    let (slot, ts) = *HOST_CLOCK.lock().unwrap();
    Ok(Clock { slot, epoch_start_timestamp: 0, epoch: 0, leader_schedule_epoch: 0, unix_timestamp: ts })
}
pub const PYTH_RECEIVER: Address = Address::from_str_const("rec5EKMGg6MxZYaMdyBfgwp4d5rB9T1VQH5pJv5LtFJ");
pub const PYTH_PRICE_UPDATE_DISC: [u8; 8] = [34, 241, 35, 99, 157, 126, 244, 205];
pub const IX_INIT_MARKET: u8 = 10;
pub const IX_INIT_SHARD: u8 = 11;
pub const IX_INIT_USER: u8 = 12;
pub const IX_DEPOSIT: u8 = 13;
pub const IX_WITHDRAW: u8 = 14;
pub const IX_SETTLE: u8 = 15;
pub const IX_AGGREGATE: u8 = 16;

#[repr(u32)]
pub enum E { BadAccount = 1, BadData, Margin, Stale, NotLiquidatable, ShardFull, NoShard, Oracle, Expired, NotBankrupt, Math, Cap, Ledger,
    FokUnfilled, Paused, NotTriggered, Vault, TokenProgram, MissingShards, Rank, Timelock, Isolated, UseAdl, Breaker, NoFill, Locked, Halted, Assert }
#[inline(always)]
fn err(e: E) -> ProgramError { ProgramError::Custom(e as u32) }

// ---------- zero-copy access ----------
#[inline(always)]
fn load<T>(a: &AccountView, pid: &Address, disc: u64, size: usize) -> Result<&'static mut T, ProgramError> {
    if !a.owned_by(pid) || a.data_len() < size { return Err(err(E::BadAccount)); }
    let p = unsafe { a.borrow_unchecked() }.as_ptr() as *mut T;
    if unsafe { *(p as *const u64) } != disc { return Err(err(E::BadAccount)); }
    Ok(unsafe { &mut *p })
}
#[inline(always)]
fn disc_of(a: &AccountView) -> u64 {
    if a.data_len() < 8 { return 0; }
    unsafe { *(a.borrow_unchecked().as_ptr() as *const u64) }
}
#[inline(always)]
fn key(a: &AccountView) -> Key { a.address().to_bytes() }
fn rd64(d: &[u8], o: usize) -> Result<u64, ProgramError> {
    d.get(o..o + 8).map(|b| u64::from_le_bytes(b.try_into().unwrap())).ok_or(err(E::BadData))
}
fn i64c(x: i128) -> Result<i64, ProgramError> { i64::try_from(x).map_err(|_| err(E::Math)) }
// i128 division is a software routine on SBF (hundreds of CU); use native u64 math when the value fits (OPT).
const FAST: i128 = 1 << 48;
/// CU: signed x unsigned product. With overflow-checks on, an i128 multiply is a checked software routine;
/// when both magnitudes are < 2^32 the exact product comes from one native u64 multiply (same result).
#[inline(always)]
fn mulf(a: i128, b: u64) -> i128 {
    let ua = a.unsigned_abs();
    if ua < (1 << 32) && b < (1 << 32) { let r = (ua as u64 * b) as i128; if a < 0 { -r } else { r } } else { a * b as i128 }
}
#[inline(always)]
fn bps_up(x: i128, b: u64) -> i128 { if x >= 0 && x < FAST && b <= 65_535 { ((x as u64 * b + 9999) / 10000) as i128 } else { (x * b as i128 + 9999) / 10000 } }
#[inline(always)]
fn bps_dn(x: i128, b: u64) -> i128 { if x >= 0 && x < FAST && b <= 65_535 { (x as u64 * b / 10000) as i128 } else { x * b as i128 / 10000 } }

// ---------- events (sol_log_data; indexers parse "Program data:" lines) ----------
#[inline(always)]
fn emit(parts: &[&[u8]]) {
    #[cfg(target_os = "solana")]
    unsafe { pinocchio::syscalls::sol_log_data(parts.as_ptr() as *const u8, parts.len() as u64) };
    #[cfg(not(target_os = "solana"))]
    let _ = parts;
}

pub const IX_AGG_INIT: u64 = 40;
pub const IX_AGG_POST: u64 = 41;
pub const IX_SET_DELEGATE: u64 = 42;
/// accs: authority(s), agg(w, program-owned, AGG_SIZE, zeroed). data: tag, feed_id[32], expo i32, max_jump_bps u64
fn agg_init(pid: &Address, a: &mut [AccountView], d: &[u8]) -> ProgramResult {
    if a.len() < 2 || !a[0].is_signer() || d.len() != 45 { return Err(err(E::BadData)); }
    if !a[1].owned_by(pid) || a[1].data_len() < AGG_SIZE { return Err(err(E::BadAccount)); }
    let ak = key(&a[0]); let p = unsafe { a[1].borrow_unchecked_mut() };
    if p[0..8] != [0u8; 8] { return Err(err(E::BadAccount)); }
    p[0..8].copy_from_slice(&AGG_DISC); p[8..40].copy_from_slice(&ak); p[40] = 1; p[41..73].copy_from_slice(&d[1..33]);
    p[89..93].copy_from_slice(&d[33..37]); p[109..117].copy_from_slice(&d[37..45]);
    Ok(())
}
/// accs: authority(s), agg(w). data: tag, price i64, conf u64, publish_time i64, n_sources u64, [ext_px i64, ext_ts i64].
/// Checks: signer == stored authority; price > 0; publish_time strictly newer and not in the future (+5 s);
/// |price - last| <= max_jump_bps of last per post (0 = off). Staleness/confidence/divergence are enforced by update_oracle.
fn agg_post(pid: &Address, a: &mut [AccountView], d: &[u8]) -> ProgramResult {
    if a.len() < 2 || !a[0].is_signer() || (d.len() != 33 && d.len() != 49) { return Err(err(E::BadData)); }
    if !a[1].owned_by(pid) || a[1].data_len() < AGG_SIZE { return Err(err(E::BadAccount)); }
    let ak = key(&a[0]); let p = unsafe { a[1].borrow_unchecked_mut() };
    if p[0..8] != AGG_DISC || p[8..40] != ak { return Err(err(E::BadAccount)); }
    let r = |b: &[u8], o: usize| i64::from_le_bytes(b[o..o + 8].try_into().unwrap());
    let (px, conf, pt, ns) = (r(d, 1), r(d, 9), r(d, 17), r(d, 25));
    let (last, last_pt, jump) = (r(p, 73), r(p, 93), r(p, 109) as u64);
    let now = clock()?.unix_timestamp;
    if px <= 0 || conf < 0 || ns <= 0 || pt <= last_pt || pt > now + 5 { return Err(err(E::Oracle)); }
    if jump != 0 && last > 0 && ((px - last).unsigned_abs() as u128) * 10000 > (last as u128) * jump as u128 { return Err(err(E::Breaker)); }
    p[101..109].copy_from_slice(&last_pt.to_le_bytes()); p[73..81].copy_from_slice(&px.to_le_bytes()); p[81..89].copy_from_slice(&conf.to_le_bytes());
    p[93..101].copy_from_slice(&pt.to_le_bytes()); p[117..125].copy_from_slice(&ns.to_le_bytes());
    let n = r(p, 125) + 1; p[125..133].copy_from_slice(&n.to_le_bytes());
    if d.len() == 49 { p[133..149].copy_from_slice(&d[33..49]); } // external perp consensus: price, publish time
    Ok(())
}
/// Owner approves / updates / revokes (expiry 0) a session-key delegate. accs: authority(s), user(r), record(w, program-owned).
/// data: tag, delegate[32], expiry_slot u64, max_qty u64. A record is bound to one user forever.
fn set_delegate(pid: &Address, a: &mut [AccountView], d: &[u8]) -> ProgramResult {
    if a.len() < 3 || d.len() != 49 { return Err(err(E::BadData)); }
    let u: &mut User = load(&a[1], pid, DISC_USER, USER_SIZE)?;
    if !a[0].is_signer() || u.authority != key(&a[0]) { return Err(err(E::BadAccount)); }
    if !a[2].owned_by(pid) || a[2].data_len() < DELEGATE_SIZE { return Err(err(E::BadAccount)); }
    let r = unsafe { &mut *(a[2].borrow_unchecked().as_ptr() as *mut Delegate) };
    if r.disc == 0 { r.disc = DISC_DELEGATE; r.user = key(&a[1]); } else if r.disc != DISC_DELEGATE || r.user != key(&a[1]) { return Err(err(E::BadAccount)); }
    r.delegate.copy_from_slice(&d[1..33]); r.expiry_slot = rd64(d, 33)?; r.max_qty = rd64(d, 41)?;
    Ok(())
}
/// Delegate authorisation for batch/place/cancel: a matching, unexpired Delegate record among the trailing accounts.
fn delegate_max_qty(pid: &Address, a: &[AccountView], signer: &Key, uk: &Key, slot: u64) -> Option<u64> {
    for x in a { if !x.owned_by(pid) || x.data_len() < DELEGATE_SIZE || disc_of(x) != DISC_DELEGATE { continue; }
        let r = unsafe { &*(x.borrow_unchecked().as_ptr() as *const Delegate) };
        if r.user == *uk && r.delegate == *signer && slot <= r.expiry_slot { return Some(r.max_qty); } }
    None
}
pub fn process_instruction(pid: &Address, accs: &mut [AccountView], data: &[u8]) -> ProgramResult {
    let tag = *data.first().ok_or(err(E::BadData))?;
    match tag as u64 {
        spec::IX_SLAB_QUOTE => slab_quote(pid, accs, data),
        spec::IX_TAKE_FAST => take_fast(pid, accs, data),
        IX_PLACE => batch(pid, accs, data, true),
        IX_BATCH => batch(pid, accs, data, false),
        IX_CANCEL => batch(pid, accs, data, false),
        IX_SET_ORACLE => update_oracle(pid, accs),
        IX_LIQUIDATE => liquidate(pid, accs, data),
        IX_BACKSTOP => adl(pid, accs),
        spec::IX_LIQUIDATE_BOOK => liquidate_book(pid, accs, data),
        spec::IX_SET_TRIGGER => set_trigger(pid, accs, data),
        spec::IX_EXEC_TRIGGER => exec_trigger(pid, accs, data),
        spec::IX_ADMIN => admin(pid, accs, data),
        spec::IX_SET_TIER => set_tier(pid, accs, data),
        spec::IX_CRANK_CLAIM => crank_claim(pid, accs),
        spec::IX_ADL_RANK => adl_rank(pid, accs),
        spec::IX_INIT_RANK => init_rank(pid, accs),
        spec::IX_INIT_POOL => init_pool(pid, accs, data),
        spec::IX_POOL_PROPOSE => pool_propose(pid, accs, data),
        spec::IX_SET_ISOLATED => set_isolated(pid, accs, data),
        spec::IX_ISO_TRANSFER => iso_transfer(pid, accs, data),
        spec::IX_INIT_MULTISIG => init_multisig(pid, accs, data),
        spec::IX_ADMIN_PROPOSE => admin_propose(pid, accs, data),
        spec::IX_SLAB_INIT => slab_init(pid, accs),
        spec::IX_FAST_INIT => fast_init(pid, accs, data),
        spec::IX_FAST_JOIN => fast_join(pid, accs),
        spec::IX_SLAB_FUND => slab_fund(pid, accs, data),
        spec::IX_SLAB_SETTLE => slab_settle(pid, accs),
        spec::IX_SLAB_KILL => slab_kill(pid, accs, data),
        spec::IX_LIQ_FAST => liquidate_fast(pid, accs, data),
        spec::IX_ASSERT => assert_state(pid, accs, data),
        spec::IX_SET_SESSION => set_session(pid, accs, data),
        spec::IX_MARKET_CFG => market_cfg(pid, accs, data),
        spec::IX_VAULT_INIT => vault_init(pid, accs, data),
        spec::IX_VAULT_ADD_SUB => vault_add_sub(pid, accs),
        spec::IX_VAULT_LINK => vault_link(pid, accs, data),
        spec::IX_VAULT_DEPOSIT => vault_deposit(pid, accs, data),
        spec::IX_VAULT_WITHDRAW => vault_withdraw(pid, accs, data),
        spec::IX_VAULT_TAKEOVER => vault_takeover(pid, accs, data),
        spec::IX_VAULT_CRANK => vault_crank(pid, accs),
        spec::IX_VAULT_SET => vault_set(pid, accs, data),
        spec::IX_ADMIN_EXECUTE => admin_execute(pid, accs),
        IX_AGG_INIT => agg_init(pid, accs, data),
        IX_AGG_POST => agg_post(pid, accs, data),
        IX_SET_DELEGATE => set_delegate(pid, accs, data),
        _ => match tag {
            IX_INIT_MARKET => init_market(pid, accs, data),
            IX_INIT_SHARD => init_shard(pid, accs, data),
            IX_INIT_USER => init_user(pid, accs),
            IX_DEPOSIT => deposit(pid, accs, data),
            IX_WITHDRAW => withdraw(pid, accs, data),
            IX_SETTLE => settle(pid, accs),
            IX_AGGREGATE => aggregate(pid, accs),
            _ => Err(ProgramError::InvalidInstructionData),
        },
    }
}

// ---------- init ----------
/// accs: admin(s), market(w), insurance(w), vault token acct, oracle
/// data: tag, vault_bump u8, feed_id[32], bucket_width, max_staleness, max_conf_bps, max_age, expo(i64), px_mul, px_div, max_oi
fn init_market(pid: &Address, a: &mut [AccountView], d: &[u8]) -> ProgramResult {
    if a.len() < 5 || !a[0].is_signer() || d.len() < 2 + 32 + 64 { return Err(err(E::BadData)); }
    if !a[1].owned_by(pid) || a[1].data_len() < MARKET_SIZE || disc_of(&a[1]) != 0 { return Err(err(E::BadAccount)); }
    if !a[2].owned_by(pid) || a[2].data_len() < INS_SIZE || disc_of(&a[2]) != 0 { return Err(err(E::BadAccount)); }
    // vault: SPL token account whose owner must be PDA ["vault", seed, bump]; seed = market, or the pool
    // (optional a[5] pool(w) + a[6] pool admin signer: joining a shared vault needs the pool admin -- AUDIT C-2/P-1)
    let mk = key(&a[1]);
    let (seed, bump, pool) = if a.len() >= 7 {
        let p: &mut Pool = load(&a[5], pid, DISC_POOL, POOL_SIZE)?;
        if !auth(pid, &p.admin, &a[6..]) || p.vault != key(&a[3]) { return Err(err(E::BadAccount)); }
        // AUDIT N-6: admission is timelocked (pool_propose) and the market must carry an OI cap within the pool's limit
        let slot = clock()?.slot;
        if p.pending_market != mk || slot < p.pending_eta { return Err(err(E::Timelock)); }
        let max_oi = rd64(d, 2 + 32 + 56)?;
        if max_oi == 0 || (p.max_market_oi != 0 && max_oi > p.max_market_oi) { return Err(err(E::Cap)); }
        p.pending_market = [0; 32]; p.pending_eta = 0;
        p.n_markets += 1; (key(&a[5]), p.vault_bump as u8, key(&a[5]))
    } else { (mk, d[1], [0u8; 32]) };
    let pool_admin = if pool != [0; 32] { Some(load::<Pool>(&a[5], pid, DISC_POOL, POOL_SIZE)?.admin) } else { None };
    let bb = [bump];
    let pda = Address::create_program_address(&[b"vault", &seed, &bb], pid).map_err(|_| err(E::BadAccount))?;
    if !a[3].owned_by(&pinocchio_token::ID) || a[3].data_len() < 165 { return Err(err(E::BadAccount)); }
    let vd = unsafe { a[3].borrow_unchecked() };
    if vd[32..64] != pda.to_bytes() { return Err(err(E::BadAccount)); }
    let m = unsafe { &mut *(a[1].borrow_unchecked().as_ptr() as *mut Market) };
    m.pool = pool; m.vault_seed = seed;
    // AUDIT V5-4: a pooled market's admin is pinned to the pool's admin (may be a timelocked m-of-n multisig), so
    // depositors trust one key set, not every admitted market's creator.
    m.disc = DISC_MARKET; m.admin = pool_admin.unwrap_or(key(&a[0])); m.insurance = key(&a[2]); m.vault = key(&a[3]); m.oracle = key(&a[4]);
    m.mint.copy_from_slice(&vd[0..32]);
    m.vault_bump = bump as u64; m.feed_id.copy_from_slice(&d[2..34]);
    let mut o = 34;
    let mut nx = || { let v = rd64(d, o); o += 8; v };
    m.bucket_width = nx()?; m.max_staleness_slots = nx()?; m.max_conf_bps = nx()?; m.max_age_secs = nx()?;
    m.expo = nx()? as i64; m.px_mul = nx()? as i64; m.px_div = nx()? as i64; m.max_oi = nx()?;
    m.imr_bps = IMR_BPS; m.mmr_bps = MMR_BPS;
    if m.bucket_width == 0 || m.px_div <= 0 || m.px_mul <= 0 { return Err(err(E::BadData)); }
    let s0 = clock()?.slot;
    m.funding_slot = s0; m.period_start = s0; m.funding_period = FUNDING_PERIOD_SLOTS; m.impact_notional = FUNDING_IMPACT_NOTIONAL;
    let ins = unsafe { &mut *(a[2].borrow_unchecked().as_ptr() as *mut Insurance) };
    ins.disc = DISC_INS; ins.market = mk; ins.balance = 0;
    Ok(())
}
/// Pooled collateral (cross-margin): one SPL vault owned by PDA ["vault", pool, bump] shared by every market
/// the pool admin admits. accs: admin(s), pool(w), vault token acct. data: tag, bump
fn init_pool(pid: &Address, a: &mut [AccountView], d: &[u8]) -> ProgramResult {
    if a.len() < 3 || !a[0].is_signer() || d.len() < 2 { return Err(err(E::BadData)); }
    if !a[1].owned_by(pid) || a[1].data_len() < POOL_SIZE || disc_of(&a[1]) != 0 { return Err(err(E::BadAccount)); }
    let pk = key(&a[1]); let bb = [d[1]];
    let pda = Address::create_program_address(&[b"vault", &pk, &bb], pid).map_err(|_| err(E::BadAccount))?;
    if !a[2].owned_by(&pinocchio_token::ID) || a[2].data_len() < 165 { return Err(err(E::BadAccount)); }
    let vd = unsafe { a[2].borrow_unchecked() };
    if vd[32..64] != pda.to_bytes() { return Err(err(E::BadAccount)); }
    let p = unsafe { &mut *(a[1].borrow_unchecked().as_ptr() as *mut Pool) };
    p.disc = DISC_POOL; p.admin = key(&a[0]); p.vault = key(&a[2]); p.vault_bump = d[1] as u64; p.mint.copy_from_slice(&vd[0..32]);
    p.max_market_oi = if d.len() >= 10 { rd64(d, 2)? } else { 0 };
    Ok(())
}
/// AUDIT N-6: pool admin proposes a market for admission; it can join after TIMELOCK_SLOTS.
/// accs: pool(w), [admin signer(s) / multisig + cosigners]. data: tag, market key[32]
fn pool_propose(pid: &Address, a: &mut [AccountView], d: &[u8]) -> ProgramResult {
    if a.is_empty() || d.len() < 33 { return Err(err(E::BadData)); }
    let p: &mut Pool = load(&a[0], pid, DISC_POOL, POOL_SIZE)?;
    if !auth(pid, &p.admin, &a[1..]) { return Err(err(E::BadAccount)); }
    p.pending_market.copy_from_slice(&d[1..33]); p.pending_eta = clock()?.slot + spec::TIMELOCK_SLOTS;
    Ok(())
}
/// m-of-n admin account. accs: payer(s), multisig(w, fresh). data: tag, m u8, n u8, keys[n]
fn init_multisig(pid: &Address, a: &mut [AccountView], d: &[u8]) -> ProgramResult {
    if a.len() < 2 || !a[0].is_signer() || d.len() < 3 { return Err(err(E::BadData)); }
    let (mm, n) = (d[1] as usize, d[2] as usize);
    if n == 0 || n > 8 || mm == 0 || mm > n || d.len() < 3 + 32 * n { return Err(err(E::BadData)); }
    if !a[1].owned_by(pid) || a[1].data_len() < MULTISIG_SIZE || disc_of(&a[1]) != 0 { return Err(err(E::BadAccount)); }
    let ms = unsafe { &mut *(a[1].borrow_unchecked().as_ptr() as *mut Multisig) };
    for i in 0..n { ms.keys[i].copy_from_slice(&d[3 + 32 * i..35 + 32 * i]); for j in 0..i { if ms.keys[j] == ms.keys[i] { return Err(err(E::BadData)); } } }
    ms.disc = DISC_MULTISIG; ms.m = mm as u64; ms.n = n as u64;
    Ok(())
}
/// Admin authorization: a signer equal to `admin`, or `admin` is a Multisig account (passed) and at least m of
/// its distinct keys signed.
fn auth(pid: &Address, admin: &Key, a: &[AccountView]) -> bool {
    if a.iter().any(|x| x.is_signer() && key(x) == *admin) { return true; }
    let Some(ms) = a.iter().find(|x| key(x) == *admin) else { return false };
    let Ok(ms) = load::<Multisig>(ms, pid, DISC_MULTISIG, MULTISIG_SIZE) else { return false };
    let mut got = 0u8; let mut cnt = 0u64;
    for x in a { if !x.is_signer() { continue; } let k = key(x);
        for i in 0..(ms.n as usize).min(8) { if ms.keys[i] == k && got & (1 << i) == 0 { got |= 1 << i; cnt += 1; } } }
    cnt >= ms.m
}
/// accs: admin(s), market, shard(w). data: tag, side u8, lo u64
fn init_shard(pid: &Address, a: &mut [AccountView], d: &[u8]) -> ProgramResult {
    if a.len() < 3 || !a[0].is_signer() || d.len() < 10 { return Err(err(E::BadData)); }
    let m: &mut Market = load(&a[1], pid, DISC_MARKET, MARKET_SIZE)?;
    if !auth(pid, &m.admin, a) { return Err(err(E::BadAccount)); }
    if !a[2].owned_by(pid) || a[2].data_len() < SHARD_SIZE || disc_of(&a[2]) != 0 { return Err(err(E::BadAccount)); }
    if m.n_shards >= MAX_SHARD_NO { return Err(err(E::Cap)); } // AUDIT L-3: shard_no fits the id high bits
    let lo = rd64(d, 2)?;
    if lo % m.bucket_width != 0 || d[1] > 1 { return Err(err(E::BadData)); }
    let s = unsafe { &mut *(a[2].borrow_unchecked().as_ptr() as *mut Shard) };
    s.disc = DISC_SHARD; s.market = key(&a[1]); s.side = d[1] as u64; s.lo = lo; s.width = m.bucket_width;
    s.shard_no = m.n_shards; m.n_shards += 1; m.n_side[d[1] as usize] += 1;
    Ok(())
}
/// accs: authority(s), user(w)
fn init_user(pid: &Address, a: &mut [AccountView]) -> ProgramResult {
    if a.len() < 2 || !a[0].is_signer() { return Err(err(E::BadData)); }
    if !a[1].owned_by(pid) || a[1].data_len() < USER_SIZE || disc_of(&a[1]) != 0 { return Err(err(E::BadAccount)); }
    let u = unsafe { &mut *(a[1].borrow_unchecked().as_ptr() as *mut User) };
    u.disc = DISC_USER; u.authority = key(&a[0]);
    Ok(())
}

// ---------- collateral ----------
/// accs: authority(s), user(w), market, vault(w), src token(w), token program. data: tag, amount
fn deposit(pid: &Address, a: &mut [AccountView], d: &[u8]) -> ProgramResult {
    if a.len() < 6 { return Err(err(E::BadData)); }
    let amt = rd64(d, 1)?;
    let u: &mut User = load(&a[1], pid, DISC_USER, USER_SIZE)?;
    let m: &Market = load(&a[2], pid, DISC_MARKET, MARKET_SIZE)?;
    if !a[0].is_signer() || u.authority != key(&a[0]) || m.vault != key(&a[3]) { return Err(err(E::BadAccount)); }
    if a[5].address() != &pinocchio_token::ID { return Err(err(E::TokenProgram)); } // AUDIT C-1
    if u.home_vault == [0; 32] { u.home_vault = m.vault; } else if u.home_vault != m.vault { return Err(err(E::Vault)); } // AUDIT C-2
    if m.paused != 0 { return Err(err(E::Paused)); }
    if m.tvl_cap != 0 {
        let vd = unsafe { a[3].borrow_unchecked() };
        if rd64(vd, 64)?.saturating_add(amt) > m.tvl_cap { return Err(err(E::Cap)); }
    }
    pinocchio_token::instructions::Transfer::new(&a[4], &a[3], &a[0], amt).invoke_with_program(a[5].address())?;
    u.cash = u.cash.checked_add(i64::try_from(amt).map_err(|_| err(E::Math))?).ok_or(err(E::Math))?;
    Ok(())
}
/// accs: authority(s), user(w), market, vault(w), dst token(w), token program, vault_pda, [shards to claim / extra markets]
fn withdraw(pid: &Address, a: &mut [AccountView], d: &[u8]) -> ProgramResult {
    if a.len() < 7 { return Err(err(E::BadData)); }
    let amt = rd64(d, 1)?;
    let u: &mut User = load(&a[1], pid, DISC_USER, USER_SIZE)?;
    let m: &Market = load(&a[2], pid, DISC_MARKET, MARKET_SIZE)?;
    if !a[0].is_signer() || u.authority != key(&a[0]) || m.vault != key(&a[3]) { return Err(err(E::BadAccount)); }
    if a[5].address() != &pinocchio_token::ID { return Err(err(E::TokenProgram)); } // AUDIT C-1
    home(u, m)?; // AUDIT C-2
    let uk = key(&a[1]);
    let clk = clock()?; let slot = clk.slot; let nw: Now = (slot, clk.unix_timestamp);
    claim_all(pid, u, &uk, &a[7..])?;
    settle_funding_all(pid, u, &a[2..])?;
    if (amt as i128) > u.cash as i128 { return Err(err(E::Margin)); }
    u.cash -= amt as i64;
    let (eq, im, _) = health(pid, u, &a[2..], nw, true)?;
    if eq < im { return Err(err(E::Margin)); }
    let bump = [m.vault_bump as u8];
    let seeds = [Seed::from(b"vault".as_slice()), Seed::from(&m.vault_seed), Seed::from(&bump)];
    pinocchio_token::instructions::Transfer::new(&a[3], &a[4], &a[6], amt)
        .invoke_signed_with_program(&[Signer::from(&seeds)], a[5].address())?;
    Ok(())
}

// ---------- margin / funding ----------
fn find_market(pid: &Address, k: &Key, a: &[AccountView]) -> Result<&'static Market, ProgramError> {
    for x in a { if x.address().to_bytes() == *k && disc_of(x) == DISC_MARKET { return load(x, pid, DISC_MARKET, MARKET_SIZE).map(|m| &*m); } }
    Err(err(E::BadAccount))
}
fn settle_funding_all(pid: &Address, u: &mut User, a: &[AccountView]) -> ProgramResult {
    for p in u.pos.iter_mut() {
        if p.market == [0; 32] { break; } // used positions are packed at the front (gc_pos)
        let m = find_market(pid, &p.market, a)?;
        let idx = m.funding_index;
        let dd = idx as i128 - p.funding_snap as i128;
        let pay = if dd == 0 { 0 } else { let r = mulf(dd, p.base.unsigned_abs()); if p.base < 0 { -r } else { r } };
        if p.flags & POS_ISO != 0 { p.iso_margin = i64c(p.iso_margin as i128 - pay)?; } else { u.cash = i64c(u.cash as i128 - pay)?; }
        p.funding_snap = idx;
    }
    Ok(())
}
/// AUDIT C-2: cash is a claim on exactly one vault; every market an account touches must use that vault.
#[inline(always)]
fn home(u: &User, m: &Market) -> ProgramResult { if u.home_vault != m.vault { Err(err(E::Vault)) } else { Ok(()) } }
/// (slot, unix_timestamp)
type Now = (u64, i64);
/// AUDIT L-4: stale if the update slot is too old OR the Pyth publish_time is older than max_age at *use* time.
fn stale(m: &Market, nw: Now) -> bool { m.oracle_px == 0 || nw.0 > m.oracle_slot + m.max_staleness_slots || nw.1.saturating_sub(m.oracle_ts) > m.max_age_secs as i64 }
/// (equity, initial req (net-of-resting), maintenance req)
const POS_ISO: u64 = 1;
/// Isolated position: equity = iso_margin + base * mark (all of the position's cash flows are booked to iso_margin).
fn iso_health(pid: &Address, u: &User, pi: usize, a: &[AccountView], nw: Now, need_fresh: bool) -> Result<(i128, i128, i128), ProgramError> {
    let p = &u.pos[pi]; let m = find_market(pid, &p.market, a)?;
    if need_fresh && stale(m, nw) && (p.base != 0 || p.bid_open != 0 || p.ask_open != 0) { return Err(err(E::Stale)); }
    let px = mark(m);
    let gross = core::cmp::max((p.base as i128 + p.bid_open as i128).abs(), (p.base as i128 - p.ask_open as i128).abs());
    Ok((p.iso_margin as i128 + mulf(p.base as i128, px), bps_up(mulf(gross, px), m.imr_bps), bps_up(mulf((p.base as i128).abs(), px), m.mmr_bps)))
}
/// Health that governs position `pi`: its own if isolated, else the cross account.
fn phealth(pid: &Address, u: &User, pi: usize, a: &[AccountView], nw: Now, need_fresh: bool) -> Result<(i128, i128, i128), ProgramError> {
    if u.pos[pi].flags & POS_ISO != 0 { iso_health(pid, u, pi, a, nw, need_fresh) } else { health(pid, u, a, nw, need_fresh) }
}
/// Book a cash flow of position `pi`: to its isolated margin if isolated, else to cross cash.
fn credit(u: &mut User, pi: usize, d: i128) -> ProgramResult {
    if u.pos[pi].flags & POS_ISO != 0 { u.pos[pi].iso_margin = i64c(u.pos[pi].iso_margin as i128 + d)?; } else { u.cash = i64c(u.cash as i128 + d)?; }
    Ok(())
}
/// Isolated margin mode for one market. accs: authority(s), user(w), market. data: tag, on u8.
/// Only while the position is flat with no resting orders; switching off returns the isolated margin to cash.
fn set_isolated(pid: &Address, a: &mut [AccountView], d: &[u8]) -> ProgramResult {
    if a.len() < 3 || d.len() < 2 { return Err(err(E::BadData)); }
    let u: &mut User = load(&a[1], pid, DISC_USER, USER_SIZE)?;
    let m: &Market = load(&a[2], pid, DISC_MARKET, MARKET_SIZE)?;
    if !a[0].is_signer() || u.authority != key(&a[0]) { return Err(err(E::BadAccount)); }
    home(u, m)?;
    let mk = key(&a[2]);
    let pi = pos_idx(u, &mk, m.funding_index)?;
    let p = &mut u.pos[pi];
    if p.base != 0 || p.bid_open != 0 || p.ask_open != 0 || p.n_orders != 0 { return Err(err(E::Isolated)); }
    if d[1] != 0 { p.flags |= POS_ISO; } else { p.flags &= !POS_ISO; u.cash = i64c(u.cash as i128 + p.iso_margin as i128)?; p.iso_margin = 0; }
    gc_pos(u);
    Ok(())
}
/// Move collateral between cross cash and an isolated position. accs: authority(s), user(w), market, [other
/// markets for cross health]. data: tag, amount i64 (> 0: cash -> isolated, < 0: isolated -> cash).
fn iso_transfer(pid: &Address, a: &mut [AccountView], d: &[u8]) -> ProgramResult {
    if a.len() < 3 { return Err(err(E::BadData)); }
    let amt = rd64(d, 1)? as i64;
    let u: &mut User = load(&a[1], pid, DISC_USER, USER_SIZE)?;
    let m: &Market = load(&a[2], pid, DISC_MARKET, MARKET_SIZE)?;
    if !a[0].is_signer() || u.authority != key(&a[0]) { return Err(err(E::BadAccount)); }
    home(u, m)?;
    let mk = key(&a[2]);
    let pi = u.pos.iter().position(|p| p.market == mk && p.flags & POS_ISO != 0).ok_or(err(E::Isolated))?;
    let clk = clock()?; let nw: Now = (clk.slot, clk.unix_timestamp);
    settle_funding_all(pid, u, &a[2..])?;
    u.cash = i64c(u.cash as i128 - amt as i128)?; u.pos[pi].iso_margin = i64c(u.pos[pi].iso_margin as i128 + amt as i128)?;
    if amt > 0 { let (e, im, _) = health(pid, u, &a[2..], nw, true)?; if u.cash < 0 || e < im { return Err(err(E::Margin)); } }
    else { let (e, im, _) = iso_health(pid, u, pi, &a[2..], nw, true)?; if u.pos[pi].iso_margin < 0 || e < im { return Err(err(E::Margin)); } }
    gc_pos(u);
    Ok(())
}
fn health(pid: &Address, u: &User, a: &[AccountView], nw: Now, need_fresh: bool) -> Result<(i128, i128, i128), ProgramError> {
    let (mut eq, mut im, mut mm) = (u.cash as i128, 0i128, 0i128);
    for p in u.pos.iter() {
        if p.market == [0; 32] { break; }
        if p.flags & POS_ISO != 0 { continue; } // isolated positions are margined on their own
        let m = find_market(pid, &p.market, a)?;
        if need_fresh && stale(m, nw) && (p.base != 0 || p.bid_open != 0 || p.ask_open != 0) { return Err(err(E::Stale)); }
        let px = mark(m);
        eq += mulf(p.base as i128, px);
        let gross = core::cmp::max((p.base as i128 + p.bid_open as i128).abs(), (p.base as i128 - p.ask_open as i128).abs());
        im += bps_up(mulf(gross, px), m.imr_bps);
        mm += bps_up(mulf((p.base as i128).abs(), px), m.mmr_bps);
    }
    Ok((eq, im, mm))
}
fn pos_idx(u: &mut User, mk: &Key, funding_index: i64) -> Result<usize, ProgramError> {
    let mut free = usize::MAX;
    for (i, p) in u.pos.iter().enumerate() {
        if p.market == *mk { return Ok(i); }
        if p.market == [0; 32] { free = i; break; }
    }
    if free == usize::MAX { return Err(err(E::Cap)); }
    u.pos[free] = Pos { market: *mk, base: 0, funding_snap: funding_index, bid_open: 0, ask_open: 0, n_orders: 0, cost: 0, iso_margin: 0, flags: 0 };
    Ok(free)
}
/// Free flat, idle positions and keep the used ones packed at the front in their order, so every loop over
/// positions stops at the first empty slot. Runs last in each instruction (indices held earlier are stale after it).
fn gc_pos(u: &mut User) {
    let mut j = 0;
    for i in 0..MAX_POS {
        let p = &mut u.pos[i];
        if p.market == [0; 32] { break; }
        if p.base == 0 { p.cost = 0; p.flags &= POS_ISO; if p.bid_open == 0 && p.ask_open == 0 && p.iso_margin == 0 && p.flags == 0 { p.market = [0; 32]; continue; } }
        if i != j { u.pos.swap(i, j); }
        j += 1;
    }
}
#[inline(always)]
/// AUDIT N-7: cost basis after a base change with signed quote `paid` (> 0 = paid). Flat -> 0; sign flip ->
/// new base valued at this trade's average price; otherwise accumulate.
fn cost_after(old_b: i64, new_b: i64, cost: i64, paid: i128) -> Result<i64, ProgramError> {
    if new_b == 0 { return Ok(0); }
    if old_b != 0 && (old_b > 0) != (new_b > 0) {
        let dq = (new_b as i128 - old_b as i128).abs();
        return i64c(new_b as i128 * paid.abs() / dq);
    }
    i64c(cost as i128 + paid)
}
fn long(b: i64) -> i64 { if b > 0 { b } else { 0 } }

/// Apply maker fills owed to `uk` from every shard in `a` (lazy maker settlement).
fn claim_all(pid: &Address, u: &mut User, uk: &Key, a: &[AccountView]) -> ProgramResult {
    for x in a {
        if disc_of(x) != DISC_SHARD || !x.owned_by(pid) { continue; }
        let s: &mut Shard = unsafe { &mut *(x.borrow_unchecked().as_ptr() as *mut Shard) };
        claim(u, uk, s)?;
    }
    Ok(())
}
fn claim(u: &mut User, uk: &Key, s: &mut Shard) -> ProgramResult {
    let mut i = 0;
    while i < s.nled as usize {
        if s.ledger[i].owner == *uk {
            let l = s.ledger[i];
            let pi = u.pos.iter().position(|p| p.market == s.market).ok_or(err(E::Ledger))?;
            let p = &mut u.pos[pi];
            let before = long(p.base);
            let ob = p.base; p.base += l.base;
            if p.flags & POS_ISO != 0 { p.iso_margin = i64c(p.iso_margin as i128 + l.quote as i128)?; } else { u.cash = i64c(u.cash as i128 + l.quote as i128)?; }
            p.cost = cost_after(ob, p.base, p.cost, -(l.quote as i128))?;
            if s.side == SIDE_BID { p.bid_open -= l.open_reduce } else { p.ask_open -= l.open_reduce }
            p.n_orders -= l.n_closed;
            s.oi_long_delta += long(p.base) - before;
            u.maker_volume += l.maker_vol;
            s.nled -= 1; s.ledger[i] = s.ledger[s.nled as usize];
            return Ok(()); // at most one entry per owner per shard
        }
        i += 1;
    }
    Ok(())
}
fn ledger_entry<'s>(s: &'s mut Shard, owner: &Key) -> Option<&'s mut Ledger> {
    let n = s.nled as usize;
    match s.ledger[..n].iter().position(|l| l.owner == *owner) {
        Some(i) => Some(&mut s.ledger[i]),
        None if n < MAX_LEDGER => { s.ledger[n] = Ledger { owner: *owner, base: 0, quote: 0, open_reduce: 0, maker_vol: 0, n_closed: 0 }; s.nled += 1; Some(&mut s.ledger[n]) }
        None => None,
    }
}
fn remove_order(s: &mut Shard, i: usize) {
    let n = s.n as usize;
    s.orders.copy_within(i + 1..n, i); s.n -= 1;
}

// ---------- matching ----------
#[inline(always)] fn kind(o: &Order) -> u64 { o.meta & 0xff }
#[inline(always)] fn level(o: &Order) -> u64 { (o.meta >> 24) & 0xff }
#[inline(always)] fn rebate_bps(o: &Order) -> u64 { (o.meta >> 8) & 0xff }
fn peg_price(oracle: u64, off: i64, lo: u64, hi: u64) -> u64 {
    let mut p = oracle as i128 + off as i128;
    if lo != 0 && p < lo as i128 { p = lo as i128; }
    if hi != 0 && p > hi as i128 { p = hi as i128; }
    if p > MAX_PX as i128 { p = MAX_PX as i128; } // AUDIT L-2: no u64 truncation
    if p < TICK as i128 { TICK } else { (p as u64) / TICK * TICK }
}
/// Effective price, re-derived lazily at match time (no requote tx for pegs / ladder levels).
#[inline(always)]
fn eff(o: &Order, bid_side: bool, oracle: u64) -> u64 {
    match kind(o) {
        KIND_PEG => peg_price(oracle, o.a1, o.a2, o.a3),
        KIND_SPLINE => { let d = o.a1.unsigned_abs() * level(o); if bid_side { o.price.saturating_sub(d).max(TICK) } else { o.price + d } }
        _ => o.price,
    }
}
#[inline(always)] fn top(o: &Order) -> u64 { if kind(o) == KIND_SPLINE { o.lvl_left } else { o.qty } }
fn consume(o: &mut Order, f: u64) {
    o.qty -= f;
    if kind(o) == KIND_SPLINE {
        o.lvl_left -= f;
        if o.lvl_left == 0 && o.qty > 0 {
            let l = level(o) + 1; o.meta = (o.meta & !(0xff << 24)) | (l << 24);
            o.lvl_left = core::cmp::min(o.a2 + o.a3 * l, o.qty);
        }
    }
}
/// Best order inside one shard: (index, effective price). Pure-limit shards are sorted -> orders[0].
#[inline(always)]
fn shard_best(s: &Shard, oracle: u64) -> Option<(usize, u64)> {
    if s.n == 0 { return None; }
    let bid = s.side == SIDE_BID;
    if s.seq & SPECIAL_BIT == 0 { return Some((0, s.orders[0].price)); }
    let mut b = (0usize, eff(&s.orders[0], bid, oracle));
    for i in 1..s.n as usize {
        let p = eff(&s.orders[i], bid, oracle);
        if (bid && p > b.1) || (!bid && p < b.1) { b = (i, p); }
    }
    Some(b)
}
/// Best order on `side` among the passed shards: (shard ptr, index, effective price). Ties: shard order, then FIFO.
fn best(a: &[AccountView], pid: &Address, mk: &Key, side: u64, oracle: u64) -> Option<(*mut Shard, usize, u64)> {
    let bid = side == SIDE_BID;
    let mut b: Option<(*mut Shard, usize, u64)> = None;
    for x in a {
        if disc_of(x) != DISC_SHARD || !x.owned_by(pid) { continue; }
        let s: &mut Shard = unsafe { &mut *(x.borrow_unchecked().as_ptr() as *mut Shard) };
        if s.market != *mk || s.side != side { continue; }
        if let Some((i, p)) = shard_best(s, oracle) {
            let better = match b { None => true, Some((_, _, bp)) => if bid { p > bp } else { p < bp } };
            if better { b = Some((s as *mut Shard, i, p)); }
        }
    }
    b
}
/// shard.seq high bit: set once a peg/spline order is inserted (disables the sorted fast path for that shard).
const SPECIAL_BIT: u64 = 1 << 63;
const RETIRED_BIT: u64 = 1 << 62;
const SEQ_MASK: u64 = (1 << ORDER_SEQ_BITS) - 1;
pub struct Fill { pub filled: u64, pub notional: i128, pub abort: bool }
const MAX_TAKE_SHARDS: usize = 16;
const EV_MAX: usize = 16;
const EV_ENTRY: usize = 56; // maker 32 | px 8 | qty 8 | order id 8
/// Batched fill event: one sol_log_data per take (or per 16 fills):
/// [EVENTS_FILL, taker_side, taker 32, n u8, n x (maker 32, px u64, qty u64, id u64)]
struct EvBuf { b: [u8; 35 + EV_MAX * EV_ENTRY], n: usize }
impl EvBuf {
    fn new(taker: &Key, taker_bid: bool) -> Self {
        let mut b = [0u8; 35 + EV_MAX * EV_ENTRY]; b[0] = EVENTS_FILL as u8; b[1] = if taker_bid { 0 } else { 1 }; b[2..34].copy_from_slice(taker);
        EvBuf { b, n: 0 }
    }
    #[inline(always)]
    fn push(&mut self, maker: &Key, px: u64, q: u64, id: u64) {
        let o = 35 + self.n * EV_ENTRY;
        self.b[o..o + 32].copy_from_slice(maker); self.b[o + 32..o + 40].copy_from_slice(&px.to_le_bytes());
        self.b[o + 40..o + 48].copy_from_slice(&q.to_le_bytes()); self.b[o + 48..o + 56].copy_from_slice(&id.to_le_bytes());
        self.n += 1; if self.n == EV_MAX { self.flush(); }
    }
    fn flush(&mut self) { if self.n > 0 { self.b[34] = self.n as u8; emit(&[&self.b[..35 + self.n * EV_ENTRY]]); self.n = 0; } }
}
#[inline(always)]
fn arrival(c: &(*mut Shard, usize, u64, bool)) -> u64 { unsafe { (*c.0).orders[c.1].meta >> 32 } }
/// Discovery bounds in force for matching: the whole range while the external market is open.
#[inline(always)]
fn bound_range(m: &Market) -> (u64, u64) {
    if m.session == 0 || m.bound_bps == 0 || m.bound_ref == 0 { return (0, u64::MAX); }
    let d = (m.bound_ref as u128 * m.bound_bps as u128 / 10000) as u64;
    (m.bound_ref.saturating_sub(d), m.bound_ref.saturating_add(d))
}
/// Re-anchor while closed (price within thresh of the bound moves the reference to the bound, until bound_max), then clamp mark.
fn apply_bounds(m: &mut Market, px: u64) {
    if m.bound_bps == 0 { return; }
    if m.session == 0 || m.bound_ref == 0 { m.bound_ref = px; m.bound_up = 0; m.bound_dn = 0; return; }
    let d = (m.bound_ref as u128 * m.bound_bps as u128 / 10000) as u64;
    let t = (d as u128 * m.bound_thresh_bps as u128 / 10000) as u64;
    if px >= m.bound_ref + t && m.bound_up < m.bound_max { m.bound_ref += d; m.bound_up += 1; }
    else if px + t <= m.bound_ref && m.bound_dn < m.bound_max { m.bound_ref -= d; m.bound_dn += 1; }
    let (lo, hi) = bound_range(m);
    m.mark_px = m.mark_px.clamp(lo, hi);
}
/// Taker matches against the best opposite order across the passed shards (effective prices).
/// Per-shard best is computed once and recomputed only for the shard just modified (no full rescan per fill).
fn take(pid: &Address, a: &[AccountView], mk: &Key, m: &Market, u: &mut User, uk: &Key, pi: usize,
        taker_bid: bool, limit: u64, mut qty: u64, slot: u64, taker_bps: u64, stp: u64) -> Result<Fill, ProgramError> {
    let opp = if taker_bid { SIDE_ASK } else { SIDE_BID };
    let mut f = Fill { filled: 0, notional: 0, abort: false };
    let mut first: *mut Shard = core::ptr::null_mut();
    let base0 = long(u.pos[pi].base);
    let oracle = m.oracle_px;
    let mut cs: [(*mut Shard, usize, u64, bool); MAX_TAKE_SHARDS] = [(core::ptr::null_mut(), 0, 0, false); MAX_TAKE_SHARDS];
    let mut ns = 0;
    let mut ro = [false; MAX_TAKE_SHARDS];
    for x in a {
        if disc_of(x) != DISC_SHARD || !x.owned_by(pid) { continue; }
        let s: &mut Shard = unsafe { &mut *(x.borrow_unchecked().as_ptr() as *mut Shard) };
        if s.market != *mk || s.side != opp { continue; }
        if ns == MAX_TAKE_SHARDS { return Err(err(E::BadData)); }
        let b = shard_best(s, oracle);
        ro[ns] = !x.is_writable();
        cs[ns] = (s as *mut Shard, b.map_or(0, |v| v.0), b.map_or(0, |v| v.1), b.is_some()); ns += 1;
    }
    // P-3: every opposite shard must be visible; shards the order won't touch may be read-only, so takers only serialize where they trade
    if m.strict_take != 0 && ns as u64 != m.n_side[opp as usize] { return Err(err(E::MissingShards)); }
    let (blo, bhi) = bound_range(m);
    let mut ev = EvBuf::new(uk, taker_bid);
    while qty > 0 {
        let mut k = usize::MAX;
        for j in 0..ns { if cs[j].3 && (k == usize::MAX || (taker_bid && cs[j].2 < cs[k].2) || (!taker_bid && cs[j].2 > cs[k].2)
            || (cs[j].2 == cs[k].2 && arrival(&cs[j]) < arrival(&cs[k]))) { k = j; } }
        if k == usize::MAX { break; }
        let (sp, i, px, _) = cs[k];
        if (taker_bid && px > limit) || (!taker_bid && px < limit) { break; }
        if px < blo || px > bhi { break; } // closed market: nothing trades outside the discovery bounds
        if ro[k] { return Err(err(E::BadAccount)); }
        let s = unsafe { &mut *sp };
        if first.is_null() { first = sp; }
        let mut o = s.orders[i];
        let expired = o.expiry_slot != 0 && slot > o.expiry_slot;
        let off_band = o.band_bps != 0 && off_band(px, oracle, o.band_bps);
        let mut stop = false;
        if o.owner == *uk {
            let live = !expired && !off_band;
            if stp == STP_CANCEL_NEWEST && live { f.abort = true; break; }
            let d = if stp > STP_CANCEL_NEWEST && live { core::cmp::min(qty, top(&o)) } else { o.qty };
            if stp > STP_CANCEL_NEWEST && live { qty -= d; }
            if s.side == SIDE_BID { u.pos[pi].bid_open -= d } else { u.pos[pi].ask_open -= d }
            consume(&mut o, d);
            if o.qty == 0 { remove_order(s, i); u.pos[pi].n_orders -= 1; } else { s.orders[i] = o }
        } else if expired || off_band {
            match ledger_entry(s, &o.owner) { Some(l) => { l.open_reduce += o.qty; l.n_closed += 1; remove_order(s, i); } None => stop = true }
        } else {
            let q = core::cmp::min(qty, top(&o));
            let notional = match px.checked_mul(q) { Some(n) => n as i128, None => px as i128 * q as i128 }; // OPT: native mul
            let rebate = bps_dn(notional, rebate_bps(&o));
            let fee = bps_up(notional, taker_bps);
            match ledger_entry(s, &o.owner) {
                None => stop = true, // ledger full: stop (crank_claim drains it)
                Some(l) => {
                    if taker_bid { l.base -= q as i64; l.quote = i64c(l.quote as i128 + notional + rebate)?; }
                    else { l.base += q as i64; l.quote = i64c(l.quote as i128 - notional + rebate)?; }
                    l.open_reduce += q; l.maker_vol += q;
                    consume(&mut o, q);
                    if o.qty == 0 { l.n_closed += 1; }
                    s.fee_accum = i64c(s.fee_accum as i128 + fee - rebate)?;
                    credit(u, pi, -fee + if taker_bid { -notional } else { notional })?;
                    let ob = u.pos[pi].base; u.pos[pi].base += if taker_bid { q as i64 } else { -(q as i64) };
                    u.pos[pi].cost = cost_after(ob, u.pos[pi].base, u.pos[pi].cost, if taker_bid { notional } else { -notional })?;
                    f.filled += q; f.notional += notional; qty -= q;
                    ev.push(&o.owner, px, q, o.id);
                    if o.qty == 0 { remove_order(s, i) } else { s.orders[i] = o }
                }
            }
        }
            if stop { break; }
        let b = shard_best(s, oracle);
        cs[k] = (sp, b.map_or(0, |v| v.0), b.map_or(0, |v| v.1), b.is_some());
    }
    ev.flush();
    if !first.is_null() { unsafe { (*first).oi_long_delta += long(u.pos[pi].base) - base0; } }
    Ok(f)
}
/// |px - oracle| / oracle > band_bps, in native u64 when it fits (OPT; same result as the u128 form).
#[inline(always)]
fn off_band(px: u64, oracle: u64, band: u64) -> bool {
    let d = px.abs_diff(oracle);
    match (d.checked_mul(10000), oracle.checked_mul(band)) { (Some(l), Some(r)) => l > r, _ => d as u128 * 10000 > oracle as u128 * band as u128 }
}
/// FOK pre-check (AUDIT M-3): can `need` be filled at `limit` from other owners' live liquidity in the passed shards?
fn fok_avail(a: &[AccountView], pid: &Address, mk: &Key, opp: u64, oracle: u64, limit: u64, slot: u64, uk: &Key, need: u64) -> bool {
    let bid_side = opp == SIDE_BID;
    let ok_px = |p: u64| if bid_side { p >= limit } else { p <= limit };
    let mut have = 0u64;
    for x in a {
        if disc_of(x) != DISC_SHARD || !x.owned_by(pid) { continue; }
        let s: &Shard = unsafe { &*(x.borrow_unchecked().as_ptr() as *const Shard) };
        if s.market != *mk || s.side != opp { continue; }
        for o in &s.orders[..s.n as usize] {
            if o.owner == *uk || (o.expiry_slot != 0 && slot > o.expiry_slot) { continue; }
            let p0 = eff(o, bid_side, oracle);
            if o.band_bps != 0 && off_band(p0, oracle, o.band_bps) { continue; }
            if kind(o) == KIND_SPLINE {
                let (mut lv, mut left, mut avail) = (level(o), o.qty, o.lvl_left);
                while left > 0 {
                    let d = o.a1.unsigned_abs() * lv;
                    let p = if bid_side { o.price.saturating_sub(d).max(TICK) } else { o.price + d };
                    if !ok_px(p) { break; }
                    have += avail; left -= avail; lv += 1; avail = core::cmp::min(o.a2 + o.a3 * lv, left);
                }
            } else if ok_px(p0) { have += o.qty; }
            if have >= need { return true; }
        }
    }
    false
}
fn insert(a: &[AccountView], pid: &Address, mk: &Key, uk: &Key, bid: bool, mut o: Order, slot: u64) -> Result<u64, ProgramError> {
    let side = if bid { SIDE_BID } else { SIDE_ASK };
    let price = o.price;
    let mut full = false;
    for x in a {
        if disc_of(x) != DISC_SHARD || !x.owned_by(pid) { continue; }
        let s: &mut Shard = unsafe { &mut *(x.borrow_unchecked().as_ptr() as *mut Shard) };
        if s.market != *mk || s.side != side || price < s.lo || price >= s.lo + s.width { continue; }
        let n = s.n as usize;
        // P-1: a shard that overflowed stays closed until it empties, so later orders can't jump ahead of the overflow shard
        if s.seq & RETIRED_BIT != 0 { if n == 0 && s.seq & SEQ_MASK < SEQ_MASK { s.seq &= !RETIRED_BIT; } else { full = true; continue; } }
        if n >= MAX_ORDERS { s.seq |= RETIRED_BIT; full = true; continue; }
        if s.seq & SEQ_MASK >= SEQ_MASK { s.seq |= RETIRED_BIT; full = true; continue; } // AUDIT L-3: never wrap; retire the shard // AUDIT H-3: overflow into another shard for the same bucket
        let mut i = n;
        // best-first by anchor price: bids desc, asks asc; FIFO within price
        for j in 0..n { let p = s.orders[j].price; if (bid && p < price) || (!bid && p > price) { i = j; break; } }
        s.orders.copy_within(i..n, i + 1);
        s.seq += 1;
        if kind(&o) != KIND_LIMIT { s.seq |= SPECIAL_BIT; }
        o.id = (s.shard_no << ORDER_SEQ_BITS) | (s.seq & SEQ_MASK); // AUDIT L-3: unique per market, no wrap
        o.meta = (o.meta & 0xffff_ffff) | ((slot & 0xffff_ffff) << 32); // P-2: arrival slot, compared across shards at equal price
        o.owner = *uk;
        s.orders[i] = o;
        s.n += 1;
        return Ok(o.id);
    }
    Err(err(if full { E::ShardFull } else { E::NoShard }))
}
/// side & CANCEL_BY_CID_BIT: `id` is a client order id, searched across all passed shards of that side.
fn cancel(a: &[AccountView], pid: &Address, mk: &Key, uk: &Key, u: &mut User, pi: usize, id: u64, side: u8, price: u64) -> ProgramResult {
    let by_cid = side as u64 & CANCEL_BY_CID_BIT != 0;
    let side = (side as u64) & 1;
    for x in a {
        if disc_of(x) != DISC_SHARD || !x.owned_by(pid) { continue; }
        let s: &mut Shard = unsafe { &mut *(x.borrow_unchecked().as_ptr() as *mut Shard) };
        if s.market != *mk || s.side != side { continue; }
        if !by_cid && (price < s.lo || price >= s.lo + s.width) { continue; }
        for i in 0..s.n as usize {
            let o = &s.orders[i];
            if o.owner == *uk && ((by_cid && o.cid == id && id != 0) || (!by_cid && o.id == id)) {
                let q = o.qty;
                if side == SIDE_BID { u.pos[pi].bid_open -= q } else { u.pos[pi].ask_open -= q }
                u.pos[pi].n_orders -= 1;
                remove_order(s, i); return Ok(());
            }
        }
        if !by_cid { return Ok(()); } // already filled/removed: idempotent cancel (MM friendly)
    }
    if by_cid { Ok(()) } else { Err(err(E::NoShard)) }
}
/// Remove every resting order of `uk` in the passed shards (liquidation). Returns count.
fn cancel_all_of(a: &[AccountView], pid: &Address, mk: &Key, uk: &Key, u: &mut User, pi: usize) -> u64 {
    let mut c = 0;
    for x in a {
        if disc_of(x) != DISC_SHARD || !x.owned_by(pid) { continue; }
        let s: &mut Shard = unsafe { &mut *(x.borrow_unchecked().as_ptr() as *mut Shard) };
        if s.market != *mk { continue; }
        let mut i = 0;
        while i < s.n as usize {
            if s.orders[i].owner == *uk {
                let q = s.orders[i].qty;
                if s.side == SIDE_BID { u.pos[pi].bid_open -= q } else { u.pos[pi].ask_open -= q }
                u.pos[pi].n_orders -= 1;
                remove_order(s, i); c += 1;
            } else { i += 1; }
        }
    }
    c
}

/// place (tag 1: tag + place_item) or batch (tag 3: spec batch layout).
/// accs: authority(s), user(w), market(r), shards(w)..., [other markets(r) for cross-margin]
fn batch(pid: &Address, a: &mut [AccountView], d: &[u8], single: bool) -> ProgramResult {
    if a.len() < 4 { return Err(err(E::BadData)); }
    let u: &mut User = load(&a[1], pid, DISC_USER, USER_SIZE)?;
    let m: &Market = load(&a[2], pid, DISC_MARKET, MARKET_SIZE)?;
    let (uk, mk) = (key(&a[1]), key(&a[2]));
    let clk = clock()?; let slot = clk.slot; let nw: Now = (slot, clk.unix_timestamp);
    // owner, or an approved session-key delegate (place/cancel only, per-order qty cap)
    let dmax = if a[0].is_signer() && u.authority == key(&a[0]) { u64::MAX } else if a[0].is_signer() {
        delegate_max_qty(pid, &a[3..], &key(&a[0]), &uk, slot).ok_or(err(E::BadAccount))? } else { return Err(err(E::BadAccount)); };
    home(u, m)?;
    let rest = &a[3..];
    let (nc, np, mut o) = if single { (0usize, 1usize, 1usize) } else {
        if d.len() < BATCH_HEADER_SIZE as usize { return Err(err(E::BadData)); }
        (d[BATCH_HEADER_N_CANCEL_OFF as usize] as usize, d[BATCH_HEADER_N_PLACE_OFF as usize] as usize, BATCH_HEADER_SIZE as usize)
    };
    if d.len() != o + nc * CANCEL_ITEM_SIZE as usize + np * PLACE_ITEM_SIZE as usize { return Err(err(E::BadData)); }
    claim_all(pid, u, &uk, rest)?;
    let pi = pos_idx(u, &mk, m.funding_index)?;
    settle_funding_all(pid, u, &a[2..])?;
    // 1) cancels first (cancel priority within the instruction)
    for _ in 0..nc {
        cancel(rest, pid, &mk, &uk, u, pi, rd64(d, o)?, d[o + 8], rd64(d, o + 9)?)?;
        o += CANCEL_ITEM_SIZE as usize;
    }
    let is_stale = stale(m, nw);
    let mut risk_up = false;
    let tier = (u.tier as usize).min(TIERS.len() - 1);
    let (taker_bps, maker_rebate) = TIERS[tier];
    for _ in 0..np {
        let it = &d[o..o + PLACE_ITEM_SIZE as usize];
        o += PLACE_ITEM_SIZE as usize;
        let bid = it[0] as u64 == SIDE_BID; let fl = it[PLACE_ITEM_FLAGS_OFF as usize] as u64; let k = it[PLACE_ITEM_KIND_OFF as usize] as u64;
        let mut price = rd64(it, PLACE_ITEM_PRICE_OFF as usize)?; let mut qty = rd64(it, PLACE_ITEM_QTY_OFF as usize)?;
        if qty > dmax { return Err(err(E::Cap)); }
        let expiry = rd64(it, PLACE_ITEM_EXPIRY_SLOT_OFF as usize)?;
        let bo = PLACE_ITEM_ORACLE_BAND_BPS_OFF as usize;
        let band = u16::from_le_bytes([it[bo], it[bo + 1]]) as u64;
        let cid = rd64(it, PLACE_ITEM_CID_OFF as usize)?;
        let (a1, a2, a3) = (rd64(it, PLACE_ITEM_A1_OFF as usize)? as i64, rd64(it, PLACE_ITEM_A2_OFF as usize)?, rd64(it, PLACE_ITEM_A3_OFF as usize)?);
        let stp = (fl & FLAG_STP_MASK) >> FLAG_STP_SHIFT;
        let reduce = fl & FLAG_REDUCE_ONLY != 0;
        let fok = fl & FLAG_FOK != 0;
        let mut ioc = fl & FLAG_IOC != 0 || fok;
        let mut post = fl & FLAG_POST_ONLY != 0;
        if k > KIND_SPLINE || qty == 0 || (k != KIND_PEG && (price == 0 || !price.is_multiple_of(TICK))) { return Err(err(E::BadData)); }
        if k != KIND_LIMIT { if reduce || ioc { return Err(err(E::BadData)); } post = true; }
        if k == KIND_SPLINE {
            if a2 == 0 || a2 > MAX_SPLINE_LEVELS || a1 <= 0 || price > MAX_PX || qty > MAX_PX || a3 > MAX_PX { return Err(err(E::BadData)); }
            // AUDIT H-1: every level price must be representable and >= 1 tick (no overflow at match time)
            let span = (a1 as u64).checked_mul(a2 - 1).ok_or(err(E::BadData))?;
            if (bid && span >= price) || (!bid && price.checked_add(span).is_none_or(|p| p > MAX_PX)) { return Err(err(E::BadData)); }
        }
        if k == KIND_PEG && (a1.unsigned_abs() > MAX_PX || a2 > MAX_PX || a3 > MAX_PX) { return Err(err(E::BadData)); }
        if k == KIND_LIMIT && price > MAX_PX { return Err(err(E::BadData)); }
        if expiry != 0 && expiry <= slot { continue; } // already expired: skip, don't fail the batch
        if reduce {
            let b = u.pos[pi].base;
            let cap = if bid && b < 0 { (-b) as u64 } else if !bid && b > 0 { b as u64 } else { 0 };
            qty = qty.min(cap); ioc = true;
            if qty == 0 { continue; }
        } else {
            if m.paused != 0 { return Err(err(E::Paused)); }
            if is_stale { return Err(err(E::Stale)); }
            risk_up = true;
        }
        if k == KIND_PEG { price = peg_price(m.oracle_px, a1, a2, a3); }
        // AUDIT H-3: minimum notional for anything that can rest (anti book-stuffing)
        if !ioc && (price as u128) * (qty as u128) < MIN_ORDER_NOTIONAL as u128 { return Err(err(E::BadData)); }
        let bo = best(rest, pid, &mk, if bid { SIDE_ASK } else { SIDE_BID }, m.oracle_px).map(|b| b.2);
        let crosses = bo.is_some_and(|p| if bid { p <= price } else { p >= price });
        if post && crosses {
            if fl & FLAG_ALO_SLIDE == 0 || k == KIND_PEG { continue; } // ALO: skip, don't fail the batch
            let p = bo.unwrap();
            price = if bid { p.saturating_sub(TICK) } else { p + TICK };
            if price == 0 { continue; }
        }
        if fok && !fok_avail(rest, pid, &mk, if bid { SIDE_ASK } else { SIDE_BID }, m.oracle_px, price, slot, &uk, qty) { continue; } // AUDIT M-3: skip only this order
        let (filled, abort) = if crosses && !post {
            let f = take(pid, rest, &mk, m, u, &uk, pi, bid, price, qty, slot, taker_bps, stp)?;
            (f.filled, f.abort)
        } else { (0, false) };
        if fok && filled < qty { return Err(err(E::FokUnfilled)); }
        let left = qty - filled;
        if left > 0 && !ioc && !abort {
            let (total, lvl, a2s) = if k == KIND_SPLINE {
                (a2 * left + a3 * (a2 * (a2 - 1) / 2), left, left) // a2 = n levels in the wire item; stored a2 = base qty
            } else { (left, 0, a2) };
            let n_lv = if k == KIND_SPLINE { a2 } else { 0 };
            let ord = Order { id: 0, owner: [0; 32], price, qty: total, expiry_slot: expiry, band_bps: band, cid,
                meta: k | (maker_rebate << 8) | (n_lv << 16), a1, a2: a2s, a3, lvl_left: lvl };
            if u.pos[pi].n_orders >= MAX_OPEN_ORDERS_PER_MARKET { return Err(err(E::Cap)); } // AUDIT M-2
            // LOAD-1: in a batch, a full bucket (64 orders) skips only this item (like ALO/FOK skips) instead of
            // failing the whole replace -- otherwise one crowded price bucket wipes every other quote and cancel in the tx.
            match insert(rest, pid, &mk, &uk, bid, ord, slot) {
                Ok(_) => {}
                Err(ProgramError::Custom(c)) if !single && c == E::ShardFull as u32 => continue,
                Err(e) => return Err(e),
            }
            if bid { u.pos[pi].bid_open += total } else { u.pos[pi].ask_open += total }
            u.pos[pi].n_orders += 1;
        }
    }
    if risk_up {
        if m.max_oi != 0 && m.oi_long > m.max_oi { return Err(err(E::Cap)); }
        let p = &u.pos[pi]; // per-account position limit (gross incl. resting orders)
        if m.max_position != 0 && core::cmp::max((p.base as i128 + p.bid_open as i128).abs(), (p.base as i128 - p.ask_open as i128).abs()) > m.max_position as i128 { return Err(err(E::Cap)); }
        let (eq, im, _) = health(pid, u, &a[2..], nw, true)?;
        if eq < im { return Err(err(E::Margin)); }
        if u.pos[pi].flags & POS_ISO != 0 { let (e2, i2, _) = iso_health(pid, u, pi, &a[2..], nw, true)?; if e2 < i2 { return Err(err(E::Margin)); } }
    }
    gc_pos(u);
    Ok(())
}

// ---------- trigger orders: TP / SL / TWAP ----------
/// accs: authority(s), user(w), market(r). data: tag, slot u8, kind u8, side u8, trigger_px, limit_px, qty, tip, interval, slices (u64s)
fn set_trigger(pid: &Address, a: &mut [AccountView], d: &[u8]) -> ProgramResult {
    if a.len() < 3 || d.len() < 4 + 6 * 8 { return Err(err(E::BadData)); }
    let u: &mut User = load(&a[1], pid, DISC_USER, USER_SIZE)?;
    let _m: &Market = load(&a[2], pid, DISC_MARKET, MARKET_SIZE)?;
    if !a[0].is_signer() || u.authority != key(&a[0]) { return Err(err(E::BadAccount)); }
    let (s, k, side) = (d[1] as usize, d[2] as u64, d[3] as u64);
    let v = |i: usize| rd64(d, 4 + 8 * i);
    // kind byte: low 6 bits = TP/SL/TWAP, bit 6 = FLAG_MARK (>>2 of 256), bit 7 = FLAG_CONFIRM_ORACLE (>>2 of 512)
    let k = (k & 0x3f) | ((k >> 6 & 1) << 8) | ((k >> 7 & 1) << 9);
    if s >= MAX_TRIGGERS || k & 0xff > TRIGGER_KIND_TWAP || side > 1 { return Err(err(E::BadData)); }
    let t = Trigger { market: key(&a[2]), kind: k, side, trigger_px: v(0)?, limit_px: v(1)?, qty: v(2)?, tip: v(3)?, interval: v(4)?,
        next_slot: clock()?.slot, slices_left: v(5)? };
    if k & 0xff == TRIGGER_KIND_TWAP && (t.slices_left == 0 || t.interval == 0) { return Err(err(E::BadData)); }
    u.triggers[s] = t;
    Ok(())
}
/// Permissionless keeper crank. accs: keeper(s), keeper user(w), target user(w), market(r), [fast book(w), slabs(r)...], shards(w)..., [other markets]
/// data: tag, slot u8
fn exec_trigger(pid: &Address, a: &mut [AccountView], d: &[u8]) -> ProgramResult {
    if a.len() < 5 || d.len() < 2 || a[1].address() == a[2].address() { return Err(err(E::BadData)); }
    let kp: &mut User = load(&a[1], pid, DISC_USER, USER_SIZE)?;
    let t: &mut User = load(&a[2], pid, DISC_USER, USER_SIZE)?;
    let m: &Market = load(&a[3], pid, DISC_MARKET, MARKET_SIZE)?;
    if !a[0].is_signer() || kp.authority != key(&a[0]) { return Err(err(E::BadAccount)); }
    home(kp, m)?; home(t, m)?;
    let (tk, mk) = (key(&a[2]), key(&a[3]));
    let si = d[1] as usize;
    if si >= MAX_TRIGGERS { return Err(err(E::BadData)); }
    let tr = t.triggers[si];
    if tr.kind == 0 || tr.market != mk { return Err(err(E::NotTriggered)); }
    let clk = clock()?; let slot = clk.slot; let nw: Now = (slot, clk.unix_timestamp);
    if stale(m, nw) { return Err(err(E::Stale)); }
    let bid = tr.side == SIDE_BID; let k8 = tr.kind & 0xff;
    let thr = |px: u64| match k8 {
        TRIGGER_KIND_TP => if bid { px <= tr.trigger_px } else { px >= tr.trigger_px },
        TRIGGER_KIND_SL => if bid { px >= tr.trigger_px } else { px <= tr.trigger_px },
        _ => false,
    };
    // same rule as perp_core::trigger_fires: oracle by default; FLAG_MARK = mark, + FLAG_CONFIRM_ORACLE = both
    let fire = if k8 == TRIGGER_KIND_TWAP { slot >= tr.next_slot }
        else if tr.kind & spec::TRIGGER_KIND_FLAG_MARK != 0 { thr(mark(m)) && (tr.kind & spec::TRIGGER_KIND_FLAG_CONFIRM_ORACLE == 0 || thr(m.oracle_px)) }
        else { thr(m.oracle_px) };
    if !fire { return Err(err(E::NotTriggered)); }
    let rest = &a[4..];
    claim_all(pid, t, &tk, rest)?;
    settle_funding_all(pid, t, &a[3..])?;
    let pi = pos_idx(t, &mk, m.funding_index)?;
    let twap = k8 == TRIGGER_KIND_TWAP;
    let mut qty = if twap { tr.qty / tr.slices_left } else { tr.qty };
    if !twap {
        let b = t.pos[pi].base;
        let cap = if bid && b < 0 { (-b) as u64 } else if !bid && b > 0 { b as u64 } else { 0 };
        qty = qty.min(cap);
    } else if m.paused != 0 { return Err(err(E::Paused)); }
    let taker_bps = TIERS[(t.tier as usize).min(TIERS.len() - 1)].0;
    if qty == 0 && !twap { t.triggers[si].kind = 0; gc_pos(t); return Ok(()); } // nothing to reduce: clear, no tip
    if qty == 0 { return Err(err(E::NotTriggered)); }
    // a FastBook among the accounts: fill against maker quotes and resting orders (best price first), same rules as take_fast
    let filled = if let Some(fx) = rest.iter().find(|x| disc_of(x) == DISC_FAST) {
        let fb: &mut FastBook = load(fx, pid, DISC_FAST, FAST_SIZE)?;
        let opp = if bid { 1usize } else { 0usize };
        if fb.market != mk || (fb.side != 0 && fb.side != opp as u64 + 1) { return Err(err(E::BadAccount)); }
        let px0 = mark(m); let band = px0 / 10000 * spec::QUOTE_BAND_BPS; let (blo, bhi) = bound_range(m);
        let (lo, hi) = (px0.saturating_sub(band).max(blo), (px0 + band).min(bhi));
        let (sl, cnt) = fast_slabs(pid, fb, rest, &tk)?;
        fast_match(pid, fb, &sl[..cnt], (fb.idx as usize) << 4 | opp << 3, opp, bid, lo, hi, rest, &mk, m, t, &tk, pi, tr.limit_px, qty, slot, taker_bps)?
    } else { take(pid, rest, &mk, m, t, &tk, pi, bid, tr.limit_px, qty, slot, taker_bps, STP_CANCEL_OLDEST)?.filled };
    if filled == 0 { return Err(err(E::NotTriggered)); } // AUDIT M-1: no tip / no slice burn without a fill
    let w = &mut t.triggers[si];
    if twap {
        w.qty -= qty.min(w.qty); w.slices_left -= 1; w.next_slot = slot + tr.interval;
        if w.slices_left == 0 { w.kind = 0; }
        let (eq, im, _) = health(pid, t, &a[3..], nw, true)?;
        if eq < im { return Err(err(E::Margin)); }
    } else { w.kind = 0; }
    let tip = core::cmp::min(tr.tip, MAX_KEEPER_TIP) as i64;
    t.cash = i64c(t.cash as i128 - tip as i128)?; kp.cash = i64c(kp.cash as i128 + tip as i128)?;
    emit(&[&[EVENTS_TRIGGER as u8, si as u8, tr.kind as u8], &tk, &filled.to_le_bytes()]);
    gc_pos(t);
    Ok(())
}

// ---------- admin / cranks ----------
/// accs: admin(s), market(w). data: tag, op u8, value u64
fn admin(pid: &Address, a: &mut [AccountView], d: &[u8]) -> ProgramResult {
    if a.len() < 2 || d.len() < 10 { return Err(err(E::BadData)); }
    let m: &mut Market = load(&a[1], pid, DISC_MARKET, MARKET_SIZE)?;
    if !auth(pid, &m.admin, a) { return Err(err(E::BadAccount)); }
    let v = rd64(d, 2)?;
    // immediate: pause and new-risk caps (cannot move funds). Everything else goes through the timelock.
    // AUDIT V5-4: on a pooled market the OI cap can only be lowered (never above the value admitted under the pool cap)
    if m.pool != [0; 32] && d[1] as u64 == ADMIN_MAX_OI && (v == 0 || v > m.max_oi) { return Err(err(E::Cap)); }
    match d[1] as u64 { ADMIN_PAUSE => m.paused = v, ADMIN_TVL_CAP => m.tvl_cap = v, ADMIN_MAX_OI => m.max_oi = v,
        spec::ADMIN_MAX_POSITION => m.max_position = v, _ => return Err(err(E::Timelock)) }
    Ok(())
}
/// Timelocked admin change. accs: admin(s), market(w). data: tag, op u8 (0 = cancel), val u64, [key 32]
fn admin_propose(pid: &Address, a: &mut [AccountView], d: &[u8]) -> ProgramResult {
    if a.len() < 2 || d.len() < 10 { return Err(err(E::BadData)); }
    let m: &mut Market = load(&a[1], pid, DISC_MARKET, MARKET_SIZE)?;
    if !auth(pid, &m.admin, a) { return Err(err(E::BadAccount)); }
    let op = d[1] as u64;
    if !(op == 0 || (spec::ADMIN_IMR..=spec::ADMIN_ORACLE2).contains(&op)) || op == spec::ADMIN_MAX_POSITION { return Err(err(E::BadData)); }
    let pv = rd64(d, 2)?;
    if m.pool != [0; 32] && match op { spec::ADMIN_IMR => pv < IMR_BPS, spec::ADMIN_MMR => pv < MMR_BPS, spec::ADMIN_SET_ADMIN => true, _ => false } {
        return Err(err(E::Cap)); // AUDIT V5-4 (also re-checked at execute)
    }
    m.pending_op = op; m.pending_val = pv; m.pending_key = [0; 32];
    if d.len() >= 42 { m.pending_key.copy_from_slice(&d[10..42]); }
    if op == spec::ADMIN_ORACLE2 { // fallback disabled immediately (safe direction); new feed id armed, key set at execute
        m.oracle2 = [0; 32]; if d.len() >= 74 { m.feed_id2.copy_from_slice(&d[42..74]); }
    }
    m.pending_eta = clock()?.slot + spec::TIMELOCK_SLOTS;
    Ok(())
}
/// Permissionless once the timelock expired. accs: market(w)
fn admin_execute(pid: &Address, a: &mut [AccountView]) -> ProgramResult {
    if a.is_empty() { return Err(err(E::BadData)); }
    let m: &mut Market = load(&a[0], pid, DISC_MARKET, MARKET_SIZE)?;
    if m.pending_op == 0 || clock()?.slot < m.pending_eta { return Err(err(E::Timelock)); }
    let v = m.pending_val;
    // AUDIT V5-4: pooled markets: margin can only be tightened (never below the spec defaults) and the admin is not transferable
    if m.pool != [0; 32] {
        let bad = match m.pending_op { spec::ADMIN_IMR => v < IMR_BPS, spec::ADMIN_MMR => v < MMR_BPS, spec::ADMIN_SET_ADMIN => true, _ => false };
        if bad { return Err(err(E::Cap)); }
    }
    match m.pending_op {
        spec::ADMIN_IMR => { if v <= m.mmr_bps || v > 10000 { return Err(err(E::BadData)); } m.imr_bps = v }
        spec::ADMIN_MMR => { if v == 0 || v >= m.imr_bps { return Err(err(E::BadData)); } m.mmr_bps = v }
        spec::ADMIN_IMPACT_NOTIONAL => { if v == 0 { return Err(err(E::BadData)); } m.impact_notional = v }
        spec::ADMIN_SET_ADMIN => { if m.pending_key == [0; 32] { return Err(err(E::BadData)); } m.admin = m.pending_key }
        spec::ADMIN_FUNDING_PERIOD => { if v == 0 { return Err(err(E::BadData)); } m.funding_period = v }
        spec::ADMIN_ORACLE2 => { m.oracle2 = m.pending_key }
        _ => return Err(err(E::BadData)),
    }
    m.pending_op = 0; m.pending_eta = 0;
    Ok(())
}
/// MM priority tier (fee tier / rebate). accs: admin(s), market(r), user(w). data: tag, tier u8
fn set_tier(pid: &Address, a: &mut [AccountView], d: &[u8]) -> ProgramResult {
    if a.len() < 3 || d.len() < 2 { return Err(err(E::BadData)); }
    let m: &Market = load(&a[1], pid, DISC_MARKET, MARKET_SIZE)?;
    let u: &mut User = load(&a[2], pid, DISC_USER, USER_SIZE)?;
    if !auth(pid, &m.admin, a) || d[1] as usize >= TIERS.len() { return Err(err(E::BadAccount)); }
    home(u, m)?; // AUDIT M-4: a market admin can only tier accounts bound to its own vault
    u.tier = d[1] as u64;
    Ok(())
}
/// Permissionless: push owed maker fills from one shard ledger into the passed user accounts (drains a full ledger).
/// accs: shard(w), users(w)...
fn crank_claim(pid: &Address, a: &mut [AccountView]) -> ProgramResult {
    if a.len() < 2 { return Err(err(E::BadData)); }
    let s: &mut Shard = load(&a[0], pid, DISC_SHARD, SHARD_SIZE)?;
    for x in a[1..].iter() {
        let u: &mut User = load(x, pid, DISC_USER, USER_SIZE)?;
        claim(u, &key(x), s)?;
    }
    Ok(())
}

// ---------- oracle / funding ----------
/// |a - b| > b * bps / 1e4, in native u64 (saturating: a product that would overflow counts as a move / huge bound)
#[inline(always)]
fn moved(a: u64, b: u64, bps: u64) -> bool { a.abs_diff(b).saturating_mul(10000) > b.saturating_mul(bps) }
/// accs: market(w), pyth PriceUpdateV2, [best bid shard, best ask shard] (optional, for premium)
fn update_oracle(pid: &Address, a: &mut [AccountView]) -> ProgramResult {
    if a.len() < 2 { return Err(err(E::BadData)); }
    let m: &mut Market = load(&a[0], pid, DISC_MARKET, MARKET_SIZE)?;
    let clk = clock()?;
    let r1 = if m.oracle == key(&a[1]) { read_pyth(pid, &a[1], &m.feed_id, m, clk.unix_timestamp) } else { Err(E::Oracle) };
    // oracle2 configured but not passed counts as unusable (the crank can't dodge the cross-check by omitting it)
    let r2 = if m.oracle2 != [0; 32] { Some(a[2..].iter().find(|x| key(x) == m.oracle2).map_or(Err(E::Oracle), |x| read_pyth(pid, x, &m.feed_id2, m, clk.unix_timestamp))) } else { None };
    // oracle2: fallback when the primary is unusable; circuit breaker when both are valid but diverge
    // AUDIT V5-5: hysteresis (trip > MAX_DIVERGENCE, auto-resume only <= RESUME) and a move check whenever
    // only one source is usable while oracle2 is configured. Manual resume: admin ADMIN_PAUSE 0 (re-trips if
    // the sources still diverge).
    let single = |m: &mut Market, p: (u64, u64, i64)| -> Option<(u64, u64, i64)> {
        if m.oracle_px != 0 && moved(p.0, m.oracle_px, spec::ORACLE_SINGLE_SOURCE_MAX_MOVE_BPS) {
            if m.paused == 0 { m.paused = spec::PAUSED_SINGLE_SOURCE; }
            return None;
        }
        Some(p)
    };
    let (px, conf_px, pt) = match (r1, r2) {
        (Ok(p1), Some(Ok(p2))) => {
            let d = (p1.0 as i128 - p2.0 as i128).abs() * 10000; let lo = p1.0.min(p2.0) as i128;
            if d > lo * spec::ORACLE_MAX_DIVERGENCE_BPS as i128 {
                if m.paused == 0 { m.paused = PAUSED_BREAKER; }
                return Ok(()); // prices not updated; new risk halted until the sources re-converge
            }
            if m.paused == PAUSED_BREAKER || m.paused == spec::PAUSED_SINGLE_SOURCE {
                if d > lo * spec::ORACLE_RESUME_BPS as i128 { return Ok(()); } // hysteresis band: stay tripped
                m.paused = 0;
            }
            p1
        }
        (Ok(p1), Some(Err(_))) => match single(m, p1) { Some(p) => p, None => return Ok(()) },
        // AUDIT V6-2: no oracle2 -> jump guard with confirmation. A move > SINGLE_SOURCE_MAX_MOVE vs the stored price
        // is held as a candidate (oracle2_px / feed_id2[0..8] are unused without oracle2) and pauses new risk; it is
        // accepted only when a LATER publish (newer publish_time) confirms it within RESUME_BPS.
        (Ok(p1), None) => {
            let jump = m.oracle_px != 0 && moved(p1.0, m.oracle_px, spec::ORACLE_SINGLE_SOURCE_MAX_MOVE_BPS);
            // (while an oracle2 change is pending, feed_id2 holds the armed feed id: no candidate, jumps just pause)
            let slot_free = m.pending_op != spec::ADMIN_ORACLE2;
            if jump {
                if !slot_free { if m.paused == 0 { m.paused = spec::PAUSED_SINGLE_SOURCE; } return Ok(()); }
                let (c, cts) = (m.oracle2_px, i64::from_le_bytes(m.feed_id2[0..8].try_into().unwrap()));
                let confirmed = c != 0 && p1.2 > cts && !moved(p1.0, c, spec::ORACLE_RESUME_BPS);
                if !confirmed {
                    m.oracle2_px = p1.0; m.feed_id2[0..8].copy_from_slice(&p1.2.to_le_bytes());
                    if m.paused == 0 { m.paused = spec::PAUSED_SINGLE_SOURCE; }
                    return Ok(());
                }
            }
            if slot_free && m.oracle2_px != 0 { m.oracle2_px = 0; m.feed_id2[0..8].copy_from_slice(&[0; 8]); }
            if m.paused == spec::PAUSED_SINGLE_SOURCE { m.paused = 0; }
            p1
        }
        (Err(_), Some(Ok(p2))) => match single(m, p2) { Some(p) => p, None => return Ok(()) },
        (Err(e), _) => return Err(err(e)),
    };
    let slot = clk.slot;
    let ds = slot.saturating_sub(m.funding_slot);
    // 1) accrue the index (quote per lot) with the current hourly rate at the stored oracle; remainder carried exactly
    if ds > 0 && m.funding_rate != 0 && m.oracle_px != 0 {
        let den = FUNDING_PPB as i128 * m.funding_period as i128;
        // CU: native i64 math when every factor is small enough that the product can't overflow (same result)
        if m.funding_rate.unsigned_abs() < (1 << 26) && m.oracle_px < (1 << 28) && ds < (1 << 8) && m.funding_rem.unsigned_abs() < (1 << 61) && den < (1 << 62) {
            let num = m.funding_rate * m.oracle_px as i64 * ds as i64 + m.funding_rem; let den = den as i64;
            m.funding_index = i64c(m.funding_index as i128 + num.div_euclid(den) as i128)?; m.funding_rem = num.rem_euclid(den);
        } else {
            let num = m.funding_rate as i128 * m.oracle_px as i128 * ds as i128 + m.funding_rem as i128;
            m.funding_index = i64c(m.funding_index as i128 + num.div_euclid(den))?; m.funding_rem = num.rem_euclid(den) as i64;
        }
    }
    // 2) premium TWAP: previous sample held over the elapsed slots
    m.prem_acc = i64c(m.prem_acc as i128 + m.prem_last as i128 * ds as i128)?; m.prem_slots += ds;
    // 3) period roll: F = avgP + clamp(I - avgP, -c, c) (8h terms), paid per period at F/8, capped (HL formula)
    if slot >= m.period_start + m.funding_period {
        m.funding_rate = funding_rate_from(m.prem_acc, m.prem_slots);
        m.prem_acc = 0; m.prem_slots = 0; m.period_start = slot;
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
    // maker slabs count as book too (complete approved set, or none)
    let (fb, fa) = fast_bbo(sh, pid, &mk, slot, px)?;
    let (ib, ia, sb, sa) = book.unwrap_or((None, None, None, None));
    let best_b = match (sb, fb) { (Some(x), Some(y)) => Some(x.max(y)), (x, y) => x.or(y) };
    let best_a = match (sa, fa) { (Some(x), Some(y)) => Some(x.min(y)), (x, y) => x.or(y) };
    let ext = read_ext(&a[1], pid, m, clk.unix_timestamp);
    if ext.is_some() { m.ext_px = ext.unwrap_or(0); m.ext_ts = clk.unix_timestamp; }
    if book.is_some() || fb.is_some() || fa.is_some() {
        m.prem_last = premium_ppb(ib.or(fb), ia.or(fa), px);
        let mid = match (best_b, best_a) { (Some(b), Some(a)) => Some((b + a) / 2), _ => None };
        // 5) mark = clamp(median(oracle, oracle + EMA(mid - oracle), mid), oracle +- MARK_CLAMP_BPS)
        //    with a fresh external perp price: median(oracle + EMA, mid, external)
        let (e, mp) = mark_from(px, m.mark_ema as i128, ds, mid, ext); m.mark_ema = i64c(e)?; m.mark_px = mp;
    } else {
        let (_, mp) = mark_from(px, m.mark_ema as i128, 0, Some((px as i128 + m.mark_ema as i128).max(1) as u64), ext); m.mark_px = mp;
    }
    m.oracle_px = px; m.oracle_conf = conf_px; m.oracle_slot = clk.slot; m.oracle_ts = pt;
    apply_bounds(m, px);
    Ok(())
}
/// Single pass: full-set check (AUDIT N-1), impact bid/ask (AUDIT N-2 level rule) and mid of the best prices.
/// Ok(None) if no shard of the market was passed. Results identical to full_set + impact_px x2 + best x2.
fn scan_book(sh: &[AccountView], pid: &Address, mk: &Key, n: u64, oracle: u64, slot: u64, notional: u64)
    -> Result<Option<(Option<u64>, Option<u64>, Option<u64>, Option<u64>)>, ProgramError> {
    let mut seen = [0u64; 4]; let mut k = 0u64;
    let mut bufs = [[(0u64, 0u64); IMPACT_MAX]; 2]; let mut ns = [0usize; 2];
    let mut bb: [Option<u64>; 2] = [None, None];
    for x in sh {
        if disc_of(x) != DISC_SHARD || !x.owned_by(pid) { continue; }
        let s: &Shard = unsafe { &*(x.borrow_unchecked().as_ptr() as *const Shard) };
        if s.market != *mk { continue; }
        let no = s.shard_no as usize; if no >= 256 { return Err(err(E::MissingShards)); }
        if seen[no / 64] >> (no % 64) & 1 == 1 { return Err(err(E::BadAccount)); }
        seen[no / 64] |= 1 << (no % 64); k += 1;
        let bid = s.side == SIDE_BID; let sd = if bid { 0 } else { 1 };
        if let Some((_, p)) = shard_best(s, oracle) {
            bb[sd] = Some(match bb[sd] { None => p, Some(q) => if bid { p.max(q) } else { p.min(q) } });
        }
        let sorted = s.seq & SPECIAL_BIT == 0;
        let (buf, nn) = (&mut bufs[sd], &mut ns[sd]);
        for o in &s.orders[..s.n as usize] {
            let p = eff(o, bid, oracle);
            if (bid && p <= oracle) || (!bid && p >= oracle) { if sorted { break; } continue; }
            if o.expiry_slot != 0 && slot > o.expiry_slot { continue; }
            if o.band_bps != 0 && off_band(p, oracle, o.band_bps) { continue; }
            let mut i = *nn; while i > 0 && (if bid { p > buf[i - 1].0 } else { p < buf[i - 1].0 }) { i -= 1; }
            if i > 0 && buf[i - 1].0 == p { buf[i - 1].1 = buf[i - 1].1.saturating_add(top(o)); continue; }
            if i >= IMPACT_MAX { if sorted { break; } continue; }
            let end = if *nn == IMPACT_MAX { *nn - 1 } else { *nn };
            buf.copy_within(i..end, i + 1); buf[i] = (p, top(o)); *nn = end + 1;
        }
    }
    if k == 0 { return Ok(None); }
    if k != n { return Err(err(E::MissingShards)); }
    let imp = |buf: &[(u64, u64)]| -> Option<u64> {
        // CU: native u64 walk when the notional is small enough that an + one level can't overflow (same result)
        if notional < (1 << 62) {
            let (mut an, mut aq) = (0u64, 0u64);
            for &(p, q) in buf {
                let need = (notional - an).div_ceil(p);
                let take = if q < need { q } else { need };
                an += take * p; aq += take; // take*p <= notional - an + p - 1 < 2^63
                if an >= notional { return Some(an / aq); }
            }
            return None;
        }
        let (mut an, mut aq) = (0u128, 0u128);
        for &(p, q) in buf {
            let left = notional as u128 - an;
            let take = (q as u128).min(left.div_ceil(p as u128));
            an += take * p as u128; aq += take;
            if an >= notional as u128 { return Some((an / aq) as u64); }
        }
        None
    };
    Ok(Some((imp(&bufs[0][..ns[0]]), imp(&bufs[1][..ns[1]]), bb[0], bb[1])))
}
/// External perp consensus from our own oracle account, in market price units. None when old, absent or from Pyth.
fn read_ext(x: &AccountView, pid: &Address, m: &Market, now: i64) -> Option<u64> {
    if !x.owned_by(pid) || x.data_len() < AGG_SIZE { return None; }
    let p = unsafe { x.borrow_unchecked() };
    if p[0..8] != AGG_DISC { return None; }
    let r = |o: usize| i64::from_le_bytes(p[o..o + 8].try_into().unwrap());
    let (v, ts) = (r(133), r(141));
    if v <= 0 || (now - ts).abs() > spec::EXT_MAX_AGE_SECS { return None; }
    let q = (v as i128 * m.px_mul as i128 / m.px_div as i128) as u64;
    if q == 0 { None } else { Some(q) }
}
/// Best bid / ask from approved maker slabs, for mid and premium. All approved slabs or none, like the shard set.
fn fast_bbo(sh: &[AccountView], pid: &Address, mk: &Key, slot: u64, px: u64) -> Result<(Option<u64>, Option<u64>), ProgramError> {
    let band = px / 10000 * spec::QUOTE_BAND_BPS;
    let (lo, hi) = (px.saturating_sub(band), px + band);
    let (mut bb, mut ba) = (None::<u64>, None::<u64>);
    // every FastBook passed contributes the sides it holds; each must come with all of its makers' slabs
    for fx in sh.iter().filter(|x| disc_of(x) == DISC_FAST) {
        let f: &FastBook = load(fx, pid, DISC_FAST, FAST_SIZE)?;
        if f.market != *mk { return Err(err(E::BadAccount)); }
        let mut seen = 0u64;
        for x in sh {
            if disc_of(x) != DISC_SLAB { continue; }
            let s: &Slab = load(x, pid, DISC_SLAB, SLAB_SIZE)?;
            let k = key(x);
            let Some(mi) = f.makers[..f.n as usize].iter().position(|e| e.slab == k) else { continue };
            if seen >> mi & 1 == 1 { return Err(err(E::BadAccount)); }
            seen |= 1 << mi;
            if s.kill != 0 || f.makers[mi].budget <= 0 { continue; }
            for sd in 0..2usize {
                if f.side != 0 && f.side != sd as u64 + 1 { continue; }
                let base = (f.idx as usize) << 4 | sd << 3;
                for l in 0..SLAB_LEVELS {
                    let q = &s.quotes[base + l]; let p = q.px as u64;
                    if q.qty == 0 || p < lo || p > hi || slot as u32 - q.slot.min(slot as u32) > spec::QUOTE_MAX_AGE_SLOTS { continue; }
                    let u_ = &f.used[mi][sd << 3 | l];
                    if u_.gen == q.gen && u_.qty >= q.qty { continue; }
                    if sd == 0 { bb = Some(bb.map_or(p, |v| v.max(p))); } else { ba = Some(ba.map_or(p, |v| v.min(p))); }
                }
            }
        }
        if seen != (1u64 << f.n) - 1 { return Err(err(E::MissingShards)); }
    }
    Ok((bb, ba))
}
/// PriceUpdateV2: disc8 | write_authority32 | verification_level (1 = Full) | feed_id32 | price i64 | conf u64 | expo i32 | publish_time i64 | ...
fn read_pyth(pid: &Address, x: &AccountView, feed: &Key, m: &Market, now: i64) -> Result<(u64, u64, i64), E> {
    if x.data_len() < 133 { return Err(E::Oracle); }
    let p = unsafe { x.borrow_unchecked() };
    let ok = (x.owned_by(&PYTH_RECEIVER) && p[0..8] == PYTH_PRICE_UPDATE_DISC) || (x.owned_by(pid) && p[0..8] == AGG_DISC);
    if !ok || p[40] != 1 || p[41..73] != *feed { return Err(E::Oracle); }
    let r = |o: usize| u64::from_le_bytes(p[o..o + 8].try_into().unwrap());
    let (price, conf) = (r(73) as i64, r(81));
    let expo = i32::from_le_bytes(p[89..93].try_into().unwrap()) as i64;
    let pt = r(93) as i64;
    if price <= 0 || expo != m.expo { return Err(E::Oracle); }
    if now - pt > m.max_age_secs as i64 { return Err(E::Stale); }
    if pt < m.oracle_ts { return Err(E::Stale); } // AUDIT L-4: no replay of an older update
    if (conf as u128) * 10000 > (price as u128) * m.max_conf_bps as u128 { return Err(E::Oracle); }
    // CU: native u64 division for the common px_mul == 1 config (same result: both operands positive)
    let (px, conf_px) = if m.px_mul == 1 && m.px_div > 0 { (price as u64 / m.px_div as u64, conf / m.px_div as u64) } else {
        ((price as i128 * m.px_mul as i128 / m.px_div as i128) as u64, (conf as i128 * m.px_mul as i128 / m.px_div as i128) as u64) };
    if px == 0 { return Err(E::Oracle); }
    Ok((px, conf_px, pt))
}
fn mark(m: &Market) -> u64 { if m.mark_px != 0 { m.mark_px } else { m.oracle_px } }
/// Same as perp_core::mark_from.
fn mark_from(px: u64, ema: i128, ds: u64, mid: Option<u64>, ext: Option<u64>) -> (i128, u64) {
    let s = match mid { Some(x) => x as i128 - px as i128, None => 0 };
    let w = core::cmp::min(ds, spec::MARK_EMA_SLOTS) as i128;
    let d = s - ema;
    // CU: native i64 when |d| * w can't overflow (same truncating division)
    let e = if d.unsigned_abs() < (1 << 40) && w < (1 << 20) { ema + ((d as i64 * w as i64) / spec::MARK_EMA_SLOTS as i64) as i128 } else { ema + d * w / spec::MARK_EMA_SLOTS as i128 };
    let adj = px as i128 + e;
    let (a, b, c) = match ext { Some(x) => (adj, mid.map_or(adj, |v| v as i128), x as i128), None => (px as i128, adj, mid.unwrap_or(px) as i128) };
    let med = a.min(b).max(a.max(b).min(c));
    let band = if px < (1 << 48) { (px * spec::MARK_CLAMP_BPS / 10000) as i128 } else { px as i128 * spec::MARK_CLAMP_BPS as i128 / 10000 };
    (e, med.clamp((px as i128 - band).max(1), px as i128 + band) as u64)
}
/// Hourly funding rate (ppb of oracle per period) from the period's premium TWAP (ppb).
fn funding_rate_from(acc: i64, n: u64) -> i64 {
    let avg = if n > 0 { acc as i128 / n as i128 } else { 0 };
    let (i8h, c8h, cap) = (FUNDING_INTEREST_8H_PPB as i128, FUNDING_PREMIUM_CLAMP_8H_PPB as i128, FUNDING_MAX_RATE_HOURLY_PPB as i128);
    let f8 = avg + (i8h - avg).clamp(-c8h, c8h);
    (f8 / FUNDING_HOURS_PER_8H as i128).clamp(-cap, cap) as i64
}
/// premium = (max(impact_bid - oracle, 0) - max(oracle - impact_ask, 0)) / oracle, in ppb.
fn premium_ppb(ib: Option<u64>, ia: Option<u64>, o: u64) -> i64 {
    let up = ib.map_or(0, |b| b.saturating_sub(o)) as i128;
    let dn = ia.map_or(0, |x| o.saturating_sub(x)) as i128;
    let x = up - dn;
    if x.unsigned_abs() < (1 << 33) && o < (1 << 62) { (x as i64 * FUNDING_PPB as i64) / o as i64 } else { (x * FUNDING_PPB as i128 / o as i128) as i64 }
}
const PAUSED_BREAKER: u64 = spec::PAUSED_BREAKER;
const IMPACT_MAX: usize = spec::IMPACT_MAX_LEVELS as usize;
/// accs: user(w), markets... (permissionless lazy funding settlement)
fn settle(pid: &Address, a: &mut [AccountView]) -> ProgramResult {
    let u: &mut User = load(&a[0], pid, DISC_USER, USER_SIZE)?;
    settle_funding_all(pid, u, &a[1..])
}
/// accs: market(w), shards(w)... : fold per-shard OI / fee counters into Market (crank, off the trade path)
fn aggregate(pid: &Address, a: &mut [AccountView]) -> ProgramResult {
    let m: &mut Market = load(&a[0], pid, DISC_MARKET, MARKET_SIZE)?;
    let mk = key(&a[0]);
    for x in a[1..].iter() {
        if disc_of(x) == DISC_FAST {
            let f: &mut FastBook = load(x, pid, DISC_FAST, FAST_SIZE)?;
            if f.market != mk { return Err(err(E::BadAccount)); }
            m.oi_long = (m.oi_long as i64 + f.oi_long_delta) as u64; f.oi_long_delta = 0;
            m.fees += f.fee_accum; f.fee_accum = 0; continue;
        }
        if disc_of(x) == DISC_INS {
            let ins: &mut Insurance = load(x, pid, DISC_INS, INS_SIZE)?;
            if ins.market != mk { return Err(err(E::BadAccount)); }
            m.oi_long = (m.oi_long as i64 + ins.oi_long_delta) as u64; ins.oi_long_delta = 0; continue;
        }
        let s: &mut Shard = load(x, pid, DISC_SHARD, SHARD_SIZE)?;
        if s.market != mk { return Err(err(E::BadAccount)); }
        m.oi_long = (m.oi_long as i64 + s.oi_long_delta) as u64; s.oi_long_delta = 0;
        m.fees += s.fee_accum; s.fee_accum = 0;
    }
    Ok(())
}

// ---------- liquidation / ADL ----------
/// AUDIT M-6: liquidation is a takeover at the protocol price (no book, no shard choice, no price discretion):
/// the liquidator assumes min(|base|, max_qty) of the target's position at the mark price, the target pays
/// LIQ_FEE_BPS to the liquidator and the same to insurance (capped by remaining equity). The liquidator must
/// stay above initial margin. Same terms for every liquidator -> self-dealing gains nothing.
/// accs: liquidator authority(s), liquidator user(w), target user(w), market(r), insurance(w), target-order shards(w)..., [other markets]
/// data: tag, max_qty u64
fn liquidate(pid: &Address, a: &mut [AccountView], d: &[u8]) -> ProgramResult { liquidate_core(pid, a, d, false) }
fn liquidate_core(pid: &Address, a: &mut [AccountView], d: &[u8], vlt: bool) -> ProgramResult {
    if a.len() < 5 { return Err(err(E::BadData)); }
    let max_qty = rd64(d, 1)?;
    if a[1].address() == a[2].address() { return Err(err(E::BadAccount)); }
    let lq: &mut User = load(&a[1], pid, DISC_USER, USER_SIZE)?;
    let t: &mut User = load(&a[2], pid, DISC_USER, USER_SIZE)?;
    let m: &Market = load(&a[3], pid, DISC_MARKET, MARKET_SIZE)?;
    let ins: &mut Insurance = load(&a[4], pid, DISC_INS, INS_SIZE)?;
    let (tk, mk) = (key(&a[2]), key(&a[3]));
    if (!vlt && !a[0].is_signer()) || lq.authority != key(&a[0]) || m.insurance != key(&a[4]) { return Err(err(E::BadAccount)); }
    home(lq, m)?; home(t, m)?;
    let clk = clock()?; let nw: Now = (clk.slot, clk.unix_timestamp);
    if stale(m, nw) { return Err(err(E::Stale)); }
    let mkts = &a[3..];
    claim_all(pid, t, &tk, &a[5..])?;
    settle_funding_all(pid, t, mkts)?; settle_funding_all(pid, lq, mkts)?;
    let pi = t.pos.iter().position(|p| p.market == mk).ok_or(err(E::NotLiquidatable))?;
    let (eq, _, mm) = phealth(pid, t, pi, mkts, nw, true)?;
    // the vault steps in only once the book had its chance: equity under 2/3 of maintenance
    if eq >= (if vlt { mm * 2 / 3 } else { mm }) { return Err(err(E::NotLiquidatable)); }
    cancel_all_of(&a[5..], pid, &mk, &tk, t, pi); // free the target's resting margin first
    // AUDIT M-2: the liquidator must pass every shard holding the target's orders / unclaimed fills (bounded by n_orders cap)
    if t.pos[pi].bid_open != 0 || t.pos[pi].ask_open != 0 || t.pos[pi].n_orders != 0 { return Err(err(E::MissingShards)); }
    let b = t.pos[pi].base;
    if b == 0 { return Err(err(E::NotLiquidatable)); }
    let q = b.unsigned_abs().min(max_qty);
    if q == 0 { return Err(err(E::BadData)); }
    let px = mark(m);
    let li = pos_idx(lq, &mk, m.funding_index)?;
    if lq.pos[li].flags & POS_ISO != 0 { return Err(err(E::Isolated)); } // takeover lands in the liquidator's cross account
    let s: i64 = if b > 0 { 1 } else { -1 };
    let n = mulf(q as i128, px);
    let l0 = long(t.pos[pi].base) + long(lq.pos[li].base);
    let (otb, olb) = (t.pos[pi].base, lq.pos[li].base);
    t.pos[pi].base -= s * q as i64; lq.pos[li].base += s * q as i64;
    credit(t, pi, s as i128 * n)?; lq.cash = i64c(lq.cash as i128 - s as i128 * n)?;
    t.pos[pi].cost = cost_after(otb, t.pos[pi].base, t.pos[pi].cost, -(s as i128) * n)?; lq.pos[li].cost = cost_after(olb, lq.pos[li].base, lq.pos[li].cost, s as i128 * n)?;
    ins.oi_long_delta += long(t.pos[pi].base) + long(lq.pos[li].base) - l0;
    if m.max_position != 0 && lq.pos[li].base.unsigned_abs() > m.max_position { return Err(err(E::Cap)); }
    // CU: the takeover is at the mark and cancels don't touch cash, so equity is unchanged: eq2 == eq (no 2nd health pass)
    let eq2 = eq;
    let fee = bps_dn(n, LIQ_FEE_BPS).min(if eq2 > 0 { eq2 / 2 } else { 0 });
    credit(t, pi, -2 * fee)?;
    lq.cash = i64c(lq.cash as i128 + fee)?;
    ins.balance = i64c(ins.balance as i128 + fee)?;
    // AUDIT N-3/N-5: insurance never goes negative. A deficit larger than the fund can't be liquidated here:
    // the account goes through ADL (socialized against ranked counterparties, remainder -> Market.bad_debt only).
    if eq2 < 0 {
        // loss order: leftover insurance, then vault capital, then ADL
        let cap = ins.balance.max(0) as i128 + if vlt { lq.cash.max(0) as i128 } else { 0 };
        if -eq2 > cap { return Err(err(E::UseAdl)); }
        let closed = if t.pos[pi].flags & POS_ISO != 0 { t.pos[pi].base == 0 } else { t.pos.iter().all(|p| p.base == 0 || p.flags & POS_ISO != 0) };
        if closed {
            let from_ins = (-eq2).min(ins.balance.max(0) as i128); let from_v = -eq2 - from_ins;
            ins.balance = i64c(ins.balance as i128 - from_ins)?; lq.cash = i64c(lq.cash as i128 - from_v)?; credit(t, pi, -eq2)?;
        }
    }
    let (le, lim, _) = health(pid, lq, mkts, nw, true)?;
    if le < lim { return Err(err(E::Margin)); }
    emit(&[&[EVENTS_LIQUIDATION as u8], &tk, &q.to_le_bytes(), &px.to_le_bytes(), &key(&a[1])]);
    gc_pos(t); gc_pos(lq);
    Ok(())
}

/// AUDIT V5-3: book liquidation, no liquidator capital. Permissionless. Cancels the target's orders in this
/// market, then sends ONE reduce-only IOC chunk (<= LIQ_BOOK_CHUNK_BPS of |base|, <= max_qty, >= 1 lot) into the
/// book, limited to mark -/+ LIQ_BOOK_BAND_BPS. Fee (LIQ_FEE_BPS of filled x mark, capped at remaining equity)
/// -> insurance. A deficit above the insurance balance reverts with UseAdl (ADL path); insurance never < 0.
/// Same rule as perp_core::Exchange::liquidate_book (conformance-tested against core + C++).
/// accs: target user(w), market(r), insurance(w), shards(w)..., [other markets]. data: tag, max_qty u64
fn liquidate_book(pid: &Address, a: &mut [AccountView], d: &[u8]) -> ProgramResult {
    if a.len() < 4 { return Err(err(E::BadData)); }
    let max_qty = rd64(d, 1)?;
    let t: &mut User = load(&a[0], pid, DISC_USER, USER_SIZE)?;
    let m: &Market = load(&a[1], pid, DISC_MARKET, MARKET_SIZE)?;
    let ins: &mut Insurance = load(&a[2], pid, DISC_INS, INS_SIZE)?;
    if m.insurance != key(&a[2]) { return Err(err(E::BadAccount)); }
    home(t, m)?;
    let (tk, mk) = (key(&a[0]), key(&a[1]));
    let clk = clock()?; let nw: Now = (clk.slot, clk.unix_timestamp);
    if stale(m, nw) { return Err(err(E::Stale)); }
    let mkts = &a[1..];
    claim_all(pid, t, &tk, &a[3..])?;
    settle_funding_all(pid, t, mkts)?;
    let pi = t.pos.iter().position(|p| p.market == mk).ok_or(err(E::NotLiquidatable))?;
    let (eq, _, mm) = phealth(pid, t, pi, mkts, nw, true)?;
    if eq >= mm { return Err(err(E::NotLiquidatable)); }
    cancel_all_of(&a[3..], pid, &mk, &tk, t, pi);
    if t.pos[pi].bid_open != 0 || t.pos[pi].ask_open != 0 || t.pos[pi].n_orders != 0 { return Err(err(E::MissingShards)); }
    let b = t.pos[pi].base;
    if b == 0 { return Err(err(E::NotLiquidatable)); }
    let ab = b.unsigned_abs();
    let px = mark(m); let bid = b < 0;
    let q = ab.min(max_qty).min(liq_chunk(ab)).min(liq_needed(eq, mm, ab, px, m.mmr_bps));
    if q == 0 { return Err(err(E::BadData)); }
    let taker_bps = TIERS[(t.tier as usize).min(TIERS.len() - 1)].0;
    // AUDIT V6-1: escalate the band (5% -> 10% -> 20%); each wider step doubles the liquidation fee (penalty to insurance)
    let (mut filled, mut step) = (0u64, 0u64);
    while step < LIQ_BOOK_BAND_STEPS {
        let band = px * (LIQ_BOOK_BAND_BPS << step) / 10000;
        let limit = if bid { px + band } else { px.saturating_sub(band).max(1) };
        filled = take(pid, &a[3..], &mk, m, t, &tk, pi, bid, limit, q, clk.slot, taker_bps, STP_CANCEL_OLDEST)?.filled;
        if filled != 0 { break; }
        step += 1;
    }
    if filled == 0 {
        // No liquidity within the widest band. With EVERY shard of the market passed (proof the book is empty there),
        // the position becomes ADL-eligible for LIQ_ADL_WINDOW_SLOTS (ADL then accepts it while below MM, not only
        // when bankrupt). Without the full shard set: NoFill.
        if m.n_shards == 0 || m.n_shards > 64 { return Err(err(E::NoFill)); }
        let mut seen = 0u64;
        for x in a[3..].iter() {
            if let Ok(sh) = load::<Shard>(x, pid, DISC_SHARD, SHARD_SIZE) { if sh.market == mk && sh.shard_no < 64 { seen |= 1 << sh.shard_no; } }
        }
        let all = if m.n_shards == 64 { u64::MAX } else { (1u64 << m.n_shards) - 1 };
        if seen & all != all { return Err(err(E::NoFill)); }
        t.pos[pi].flags = (t.pos[pi].flags & POS_ISO) | spec::POS_FLAG_ADL_ELIGIBLE | (clk.slot << 8);
        emit(&[&[spec::EVENTS_LIQ_NOFILL as u8], &tk, &clk.slot.to_le_bytes()]);
        return Ok(());
    }
    let f = Fill1 { filled };
    let (eq2, _, _) = phealth(pid, t, pi, mkts, nw, false)?;
    let n = mulf(f.filled as i128, px);
    let fee = bps_dn(n, LIQ_FEE_BPS << step).min(eq2.max(0));
    credit(t, pi, -fee)?; pay_liq_fee(pid, &a[3..], &tk, m, ins, fee)?;
    if eq2 < 0 {
        if -eq2 > ins.balance.max(0) as i128 { return Err(err(E::UseAdl)); }
        let closed = if t.pos[pi].flags & POS_ISO != 0 { t.pos[pi].base == 0 } else { t.pos.iter().all(|p| p.base == 0 || p.flags & POS_ISO != 0) };
        if closed { ins.balance = i64c(ins.balance as i128 + eq2)?; credit(t, pi, -eq2)?; }
    }
    emit(&[&[EVENTS_LIQUIDATION as u8], &tk, &f.filled.to_le_bytes(), &px.to_le_bytes(), &[0u8; 32]]);
    gc_pos(t);
    Ok(())
}
/// Liquidation fee: LIQ_KEEPER_SHARE_BPS goes to the keeper that sent the liquidation (the first other User account passed;
/// it must hold cash against the same vault), the rest to insurance. Without a keeper account all of it goes to insurance.
/// Paying whoever does the work lets anyone run a liquidator, so liquidations don't hang on one operator.
fn pay_liq_fee(pid: &Address, rest: &[AccountView], tk: &Key, m: &Market, ins: &mut Insurance, fee: i128) -> ProgramResult {
    let mut to_ins = fee;
    if let Some(x) = rest.iter().find(|x| x.owned_by(pid) && disc_of(x) == DISC_USER && key(x) != *tk) {
        let k: &mut User = load(x, pid, DISC_USER, USER_SIZE)?;
        if k.home_vault != m.vault { return Err(err(E::Vault)); }
        let share = fee * spec::LIQ_KEEPER_SHARE_BPS as i128 / 10000;
        k.cash = i64c(k.cash as i128 + share)?; to_ins -= share;
    }
    ins.balance = i64c(ins.balance as i128 + to_ins)?;
    Ok(())
}
struct Fill1 { filled: u64 }
/// Lots to close so that afterwards equity >= 1.25 x maintenance. Closing q of |base| lots at the mark frees q/|base| of
/// the maintenance requirement and costs at most the widest-band fee (LIQ_FEE_BPS << 2), so
/// q >= (1.25 mm - eq) / (notional x (1.25 mmr - worst fee) / |base|). Partial liquidation like Hyperliquid's: an account just
/// under maintenance loses a sliver instead of a fixed 25% chunk (the chunk stays as the cap per transaction). At least 1 lot.
fn liq_needed(eq: i128, mm: i128, ab: u64, px: u64, mmr_bps: u64) -> u64 {
    let worst_fee = (LIQ_FEE_BPS << (LIQ_BOOK_BAND_STEPS - 1)) as i128;
    let relief = mulf(ab as i128, px) * (5 * mmr_bps as i128 - 4 * worst_fee); // x 40000 per |base| lots
    let deficit = (5 * mm - 4 * eq).max(0) * 10000; // (1.25 mm - eq) x 40000
    if relief <= 0 { return u64::MAX; }
    u64::try_from((deficit * ab as i128 + relief - 1) / relief).unwrap_or(u64::MAX).max(1)
}
#[inline(always)]
fn liq_chunk(ab: u64) -> u64 { ((ab as u128 * LIQ_BOOK_CHUNK_BPS as u128 + 9999) / 10000).max(1) as u64 }
// ---------- ADL ranking (AUDIT M-5) ----------
/// ADL score of position `pi` at price `px`: (PnL / equity) x (|notional| / equity), both x1e4.
/// None = not eligible (flat, not profitable, or equity <= 0).
fn adl_score(u: &User, pi: usize, px: u64, eq: i128) -> Option<i64> {
    let p = &u.pos[pi];
    if p.base == 0 || eq <= 0 { return None; }
    let notional = mulf(p.base as i128, px);
    let pnl = notional - p.cost as i128;
    if pnl <= 0 { return None; }
    let a = pnl.checked_mul(10000).map(|x| x / eq);
    let b = notional.abs().checked_mul(10000).map(|x| x / eq);
    let s = match (a, b) { (Some(a), Some(b)) => a.checked_mul(b).map(|x| x / 10000).unwrap_or(i128::MAX), _ => i128::MAX };
    Some(s.min(i64::MAX as i128) as i64)
}
fn rank_n(r: &Rank, side: usize) -> usize { (if side == 0 { r.n_long } else { r.n_short }) as usize }
fn rank_set_n(r: &mut Rank, side: usize, n: usize) { if side == 0 { r.n_long = n as u64 } else { r.n_short = n as u64 } }
fn rank_remove(r: &mut Rank, side: usize, user: &Key) {
    let (o, n) = (side * RANK_N, rank_n(r, side));
    if let Some(i) = r.entries[o..o + n].iter().position(|e| e.user == *user) {
        r.entries.copy_within(o + i + 1..o + n, o + i); rank_set_n(r, side, n - 1);
    }
}
/// insert sorted by score desc (ties: earlier entry first); full list keeps the top RANK_N.
fn rank_insert(r: &mut Rank, side: usize, user: &Key, score: i64) {
    let (o, n) = (side * RANK_N, rank_n(r, side));
    let i = r.entries[o..o + n].iter().position(|e| e.score < score).unwrap_or(n);
    if i >= RANK_N { return; }
    let end = if n == RANK_N { n - 1 } else { n };
    r.entries.copy_within(o + i..o + end, o + i + 1);
    r.entries[o + i] = RankEntry { user: *user, score };
    rank_set_n(r, side, end + 1);
}
/// accs: admin(s), market(w), rank(w)
fn init_rank(pid: &Address, a: &mut [AccountView]) -> ProgramResult {
    if a.len() < 3 { return Err(err(E::BadData)); }
    let m: &mut Market = load(&a[1], pid, DISC_MARKET, MARKET_SIZE)?;
    if !auth(pid, &m.admin, a) || m.adl_rank != [0; 32] { return Err(err(E::BadAccount)); }
    if !a[2].owned_by(pid) || a[2].data_len() < RANK_SIZE || disc_of(&a[2]) != 0 { return Err(err(E::BadAccount)); }
    let r = unsafe { &mut *(a[2].borrow_unchecked().as_ptr() as *mut Rank) };
    r.disc = DISC_RANK; r.market = key(&a[1]); m.adl_rank = key(&a[2]);
    Ok(())
}
/// Permissionless: (re)score one account in this market's ADL queue at the current price.
/// accs: market(r), rank(w), user(w), [other markets of the user]
fn adl_rank(pid: &Address, a: &mut [AccountView]) -> ProgramResult {
    if a.len() < 3 { return Err(err(E::BadData)); }
    let m: &Market = load(&a[0], pid, DISC_MARKET, MARKET_SIZE)?;
    let r: &mut Rank = load(&a[1], pid, DISC_RANK, RANK_SIZE)?;
    let u: &mut User = load(&a[2], pid, DISC_USER, USER_SIZE)?;
    if m.adl_rank != key(&a[1]) { return Err(err(E::BadAccount)); }
    home(u, m)?;
    let clk = clock()?; let nw: Now = (clk.slot, clk.unix_timestamp);
    if stale(m, nw) { return Err(err(E::Stale)); }
    let (uk, mk) = (key(&a[2]), key(&a[0]));
    settle_funding_all(pid, u, a)?;
    rank_remove(r, 0, &uk); rank_remove(r, 1, &uk);
    if let Some(pi) = u.pos.iter().position(|p| p.market == mk) { // AUDIT V5-1: isolated positions are ranked too (own equity)
        let (eq, _, _) = phealth(pid, u, pi, a, nw, false)?;
        if let Some(s) = adl_score(u, pi, mark(m), eq) { rank_insert(r, if u.pos[pi].base > 0 { 0 } else { 1 }, &uk, s); }
    }
    Ok(())
}
/// ADL / backstop for a bankrupt account (equity < 0). The counterparty is NOT chosen by the caller: the
/// program walks the on-chain ranking (opposite side, best score first), re-scores each entry, evicts
/// ineligible ones, and uses the first eligible. The caller must pass the user accounts of every entry it walks.
/// Closes at the mark; deficit -> insurance, then the counterparty up to its own equity (it can never be made
/// bankrupt), remainder recorded as market bad debt.
/// accs: target user(w), market(w), insurance(w), rank(w), ranked users(w)..., [other markets]
fn adl(pid: &Address, a: &mut [AccountView]) -> ProgramResult {
    if a.len() < 5 { return Err(err(E::BadAccount)); }
    let t: &mut User = load(&a[0], pid, DISC_USER, USER_SIZE)?;
    let m: &mut Market = load(&a[1], pid, DISC_MARKET, MARKET_SIZE)?;
    let ins: &mut Insurance = load(&a[2], pid, DISC_INS, INS_SIZE)?;
    let r: &mut Rank = load(&a[3], pid, DISC_RANK, RANK_SIZE)?;
    if m.insurance != key(&a[2]) || m.adl_rank != key(&a[3]) { return Err(err(E::BadAccount)); }
    home(t, m)?;
    let clk = clock()?; let nw: Now = (clk.slot, clk.unix_timestamp);
    if stale(m, nw) { return Err(err(E::Stale)); }
    let (tk, mk) = (key(&a[0]), key(&a[1]));
    let mkts = &a[1..];
    settle_funding_all(pid, t, mkts)?;
    let ti = t.pos.iter().position(|p| p.market == mk).ok_or(err(E::NotBankrupt))?;
    let (eq, _, tmm) = phealth(pid, t, ti, mkts, nw, true)?;
    // AUDIT V6-1: a position marked ADL-eligible by liquidate_book (book empty within the widest band) may be
    // deleveraged while below maintenance, inside the window; otherwise ADL needs bankruptcy (equity < 0).
    let fl = t.pos[ti].flags;
    let eligible = fl & spec::POS_FLAG_ADL_ELIGIBLE != 0 && clk.slot <= (fl >> 8) + LIQ_ADL_WINDOW_SLOTS && eq < tmm;
    if eq >= 0 && !eligible { return Err(err(E::NotBankrupt)); }
    let tb = t.pos[ti].base;
    if tb == 0 { return Err(err(E::NotBankrupt)); }
    let side = if tb > 0 { 1 } else { 0 }; // counterparties hold the opposite position
    let px = mark(m);
    // AUDIT N-4: every candidate passed by the caller is re-scored now (fresh ranking at use time)
    // Entries that can't be used (the target itself, non-user / foreign accounts) are evicted here too, so after
    // this loop every passed account is freshly scored and the walk needs no second health pass (CU).
    for ca in a[4..].iter() {
        let ck = key(ca);
        rank_remove(r, side, &ck);
        if ck == tk { continue; }
        let Ok(c) = load::<User>(ca, pid, DISC_USER, USER_SIZE) else { continue };
        if home(c, m).is_err() { continue; }
        settle_funding_all(pid, c, mkts)?;
        if let Some(ci) = c.pos.iter().position(|p| p.market == mk && ((p.base > 0) == (side == 0)) && p.base != 0) {
            let (ce, _, _) = phealth(pid, c, ci, mkts, nw, false)?;
            if let Some(s) = adl_score(c, ci, px, ce) { rank_insert(r, side, &ck, s); }
        }
    }
    if rank_n(r, side) == 0 { return Err(err(E::Rank)); }
    let ck = r.entries[side * RANK_N].user;
    let ca = a[4..].iter().find(|x| key(x) == ck).ok_or(err(E::Rank))?; // top entry must be passed (and was just re-scored)
    let c: &mut User = load(ca, pid, DISC_USER, USER_SIZE)?;
    let ci = c.pos.iter().position(|p| p.market == mk).ok_or(err(E::Rank))?;
    let cb = c.pos[ci].base;
    let q = tb.unsigned_abs().min(cb.unsigned_abs()) as i64;
    let dq = if tb > 0 { -q } else { q }; // target reduces toward 0
    let n = mulf(dq as i128, px);
    let l0 = long(tb) + long(cb);
    let (otb, ocb) = (t.pos[ti].base, c.pos[ci].base);
    t.pos[ti].base += dq; c.pos[ci].base -= dq;
    m.oi_long = (m.oi_long as i64 + long(t.pos[ti].base) + long(c.pos[ci].base) - l0) as u64;
    credit(t, ti, -n)?; credit(c, ci, n)?;
    t.pos[ti].cost = cost_after(otb, t.pos[ti].base, t.pos[ti].cost, n)?; c.pos[ci].cost = cost_after(ocb, c.pos[ci].base, c.pos[ci].cost, -n)?;
    let (mut eq2, _, _) = phealth(pid, t, ti, mkts, nw, false)?;
    if eligible && eq2 > 0 { // non-bankrupt fallback ADL pays the max-step liquidation fee to insurance
        let fee = bps_dn(n.abs(), LIQ_FEE_BPS << (LIQ_BOOK_BAND_STEPS - 1)).min(eq2);
        credit(t, ti, -fee)?; ins.balance = i64c(ins.balance as i128 + fee)?; eq2 -= fee;
    }
    let (mut ce, _, _) = phealth(pid, c, ci, mkts, nw, false)?; // one health pass; equity is linear in cash / iso_margin
    if eq2 < 0 {
        let deficit = -eq2;
        let from_ins = deficit.min(ins.balance.max(0) as i128);
        let from_c = (deficit - from_ins).min(ce.max(0)); // AUDIT M-5: never push the counterparty below zero
        ins.balance = i64c(ins.balance as i128 - from_ins)?;
        credit(c, ci, -from_c)?; ce -= from_c;
        credit(t, ti, deficit)?; // VERIFY-1: the unrecovered part is carried in bad_debt, not also in the target's cash
        m.bad_debt = i64c(m.bad_debt as i128 + deficit - from_ins - from_c)?;
    }
    // re-rank the counterparty at its new size
    rank_remove(r, side, &ck);
    if let Some(s) = adl_score(c, ci, px, ce) { rank_insert(r, side, &ck, s); }
    emit(&[&[EVENTS_ADL as u8], &tk, &ck, &q.to_le_bytes()]);
    gc_pos(t); gc_pos(c);
    Ok(())
}


// ---------- fast quote path ----------
/// accs: authority(s), user(r), slab(w, zeroed)
fn slab_init(pid: &Address, a: &mut [AccountView]) -> ProgramResult {
    if a.len() < 3 { return Err(err(E::BadData)); }
    let u: &mut User = load(&a[1], pid, DISC_USER, USER_SIZE)?;
    if !a[0].is_signer() || u.authority != key(&a[0]) { return Err(err(E::BadAccount)); }
    if !a[2].owned_by(pid) || a[2].data_len() < SLAB_SIZE || disc_of(&a[2]) != 0 { return Err(err(E::BadAccount)); }
    let s = unsafe { &mut *(a[2].borrow_unchecked().as_ptr() as *mut Slab) };
    s.disc = DISC_SLAB; s.authority = key(&a[0]); s.user = key(&a[1]);
    Ok(())
}
/// accs: admin(s), market(r), fast(w, zeroed). data: tag, idx u64, [side u8: 0 both, 1 bids, 2 asks]
fn fast_init(pid: &Address, a: &mut [AccountView], d: &[u8]) -> ProgramResult {
    if a.len() < 3 { return Err(err(E::BadData)); }
    let m: &Market = load(&a[1], pid, DISC_MARKET, MARKET_SIZE)?;
    if !auth(pid, &m.admin, a) { return Err(err(E::BadAccount)); }
    if !a[2].owned_by(pid) || a[2].data_len() < FAST_SIZE || disc_of(&a[2]) != 0 { return Err(err(E::BadAccount)); }
    let idx = rd64(d, 1)?;
    let side = if d.len() > 9 { d[9] as u64 } else { 0 };
    if idx as usize >= SLAB_QUOTES / (2 * SLAB_LEVELS) || side > 2 { return Err(err(E::BadData)); }
    let f = unsafe { &mut *(a[2].borrow_unchecked().as_ptr() as *mut FastBook) };
    f.disc = DISC_FAST; f.market = key(&a[1]); f.idx = idx; f.side = side;
    Ok(())
}
/// Admin approves a maker for this market. accs: admin(s), market(r), fast(w), slab(r)
fn fast_join(pid: &Address, a: &mut [AccountView]) -> ProgramResult {
    if a.len() < 4 { return Err(err(E::BadData)); }
    let m: &Market = load(&a[1], pid, DISC_MARKET, MARKET_SIZE)?;
    let f: &mut FastBook = load(&a[2], pid, DISC_FAST, FAST_SIZE)?;
    let s: &mut Slab = load(&a[3], pid, DISC_SLAB, SLAB_SIZE)?;
    if !auth(pid, &m.admin, a) || f.market != key(&a[1]) { return Err(err(E::BadAccount)); }
    let (n, sk) = (f.n as usize, key(&a[3]));
    if n >= FAST_MAKERS { return Err(err(E::Cap)); }
    if f.makers[..n].iter().any(|x| x.slab == sk) { return Err(err(E::BadAccount)); }
    f.makers[n] = FastMaker { slab: sk, user: s.user, margin: 0, budget: 0, base: 0, quote: 0, volume: 0, target: 0 };
    f.n += 1;
    Ok(())
}
/// Maker overwrites its own quotes: no margin read and no market account, so it conflicts with nothing but takers reading this slab.
/// accs: authority(s), slab(w). data: tag, items (idx u16, px u32, qty u32)
fn slab_quote(pid: &Address, a: &mut [AccountView], d: &[u8]) -> ProgramResult {
    if a.len() < 2 || !a[0].is_signer() { return Err(err(E::BadAccount)); }
    let s: &mut Slab = load(&a[1], pid, DISC_SLAB, SLAB_SIZE)?;
    if s.authority != key(&a[0]) { return Err(err(E::BadAccount)); }
    let body = &d[1..];
    if body.len() % spec::QUOTE_ITEM_SIZE != 0 { return Err(err(E::BadData)); }
    let slot = clock()?.slot as u32;
    s.gen = s.gen.wrapping_add(1); let g = s.gen as u32;
    for it in body.chunks_exact(spec::QUOTE_ITEM_SIZE) {
        let i = u16::from_le_bytes([it[0], it[1]]) as usize;
        if i >= SLAB_QUOTES { return Err(err(E::BadData)); }
        let q = unsafe { s.quotes.get_unchecked_mut(i) };
        q.px = u32::from_le_bytes([it[2], it[3], it[4], it[5]]); q.qty = u32::from_le_bytes([it[6], it[7], it[8], it[9]]);
        q.gen = g; q.slot = slot;
    }
    s.count = s.count.wrapping_add((body.len() / spec::QUOTE_ITEM_SIZE) as u64);
    Ok(())
}
/// Emergency stop for all quotes of a maker. accs: authority(s), slab(w). data: tag, on u8
fn slab_kill(pid: &Address, a: &mut [AccountView], d: &[u8]) -> ProgramResult {
    if a.len() < 2 || !a[0].is_signer() || d.len() < 2 { return Err(err(E::BadData)); }
    let s: &mut Slab = load(&a[1], pid, DISC_SLAB, SLAB_SIZE)?;
    if s.authority != key(&a[0]) { return Err(err(E::BadAccount)); }
    s.kill = d[1] as u64;
    Ok(())
}
#[inline(always)]
fn maker_budget(m: &Market, margin: i64) -> i64 { i64::try_from(margin.max(0) as i128 * 10000 / m.imr_bps as i128).unwrap_or(i64::MAX) }
/// Lock (amount > 0) or release (amount < 0) quoting margin. A maker can only fill up to margin / IMR of notional between settles.
/// Optional target turns on auto mode (0 turns it off); see slab_settle.
/// accs: authority(s), user(w), market(r), fast(w), [other markets]. data: tag, amount i64, [target i64]
fn slab_fund(pid: &Address, a: &mut [AccountView], d: &[u8]) -> ProgramResult {
    if a.len() < 4 { return Err(err(E::BadData)); }
    let amt = rd64(d, 1)? as i64;
    let tgt = if d.len() > 9 { let t = rd64(d, 9)? as i64; if t < 0 { return Err(err(E::BadData)); } Some(t) } else { None };
    let u: &mut User = load(&a[1], pid, DISC_USER, USER_SIZE)?;
    let m: &Market = load(&a[2], pid, DISC_MARKET, MARKET_SIZE)?;
    let f: &mut FastBook = load(&a[3], pid, DISC_FAST, FAST_SIZE)?;
    let (uk, mk) = (key(&a[1]), key(&a[2]));
    if !a[0].is_signer() || u.authority != key(&a[0]) || f.market != mk { return Err(err(E::BadAccount)); }
    home(u, m)?;
    let clk = clock()?; let nw: Now = (clk.slot, clk.unix_timestamp);
    let n = f.n as usize;
    let e = f.makers[..n].iter_mut().find(|x| x.user == uk).ok_or(err(E::BadAccount))?;
    if amt < 0 && (e.base != 0 || e.quote != 0) { return Err(err(E::Ledger)); } // settle first
    if let Some(t) = tgt { e.target = t; }
    let nm = i64c(e.margin as i128 + amt as i128)?;
    if nm < 0 { return Err(err(E::Margin)); }
    settle_funding_all(pid, u, &a[2..])?;
    u.cash = i64c(u.cash as i128 - amt as i128)?; e.margin = nm;
    e.budget = maker_budget(m, nm);
    if amt > 0 { let (eq, im, _) = health(pid, u, &a[2..], nw, true)?; if u.cash < 0 || eq < im { return Err(err(E::Margin)); } }
    Ok(())
}
/// Permissionless: move a maker's fills into its User and refill the budget. A maker below maintenance gets its margin back
/// in cash and stops quoting, so normal liquidation sees all of its equity.
/// accs: user(w), market(r), fast(w), [other markets]
fn slab_settle(pid: &Address, a: &mut [AccountView]) -> ProgramResult {
    if a.len() < 3 { return Err(err(E::BadData)); }
    let u: &mut User = load(&a[0], pid, DISC_USER, USER_SIZE)?;
    let m: &Market = load(&a[1], pid, DISC_MARKET, MARKET_SIZE)?;
    let f: &mut FastBook = load(&a[2], pid, DISC_FAST, FAST_SIZE)?;
    let (uk, mk) = (key(&a[0]), key(&a[1]));
    if f.market != mk { return Err(err(E::BadAccount)); }
    home(u, m)?;
    let clk = clock()?; let nw: Now = (clk.slot, clk.unix_timestamp);
    let n = f.n as usize;
    let e = f.makers[..n].iter_mut().find(|x| x.user == uk).ok_or(err(E::BadAccount))?;
    settle_funding_all(pid, u, &a[1..])?;
    if e.base != 0 || e.quote != 0 {
        let pi = pos_idx(u, &mk, m.funding_index)?;
        let (ob, before) = (u.pos[pi].base, long(u.pos[pi].base));
        u.pos[pi].base = ob.checked_add(e.base).ok_or(err(E::Math))?;
        credit(u, pi, e.quote as i128)?;
        u.pos[pi].cost = cost_after(ob, u.pos[pi].base, u.pos[pi].cost, -(e.quote as i128))?;
        f.oi_long_delta += long(u.pos[pi].base) - before;
        u.maker_volume += e.volume;
        e.base = 0; e.quote = 0; e.volume = 0;
    }
    let (eq, _, mm) = health(pid, u, &a[1..], nw, false)?;
    if eq < mm { u.cash = i64c(u.cash as i128 + e.margin as i128)?; e.margin = 0; }
    else if e.target > 0 {
        // Auto mode works like cross margin through the keeper's settles: idle cash tops the margin up to target,
        // and margin goes back to cash first when the rest of the account falls short of initial margin. Skipped on a stale price.
        if let Ok((eq, im, _)) = health(pid, u, &a[1..], nw, true) {
            let mv: i128 = if eq < im { -((im - eq).min(e.margin as i128)) }
                else if e.target > e.margin { ((e.target - e.margin) as i128).min((u.cash as i128).max(0)).min(eq - im) }
                else { -((e.margin - e.target) as i128) };
            u.cash = i64c(u.cash as i128 - mv)?; e.margin = i64c(e.margin as i128 + mv)?;
        }
    }
    e.budget = maker_budget(m, e.margin);
    gc_pos(u);
    Ok(())
}
/// Taker order against maker slabs and, when opposite-side shards are passed, resting book orders. Better price wins.
/// accs: authority(s), user(w), market(r), fast(w), slabs(r) x ns, [shards], [other markets]
/// data: tag, side u8, flags u8, ns u8, limit u64, qty u64
fn take_fast(pid: &Address, a: &mut [AccountView], d: &[u8]) -> ProgramResult {
    if a.len() < 4 || d.len() != 20 { return Err(err(E::BadData)); }
    let (bid, fl, ns) = (d[1] as u64 == SIDE_BID, d[2] as u64, d[3] as usize);
    let (limit, mut left) = (rd64(d, 4)?, rd64(d, 12)?);
    if ns > FAST_MAKERS || a.len() < 4 + ns || left == 0 || left > u32::MAX as u64 { return Err(err(E::BadData)); }
    let u: &mut User = load(&a[1], pid, DISC_USER, USER_SIZE)?;
    let m: &Market = load(&a[2], pid, DISC_MARKET, MARKET_SIZE)?;
    let f: &mut FastBook = load(&a[3], pid, DISC_FAST, FAST_SIZE)?;
    let (uk, mk) = (key(&a[1]), key(&a[2]));
    if f.market != mk { return Err(err(E::BadAccount)); }
    let clk = clock()?; let slot = clk.slot; let nw: Now = (slot, clk.unix_timestamp);
    let rest = &a[4 + ns..];
    let dmax = if a[0].is_signer() && u.authority == key(&a[0]) { u64::MAX } else if a[0].is_signer() {
        delegate_max_qty(pid, rest, &key(&a[0]), &uk, slot).ok_or(err(E::BadAccount))? } else { return Err(err(E::BadAccount)); };
    if left > dmax { return Err(err(E::Cap)); }
    home(u, m)?;
    claim_all(pid, u, &uk, rest)?;
    let pi = pos_idx(u, &mk, m.funding_index)?;
    settle_funding_all(pid, u, &a[2..])?;
    let reduce = fl & FLAG_REDUCE_ONLY != 0;
    if reduce {
        let b = u.pos[pi].base;
        let cap = if bid && b < 0 { (-b) as u64 } else if !bid && b > 0 { b as u64 } else { 0 };
        left = left.min(cap);
        if left == 0 { gc_pos(u); return Ok(()); }
    } else {
        if m.paused != 0 { return Err(err(E::Paused)); }
        if stale(m, nw) { return Err(err(E::Stale)); }
    }
    let px0 = mark(m);
    let band = px0 / 10000 * spec::QUOTE_BAND_BPS;
    let (blo, bhi) = bound_range(m);
    let (lo, hi) = (px0.saturating_sub(band).max(blo), (px0 + band).min(bhi));
    let opp = if bid { 1usize } else { 0usize };
    if f.side != 0 && f.side != opp as u64 + 1 { return Err(err(E::BadAccount)); } // split market: buyers use the ask book, sellers the bid book
    let base_ix = (f.idx as usize) << 4 | opp << 3;
    let mut sl: [(*const Slab, usize); FAST_MAKERS] = [(core::ptr::null(), 0); FAST_MAKERS];
    let mut cnt = 0;
    for x in a[4..4 + ns].iter() {
        let s: &Slab = load(x, pid, DISC_SLAB, SLAB_SIZE)?;
        let k = key(x);
        let mi = f.makers[..f.n as usize].iter().position(|e| e.slab == k).ok_or(err(E::BadAccount))?;
        if s.kill != 0 || s.user == uk { continue; } // no self-trade against own slab
        sl[cnt] = (s as *const Slab, mi); cnt += 1;
    }
    let tier = (u.tier as usize).min(TIERS.len() - 1);
    let taker_bps = TIERS[tier].0;
    fast_match(pid, f, &sl[..cnt], base_ix, opp, bid, lo, hi, rest, &mk, m, u, &uk, pi, limit, left, slot, taker_bps)?;
    if !reduce {
        if m.max_oi != 0 && m.oi_long > m.max_oi { return Err(err(E::Cap)); }
        let p = &u.pos[pi];
        if m.max_position != 0 && core::cmp::max((p.base as i128 + p.bid_open as i128).abs(), (p.base as i128 - p.ask_open as i128).abs()) > m.max_position as i128 { return Err(err(E::Cap)); }
        let (eq, im, _) = health(pid, u, &a[2..], nw, true)?;
        if eq < im { return Err(err(E::Margin)); }
        if p.flags & POS_ISO != 0 { let (e2, i2, _) = iso_health(pid, u, pi, &a[2..], nw, true)?; if e2 < i2 { return Err(err(E::Margin)); } }
    }
    gc_pos(u);
    Ok(())
}

/// Taker matching shared by take_fast and trigger orders: the better of the best maker quote and the best resting order fills
/// first, until `left` lots are done or nothing is inside `limit`. Books the taker's side, maker fills, fees and OI. Returns lots.
fn fast_match(pid: &Address, f: &mut FastBook, sl: &[(*const Slab, usize)], base_ix: usize, opp: usize, bid: bool, lo: u64, hi: u64,
              rest: &[AccountView], mk: &Key, m: &Market, u: &mut User, uk: &Key, pi: usize, limit: u64, mut left: u64, slot: u64,
              taker_bps: u64) -> Result<u64, ProgramError> {
    let (want, base0) = (left, long(u.pos[pi].base));
    let mut ev = EvBuf::new(uk, bid);
    while left > 0 {
        let (bk, bl, bp, ba) = best_quote(f, sl, base_ix, opp, bid, slot, lo, hi).unwrap_or((usize::MAX, 0, 0, 0));
        // resting orders strictly better than the best quote (or all of them when no quote is usable)
        if !rest.is_empty() || m.strict_take != 0 {
            let lim = if bk == usize::MAX { limit } else if bid { limit.min(bp - 1) } else { limit.max(bp + 1) };
            let tf = take(pid, rest, mk, m, u, uk, pi, bid, lim, left, slot, taker_bps, STP_CANCEL_OLDEST)?;
            left -= tf.filled;
            if left == 0 || tf.abort { break; }
        }
        if bk == usize::MAX || (bid && bp > limit) || (!bid && bp < limit) { break; }
        let (sp, mi) = sl[bk];
        let q = unsafe { &(*sp).quotes[base_ix + bl] };
        let fq = ba.min(left);
        let nt = fill_maker(f, mi, q, opp << 3 | bl, bid, bp, fq)?;
        let fee = bps_up(nt as i128, taker_bps);
        let sg: i128 = if bid { 1 } else { -1 };
        f.fee_accum = i64c(f.fee_accum as i128 + fee)?;
        credit(u, pi, -fee - sg * nt as i128)?;
        let ob = u.pos[pi].base; u.pos[pi].base = ob.checked_add(sg as i64 * fq as i64).ok_or(err(E::Math))?;
        u.pos[pi].cost = cost_after(ob, u.pos[pi].base, u.pos[pi].cost, sg * nt as i128)?;
        ev.push(&f.makers[mi].user, bp, fq, (mi as u64) << 32 | q.gen as u64);
        left -= fq;
    }
    ev.flush();
    f.oi_long_delta += long(u.pos[pi].base) - base0;
    f.fills += 1;
    Ok(want - left)
}
/// Approved makers' slabs found in `xs`, skipping killed ones and the taker's own. Every slab must belong to this book.
fn fast_slabs(pid: &Address, f: &FastBook, xs: &[AccountView], uk: &Key) -> Result<([(*const Slab, usize); FAST_MAKERS], usize), ProgramError> {
    let mut sl: [(*const Slab, usize); FAST_MAKERS] = [(core::ptr::null(), 0); FAST_MAKERS];
    let mut cnt = 0;
    for x in xs.iter().filter(|x| disc_of(x) == DISC_SLAB) {
        let s: &Slab = load(x, pid, DISC_SLAB, SLAB_SIZE)?;
        let k = key(x);
        let mi = f.makers[..f.n as usize].iter().position(|e| e.slab == k).ok_or(err(E::BadAccount))?;
        if s.kill != 0 || s.user == *uk || cnt == FAST_MAKERS { continue; }
        sl[cnt] = (s as *const Slab, mi); cnt += 1;
    }
    Ok((sl, cnt))
}
/// Best usable maker quote on the side a taker hits, inside [lo, hi]: (slab slot, level, price, lots available).
/// Better price first, then the older quote. Lots are capped by the maker's remaining budget.
#[inline(always)]
fn best_quote(f: &FastBook, sl: &[(*const Slab, usize)], base_ix: usize, opp: usize, bid: bool, slot: u64, lo: u64, hi: u64) -> Option<(usize, usize, u64, u64)> {
    let (mut bk, mut bl, mut bp, mut ba, mut bs) = (usize::MAX, 0usize, 0u64, 0u64, u32::MAX);
    for (k, &(sp, mi)) in sl.iter().enumerate() {
        let s = unsafe { &*sp };
        let e = &f.makers[mi];
        if e.budget <= 0 { continue; }
        for l in 0..SLAB_LEVELS {
            let q = unsafe { s.quotes.get_unchecked(base_ix + l) };
            if q.qty == 0 || slot as u32 - q.slot.min(slot as u32) > spec::QUOTE_MAX_AGE_SLOTS { continue; }
            let p = q.px as u64;
            if p == 0 || p < lo || p > hi { continue; }
            let u_ = unsafe { f.used.get_unchecked(mi).get_unchecked(opp << 3 | l) };
            let av = (if u_.gen == q.gen { q.qty.saturating_sub(u_.qty) } else { q.qty }) as u64;
            let av = av.min(e.budget as u64 / p);
            if av == 0 { continue; }
            if bk == usize::MAX || (bid && p < bp) || (!bid && p > bp) || (p == bp && q.slot < bs) { bk = k; bl = l; bp = p; ba = av; bs = q.slot; }
        }
    }
    if bk == usize::MAX { None } else { Some((bk, bl, bp, ba)) }
}
/// Book `fq` lots at `p` against maker `mi` (quote level `ul` in its used table). Returns the notional.
#[inline(always)]
fn fill_maker(f: &mut FastBook, mi: usize, q: &Quote, ul: usize, bid: bool, p: u64, fq: u64) -> Result<u64, ProgramError> {
    let nt = p * fq; // px < 2^32, qty < 2^32
    let e = &mut f.makers[mi];
    let sg: i64 = if bid { 1 } else { -1 };
    e.budget -= nt as i64;
    e.base = e.base.checked_sub(sg * fq as i64).ok_or(err(E::Math))?;
    e.quote = i64c(e.quote as i128 + sg as i128 * nt as i128)?;
    e.volume += fq;
    let us = &mut f.used[mi][ul];
    if us.gen == q.gen { us.qty += fq as u32; } else { us.gen = q.gen; us.qty = fq as u32; }
    Ok(nt)
}
/// Liquidation against maker quotes, for markets (or moments) where the fast book holds the liquidity. Same rules as
/// liquidate_book: below maintenance, 25% chunks, fee to insurance, bankrupt remainder to insurance or UseAdl.
/// When nothing fills, the market has no shards and every approved maker's slab was passed, the position becomes
/// ADL-eligible (the fast book is provably empty within the band). Accounts with resting book orders go through liquidate_book.
/// accs: user(w), market(r), insurance(w), fast(w), slabs(r) x ns, [other markets]. data: tag, ns u8, max_qty u64
fn liquidate_fast(pid: &Address, a: &mut [AccountView], d: &[u8]) -> ProgramResult {
    if a.len() < 4 || d.len() != 10 { return Err(err(E::BadData)); }
    let (ns, max_qty) = (d[1] as usize, rd64(d, 2)?);
    if ns > FAST_MAKERS || a.len() < 4 + ns { return Err(err(E::BadData)); }
    let t: &mut User = load(&a[0], pid, DISC_USER, USER_SIZE)?;
    let m: &Market = load(&a[1], pid, DISC_MARKET, MARKET_SIZE)?;
    let ins: &mut Insurance = load(&a[2], pid, DISC_INS, INS_SIZE)?;
    let f: &mut FastBook = load(&a[3], pid, DISC_FAST, FAST_SIZE)?;
    let (tk, mk) = (key(&a[0]), key(&a[1]));
    if m.insurance != key(&a[2]) || f.market != mk { return Err(err(E::BadAccount)); }
    home(t, m)?;
    let clk = clock()?; let nw: Now = (clk.slot, clk.unix_timestamp); let slot = clk.slot;
    if stale(m, nw) { return Err(err(E::Stale)); }
    let mkts = &a[1..];
    settle_funding_all(pid, t, mkts)?;
    let pi = t.pos.iter().position(|p| p.market == mk).ok_or(err(E::NotLiquidatable))?;
    let (eq, _, mm) = phealth(pid, t, pi, mkts, nw, true)?;
    if eq >= mm { return Err(err(E::NotLiquidatable)); }
    if t.pos[pi].bid_open != 0 || t.pos[pi].ask_open != 0 || t.pos[pi].n_orders != 0 { return Err(err(E::MissingShards)); }
    let b = t.pos[pi].base;
    if b == 0 { return Err(err(E::NotLiquidatable)); }
    let ab = b.unsigned_abs();
    let px = mark(m); let bid = b < 0;
    let mut left = ab.min(max_qty).min(liq_chunk(ab)).min(liq_needed(eq, mm, ab, px, m.mmr_bps));
    if left == 0 { return Err(err(E::BadData)); }
    // same price window as an ordinary take: a liquidation never reaches quotes a taker couldn't
    let band = px / 10000 * spec::QUOTE_BAND_BPS;
    let (blo, bhi) = bound_range(m);
    let (lo, hi) = (px.saturating_sub(band).max(blo).max(1), (px + band).min(bhi));
    let opp = if bid { 1usize } else { 0usize };
    if f.side != 0 && f.side != opp as u64 + 1 { return Err(err(E::BadAccount)); }
    let base_ix = (f.idx as usize) << 4 | opp << 3;
    let mut sl: [(*const Slab, usize); FAST_MAKERS] = [(core::ptr::null(), 0); FAST_MAKERS];
    let (mut cnt, mut seen) = (0usize, 0u64);
    for x in a[4..4 + ns].iter() {
        let s: &Slab = load(x, pid, DISC_SLAB, SLAB_SIZE)?;
        let k = key(x);
        let mi = f.makers[..f.n as usize].iter().position(|e| e.slab == k).ok_or(err(E::BadAccount))?;
        seen |= 1 << mi;
        if s.kill != 0 || s.user == tk { continue; }
        sl[cnt] = (s as *const Slab, mi); cnt += 1;
    }
    let base0 = long(t.pos[pi].base);
    let (mut filled, mut notional) = (0u64, 0i128);
    let mut ev = EvBuf::new(&tk, bid);
    while left > 0 {
        let Some((bk, bl, bp, ba)) = best_quote(f, &sl[..cnt], base_ix, opp, bid, slot, lo, hi) else { break };
        let (sp, mi) = sl[bk];
        let q = unsafe { &(*sp).quotes[base_ix + bl] };
        let fq = ba.min(left);
        let nt = fill_maker(f, mi, q, opp << 3 | bl, bid, bp, fq)?;
        let sg: i128 = if bid { 1 } else { -1 };
        credit(t, pi, -sg * nt as i128)?;
        let ob = t.pos[pi].base; t.pos[pi].base = ob.checked_add(sg as i64 * fq as i64).ok_or(err(E::Math))?;
        t.pos[pi].cost = cost_after(ob, t.pos[pi].base, t.pos[pi].cost, sg * nt as i128)?;
        ev.push(&f.makers[mi].user, bp, fq, (mi as u64) << 32 | q.gen as u64);
        filled += fq; notional += nt as i128; left -= fq;
    }
    ev.flush();
    if filled == 0 {
        let all = if f.n >= 64 { u64::MAX } else { (1u64 << f.n) - 1 };
        if m.n_shards != 0 || seen & all != all { return Err(err(E::NoFill)); }
        t.pos[pi].flags = (t.pos[pi].flags & POS_ISO) | spec::POS_FLAG_ADL_ELIGIBLE | (slot << 8);
        emit(&[&[spec::EVENTS_LIQ_NOFILL as u8], &tk, &slot.to_le_bytes()]);
        return Ok(());
    }
    f.oi_long_delta += long(t.pos[pi].base) - base0;
    f.fills += 1;
    let (eq2, _, _) = phealth(pid, t, pi, mkts, nw, false)?;
    let fee = bps_dn(notional, LIQ_FEE_BPS).min(eq2.max(0));
    credit(t, pi, -fee)?; pay_liq_fee(pid, &a[4 + ns..], &tk, m, ins, fee)?;
    if eq2 < 0 {
        if -eq2 > ins.balance.max(0) as i128 { return Err(err(E::UseAdl)); }
        let closed = if t.pos[pi].flags & POS_ISO != 0 { t.pos[pi].base == 0 } else { t.pos.iter().all(|p| p.base == 0 || p.flags & POS_ISO != 0) };
        if closed { ins.balance = i64c(ins.balance as i128 + eq2)?; credit(t, pi, -eq2)?; }
    }
    emit(&[&[EVENTS_LIQUIDATION as u8], &tk, &filled.to_le_bytes(), &px.to_le_bytes(), &[0u8; 32]]);
    gc_pos(t);
    Ok(())
}

/// Atomic guard: fails the whole transaction unless the account ends where the caller expects. Put it last in a multi-leg
/// transaction (pair trade, multi-market rebalance, perp + spot hedge): a leg that fills short reverts every leg.
/// Checks position bounds per market, token account balances (the spot leg), and a minimum cross equity at the stored mark.
/// accs: user(r), markets(r) x np, token accounts(r) x nt, [other markets the user holds, for the equity check]
/// data: tag, np u8, nt u8, min_equity i64 (i64::MIN = unchecked), np x (min_base i64, max_base i64), nt x min_amount u64
fn assert_state(pid: &Address, a: &mut [AccountView], d: &[u8]) -> ProgramResult {
    if d.len() < 11 { return Err(err(E::BadData)); }
    let (np, nt) = (d[1] as usize, d[2] as usize);
    if d.len() != 11 + np * 16 + nt * 8 || a.len() < 1 + np + nt { return Err(err(E::BadData)); }
    let u: &User = load(&a[0], pid, DISC_USER, USER_SIZE)?;
    for i in 0..np {
        let _m: &Market = load(&a[1 + i], pid, DISC_MARKET, MARKET_SIZE)?;
        let mk = key(&a[1 + i]);
        let (lo, hi) = (rd64(d, 11 + i * 16)? as i64, rd64(d, 19 + i * 16)? as i64);
        let mut b = 0i64;
        for p in u.pos.iter() { if p.market == [0; 32] { break; } if p.market == mk { b = p.base; break; } }
        if b < lo || b > hi { return Err(err(E::Assert)); }
    }
    for j in 0..nt {
        let t = &a[1 + np + j];
        if !t.owned_by(&pinocchio_token::ID) || t.data_len() < 165 { return Err(err(E::BadAccount)); }
        let amt = u64::from_le_bytes(unsafe { t.borrow_unchecked() }[64..72].try_into().unwrap());
        if amt < rd64(d, 11 + np * 16 + j * 8)? { return Err(err(E::Assert)); }
    }
    let min_eq = rd64(d, 3)? as i64;
    if min_eq != i64::MIN {
        let clk = clock()?;
        let (eq, _, _) = health(pid, u, &a[1..], (clk.slot, clk.unix_timestamp), false)?;
        if eq < min_eq as i128 { return Err(err(E::Assert)); }
    }
    Ok(())
}


// ---------- market config / session ----------
/// accs: signer(s) (admin or session_auth), market(w). data: tag, session u8
fn set_session(pid: &Address, a: &mut [AccountView], d: &[u8]) -> ProgramResult {
    if a.len() < 2 || d.len() < 2 || d[1] > 1 { return Err(err(E::BadData)); }
    let m: &mut Market = load(&a[1], pid, DISC_MARKET, MARKET_SIZE)?;
    let by_key = a[0].is_signer() && m.session_auth != [0; 32] && key(&a[0]) == m.session_auth;
    if !by_key && !auth(pid, &m.admin, a) { return Err(err(E::BadAccount)); }
    if m.session != d[1] as u64 { m.session = d[1] as u64; m.bound_up = 0; m.bound_dn = 0; if m.session == 1 { m.bound_ref = m.oracle_px; } }
    Ok(())
}
/// accs: admin(s), market(w). data: tag, bound_bps, thresh_bps, max_resets, strict_take (u64s), session_auth[32]
fn market_cfg(pid: &Address, a: &mut [AccountView], d: &[u8]) -> ProgramResult {
    if a.len() < 2 || d.len() != 65 { return Err(err(E::BadData)); }
    let m: &mut Market = load(&a[1], pid, DISC_MARKET, MARKET_SIZE)?;
    if !auth(pid, &m.admin, a) { return Err(err(E::BadAccount)); }
    let (b, t) = (rd64(d, 1)?, rd64(d, 9)?);
    if b > 10000 || t > 10000 { return Err(err(E::BadData)); }
    m.bound_bps = b; m.bound_thresh_bps = t; m.bound_max = rd64(d, 17)?; m.strict_take = rd64(d, 25)?;
    m.session_auth.copy_from_slice(&d[33..65]);
    Ok(())
}

// ---------- HLP-style vault ----------
/// accs: admin(s), vault(w, zeroed), market(r). data: tag, lockup_slots, day_cap_bps, perf_bps, dd_bps
fn vault_init(pid: &Address, a: &mut [AccountView], d: &[u8]) -> ProgramResult {
    if a.len() < 3 || !a[0].is_signer() || d.len() != 33 { return Err(err(E::BadData)); }
    if !a[1].owned_by(pid) || a[1].data_len() < VAULT_SIZE || disc_of(&a[1]) != 0 { return Err(err(E::BadAccount)); }
    let m: &Market = load(&a[2], pid, DISC_MARKET, MARKET_SIZE)?;
    let (cap, perf, dd) = (rd64(d, 9)?, rd64(d, 17)?, rd64(d, 25)?);
    if cap > 10000 || perf > 3000 || dd > 10000 { return Err(err(E::BadData)); }
    let v = unsafe { &mut *(a[1].borrow_unchecked().as_ptr() as *mut Vault) };
    v.disc = DISC_VAULT; v.admin = key(&a[0]); v.token_vault = m.vault; v.lockup_slots = rd64(d, 1)?; v.day_cap_bps = cap; v.perf_bps = perf; v.dd_bps = dd;
    Ok(())
}
/// accs: admin(s), vault(w), user(w, zeroed). The sub-account answers only to the program, on the vault's behalf.
fn vault_add_sub(pid: &Address, a: &mut [AccountView]) -> ProgramResult {
    if a.len() < 3 { return Err(err(E::BadData)); }
    let v: &mut Vault = load(&a[1], pid, DISC_VAULT, VAULT_SIZE)?;
    if !auth(pid, &v.admin, a) || v.n_subs as usize >= VAULT_SUBS { return Err(err(E::BadAccount)); }
    if !a[2].owned_by(pid) || a[2].data_len() < USER_SIZE || disc_of(&a[2]) != 0 { return Err(err(E::BadAccount)); }
    let u = unsafe { &mut *(a[2].borrow_unchecked().as_ptr() as *mut User) };
    u.disc = DISC_USER; u.authority = key(&a[1]); u.home_vault = v.token_vault;
    v.subs[v.n_subs as usize] = key(&a[2]); v.n_subs += 1;
    if v.n_subs == 1 { v.active = 1; }
    Ok(())
}
/// accs: admin(s), vault(r), market(r), link(w, zeroed or own). data: tag, cap_bps
fn vault_link(pid: &Address, a: &mut [AccountView], d: &[u8]) -> ProgramResult {
    if a.len() < 4 { return Err(err(E::BadData)); }
    let v: &mut Vault = load(&a[1], pid, DISC_VAULT, VAULT_SIZE)?;
    let _m: &Market = load(&a[2], pid, DISC_MARKET, MARKET_SIZE)?;
    if !auth(pid, &v.admin, a) { return Err(err(E::BadAccount)); }
    if !a[3].owned_by(pid) || a[3].data_len() < VLINK_SIZE { return Err(err(E::BadAccount)); }
    let l = unsafe { &mut *(a[3].borrow_unchecked().as_ptr() as *mut VaultLink) };
    if l.disc != 0 && (l.disc != DISC_VLINK || l.vault != key(&a[1]) || l.market != key(&a[2])) { return Err(err(E::BadAccount)); }
    l.disc = DISC_VLINK; l.vault = key(&a[1]); l.market = key(&a[2]); l.cap_bps = rd64(d, 1)?;
    Ok(())
}
/// Vault value: equity of every active sub-account. Flat, unused subs need no account.
fn vault_nav(pid: &Address, v: &Vault, a: &[AccountView], nw: Now) -> Result<i128, ProgramError> {
    let mut nav = 0i128;
    for i in 0..v.n_subs as usize {
        if v.active >> i & 1 == 0 { continue; }
        let x = a.iter().find(|x| key(x) == v.subs[i]).ok_or(err(E::MissingShards))?;
        let u: &mut User = load(x, pid, DISC_USER, USER_SIZE)?;
        settle_funding_all(pid, u, a)?;
        nav += health(pid, u, a, nw, true)?.0;
    }
    Ok(nav)
}
/// Remember the value, track the peak, and stop new takeovers past the drawdown limit.
fn vault_mark(v: &mut Vault, nav: i128, slot: u64) -> ProgramResult {
    v.nav = i64c(nav)?; v.nav_slot = slot;
    if v.nav > v.peak_nav { v.peak_nav = v.nav; }
    if v.dd_bps != 0 && (v.nav as i128) * 10000 < v.peak_nav as i128 * (10000 - v.dd_bps as i128) { v.halted = 1; }
    Ok(())
}
const VV: i128 = spec::VAULT_VIRTUAL as i128;
/// accs: owner(s), vault(w), depositor(w, zeroed or own), subs[0](w), market(r), token vault(w), src(w), token program, [active subs, markets]
/// data: tag, amount
fn vault_deposit(pid: &Address, a: &mut [AccountView], d: &[u8]) -> ProgramResult {
    if a.len() < 8 || !a[0].is_signer() { return Err(err(E::BadData)); }
    let amt = rd64(d, 1)?;
    if amt == 0 { return Err(err(E::BadData)); }
    let v: &mut Vault = load(&a[1], pid, DISC_VAULT, VAULT_SIZE)?;
    let m: &Market = load(&a[4], pid, DISC_MARKET, MARKET_SIZE)?;
    if v.n_subs == 0 || key(&a[3]) != v.subs[0] || key(&a[5]) != v.token_vault || m.vault != v.token_vault || v.halted != 0 { return Err(err(E::BadAccount)); }
    if a[7].address() != &pinocchio_token::ID { return Err(err(E::TokenProgram)); }
    if !a[2].owned_by(pid) || a[2].data_len() < VDEP_SIZE { return Err(err(E::BadAccount)); }
    let vd = unsafe { &mut *(a[2].borrow_unchecked().as_ptr() as *mut VaultDeposit) };
    let (vk, ok) = (key(&a[1]), key(&a[0]));
    if vd.disc == 0 { vd.disc = DISC_VDEP; vd.vault = vk; vd.owner = ok; }
    else if vd.disc != DISC_VDEP || vd.vault != vk || vd.owner != ok { return Err(err(E::BadAccount)); }
    let clk = clock()?; let nw: Now = (clk.slot, clk.unix_timestamp);
    let nav = vault_nav(pid, v, &a[3..], nw)?;
    if nav + VV <= 0 { return Err(err(E::Halted)); }
    let minted = amt as i128 * (v.shares as i128 + VV) / (nav + VV);
    if minted <= 0 { return Err(err(E::BadData)); }
    pinocchio_token::instructions::Transfer::new(&a[6], &a[5], &a[0], amt).invoke_with_program(a[7].address())?;
    let s0: &mut User = load(&a[3], pid, DISC_USER, USER_SIZE)?;
    s0.cash = i64c(s0.cash as i128 + amt as i128)?;
    v.shares += u64::try_from(minted).map_err(|_| err(E::Math))?;
    vd.shares += minted as u64; vd.basis = i64c(vd.basis as i128 + amt as i128)?; vd.unlock_slot = clk.slot + v.lockup_slots;
    vault_mark(v, nav + amt as i128, clk.slot)
}
/// accs: owner(s), vault(w), depositor(w), subs[0](w), market(r), token vault(w), dst(w), token program, vault pda, [active subs, markets]
/// data: tag, shares
fn vault_withdraw(pid: &Address, a: &mut [AccountView], d: &[u8]) -> ProgramResult {
    if a.len() < 9 || !a[0].is_signer() { return Err(err(E::BadData)); }
    let sh = rd64(d, 1)?;
    let v: &mut Vault = load(&a[1], pid, DISC_VAULT, VAULT_SIZE)?;
    let vd: &mut VaultDeposit = load(&a[2], pid, DISC_VDEP, VDEP_SIZE)?;
    let m: &Market = load(&a[4], pid, DISC_MARKET, MARKET_SIZE)?;
    if vd.vault != key(&a[1]) || vd.owner != key(&a[0]) || key(&a[3]) != v.subs[0] || key(&a[5]) != v.token_vault || m.vault != v.token_vault { return Err(err(E::BadAccount)); }
    if a[7].address() != &pinocchio_token::ID { return Err(err(E::TokenProgram)); }
    if sh == 0 || sh > vd.shares { return Err(err(E::BadData)); }
    let clk = clock()?; let nw: Now = (clk.slot, clk.unix_timestamp);
    if clk.slot < vd.unlock_slot { return Err(err(E::Locked)); }
    let nav = vault_nav(pid, v, &a[3..], nw)?;
    let gross = sh as i128 * (nav + VV) / (v.shares as i128 + VV);
    let cost = vd.basis as i128 * sh as i128 / vd.shares as i128;
    let fee = if gross > cost { (gross - cost) * v.perf_bps as i128 / 10000 } else { 0 }; // kept in the vault for remaining holders
    let out = gross - fee;
    if out <= 0 { return Err(err(E::BadData)); }
    if v.day_cap_bps != 0 {
        if clk.slot >= v.day_start + spec::SLOTS_PER_DAY { v.day_start = clk.slot; v.day_out = 0; }
        if (v.day_out as i128 + out) * 10000 > nav * v.day_cap_bps as i128 { return Err(err(E::Cap)); }
        v.day_out = i64c(v.day_out as i128 + out)?;
    }
    let s0: &mut User = load(&a[3], pid, DISC_USER, USER_SIZE)?;
    if out > s0.cash as i128 { return Err(err(E::Margin)); }
    s0.cash = i64c(s0.cash as i128 - out)?;
    let (eq, im, _) = health(pid, s0, &a[3..], nw, true)?;
    if eq < im { return Err(err(E::Margin)); }
    v.shares -= sh; vd.shares -= sh; vd.basis = i64c(vd.basis as i128 - cost)?;
    vault_mark(v, nav - out, clk.slot)?;
    let bump = [m.vault_bump as u8];
    let seeds = [Seed::from(b"vault".as_slice()), Seed::from(&m.vault_seed), Seed::from(&bump)];
    pinocchio_token::instructions::Transfer::new(&a[5], &a[6], &a[8], out as u64)
        .invoke_signed_with_program(&[Signer::from(&seeds)], a[7].address())
}
/// Permissionless: the vault takes over a position the book could not close, at the mark.
/// accs: vault(w), sub(w), target(w), market(r), insurance(w), link(r), target shards(w)..., [other markets]. data: tag, max_qty
fn vault_takeover(pid: &Address, a: &mut [AccountView], d: &[u8]) -> ProgramResult {
    if a.len() < 6 { return Err(err(E::BadData)); }
    let v: &mut Vault = load(&a[0], pid, DISC_VAULT, VAULT_SIZE)?;
    let l: &VaultLink = load(&a[5], pid, DISC_VLINK, VLINK_SIZE)?;
    let m: &Market = load(&a[3], pid, DISC_MARKET, MARKET_SIZE)?;
    let (vk, mk, sk) = (key(&a[0]), key(&a[3]), key(&a[1]));
    if v.halted != 0 { return Err(err(E::Halted)); }
    if l.vault != vk || l.market != mk { return Err(err(E::BadAccount)); }
    let si = v.subs[..v.n_subs as usize].iter().position(|k| *k == sk).ok_or(err(E::BadAccount))?;
    liquidate_core(pid, a, d, true)?;
    let u: &User = load(&a[1], pid, DISC_USER, USER_SIZE)?;
    if let Some(p) = u.pos.iter().find(|p| p.market == mk) {
        if mulf((p.base as i128).abs(), mark(m)) * 10000 > v.nav.max(0) as i128 * l.cap_bps as i128 { return Err(err(E::Cap)); }
    }
    v.active |= 1 << si;
    Ok(())
}
/// Permissionless: refresh value, peak and the drawdown stop. accs: vault(w), [active subs, markets]
fn vault_crank(pid: &Address, a: &mut [AccountView]) -> ProgramResult {
    if a.len() < 2 { return Err(err(E::BadData)); }
    let v: &mut Vault = load(&a[0], pid, DISC_VAULT, VAULT_SIZE)?;
    let clk = clock()?; let nw: Now = (clk.slot, clk.unix_timestamp);
    let nav = vault_nav(pid, v, &a[1..], nw)?;
    vault_mark(v, nav, clk.slot)
}
/// accs: admin(s), vault(w). data: tag, lockup_slots, day_cap_bps, dd_bps, halted
fn vault_set(pid: &Address, a: &mut [AccountView], d: &[u8]) -> ProgramResult {
    if a.len() < 2 || d.len() != 33 { return Err(err(E::BadData)); }
    let v: &mut Vault = load(&a[1], pid, DISC_VAULT, VAULT_SIZE)?;
    if !auth(pid, &v.admin, a) { return Err(err(E::BadAccount)); }
    v.lockup_slots = rd64(d, 1)?; v.day_cap_bps = rd64(d, 9)?; v.dd_bps = rd64(d, 17)?; v.halted = rd64(d, 25)?;
    if v.day_cap_bps > 10000 || v.dd_bps > 10000 { return Err(err(E::BadData)); }
    Ok(())
}

// ---------- Kani proofs of program-level arithmetic fast paths ----------
// Each fast path (native u64) must give the same value as the checked i128 path it replaces.
#[cfg(kani)]
mod kani_proofs {
    use super::*;
    #[kani::proof]
    fn mulf_fast_path_exact() {
        let a: i128 = kani::any(); let b: u64 = kani::any();
        kani::assume(a.unsigned_abs() < (1u128 << 32) && b < (1u64 << 32));
        let r = mulf(a, b);
        // the u64 product cannot overflow and equals the wide product
        assert!((a.unsigned_abs() as u64).checked_mul(b).is_some());
        assert_eq!(r, a * b as i128);
    }
    #[kani::proof]
    fn bps_fast_path_no_overflow() {
        let x: i128 = kani::any(); let b: u64 = kani::any();
        kani::assume(x >= 0 && x < FAST && b <= 65_535);
        assert!((x as u64).checked_mul(b).and_then(|v| v.checked_add(9999)).is_some());
    }
    #[kani::proof]
    fn moved_matches_wide_formula() {
        let a: u64 = kani::any(); let b: u64 = kani::any(); let bps: u64 = kani::any();
        kani::assume(a < (1 << 50) && b < (1 << 50) && bps <= 10_000);
        assert_eq!(moved(a, b, bps), (a as i128 - b as i128).abs() * 10000 > b as i128 * bps as i128);
    }
    #[kani::proof]
    fn liq_chunk_bounds() {
        let ab: u64 = kani::any(); kani::assume(ab > 0 && ab < (1u64 << 40));
        let q = liq_chunk(ab);
        assert!(q >= 1 && q <= ab);
    }
}
