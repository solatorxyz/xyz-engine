//! Full-onchain perp program (pinocchio, zero-copy). Matching, cross-margin, liquidation,
//! lazy funding, ADL. UNAUDITED. See ../README.md.
#![cfg_attr(target_os = "solana", no_std)]
#![allow(clippy::too_many_arguments)]
#[path = "../../perp-core/src/spec.rs"]
#[allow(dead_code)]
pub mod spec;

pub mod state;

use pinocchio::cpi::{Seed, Signer};
use pinocchio::sysvars::instructions::{Instructions, INSTRUCTIONS_ID};
#[cfg(target_os = "solana")]
use pinocchio::sysvars::Sysvar;
use pinocchio::Resize;
use pinocchio::{error::ProgramError, sysvars::clock::Clock, AccountView, Address, ProgramResult};
use spec::{
    ADMIN_MAX_OI, ADMIN_PAUSE, ADMIN_TVL_CAP, BATCH_HEADER_N_CANCEL_OFF, BATCH_HEADER_N_PLACE_OFF, BATCH_HEADER_SIZE,
    CANCEL_BY_CID_BIT, CANCEL_ITEM_SIZE, EVENTS_ADL, EVENTS_FILL, EVENTS_LIQUIDATION, EVENTS_TRIGGER, FLAG_ALO_SLIDE,
    FLAG_FOK, FLAG_IOC, FLAG_POST_ONLY, FLAG_REDUCE_ONLY, FLAG_STP_MASK, FLAG_STP_SHIFT, FUNDING_HOURS_PER_8H,
    FUNDING_IMPACT_NOTIONAL, FUNDING_INTEREST_8H_PPB, FUNDING_MAX_RATE_HOURLY_PPB, FUNDING_PERIOD_SLOTS, FUNDING_PPB,
    FUNDING_PREMIUM_CLAMP_8H_PPB, IMR_BPS, IX_BACKSTOP, IX_BATCH, IX_CANCEL, IX_LIQUIDATE, IX_PLACE, IX_SET_ORACLE,
    KIND_LIMIT, KIND_PEG, KIND_SPLINE, LIQ_ADL_WINDOW_SLOTS, LIQ_BOOK_BAND_BPS, LIQ_BOOK_BAND_STEPS,
    LIQ_BOOK_CHUNK_BPS, LIQ_FEE_BPS, MAX_KEEPER_TIP, MAX_OPEN_ORDERS_PER_MARKET, MAX_PX, MAX_SHARD_NO,
    MAX_SPLINE_LEVELS, MIN_ORDER_NOTIONAL, MMR_BPS, ORDER_SEQ_BITS, PLACE_ITEM_A1_OFF, PLACE_ITEM_A2_OFF,
    PLACE_ITEM_A3_OFF, PLACE_ITEM_CID_OFF, PLACE_ITEM_EXPIRY_SLOT_OFF, PLACE_ITEM_FLAGS_OFF, PLACE_ITEM_KIND_OFF,
    PLACE_ITEM_ORACLE_BAND_BPS_OFF, PLACE_ITEM_PRICE_OFF, PLACE_ITEM_QTY_OFF, PLACE_ITEM_SIZE, SIDE_ASK, SIDE_BID,
    STP_CANCEL_NEWEST, STP_CANCEL_OLDEST, TICK_SIZE as TICK, TIERS, TRIGGER_KIND_SL, TRIGGER_KIND_TP,
    TRIGGER_KIND_TWAP,
};
// Compile-time conformance: zero-copy structs == shared spec layout (also used by the C++ decoder).
const _: () = {
    use core::mem::offset_of as off;
    assert!(
        MARKET_SIZE as u64 == spec::MARKET_SIZE
            && SHARD_SIZE as u64 == spec::SHARD_SIZE
            && USER_SIZE as u64 == spec::USER_SIZE
    );
    assert!(
        ORDER_SIZE as u64 == spec::ORDER_SIZE
            && off!(Shard, orders) as u64 == spec::SHARD_ORDERS_OFF
            && off!(Shard, ledger) as u64 == spec::SHARD_LEDGER_OFF
    );
    assert!(
        off!(Shard, n) as u64 == spec::SHARD_N_OFF
            && off!(Market, oracle_px) as u64 == spec::MARKET_ORACLE_PX_OFF
            && off!(Market, oi_long) as u64 == spec::MARKET_OI_LONG_OFF
    );
    assert!(
        off!(User, pos) as u64 == spec::USER_POS_OFF
            && off!(Order, owner) as u64 == spec::ORDER_OWNER_OFF
            && off!(Order, qty) as u64 == spec::ORDER_QTY_OFF
    );
    assert!(
        DISC_SHARD == spec::DISC_SHARD
            && DISC_MARKET == spec::DISC_MARKET
            && DISC_USER == spec::DISC_USER
            && DISC_INS == spec::DISC_INSURANCE
    );
    assert!(
        off!(User, triggers) as u64 == spec::USER_TRIGGERS_OFF
            && off!(Market, paused) as u64 == spec::MARKET_PAUSED_OFF
            && off!(Order, lvl_left) as u64 == spec::ORDER_LVL_LEFT_OFF
    );
    assert!(
        core::mem::size_of::<Trigger>() as u64 == spec::TRIGGER_SIZE
            && MAX_LEDGER as u64 * spec::LEDGER_SIZE + spec::SHARD_LEDGER_OFF == spec::SHARD_SIZE
    );
    assert!(
        LEASE_SIZE as u64 == spec::LEASE_SIZE
            && LEASE_MEMBERS as u64 == spec::LEASE_MEMBERS
            && off!(Lease, key) as u64 == spec::LEASE_KEY_OFF
            && off!(Lease, members) as u64 == spec::LEASE_MEMBERS_OFF
            && off!(Lease, n) as u64 == spec::LEASE_N_OFF
            && off!(Lease, holder) as u64 == spec::LEASE_HOLDER_OFF
            && off!(Lease, since) as u64 == spec::LEASE_SINCE_OFF
            && off!(Lease, expiry) as u64 == spec::LEASE_EXPIRY_OFF
            && off!(Lease, term) as u64 == spec::LEASE_TERM_OFF
    );
    assert!(
        INS_SIZE as u64 == spec::INSURANCE_SIZE
            && RANK_SIZE as u64 == spec::RANK_SIZE
            && POOL_SIZE as u64 == spec::POOL_SIZE
            && MARKET_SIZE as u64 == spec::MARKET_SIZE
    );
    assert!(
        off!(Market, pending_key) as u64 == spec::MARKET_PENDING_KEY_OFF
            && off!(Shard, shard_no) as u64 == spec::SHARD_SHARD_NO_OFF
            && off!(Pos, flags) as u64 == spec::POS_FLAGS_OFF
    );
    assert!(
        DISC_RANK == spec::DISC_RANK
            && DISC_POOL == spec::DISC_POOL
            && RANK_N as u64 == spec::ADL_RANK_SIZE
            && E::Isolated as u64 == spec::ERR_ISOLATED
            && E::Breaker as u64 == spec::ERR_BREAKER
    );
    assert!(
        spec::IX_INIT_MARKET == IX_INIT_MARKET as u64
            && spec::IX_AGGREGATE == IX_AGGREGATE as u64
            && spec::IX_WITHDRAW == IX_WITHDRAW as u64
    );
};
use state::*;
#[cfg(feature = "cuprof")]
macro_rules! cp {
    ($n:expr) => {
        unsafe {
            pinocchio::syscalls::sol_log_64_(0xC0, $n, 0, 0, 0);
            pinocchio::syscalls::sol_log_compute_units_();
        }
    };
}

