//! Zero-copy fixed-size account layouts. All fields u64/i64/[u8;32] (8-byte aligned, no padding).
//! Mirrored in mm-bot/accounts.hpp (decoder); offsets checked by tests.
pub type Key = [u8; 32];
pub const DISC_MARKET: u64 = 0x504552504d4b5401; // "PERPMKT\x01"
pub const DISC_SHARD: u64 = 0x5045525053485201;
pub const DISC_USER: u64 = 0x5045525055535201;
pub const DISC_INS: u64 = 0x50455250494e5301;
pub const DISC_RANK: u64 = 0x5045525052414e01;
pub const DISC_POOL: u64 = 0x50455250504f4f01;
pub const RANK_N: usize = 8;
pub const MAX_ORDERS: usize = 64;
pub const MAX_LEDGER: usize = 32;
pub const MAX_TRIGGERS: usize = 4;
pub const MAX_POS: usize = 16;

#[repr(C)]
pub struct Market {
    pub disc: u64,
    pub admin: Key, pub mint: Key, pub vault: Key, pub insurance: Key, pub oracle: Key, pub feed_id: Key,
    pub vault_bump: u64, pub bucket_width: u64, pub imr_bps: u64, pub mmr_bps: u64,
    pub max_staleness_slots: u64, pub max_conf_bps: u64, pub max_age_secs: u64,
    pub expo: i64, pub px_mul: i64, pub px_div: i64,
    pub oracle_px: u64, pub oracle_conf: u64, pub oracle_slot: u64,
    pub funding_index: i64, pub funding_rate: i64, pub funding_slot: u64,
    /// Long open interest, aggregated LAZILY from shard counters (not written per trade).
    pub oi_long: u64, pub max_oi: u64, pub fees: i64,
    /// admin: 1 = paused (no new risk/deposits; cancels, reduce-only, withdraw, liquidation still work)
    pub paused: u64, pub tvl_cap: u64, pub event_seq: u64,
    /// AUDIT L-4: Pyth publish_time of the stored price (freshness checked at use time, monotonic updates)
    pub oracle_ts: i64,
    /// optional second oracle (PriceUpdateV2, own feed id) used as fallback / divergence circuit breaker
    pub oracle2: Key, pub feed_id2: Key, pub oracle2_px: u64,
    /// shards created (order ids are shard_no << 40 | seq, AUDIT L-3)
    pub n_shards: u64,
    /// on-chain ADL ranking account (AUDIT M-5); bad debt not absorbable by insurance/counterparty
    pub adl_rank: Key, pub bad_debt: i64,
    /// funding v4: remainder (quote*ppb*slots), premium TWAP accumulator, last sample, period start/len, impact notional
    pub funding_rem: i64, pub prem_acc: i64, pub prem_slots: u64, pub prem_last: i64, pub period_start: u64, pub funding_period: u64, pub impact_notional: u64,
    /// mark = median(oracle, oracle + EMA(mid - oracle), mid)
    pub mark_px: u64, pub mark_ema: i64,
    /// collateral pool (zero = market-owned vault); PDA seed key for the vault authority
    pub pool: Key, pub vault_seed: Key,
    /// per-account max |position| (lots), 0 = none
    pub max_position: u64,
    /// timelocked admin change: op, value, earliest slot, key (new admin / oracle2)
    pub pending_op: u64, pub pending_val: u64, pub pending_eta: u64, pub pending_key: Key,
    /// resting-book shards per side; strict_take = 1 makes takers prove they saw every opposite shard
    pub n_side: [u64; 2], pub strict_take: u64,
    /// discovery bounds: mark stays within bound_ref +- bound_bps; while closed the reference re-anchors up to bound_max times per direction
    pub bound_ref: u64, pub bound_bps: u64, pub bound_thresh_bps: u64, pub bound_max: u64, pub bound_up: u64, pub bound_dn: u64,
    /// 0 = external market open, 1 = closed (internal pricing)
    pub session: u64,
    /// external perp consensus price and its publish time
    pub ext_px: u64, pub ext_ts: i64,
    /// key allowed to flip `session` (the keeper), besides the admin
    pub session_auth: Key,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Order { pub id: u64, pub owner: Key, pub price: u64, pub qty: u64, pub expiry_slot: u64, pub band_bps: u64,
    /// client order id
    pub cid: u64,
    /// kind (bits 0-7) | maker rebate bps snapshot (8-15) | spline levels n (16-23) | current level (24-31)
    pub meta: u64,
    /// peg: a1 offset, a2 min, a3 max. spline: a1 step, a2 base qty, a3 qty step.
    pub a1: i64, pub a2: u64, pub a3: u64, pub lvl_left: u64 }

/// Maker fills owed to a user account; claimed lazily when the maker touches the shard.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Ledger { pub owner: Key, pub base: i64, pub quote: i64, pub open_reduce: u64, pub maker_vol: u64, pub n_closed: u64 }

#[repr(C)]
pub struct Shard {
    pub disc: u64, pub market: Key,
    pub side: u64, pub lo: u64, pub width: u64, pub seq: u64, pub n: u64, pub nled: u64,
    /// Per-shard accumulators (moved off the Market write path).
    pub oi_long_delta: i64, pub fee_accum: i64,
    /// per-market shard number (high bits of order ids)
    pub shard_no: u64,
    pub orders: [Order; MAX_ORDERS],
    pub ledger: [Ledger; MAX_LEDGER],
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Pos { pub market: Key, pub base: i64, pub funding_snap: i64, pub bid_open: u64, pub ask_open: u64,
    /// resting orders + unclaimed closed orders in this market (cap MAX_OPEN_ORDERS_PER_MARKET bounds shards a liquidator must pass)
    pub n_orders: u64,
    /// net quote paid since the position was opened (PnL = base*mark - cost); reset when flat. ADL ranking.
    pub cost: i64,
    /// isolated margin allocated to this position (flags & POS_FLAG_ISOLATED)
    pub iso_margin: i64, pub flags: u64 }

/// TP/SL (reduce-only, one-shot) or TWAP (slice every `interval` slots); executed by a permissionless keeper.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Trigger { pub market: Key, pub kind: u64, pub side: u64, pub trigger_px: u64, pub limit_px: u64, pub qty: u64, pub tip: u64,
    pub interval: u64, pub next_slot: u64, pub slices_left: u64 }

#[repr(C)]
pub struct User {
    pub disc: u64, pub authority: Key, pub cash: i64, pub tier: u64, pub maker_volume: u64,
    /// collateral vault this account is bound to (set on first deposit). Cash is only valid against this vault.
    pub home_vault: Key,
    pub pos: [Pos; MAX_POS],
    pub triggers: [Trigger; MAX_TRIGGERS],
}

#[repr(C)]
pub struct Insurance { pub disc: u64, pub market: Key, pub balance: i64,
    /// OI change from liquidation takeovers (folded by `aggregate`, keeps Market read-only in liquidate)
    pub oi_long_delta: i64 }

#[repr(C)]
#[derive(Clone, Copy)]
pub struct RankEntry { pub user: Key, pub score: i64 }
/// ADL queue: top RANK_N profitable+leveraged accounts per side (0 = long, 1 = short), score desc.
#[repr(C)]
pub struct Rank { pub disc: u64, pub market: Key, pub n_long: u64, pub n_short: u64, pub entries: [RankEntry; 2 * RANK_N] }

/// Pooled collateral: one vault shared by admin-approved markets (cross-margin across markets).
#[repr(C)]
pub struct Pool { pub disc: u64, pub admin: Key, pub mint: Key, pub vault: Key, pub vault_bump: u64, pub n_markets: u64,
    /// AUDIT N-6: market admission is timelocked (pool_propose -> init_market after eta) and exposure-capped
    pub pending_market: Key, pub pending_eta: u64, pub max_market_oi: u64 }
/// m-of-n admin: a market/pool admin key may be the address of this account; signers among keys authorize.
#[repr(C)]
pub struct Multisig { pub disc: u64, pub m: u64, pub n: u64, pub keys: [Key; 8] }
pub const DISC_MULTISIG: u64 = 0x504552504d534701;
pub const MULTISIG_SIZE: usize = core::mem::size_of::<Multisig>();

pub const MARKET_SIZE: usize = core::mem::size_of::<Market>();
pub const SHARD_SIZE: usize = core::mem::size_of::<Shard>();
pub const USER_SIZE: usize = core::mem::size_of::<User>();
pub const INS_SIZE: usize = core::mem::size_of::<Insurance>();
pub const RANK_SIZE: usize = core::mem::size_of::<Rank>();
pub const POOL_SIZE: usize = core::mem::size_of::<Pool>();
pub const ORDER_SIZE: usize = core::mem::size_of::<Order>();
pub const SHARD_ORDERS_OFF: usize = core::mem::offset_of!(Shard, orders);
pub const SHARD_N_OFF: usize = core::mem::offset_of!(Shard, n);
pub const MARKET_ORACLE_PX_OFF: usize = core::mem::offset_of!(Market, oracle_px);
pub const MARKET_ORACLE_SLOT_OFF: usize = core::mem::offset_of!(Market, oracle_slot);

/// Keeper-posted aggregated price (our own oracle; no Pyth). Byte layout mirrors PriceUpdateV2's price fields so the
/// same reader handles both: [0..8] disc, [8..40] authority (sole updater), [40] 1, [41..73] feed id, 73 price i64,
/// 81 conf u64, 89 expo i32, 93 publish_time i64, 101 prev_publish_time, 109 max_jump_bps, 117 n_sources, 125 posts.
pub const AGG_DISC: [u8; 8] = *b"PERPAGG\x01";
pub const AGG_SIZE: usize = 160;
pub const DISC_DELEGATE: u64 = 0x5045525044454c01; // "PERPDEL\x01"
/// Session key / one-click trading: `delegate` may place and cancel orders for `user` until `expiry_slot`,
/// each order qty <= `max_qty`. It can never withdraw, transfer, change margin mode or set triggers (those stay authority-only).
#[repr(C)]
pub struct Delegate { pub disc: u64, pub user: Key, pub delegate: Key, pub expiry_slot: u64, pub max_qty: u64 }
pub const DELEGATE_SIZE: usize = core::mem::size_of::<Delegate>();

// ---------- fast quote path ----------
pub const DISC_SLAB: u64 = 0x50455250534c4201; // "PERPSLB\x01"
pub const DISC_FAST: u64 = 0x5045525046415301; // "PERPFAS\x01"
pub const SLAB_QUOTES: usize = 2048;
pub const SLAB_LEVELS: usize = 8;
pub const FAST_MAKERS: usize = 8;
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Quote { pub px: u32, pub qty: u32, pub gen: u32, pub slot: u32 }
/// A maker's quotes for every market: written by the maker alone, read by takers. Index = market idx << 4 | side << 3 | level.
#[repr(C)]
pub struct Slab { pub disc: u64, pub authority: Key, pub user: Key, pub gen: u64, pub count: u64, pub kill: u64, pub quotes: [Quote; SLAB_QUOTES] }
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Used { pub gen: u32, pub qty: u32 }
/// Margin locked for quoting here, the notional it may still fill, and fills not yet moved to the maker's User.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct FastMaker { pub slab: Key, pub user: Key, pub margin: i64, pub budget: i64, pub base: i64, pub quote: i64, pub volume: u64 }
/// Per-market fill state: takers write here, so quotes and fills never contend on a write lock.
#[repr(C)]
pub struct FastBook { pub disc: u64, pub market: Key, pub idx: u64, pub n: u64, pub oi_long_delta: i64, pub fee_accum: i64, pub fills: u64,
    pub makers: [FastMaker; FAST_MAKERS], pub used: [[Used; 2 * SLAB_LEVELS]; FAST_MAKERS] }
pub const SLAB_SIZE: usize = core::mem::size_of::<Slab>();
pub const FAST_SIZE: usize = core::mem::size_of::<FastBook>();

// ---------- HLP-style vault ----------
pub const DISC_VAULT: u64 = 0x5045525056415501; // "PERPVAU\x01"
pub const DISC_VDEP: u64 = 0x5045525056445001; // "PERPVDP\x01"
pub const DISC_VLINK: u64 = 0x50455250564c4b01; // "PERPVLK\x01"
pub const VAULT_SUBS: usize = 32;
/// Liquidity, insurance and liquidation takeover in one pool. Positions sit in program-controlled User sub-accounts
/// (authority = this account); subs[0] holds the idle cash. Sub-accounts in `active` must be passed to price the vault.
#[repr(C)]
pub struct Vault { pub disc: u64, pub admin: Key, pub token_vault: Key, pub shares: u64, pub nav: i64, pub peak_nav: i64, pub nav_slot: u64,
    pub lockup_slots: u64, pub day_cap_bps: u64, pub perf_bps: u64, pub dd_bps: u64, pub halted: u64, pub day_start: u64, pub day_out: i64,
    pub active: u64, pub n_subs: u64, pub subs: [Key; VAULT_SUBS] }
#[repr(C)]
pub struct VaultDeposit { pub disc: u64, pub vault: Key, pub owner: Key, pub shares: u64, pub unlock_slot: u64, pub basis: i64 }
/// Markets the vault may take positions in, with an inventory cap as bps of NAV.
#[repr(C)]
pub struct VaultLink { pub disc: u64, pub vault: Key, pub market: Key, pub cap_bps: u64 }
pub const VAULT_SIZE: usize = core::mem::size_of::<Vault>();
pub const VDEP_SIZE: usize = core::mem::size_of::<VaultDeposit>();
pub const VLINK_SIZE: usize = core::mem::size_of::<VaultLink>();
