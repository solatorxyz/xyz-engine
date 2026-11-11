// Restored from mm-bot perp_spec.hpp (spec v7): the original spec.rs was not in the AWS archive.
pub const SPEC_VERSION: u64 = 7;
pub const TICK_SIZE: u64 = 1;
pub const LOT_SIZE: u64 = 1;
pub const BUCKET_WIDTH: u64 = 100;
pub const IMR_BPS: u64 = 1000;
pub const MMR_BPS: u64 = 500;
pub const MAX_STALENESS_SLOTS: u64 = 3;
pub const LIQ_FEE_BPS: u64 = 25;
pub const LIQ_KEEPER_SHARE_BPS: u64 = 2000;
pub const LIQ_BAND_PCT: u64 = 5;
pub const LIQ_BOOK_CHUNK_BPS: u64 = 2500;
pub const LIQ_BOOK_BAND_BPS: u64 = 500;
pub const LIQ_BOOK_BAND_STEPS: u64 = 3;
pub const LIQ_ADL_WINDOW_SLOTS: u64 = 25;
pub const PAUSED_ADMIN: u64 = 1;
pub const PAUSED_BREAKER: u64 = 2;
pub const PAUSED_SINGLE_SOURCE: u64 = 3;
pub const FLAG_POST_ONLY: u64 = 1;
pub const FLAG_REDUCE_ONLY: u64 = 2;
pub const FLAG_IOC: u64 = 4;
pub const FLAG_FOK: u64 = 8;
pub const FLAG_ALO_SLIDE: u64 = 16;
pub const FLAG_STP_SHIFT: u64 = 5;
pub const FLAG_STP_MASK: u64 = 96;
/// Replace: the place lands only if the cancel item at the same index removed a live order (a quote that was
/// filled or expired in the meantime is not re-posted).
pub const FLAG_IF_CANCELLED: u64 = 128;
pub const SIDE_BID: u64 = 0;
pub const SIDE_ASK: u64 = 1;
pub const IX_PLACE: u64 = 1;
pub const IX_CANCEL: u64 = 2;
pub const IX_BATCH: u64 = 3;
pub const IX_SET_ORACLE: u64 = 4;
pub const IX_LIQUIDATE: u64 = 5;
pub const IX_BACKSTOP: u64 = 6;
pub const IX_INIT_MARKET: u64 = 10;
pub const IX_INIT_SHARD: u64 = 11;
pub const IX_INIT_USER: u64 = 12;
pub const IX_DEPOSIT: u64 = 13;
pub const IX_WITHDRAW: u64 = 14;
pub const IX_SETTLE: u64 = 15;
pub const IX_AGGREGATE: u64 = 16;
pub const IX_SET_TRIGGER: u64 = 20;
pub const IX_EXEC_TRIGGER: u64 = 21;
pub const IX_ADMIN: u64 = 22;
pub const IX_SET_TIER: u64 = 23;
pub const IX_CRANK_CLAIM: u64 = 24;
pub const IX_ADL_RANK: u64 = 25;
pub const IX_INIT_RANK: u64 = 26;
pub const IX_INIT_POOL: u64 = 27;
pub const IX_ADMIN_PROPOSE: u64 = 28;
pub const IX_ADMIN_EXECUTE: u64 = 29;
pub const IX_SET_ISOLATED: u64 = 30;
pub const IX_ISO_TRANSFER: u64 = 31;
pub const IX_POOL_PROPOSE: u64 = 32;
pub const IX_INIT_MULTISIG: u64 = 33;
pub const IX_LIQUIDATE_BOOK: u64 = 34;
pub const STP_CANCEL_OLDEST: u64 = 0;
pub const STP_CANCEL_NEWEST: u64 = 1;
pub const STP_DECREMENT: u64 = 2;
pub const KIND_LIMIT: u64 = 0;
pub const KIND_PEG: u64 = 1;
pub const KIND_SPLINE: u64 = 2;
pub const TRIGGER_KIND_NONE: u64 = 0;
pub const TRIGGER_KIND_TP: u64 = 1;
pub const TRIGGER_KIND_SL: u64 = 2;
pub const TRIGGER_KIND_TWAP: u64 = 3;
pub const TRIGGER_KIND_FLAG_MARK: u64 = 256;
pub const TRIGGER_KIND_FLAG_CONFIRM_ORACLE: u64 = 512;
pub const ADMIN_PAUSE: u64 = 1;
pub const ADMIN_TVL_CAP: u64 = 2;
pub const ADMIN_MAX_OI: u64 = 3;
pub const ADMIN_IMR: u64 = 4;
pub const ADMIN_MMR: u64 = 5;
pub const ADMIN_MAX_POSITION: u64 = 6;
pub const ADMIN_IMPACT_NOTIONAL: u64 = 7;
pub const ADMIN_SET_ADMIN: u64 = 8;
pub const ADMIN_FUNDING_PERIOD: u64 = 9;
pub const ADMIN_ORACLE2: u64 = 10;
/// Timelocked change of the primary oracle account (e.g. to an OracleSet of independent publishers). key = new oracle.
pub const ADMIN_ORACLE: u64 = 11;
/// Timelocked: price signer of the market's registered FastBooks (key; zero = none). Clearing it is immediate (fast_signer).
pub const ADMIN_FAST_SIGNER: u64 = 12;
/// Timelocked: discovery bounds / strict take / session key. val = bound_bps | thresh_bps << 16 | bound_max << 32 | strict << 48,
/// key = session_auth.
pub const ADMIN_MARKET_CFG: u64 = 13;
pub const EVENTS_FILL: u64 = 1;
pub const EVENTS_LIQUIDATION: u64 = 2;
pub const EVENTS_TRIGGER: u64 = 3;
pub const EVENTS_ADL: u64 = 4;
pub const EVENTS_LIQ_NOFILL: u64 = 5;
pub const EVENTS_FAST_FILL: u64 = 6;
pub const EVENTS_FAST_BATCH: u64 = 7;
pub const CANCEL_BY_CID_BIT: u64 = 128;
/// Cancel item side byte: every resting order of the account in the passed shards of this market, both sides (id ignored).
pub const CANCEL_ALL_BIT: u64 = 64;
pub const MAX_SPLINE_LEVELS: u64 = 16;
pub const MAX_KEEPER_TIP: u64 = 1000000;
/// A TP/SL with no limit price trades as a market order bounded at the mark -/+ this (bps).
pub const TRIGGER_MARKET_SLIP_BPS: u64 = 1000;
pub const MAX_OPEN_ORDERS_PER_MARKET: u64 = 16;
pub const MIN_ORDER_NOTIONAL: u64 = 1000;
pub const MAX_PX: u64 = 4611686018427387904;
pub const ORDER_SEQ_BITS: u64 = 40;
pub const MAX_SHARD_NO: u64 = 8388608;
pub const ADL_RANK_SIZE: u64 = 8;
pub const TIMELOCK_SLOTS: u64 = 216000;
pub const IMPACT_MAX_LEVELS: u64 = 64;
pub const MULTISIG_MAX: u64 = 8;
pub const FUNDING_PPB: u64 = 1000000000;
pub const FUNDING_PERIOD_SLOTS: u64 = 9000;
pub const FUNDING_INTEREST_8H_PPB: u64 = 100000;
pub const FUNDING_PREMIUM_CLAMP_8H_PPB: u64 = 500000;
/// 0.5% an hour, the cap Hyperliquid documents (was 4%).
pub const FUNDING_MAX_RATE_HOURLY_PPB: u64 = 5000000;
pub const FUNDING_IMPACT_NOTIONAL: u64 = 1000000;
pub const FUNDING_HOURS_PER_8H: u64 = 8;
pub const MARK_EMA_SLOTS: u64 = 375;
/// One book sample moves the mid EMA by at most this many slots' worth (gap / MARK_EMA_SLOTS). The mid credited is the
/// smaller (same-signed) of the previous sample's and this one's: a skew counts only for slots it was present at both ends
/// of, never for a gap the caller chose to wait. Pushers sample every ~2 s (5 slots), so an honest book runs at full speed.
pub const MARK_SAMPLE_MAX_SLOTS: u64 = 5;
pub const MARK_CLAMP_BPS: u64 = 500;
pub const ORACLE_MAX_DIVERGENCE_BPS: u64 = 100;
pub const ORACLE_RESUME_BPS: u64 = 50;
pub const ORACLE_SINGLE_SOURCE_MAX_MOVE_BPS: u64 = 500;
pub const POS_FLAG_ISOLATED: u64 = 1;
pub const POS_FLAG_ADL_ELIGIBLE: u64 = 2;
/// Maker position slot kept while the maker has margin locked on a FastBook of this market, so a settle always has a slot.
pub const POS_FLAG_MAKER_PIN: u64 = 4;
pub const ERR_BAD_ACCOUNT: u64 = 1;
pub const ERR_BAD_DATA: u64 = 2;
pub const ERR_MARGIN: u64 = 3;
pub const ERR_STALE: u64 = 4;
pub const ERR_NOT_LIQUIDATABLE: u64 = 5;
pub const ERR_SHARD_FULL: u64 = 6;
pub const ERR_NO_SHARD: u64 = 7;
pub const ERR_ORACLE: u64 = 8;
pub const ERR_EXPIRED: u64 = 9;
pub const ERR_NOT_BANKRUPT: u64 = 10;
pub const ERR_MATH: u64 = 11;
pub const ERR_CAP: u64 = 12;
pub const ERR_LEDGER: u64 = 13;
pub const ERR_FOK_UNFILLED: u64 = 14;
pub const ERR_PAUSED: u64 = 15;
pub const ERR_NOT_TRIGGERED: u64 = 16;
pub const ERR_VAULT: u64 = 17;
pub const ERR_TOKEN_PROGRAM: u64 = 18;
pub const ERR_MISSING_SHARDS: u64 = 19;
pub const ERR_RANK: u64 = 20;
pub const ERR_TIMELOCK: u64 = 21;
pub const ERR_ISOLATED: u64 = 22;
pub const ERR_USE_ADL: u64 = 23;
pub const ERR_BREAKER: u64 = 24;
pub const ERR_NO_FILL: u64 = 25;
pub const BATCH_HEADER_TAG_OFF: u64 = 0;
pub const BATCH_HEADER_MARKET_OFF: u64 = 1;
pub const BATCH_HEADER_N_CANCEL_OFF: u64 = 3;
pub const BATCH_HEADER_N_PLACE_OFF: u64 = 4;
pub const BATCH_HEADER_SIZE: u64 = 5;
pub const CANCEL_ITEM_ORDER_ID_OFF: u64 = 0;
pub const CANCEL_ITEM_SIDE_OFF: u64 = 8;
pub const CANCEL_ITEM_PRICE_OFF: u64 = 9;
pub const CANCEL_ITEM_SIZE: u64 = 17;
pub const PLACE_ITEM_SIDE_OFF: u64 = 0;
pub const PLACE_ITEM_FLAGS_OFF: u64 = 1;
pub const PLACE_ITEM_KIND_OFF: u64 = 2;
pub const PLACE_ITEM_PRICE_OFF: u64 = 3;
pub const PLACE_ITEM_QTY_OFF: u64 = 11;
pub const PLACE_ITEM_EXPIRY_SLOT_OFF: u64 = 19;
pub const PLACE_ITEM_ORACLE_BAND_BPS_OFF: u64 = 27;
pub const PLACE_ITEM_CID_OFF: u64 = 29;
pub const PLACE_ITEM_A1_OFF: u64 = 37;
pub const PLACE_ITEM_A2_OFF: u64 = 45;
pub const PLACE_ITEM_A3_OFF: u64 = 53;
pub const PLACE_ITEM_SIZE: u64 = 61;
pub const ORDER_ID_OFF: u64 = 0;
pub const ORDER_OWNER_OFF: u64 = 8;
pub const ORDER_PRICE_OFF: u64 = 40;
pub const ORDER_QTY_OFF: u64 = 48;
pub const ORDER_EXPIRY_SLOT_OFF: u64 = 56;
pub const ORDER_BAND_BPS_OFF: u64 = 64;
pub const ORDER_CID_OFF: u64 = 72;
pub const ORDER_META_OFF: u64 = 80;
pub const ORDER_A1_OFF: u64 = 88;
pub const ORDER_A2_OFF: u64 = 96;
pub const ORDER_A3_OFF: u64 = 104;
pub const ORDER_LVL_LEFT_OFF: u64 = 112;
pub const ORDER_SIZE: u64 = 120;
pub const LEDGER_OWNER_OFF: u64 = 0;
pub const LEDGER_BASE_OFF: u64 = 32;
pub const LEDGER_QUOTE_OFF: u64 = 40;
pub const LEDGER_OPEN_REDUCE_OFF: u64 = 48;
pub const LEDGER_MAKER_VOL_OFF: u64 = 56;
pub const LEDGER_N_CLOSED_OFF: u64 = 64;
pub const LEDGER_SIZE: u64 = 72;
pub const POS_MARKET_OFF: u64 = 0;
pub const POS_BASE_OFF: u64 = 32;
pub const POS_FUNDING_SNAP_OFF: u64 = 40;
pub const POS_BID_OPEN_OFF: u64 = 48;
pub const POS_ASK_OPEN_OFF: u64 = 56;
pub const POS_N_ORDERS_OFF: u64 = 64;
pub const POS_COST_OFF: u64 = 72;
pub const POS_ISO_MARGIN_OFF: u64 = 80;
pub const POS_FLAGS_OFF: u64 = 88;
pub const POS_SIZE: u64 = 96;
pub const SHARD_DISC_OFF: u64 = 0;
pub const SHARD_MARKET_OFF: u64 = 8;
pub const SHARD_SIDE_OFF: u64 = 40;
pub const SHARD_LO_OFF: u64 = 48;
pub const SHARD_WIDTH_OFF: u64 = 56;
pub const SHARD_SEQ_OFF: u64 = 64;
pub const SHARD_N_OFF: u64 = 72;
pub const SHARD_NLED_OFF: u64 = 80;
pub const SHARD_OI_LONG_DELTA_OFF: u64 = 88;
pub const SHARD_FEE_ACCUM_OFF: u64 = 96;
pub const SHARD_SHARD_NO_OFF: u64 = 104;
pub const SHARD_ORDERS_OFF: u64 = 112;
pub const SHARD_LEDGER_OFF: u64 = 7792;
pub const SHARD_SIZE: u64 = 10096;
pub const USER_DISC_OFF: u64 = 0;
pub const USER_AUTHORITY_OFF: u64 = 8;
pub const USER_CASH_OFF: u64 = 40;
pub const USER_TIER_OFF: u64 = 48;
pub const USER_MAKER_VOLUME_OFF: u64 = 56;
pub const USER_HOME_VAULT_OFF: u64 = 64;
pub const USER_POS_OFF: u64 = 96;
pub const USER_TRIGGERS_OFF: u64 = 1632;
pub const USER_SIZE: u64 = 2048;
pub const MARKET_DISC_OFF: u64 = 0;
pub const MARKET_ADMIN_OFF: u64 = 8;
pub const MARKET_MINT_OFF: u64 = 40;
pub const MARKET_VAULT_OFF: u64 = 72;
pub const MARKET_INSURANCE_OFF: u64 = 104;
pub const MARKET_ORACLE_OFF: u64 = 136;
pub const MARKET_FEED_ID_OFF: u64 = 168;
pub const MARKET_VAULT_BUMP_OFF: u64 = 200;
pub const MARKET_BUCKET_WIDTH_OFF: u64 = 208;
pub const MARKET_IMR_BPS_OFF: u64 = 216;
pub const MARKET_MMR_BPS_OFF: u64 = 224;
pub const MARKET_MAX_STALENESS_SLOTS_OFF: u64 = 232;
pub const MARKET_MAX_CONF_BPS_OFF: u64 = 240;
pub const MARKET_MAX_AGE_SECS_OFF: u64 = 248;
pub const MARKET_EXPO_OFF: u64 = 256;
pub const MARKET_PX_MUL_OFF: u64 = 264;
pub const MARKET_PX_DIV_OFF: u64 = 272;
pub const MARKET_ORACLE_PX_OFF: u64 = 280;
pub const MARKET_ORACLE_CONF_OFF: u64 = 288;
pub const MARKET_ORACLE_SLOT_OFF: u64 = 296;
pub const MARKET_FUNDING_INDEX_OFF: u64 = 304;
pub const MARKET_FUNDING_RATE_OFF: u64 = 312;
pub const MARKET_FUNDING_SLOT_OFF: u64 = 320;
pub const MARKET_OI_LONG_OFF: u64 = 328;
pub const MARKET_MAX_OI_OFF: u64 = 336;
pub const MARKET_FEES_OFF: u64 = 344;
pub const MARKET_PAUSED_OFF: u64 = 352;
pub const MARKET_TVL_CAP_OFF: u64 = 360;
pub const MARKET_EVENT_SEQ_OFF: u64 = 368;
pub const MARKET_ORACLE_TS_OFF: u64 = 376;
pub const MARKET_ORACLE2_OFF: u64 = 384;
pub const MARKET_FEED_ID2_OFF: u64 = 416;
pub const MARKET_ORACLE2_PX_OFF: u64 = 448;
pub const MARKET_N_SHARDS_OFF: u64 = 456;
pub const MARKET_ADL_RANK_OFF: u64 = 464;
pub const MARKET_BAD_DEBT_OFF: u64 = 496;
pub const MARKET_FUNDING_REM_OFF: u64 = 504;
pub const MARKET_PREM_ACC_OFF: u64 = 512;
pub const MARKET_PREM_SLOTS_OFF: u64 = 520;
pub const MARKET_PREM_LAST_OFF: u64 = 528;
pub const MARKET_PERIOD_START_OFF: u64 = 536;
pub const MARKET_FUNDING_PERIOD_OFF: u64 = 544;
pub const MARKET_IMPACT_NOTIONAL_OFF: u64 = 552;
pub const MARKET_MARK_PX_OFF: u64 = 560;
pub const MARKET_MARK_EMA_OFF: u64 = 568;
pub const MARKET_POOL_OFF: u64 = 576;
pub const MARKET_VAULT_SEED_OFF: u64 = 608;
pub const MARKET_MAX_POSITION_OFF: u64 = 640;
pub const MARKET_PENDING_OP_OFF: u64 = 648;
pub const MARKET_PENDING_VAL_OFF: u64 = 656;
pub const MARKET_PENDING_ETA_OFF: u64 = 664;
pub const MARKET_PENDING_KEY_OFF: u64 = 672;
pub const MARKET_SIZE: u64 = 896;
pub const TRIGGER_MARKET_OFF: u64 = 0;
pub const TRIGGER_KIND_OFF: u64 = 32;
pub const TRIGGER_SIDE_OFF: u64 = 40;
pub const TRIGGER_TRIGGER_PX_OFF: u64 = 48;
pub const TRIGGER_LIMIT_PX_OFF: u64 = 56;
pub const TRIGGER_QTY_OFF: u64 = 64;
pub const TRIGGER_TIP_OFF: u64 = 72;
pub const TRIGGER_INTERVAL_OFF: u64 = 80;
pub const TRIGGER_NEXT_SLOT_OFF: u64 = 88;
pub const TRIGGER_SLICES_LEFT_OFF: u64 = 96;
pub const TRIGGER_SIZE: u64 = 104;
pub const INSURANCE_DISC_OFF: u64 = 0;
pub const INSURANCE_MARKET_OFF: u64 = 8;
pub const INSURANCE_BALANCE_OFF: u64 = 40;
pub const INSURANCE_OI_LONG_DELTA_OFF: u64 = 48;
pub const INSURANCE_SIZE: u64 = 56;
pub const RANK_ENTRY_USER_OFF: u64 = 0;
pub const RANK_ENTRY_SCORE_OFF: u64 = 32;
pub const RANK_ENTRY_SIZE: u64 = 40;
pub const RANK_DISC_OFF: u64 = 0;
pub const RANK_MARKET_OFF: u64 = 8;
pub const RANK_N_LONG_OFF: u64 = 40;
pub const RANK_N_SHORT_OFF: u64 = 48;
pub const RANK_ENTRIES_OFF: u64 = 56;
pub const RANK_SIZE: u64 = 696;
pub const POOL_DISC_OFF: u64 = 0;
pub const POOL_ADMIN_OFF: u64 = 8;
pub const POOL_MINT_OFF: u64 = 40;
pub const POOL_VAULT_OFF: u64 = 72;
pub const POOL_VAULT_BUMP_OFF: u64 = 104;
pub const POOL_N_MARKETS_OFF: u64 = 112;
pub const POOL_PENDING_MARKET_OFF: u64 = 120;
pub const POOL_PENDING_ETA_OFF: u64 = 152;
pub const POOL_MAX_MARKET_OI_OFF: u64 = 160;
pub const POOL_SIZE: u64 = 168;
pub const MULTISIG_DISC_OFF: u64 = 0;
pub const MULTISIG_M_OFF: u64 = 8;
pub const MULTISIG_N_OFF: u64 = 16;
pub const MULTISIG_KEYS_OFF: u64 = 24;
pub const MULTISIG_SIZE: u64 = 280;
pub const DISC_MARKET: u64 = 5784119801274913793;
pub const DISC_SHARD: u64 = 5784119801375379969;
pub const DISC_USER: u64 = 5784119801409655297;
pub const DISC_INSURANCE: u64 = 5784119801208001281;
pub const DISC_RANK: u64 = 5784119801358142977;
pub const DISC_POOL: u64 = 5784119801325506305;
pub const DISC_MULTISIG: u64 = 5784119801275434753;
pub const TIERS: [(u64, u64); 2] = [(5, 0), (5, 2)];
pub const IX_SLAB_INIT: u64 = 60;
pub const IX_FAST_INIT: u64 = 61;
pub const IX_FAST_JOIN: u64 = 62;
pub const IX_SLAB_QUOTE: u64 = 63;
pub const IX_SLAB_FUND: u64 = 64;
pub const IX_SLAB_SETTLE: u64 = 65;
pub const IX_TAKE_FAST: u64 = 66;
pub const IX_SLAB_KILL: u64 = 67;
pub const IX_LIQ_FAST: u64 = 70;
pub const IX_ASSERT: u64 = 71;
pub const IX_TAKE_FAST_BATCH: u64 = 72;
pub const QUOTE_MAX_AGE_SLOTS: u32 = 30;
pub const QUOTE_BAND_BPS: u64 = 500;
pub const QUOTE_ITEM_SIZE: usize = 10;
/// Grow a slab / fast book made before the ladder settings to the current size.
pub const IX_SLAB_GROW: u64 = 73;
pub const IX_FAST_SIGNER: u64 = 74;
pub const IX_FAST_PX: u64 = 75;
/// Admin registers an existing FastBook as its market's book for its side(s). accs: admin(s), market(w), fast(r)
pub const IX_FAST_REGISTER: u64 = 76;
/// Permissionless, for an account below maintenance in a market: claim its fills and cancel its resting orders in the passed
/// shards of that market (as a liquidation would). Liquidations / ADL require no resting orders on the judged positions.
pub const IX_LIQ_CANCEL: u64 = 77;
/// Post-and-push: update_oracle (same accounts as IX 4) that never fails the transaction. A publisher appends it to its own
/// price post so the market applies the price the moment the post lands; when the oracle is unusable at that moment
/// (quorum, stale, confidence, missing member) the market is left exactly as it was and the price post still lands.
pub const IX_UPDATE_ORACLE_SOFT: u64 = 78;
/// Signed price: a 64-byte message signed by the book's price signer and checked by the Ed25519 program in the instruction
/// right before the take: FAST_PX_MAGIC, market key, px u64 (price units), slot u64, ts_ms u64. The newest one (by ts_ms)
/// a book has seen stands in for the oracle in pegged ladders and tolerance checks while it is at most FAST_PX_MAX_SLOTS old.
pub const FAST_PX_MAGIC: [u8; 8] = *b"etfastpx";
pub const FAST_PX_MSG: usize = 64;
pub const FAST_PX_MAX_SLOTS: u64 = 8;
/// Default maker guard on the fast path (no opt-in). A fixed-price ladder without its own tolerance does not fill at a level
/// more than FAST_GUARD_BPS through the book's price (a bid above it, an ask below it): after a move such a quote is stale,
/// not an offer, and it must not depend on the maker's cancel landing first. Pegged ladders move with the price and ladders
/// with a tolerance keep the maker's own limit.
pub const FAST_GUARD_BPS: u64 = 10;
/// Maker quotes fill only while the price they are judged against is recent: a signed price at most FAST_PX_MAX_SLOTS old, or
/// an oracle update at most this many slots old (half the usual staleness limit). Resting orders are not affected.
pub const FAST_GUARD_MAX_AGE_SLOTS: u64 = 75;
/// A signed price further than this from the oracle is ignored.
pub const FAST_PX_MAX_DEV_BPS: u64 = 100;
/// Oracle band (bps of the oracle price) added to the initial and maintenance requirements: the oracle's confidence —
/// for an OracleSet the publishers' dispersion — so margin tightens by itself while sources disagree or a venue is thin.
/// Equity itself stays at the mark (bankruptcy, ADL and NAV are unchanged); only the requirements widen. Capped here.
pub const BAND_MAX_BPS: u64 = 50;
/// A take may hand over a signed price at most this many slots older than now (no picking the best of the last few).
pub const FAST_PX_TAKE_SLOTS: u64 = 2;
/// Quote levels one liquidate_fast call may sweep. Bounds a call to ~100k CU whatever the book looks like; the keeper calls again
/// (the chunk rule still caps each call's size), like Hyperliquid's repeated partial liquidations.
pub const LIQ_FAST_MAX_LEVELS: u32 = 24;
/// Ladder flags bits 8-15: the ladder fills only while the book's signed price is at most that many slots old (0 = no need;
/// at most FAST_PX_MAX_SLOTS counts).
pub const LADDER_FRESH_SHIFT: u16 = 8;
/// In a pegged ladder (LADDER_PEG) a level's price is oracle + px - QUOTE_PEG_BIAS (price units).
pub const QUOTE_PEG_BIAS: u32 = 1 << 31;
pub const LADDER_PEG: u16 = 1;
/// Next-price execution. A ladder with LADDER_NEXT never fills an immediate take: only an order that was committed first and is
/// filled at the price the book's signer signed for the slot NEXT_DELAY_SLOTS after the commitment (one price a market a slot,
/// message NEXT_PX_MAGIC, market key, px u64, slot u64, ts_ms u64 -- the layout of the fast price message). The taker decides
/// on what it knows at the commitment and cannot take it back; nobody chooses the price. Liquidations still fill these ladders
/// at the book's current price.
pub const LADDER_NEXT: u16 = 2;
/// A market order committed now and filled at the next price (set_trigger; executed by anyone through exec_trigger).
pub const TRIGGER_KIND_NEXT: u64 = 4;
/// Kind flag: a stop, take-profit or TWAP slice a keeper has committed for next-price execution (exec_trigger with arm = 1).
pub const TRIGGER_KIND_FLAG_ARMED: u64 = 1024;
pub const NEXT_DELAY_SLOTS: u64 = 2;
/// Slots after the pricing slot during which the commitment can still be filled; after that it lapses (a market order is
/// dropped, a stop or TWAP slice can be committed again).
pub const NEXT_EXPIRY_SLOTS: u64 = 40;
pub const NEXT_PX_MAGIC: [u8; 8] = *b"etnextpx";
/// How long a quote on a next-price ladder stays usable without being re-sent. Such a quote is an offset from a price that is
/// signed afresh for every fill, so its age says nothing about its price; the limit only bounds how long a maker that has
/// stopped (and can no longer pull its quotes) keeps trading. An ordinary quote keeps QUOTE_MAX_AGE_SLOTS.
pub const NEXT_QUOTE_MAX_AGE_SLOTS: u32 = 150;
/// slab_quote items with idx = QUOTE_ITEM_REF | ladder set (oracle price quoted against = px, tolerance bps = qty);
/// idx = QUOTE_ITEM_INV | ladder set the inventory cap in lots (px; 0 = none) and the ladder flags (qty: LADDER_PEG).
/// ladder = market idx << 1 | side. Settings items go before the quote items.
pub const QUOTE_ITEM_REF: u16 = 0x8000;
pub const QUOTE_ITEM_INV: u16 = 0xC000;
pub const IX_SET_SESSION: u64 = 68;
pub const IX_MARKET_CFG: u64 = 69;
pub const IX_VAULT_INIT: u64 = 50;
pub const IX_VAULT_ADD_SUB: u64 = 51;
pub const IX_VAULT_LINK: u64 = 52;
pub const IX_VAULT_DEPOSIT: u64 = 53;
pub const IX_VAULT_WITHDRAW: u64 = 54;
pub const IX_VAULT_TAKEOVER: u64 = 55;
pub const IX_VAULT_CRANK: u64 = 56;
pub const IX_VAULT_SET: u64 = 57;
/// The vault quotes for itself (its depositors are the house). A vault slab's quoting authority is the vault account, which
/// can sign nothing, so only these instructions write it.
/// Vault admin makes a slab for one of the vault's sub-accounts. accs: admin(s), vault(w), sub(r), slab(w, zeroed)
pub const IX_VAULT_SLAB_INIT: u64 = 58;
/// Market admin and vault admin approve the vault's slab on a book. accs: admin(s), market(r), fast(w), slab(w), vault(r), [signers / multisigs]
pub const IX_VAULT_JOIN: u64 = 59;
/// The vault's operator writes quotes: same items as IX_SLAB_QUOTE, checked against the vault's quote rules.
/// accs: operator(s), vault(r), slab(w)
pub const IX_VAULT_QUOTE: u64 = 79;
/// Lock or release a sub-account's quoting margin (IX_SLAB_FUND for a vault). The vault admin may do either; the operator may
/// only release or lower the target. accs: signer(s), sub(w), market(r), fast(w), vault(w), [other markets]. data: tag, amount i64, [target i64]
pub const IX_VAULT_FUND: u64 = 80;
/// Stop (1) or resume (0) all quotes of a vault slab: admin or operator. Anyone may stop them while the vault is halted.
/// accs: signer, vault(r), slab(w). data: tag, on u8
pub const IX_VAULT_KILL: u64 = 81;
/// Vault admin names the operator and the quote rules. accs: admin(s), vault(w). data: tag, operator [32], min_edge_bps, max_tol_bps, fresh
pub const IX_VAULT_OPERATOR: u64 = 82;
/// Move idle cash between two sub-accounts of one vault (admin or operator): no value leaves the vault and both stay counted
/// in its value. accs: signer(s), vault(w), from sub(w), to sub(w), [from's markets, and the books it quotes on]. data: tag, amount u64
pub const IX_VAULT_MOVE: u64 = 83;
/// Permissionless: value one sub-account of a vault and remember it (its equity, the margin it has locked in the books,
/// funding settled; refused while fills are waiting to be settled into it). An account with nothing in it drops out of the
/// vault's value until it is used again. accs: vault(w), sub(w), [the sub's markets, the books it quotes on, the shards of
/// markets where it rests orders]. data: tag
pub const IX_VAULT_VALUE: u64 = 84;
/// A signing lease: which one of a key's hosts may sign for a slot. The next-price signer runs on more than one machine,
/// all holding the same key, and "one price a market a slot" must survive a machine that is slow, cut off or restarted.
/// The lease gives every slot to at most one member: a member signs for slot s only while the chain says it holds the
/// lease over s. The key itself puts its signing under the lease (once) and names up to LEASE_MEMBERS host identities.
/// The lease account is the key's `create_with_seed(key, LEASE_SEED, program)`, which only the key can create, so every
/// host derives the same one. accs: key(s), lease(w, zeroed). data: tag, n u8, n x member [32]
pub const IX_LEASE_INIT: u64 = 85;
/// A member takes the lease, or the holder extends it, up to slot `until` (inclusive). The holder can only extend; anyone
/// else can take it only once the chain is past the holder's last slot, and then holds from the current slot on. So the
/// slots two members were ever given never overlap, whatever their clocks or connections do.
/// accs: member(s), lease(w). data: tag, until u64
pub const IX_LEASE_CLAIM: u64 = 86;
pub const LEASE_SEED: &str = "lease";
pub const LEASE_MEMBERS: u64 = 4;
/// Furthest ahead a lease can reach (2 minutes): the longest a dead holder can keep the others out.
pub const LEASE_MAX_SLOTS: u64 = 300;
pub const LEASE_SIZE: u64 = 208;
pub const LEASE_KEY_OFF: u64 = 8;
pub const LEASE_MEMBERS_OFF: u64 = 40;
pub const LEASE_N_OFF: u64 = 168;
pub const LEASE_HOLDER_OFF: u64 = 176;
pub const LEASE_SINCE_OFF: u64 = 184;
pub const LEASE_EXPIRY_OFF: u64 = 192;
pub const LEASE_TERM_OFF: u64 = 200;
/// How old a remembered sub-account value may be when a deposit, withdrawal, takeover or crank uses it in place of the
/// account (30 s). It bounds what a depositor or withdrawer can gain by picking a remembered value over the live one:
/// the market's move on that sub-account's inventory over this window, on their own share.
pub const VAULT_VALUE_MAX_AGE_SLOTS: u64 = 75;
/// Highest edge the admin can demand of the vault's quotes.
pub const VAULT_MAX_EDGE_BPS: u64 = 1000;
/// Widest tolerance (reference price vs the book's price) a vault's quote rules can allow.
pub const VAULT_MAX_TOL_BPS: u64 = 2000;
pub const ERR_LOCKED: u64 = 26;
pub const ERR_HALTED: u64 = 27;
pub const VAULT_VIRTUAL: u64 = 1000000;
pub const SLOTS_PER_DAY: u64 = 216000;
pub const EXT_MAX_AGE_SECS: i64 = 5;