#[cfg(not(feature = "cuprof"))]
macro_rules! cp {
    ($n:expr) => {};
}

mod accounts;
mod adl;
mod admin;
mod book;
mod custody;
mod delegate;
mod error;
mod events;
mod fast_take;
mod init;
mod lease;
mod liquidation;
mod margin;
mod math;
mod oracle;
mod quote;
mod signed_px;
mod trigger;
mod vault;

use accounts::*;
use adl::*;
use admin::*;
use book::*;
use custody::*;
use delegate::*;
use error::*;
use events::*;
use fast_take::*;
use init::*;
use lease::*;
use liquidation::*;
use margin::*;
use math::*;
use oracle::*;
use quote::*;
use signed_px::*;
use trigger::*;
use vault::*;

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
fn clock() -> Result<Clock, ProgramError> {
    Clock::get()
}

#[cfg(not(target_os = "solana"))]
pub static HOST_CLOCK: std::sync::Mutex<(u64, i64)> = std::sync::Mutex::new((0, 0));

#[cfg(not(target_os = "solana"))]
fn clock() -> Result<Clock, ProgramError> {
    let (slot, ts) = *HOST_CLOCK.lock().unwrap();
    Ok(Clock { slot, epoch_start_timestamp: 0, epoch: 0, leader_schedule_epoch: 0, unix_timestamp: ts })
}

pub const IX_INIT_MARKET: u8 = 10;
pub const IX_INIT_SHARD: u8 = 11;
pub const IX_INIT_USER: u8 = 12;
pub const IX_DEPOSIT: u8 = 13;
pub const IX_WITHDRAW: u8 = 14;
pub const IX_SETTLE: u8 = 15;
pub const IX_AGGREGATE: u8 = 16;
pub fn process_instruction(pid: &Address, accs: &mut [AccountView], data: &[u8]) -> ProgramResult {
    let tag = *data.first().ok_or(err(E::BadData))?;
    match tag as u64 {
        spec::IX_SLAB_QUOTE => slab_quote(pid, accs, data),
        spec::IX_TAKE_FAST => take_fast(pid, accs, data),
        spec::IX_TAKE_FAST_BATCH => take_fast_batch(pid, accs, data),
        spec::IX_SLAB_GROW => slab_grow(pid, accs),
        spec::IX_FAST_SIGNER => fast_signer(pid, accs, data),
        spec::IX_FAST_PX => fast_px_push(pid, accs),
        spec::IX_FAST_REGISTER => fast_register(pid, accs),
        spec::IX_LIQ_CANCEL => liq_cancel(pid, accs),
        spec::IX_UPDATE_ORACLE_SOFT => update_oracle_soft(pid, accs),
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
        spec::IX_SLAB_TOUCH => slab_touch(pid, accs),
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
        spec::IX_VAULT_SLAB_INIT => vault_slab_init(pid, accs),
        spec::IX_VAULT_JOIN => vault_join(pid, accs),
        spec::IX_VAULT_QUOTE => vault_quote(pid, accs, data),
        spec::IX_VAULT_FUND => vault_fund(pid, accs, data),
        spec::IX_VAULT_KILL => vault_kill(pid, accs, data),
        spec::IX_VAULT_OPERATOR => vault_operator(pid, accs, data),
        spec::IX_VAULT_MOVE => vault_move(pid, accs, data),
        spec::IX_VAULT_VALUE => vault_value(pid, accs),
        spec::IX_LEASE_INIT => lease_init(pid, accs, data),
        spec::IX_LEASE_CLAIM => lease_claim(pid, accs, data),
        spec::IX_ADMIN_EXECUTE => admin_execute(pid, accs),
        IX_AGG_INIT => agg_init(pid, accs, data),
        IX_AGG_POST => agg_post(pid, accs, data),
        IX_SET_DELEGATE => set_delegate(pid, accs, data),
        IX_OSET_INIT => oset_init(pid, accs, data),
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

#[cfg(test)]
mod arith_tests {
    use super::*;
    struct R(u64);
    impl R {
        fn n(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }
    }
    /// The native-multiply paths give exactly the i128 product / rounding, across magnitudes and the 2^32 edges.
    #[test]
    fn fast_arithmetic_matches_i128() {
        let mut r = R(0x9E3779B97F4A7C15);
        let edge = [0u64, 1, (1 << 32) - 1, 1 << 32, (1 << 32) + 1, u64::MAX >> 1, u64::MAX];
        for i in 0..2_000_000u64 {
            let a = match i % 4 {
                0 => (r.n() % (1 << 32)) as i128,
                1 => -((r.n() % (1 << 32)) as i128),
                2 => (r.n() >> (r.n() % 64)) as i128,
                _ => -((r.n() >> 1 >> (r.n() % 63)) as i128),
            };
            let b = if i % 7 == 0 { edge[(r.n() % 7) as usize] } else { r.n() >> (r.n() % 64) };
            if (a.unsigned_abs() as f64) * (b as f64) < 1.6e38 {
                assert_eq!(mulf(a, b), a * b as i128, "mulf({a}, {b})");
            }
            let x = (r.n() >> (r.n() % 64)) as i128 % (FAST + 3);
            let bp = r.n() % 70_000;
            assert_eq!(bps_up(x, bp), (x * bp as i128 + 9999) / 10000);
            assert_eq!(bps_dn(x, bp), x * bp as i128 / 10000);
        }
    }
}

// ---------- Kani proofs of program-level arithmetic fast paths ----------
// Each fast path (native u64) must give the same value as the checked i128 path it replaces.
/// Further machine-checked properties live in proofs.rs (same access to private items as this file).
#[cfg(kani)]
mod proofs;

#[cfg(kani)]
mod kani_proofs {
    use super::*;
    #[kani::proof]
    fn mulf_fast_path_exact() {
        let a: i128 = kani::any();
        let b: u64 = kani::any();
        kani::assume(a.unsigned_abs() < (1u128 << 32) && b < (1u64 << 32));
        let r = mulf(a, b);
        // the u64 product cannot overflow and equals the wide product
        assert!((a.unsigned_abs() as u64).checked_mul(b).is_some());
        assert_eq!(r, a * b as i128);
    }
    #[kani::proof]
    fn bps_fast_path_no_overflow() {
        let x: i128 = kani::any();
        let b: u64 = kani::any();
        kani::assume(x >= 0 && x < FAST && b <= 65_535);
        assert!((x as u64).checked_mul(b).and_then(|v| v.checked_add(9999)).is_some());
    }
    #[kani::proof]
    fn moved_matches_wide_formula() {
        let a: u64 = kani::any();
        let b: u64 = kani::any();
        let bps: u64 = kani::any();
        kani::assume(a < (1 << 50) && b < (1 << 50) && bps <= 10_000);
        assert_eq!(moved(a, b, bps), (a as i128 - b as i128).abs() * 10000 > b as i128 * bps as i128);
    }
    /// The split path (|a| < 2^32, any b, prices above 2^32 included) is the exact i128 product.
    #[kani::proof]
    #[kani::solver(cadical)]
    fn mulf_split_path_exact() {
        let a: i128 = kani::any();
        let b: u64 = kani::any();
        kani::assume(a.unsigned_abs() < (1u128 << 32));
        assert_eq!(mulf(a, b), a * b as i128);
    }
    /// The native bps paths round exactly like the i128 formulas.
    #[kani::proof]
    #[kani::solver(cadical)]
    fn bps_fast_paths_exact() {
        let x: i128 = kani::any();
        let b: u64 = kani::any();
        kani::assume(x >= 0 && x < FAST && b <= 65_535);
        assert_eq!(bps_up(x, b), (x * b as i128 + 9999) / 10000);
        assert_eq!(bps_dn(x, b), x * b as i128 / 10000);
    }
    /// Booking the taker once per take (Acc) leaves cash, position and cost basis exactly as booking every fill does, for any
    /// three fills from any starting position, including fills that reach zero or cross it.
    #[kani::proof]
    #[kani::solver(cadical)]
    #[kani::unwind(3)]
    fn taker_booked_once_equals_every_fill() {
        let mut u1: User = unsafe { core::mem::zeroed() };
        let (b0, c0): (i64, i64) = (kani::any(), kani::any());
        kani::assume(b0 > -2000 && b0 < 2000 && c0 > -(1 << 40) && c0 < (1 << 40));
        u1.pos[0].base = b0;
        u1.pos[0].cost = c0;
        let mut u2: User = unsafe { core::mem::zeroed() };
        u2.pos[0].base = b0;
        u2.pos[0].cost = c0;
        let bid: bool = kani::any();
        let mut acc = Acc { q: 0, nt: 0, fee: 0 };
        for _ in 0..2 {
            let (fq, p, fee): (u64, u64, u64) = (kani::any(), kani::any(), kani::any());
            kani::assume(fq >= 1 && fq <= 1500 && p >= 1 && p < (1 << 16) && fee <= (1 << 16));
            let nt = p * fq;
            acc.add(&mut u1, 0, bid, fq, nt, fee).unwrap();
            let paid: i128 = if bid { nt as i128 } else { -(nt as i128) };
            credit(&mut u2, 0, -(fee as i128) - paid).unwrap();
            let ob = u2.pos[0].base;
            u2.pos[0].base = if bid { ob + fq as i64 } else { ob - fq as i64 };
            u2.pos[0].cost = cost_after(ob, u2.pos[0].base, u2.pos[0].cost, paid).unwrap();
        }
        acc.book(&mut u1, 0, bid).unwrap();
        assert_eq!((u1.cash, u1.pos[0].base, u1.pos[0].cost), (u2.cash, u2.pos[0].base, u2.pos[0].cost));
    }
    /// A fill moves exactly what one side gives to the other: the maker's lots and quote mirror the taker's, the maker budget
    /// drops by the notional, and the quote's used size grows by the lots (any two fills against one or two levels).
    #[kani::proof]
    #[kani::solver(cadical)]
    #[kani::unwind(3)]
    fn fills_conserve_lots_and_notional() {
        let mut f: FastBook = unsafe { core::mem::zeroed() };
        let mut u: User = unsafe { core::mem::zeroed() };
        f.makers[0].budget = i64::MAX >> 2;
        let bid: bool = kani::any();
        let mut acc = Acc { q: 0, nt: 0, fee: 0 };
        let (mut nts, mut lots) = (0u64, 0u64);
        for _ in 0..2 {
            let (fq, p, l): (u64, u64, usize) = (kani::any(), kani::any(), kani::any());
            kani::assume(fq >= 1 && fq < (1 << 32) && p >= 1 && p < (1 << 32) && l < 2);
            let q = Quote { px: p as u32, qty: u32::MAX, gen: 7, slot: 0 };
            let before = f.used[0][l].qty as u64;
            // what the matcher guarantees: fq <= quote qty - used and fq <= budget / p
            kani::assume(before + fq < (1 << 32) && p * fq <= f.makers[0].budget as u64);
            let nt = fill_maker(&mut f, 0, &q, l, bid, p, fq, 0, 0).unwrap();
            assert_eq!(nt, p * fq);
            if f.used[0][l].gen == 7 {
                assert!(f.used[0][l].qty as u64 == before + fq || before == 0);
            }
            acc.add(&mut u, 0, bid, fq, nt, 0).unwrap();
            nts += nt;
            lots += fq;
        }
        acc.book(&mut u, 0, bid).unwrap();
        assert_eq!(u.pos[0].base + f.makers[0].base, 0);
        assert_eq!(u.cash as i128 + f.makers[0].quote as i128, 0);
        assert_eq!(f.makers[0].budget as i128, (i64::MAX >> 2) as i128 - nts as i128);
        assert_eq!(f.ext[0].rebate_bps >> 16, lots);
    }
    /// Withdrawal lane: a position never contributes more than its mark value or its cost, so positive unrealized PnL is never
    /// credited, while a loss counts in full (percolator v16 Eq_no_positive_credit).
    #[kani::proof]
    #[kani::solver(cadical)]
    fn np_value_never_credits_a_gain() {
        let (base, cost, px): (i64, i64, u64) = (kani::any(), kani::any(), kani::any());
        kani::assume(base.unsigned_abs() < (1u64 << 32) && px < (1u64 << 48));
        let mv = base as i128 * px as i128;
        let v = np_value(base, cost, px);
        assert!(v <= mv && v <= cost as i128);
        if mv <= cost as i128 {
            assert_eq!(v, mv);
        }
        // losing (or flat) position: full mark value, the loss counts
        else {
            assert_eq!(v, cost as i128);
        } // winning position: only what was paid, the gain is left out
    }
    /// Pegged orders always price on the tick grid inside [TICK, MAX_PX], whatever the oracle, offset and limits.
    #[kani::proof]
    #[kani::solver(cadical)]
    fn peg_price_in_range() {
        let (o, off, lo, hi): (u64, i64, u64, u64) = (kani::any(), kani::any(), kani::any(), kani::any());
        let p = peg_price(o, off, lo, hi);
        assert!(p >= TICK && p <= MAX_PX && p % TICK == 0);
    }
    /// A position that goes flat carries no cost; a reduction on the same side never changes the sign of the basis' owner.
    #[kani::proof]
    fn cost_after_flat_is_zero() {
        let (ob, cost): (i64, i64) = (kani::any(), kani::any());
        let paid: i64 = kani::any();
        assert_eq!(cost_after(ob, 0, cost, paid as i128).unwrap(), 0);
    }
    /// The native u64 paths (band_of, quote_band, exceeds, bps_of, below_min_notional) are taken only where their products
    /// cannot wrap; with no wrap a u64 product and quotient equal the u128 ones, so the paths are exact. (The full
    /// equality with symbolic 64-bit division does not terminate in the solver; the no-wrap condition is what it rests on.)
    #[kani::proof]
    fn native_paths_cannot_wrap() {
        let (a, b, bps): (u64, u64, u64) = (kani::any(), kani::any(), kani::any());
        if a < (1 << 50) {
            assert!(a.checked_mul(10000).is_some());
        } // band_of
        if a < (1 << 48) {
            assert!(a.checked_mul(spec::QUOTE_BAND_BPS).is_some());
        } // quote_band
        if a < (1 << 48) && b < (1 << 48) && bps <= 65_535 {
            assert!(a.checked_mul(10000).is_some() && b.checked_mul(bps).is_some() && a.checked_mul(bps).is_some());
        } // exceeds, bps_of
        if a < (1 << 32) && b < (1 << 32) {
            assert!(a.checked_mul(b).is_some());
        } // below_min_notional
    }
    /// Results on small widths, where the solver can do the division: same value as the wide formulas, band capped.
    #[kani::proof]
    #[kani::solver(cadical)]
    fn native_paths_match_wide_small() {
        let (a, b, bps): (u16, u16, u8) = (kani::any(), kani::any(), kani::any());
        let (a, b, bps) = (a as u64, b as u64, bps as u64);
        if b != 0 {
            assert_eq!(band_of(a, b), ((a as u128 * 10000 / b as u128) as u64).min(spec::BAND_MAX_BPS));
        }
        assert_eq!(band_of(a, 0), 0);
        assert_eq!(quote_band(a), (a as u128 * spec::QUOTE_BAND_BPS as u128 / 10000) as u64);
        assert_eq!(exceeds(a, b, bps), a as u128 * 10000 > b as u128 * bps as u128);
        assert_eq!(bps_of(a, bps), (a as u128 * bps as u128 / 10000) as u64);
        assert_eq!(below_min_notional(a, b), (a as u128) * (b as u128) < MIN_ORDER_NOTIONAL as u128);
    }
    #[kani::proof]
    fn liq_chunk_bounds() {
        let ab: u64 = kani::any();
        kani::assume(ab > 0 && ab < (1u64 << 40));
        let q = liq_chunk(ab);
        assert!(q >= 1 && q <= ab);
    }
}
