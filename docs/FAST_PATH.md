# Fast quote path

Makers quote into their own `Slab`. Takers fill against slabs through one per-market `FastBook`. Quotes and fills never take the same write lock. Collateral, fees, funding and liquidation are all the real engine's.

## Accounts
| Account | Size | Written by | Holds |
|---|---|---|---|
| Slab | 32864 | the maker only | 2048 quotes: index = market idx << 4, side << 3, level (0 bid, 1 ask; 8 levels) |
| FastBook | 2000 | takers, settle, liquidation | per maker: locked margin, budget, unsettled fills, volume, auto target. Per-level used qty |
| User | 2048 | owner, keepers | 16 positions, packed at the front; triggers at 1632 |

## Instructions
| Tag | Name | Notes |
|---|---|---|
| 60 | slab_init | authority, user, slab |
| 61 | fast_init | admin; one FastBook per market (idx = market's slot in every slab) |
| 62 | fast_join | admin approves a maker for a market (max 8) |
| 63 | slab_quote | up to 128 items per tx (95 items 3,018 CU). Each touched ladder must be best-first (bids falling, asks rising; equal prices allowed) or the tx fails. No margin read. Quotes expire after 30 slots and only fill within ±5% of mark |
| 64 | slab_fund | lock or release quoting margin; optional `target` > 0 turns on auto mode |
| 65 | slab_settle | permissionless; moves fills into the maker's User, refills budget. Auto mode tops up or hands margin back. Below maintenance returns all margin |
| 66 | take_fast | IOC; best price across slabs and, if shards are passed, resting orders. 2,235 CU for 1 fill on 1 maker, 3,882 on 8 makers, 6,264 for 5 fills on 8 makers |
| 67 | slab_kill | maker's stop switch |
| 73 | slab_grow | grows a slab / fast book made by an earlier version to the current size (new settings zeroed = off); pay the extra rent into the account first |
| 72 | take_fast_batch | many takers in one tx, signed once by a relayer holding their session delegates; bad orders are skipped. About 2,056 CU per taker at 10 |
| 70 | liquidate_fast | 25% chunk against maker quotes; same price window as a take; fee to insurance; empty fast book with every maker passed and no shards opens the ADL window |

A maker can fill at most margin / IMR of notional between settles. That bounds what an unsettled maker can lose.

## Keeper duties (keeper/crank.py)
- Settle every maker with unsettled fills every tick. Auto-mode makers drifting from target get a settle at most every 30 s.
- Health is judged with unsettled fills included. A liquidation tx runs `slab_settle` for the target first, then the liquidation, atomically.
- Fast-only markets go to `liquidate_fast`. A book NoFill falls back to `liquidate_fast`, then vault takeover, then ADL.
- Priority fee is the p75 of recent fees on the tx's writable accounts, capped by `CRANK_CU_PRICE_MAX`.

## Upgrade notes
- User layout changed (4 to 16 positions, triggers moved to offset 1632). Accounts made by the live devnet program must be migrated or recreated before this version goes live.
- FastBook is 2000 bytes (auto target added). Nothing on devnet uses it yet.

## Frontend (Codex owns UI)
Use `sdk/ts/everytick.ts` (builders, decoders, `fastDepth`). `sdk/ts/smoke.ts` shows reading a FastBook, slabs, a User and a Market from RPC.
- Order book: `fastDepth(decodeFastBook(fast), slabs, slot, mark)` gives live maker levels (used qty, age, ±5% band and budget applied). Merge with shard orders.
- Trading: `take_fast` with all approved slabs for the market. Show fills from the program's fill events.
- Positions: decode_user plus, for makers, the FastBook entry (unsettled base and quote) so equity matches what liquidation sees.
- Maker panel: margin, budget, auto target, kill switch.

## Liquidation (2026-11-04)
- Partial: one liquidation closes only what brings equity back to 1.25x maintenance (worst-band fee assumed), capped at the
  25% chunk. Example from the tests: a 9x long just under maintenance closed 10,674 of 90,000 lots, not 22,500.
- Keeper reward: append your own User account (same vault) to liquidate_book / liquidate_fast and receive 20% of the
  liquidation fee (LIQ_KEEPER_SHARE_BPS); insurance gets the rest. No keeper account: all to insurance.
- Crank is event-driven: programSubscribe streams (markets, users, fast books) keep a cache and wake it on any change; a full
  read runs every CRANK_REFRESH_S (60 s) and on every tick when the stream is down. It warms up its keeper reward account at
  start, liquidates the position holding the most maintenance first, and when the band has no liquidity sends the
  liquidation and the ADL in one transaction. Local e2e (9% crash on a 9x long): 4.6 s -> 1.4 s from crash to closed.
- Trigger orders (TP / SL / TWAP, IX 21) fill on the fast path too: pass the FastBook for the side the trigger trades and its
  slabs after the market account. Matching is the same as take_fast (best of maker quotes and resting orders, mark +-5%,
  no self-trade, maker budgets). The crank does this automatically.
- Liquidation price for the UI: `liquidation_price()` (Python) / `liquidationPrice()` (TS) solve equity = maintenance for one
  position with the others held at their marks. Checked against the program: 2 ticks above it liquidation is refused, 2 below it
  goes through.

## Matching cost (2026-11-04)
The hot FastBook account takes at most 12M CU per block (mainnet per-account limit; blocks are 100M CU), so CU per take sets
how many takes one book side can clear per block. A take tx also pays about 1,400 CU outside the program (signature, write locks).
- The best-quote scan checks price before anything else: a level that cannot beat the best so far costs one compare, so a
  ladder quoted best-first is decided at its first usable level. Same winner as before (price, older quote, earlier slab).
- A take that needs a second fill on a book with several makers keeps each slab's own best level and rescans only the slab it
  just hit (a fill changes only that maker's budget and used table). One-fill takes never enter that path.
- Slabs passed in FastBook order skip the maker lookup.

| take (LiteSVM) | before | after |
|---|---|---|
| 1 maker, 1 fill | 2,721 | 2,485 |
| 8 makers, 1 fill | 6,762 | 4,379 |
| 8 makers, 5 fills | 23,929 | 10,558 |
| 8 makers, 5 fills, ladders worst-first | 24,174 | 14,577 |

The reference model test (300 seeds, 20,079 takes, 3 makers, unsorted ladders with ties) matches fill for fill.

## Hyperliquid-grade pass (2026-11-05, branch hl-perf, on devnet 7AY437)
Rules a client must follow:
- **Ladders are best-first.** slab_quote rejects a ladder whose bids rise or asks fall from level 0 down (ties are fine). The
  matcher stops at the first usable level of each slab because of this. The MM bots already quote this way.
- **Priority:** better price, then the older quote (lower slot), then the earlier slab, then the lower level. Within one price
  a refill starts again at that price group's first usable level, so a newer equal-price level never jumps an older one.
- **Fill events are 16 bytes per fill.** take_fast logs EVENTS_FAST_FILL (6): `[6, taker_side, taker 32, n, n x (px u32, qty u32,
  id u64)]`, id = maker index << 32 | quote gen. take_fast_batch logs one EVENTS_FAST_BATCH (7): `[7, 0, fast book 32, n, entries]`,
  id = order slot << 40 | sell << 39 | maker index << 32 | gen.
- **take_fast_batch (72):** data `tag, nm u8, ns u8, n u8, n x (side u8, flags u8, limit u64, qty u64)`; accounts market,
  other markets x nm, fast(w), slabs x ns, n x [user(w), signer, delegate record or any account when the signer is the owner].
  Margin is checked up front for the order's worst case (stricter than take_fast). No resting-book orders, no strict_take markets.
- **Several FastBooks per side:** a market can have N books per side; takes on different books run in parallel (N x the
  per-side ceiling of 12M CU per block). A maker on N books offers N x its size. No program change was needed.

Where the CU went: the taker's position is booked once per take (exact at a zero crossing), the event buffer is not zeroed,
and multiplies use native 64-bit ops where the bounds are proven (a checked u64/i64/i128 multiply is a software routine on SBF).

| take (LiteSVM) | 2026-11-04 | now |
|---|---|---|
| 1 maker, 1 fill | 2,485 | 2,235 |
| 1 fill, 2 slabs | 2,769 | 2,490 |
| 5 fills, 2 slabs | 7,192 | 4,570 |
| 8 makers, 1 fill | 4,379 | 3,882 |
| 8 makers, 5 fills | 10,558 | 6,264 |

Checks: reference model fill for fill; 300 random batches equal to the same takes one by one; 2M random arithmetic cases;
Kani proofs for the split multiply, fee rounding, booking once and fill conservation (`cargo kani --harness <name>` in program/).
`--features cuprof` builds a version that logs CU at checkpoints in take_fast.

Latency: `sidecar` tpu_relay (127.0.0.1:8897) sends transactions straight to the upcoming leaders over QUIC and forwards
reads to the RPC proxy. Oracle send to land went 2.5 s -> 0.7 s on devnet. lat_probe2 measures requotes and takes on one websocket.

## Maker protection (2026-11-05, on devnet 7AY437)
For market makers: quotes that follow the oracle without a requote, a self-pull when the oracle runs away, a hard inventory cap,
and a rebate. All settings are per ladder (market, side), written by the maker in slab_quote and read by takes; nothing here
touches a shared account on the quote path.
- **Settings items** go first in a slab_quote, before the quote items. `idx = 0xC000 | ladder` sets the inventory cap (px, lots,
  0 = none) and flags (qty: 1 = pegged). `idx = 0x8000 | ladder` sets the price quoted against (px) and a tolerance in bps (qty,
  0 = off). ladder = market idx << 1 | side (0 bid, 1 ask). SDK: `ladder_items()` / `ladderItems()`.
- **Pegged ladder:** each level's px is an offset, price = oracle + px - 2^31. When the oracle updates, every pegged quote moves
  with it; the maker sends nothing. Ladders stay best-first on the raw offsets.
- **Tolerance:** the ladder stops filling once |oracle - ref| > tol_bps of ref. Fixed-price makers send ref with each requote.
- **Inventory cap:** the ladder stops at inv_max lots of inventory on its side (bids: long, asks: short). Inventory = position at
  the maker's last settle (copied into the book as `ext[i].inv0`) + unsettled fast fills. Trades elsewhere count from the next settle.
- **Rebate:** the maker's tier rebate (TIERS[tier].1 bps, copied at each settle / fund into `ext[i].rebate_bps`) is paid per fill
  out of the book's fees, never above the taker fee. Liquidation fills pay no rebate.
- **Self-settle:** slab_settle is permissionless; fast_mm `settle_s N` settles its own fills every N s, so it never waits on a keeper.
- fast_mm config: `peg 1`, `tol_bps N`, `inv_cap N`, `settle_s N`.

Layout: Slab 32,864 -> 36,960 (Ladder[256] appended), FastBook 2,008 -> 2,136 (MakerExt[8] appended). Older accounts keep working
after slab_grow (devnet: 50 books and 2 slabs grown, 0.076 SOL). The devnet books were 2,000 bytes (made before the split-side
field), so takes on 7AY437 failed with BadAccount until they were grown.

| CU (LiteSVM) | before | with protection |
|---|---|---|
| slab_quote, 95 items | 3,018 | 3,004 |
| take, 1 maker, 1 fill | 2,235 | 2,315 |
| take, 8 makers, 5 fills | 6,264 | 6,765 |
| batch of 10, per taker | 2,056 | 2,116 |

Devnet latency through our own RPC node (Tokyo, non-voting agave 4.3.2; 60 samples each, all landed):
requote processed p50 0.097 s / p95 0.46 s, confirmed p50 0.15 s / p95 0.87 s; take processed p50 0.073 s / p95 0.45 s,
confirmed p50 0.087 s / p95 0.85 s.

## Signed fresh prices (2026-11-05, on devnet 7AY437)
Pegged quotes used to follow the posted oracle, which lands 0.5-1 s after the market moves; a taker who sees the move first
hits quotes that are still on the old price. Now a take can carry a price signed off chain, and the book prices pegged quotes
off the newest signed price instead of the oracle.
- **Message:** 64 bytes `"etfastpx" | market key | px u64 (price units) | slot u64 | ts_ms u64`, signed by the book's
  `px_signer` (IX 74, admin). Verified by the Ed25519 program in the instruction right before the take; the program reads the
  key, signature and message only from that instruction's own data (instruction index u16::MAX), so nothing can point it elsewhere.
- **Book state:** FastBook keeps the newest price by ts_ms (`fast_px`, `fast_slot`, `fast_ts`). An older or foreign signature,
  another market's price, or one more than 200 bps from the oracle is ignored. The signer's slot is clamped to now.
- **Use:** while `fast_slot` is at most 8 slots old the signed price replaces the oracle for pegged ladders and tolerance checks.
  Anyone can push a newer price without trading (IX 75 fast_px_push).
- **Fresh-only ladders:** ladder flags bits 8-15 = N: the ladder fills only while the book's signed price is at most N slots old
  (N up to 8). A taker then has to bring a recent signature; the newest one any taker has used wins, so cherry-picking an
  older signature is bounded to the last N slots and to signatures newer than the book's.
- **Signer:** `aggregator/px_signer.py` signs the keeper's consensus (FAIR_OUT) every change / 200 ms and keeps the last 64 per
  book in a tmpfs file. SDK: `fast_px_message`, `ed25519_ix`, `take_fast(..., signed=True)`, `fast_signer`, `fast_px_push`,
  `ladder_items(..., fresh_slots=N)`; fast_mm config `fresh_slots N`.
- FastBook 2,136 -> 2,192 bytes (slab_grow; devnet 50 books grown for 0.014 SOL). Program 352,264 B (md5 f06722f31f4a).

| CU (LiteSVM) | before | now |
|---|---|---|
| take, 1 maker, 1 fill, no signed price | 2,315 | 2,355 |
| take, 8 makers, 5 fills | 6,765 | 6,840 |
| take with a signed price (Ed25519 parse + store) | - | +344 |

Slot units: mainnet slots are 200 ms since 2026-10-09 (SIMD-0525), devnet still about 400 ms. Every *_SLOTS constant is half
the wall time on mainnet (FAST_PX_MAX_SLOTS 8 = 1.6 s, QUOTE_MAX_AGE_SLOTS 30 = 6 s, TIMELOCK_SLOTS 216000 = 12 h,
SLOTS_PER_DAY and the default funding period 9000 slots = 30 min). Before mainnet the long durations must move to the clock's
unix time or be doubled (not done yet).

## Review fixes (hl-fix)

Each item below was a reproduced exploit on 6c48710 and now has a regression test (`program-tests/tests/fast.rs`, `reg_*`).

- **Budget only refills on settle.** `slab_fund` while fills are unsettled adds the new margin's budget to what is left instead of resetting it (an amount-0 fund used to refill the whole budget, so unsettled exposure grew without margin).
- **Each maker once per take.** `collect_slabs` (take_fast, take_fast_batch, triggers, liquidate_fast) refuses a slab that appears twice; a repeat used to fill the same quote again past its size, budget and inventory cap.
- **Reduce-only cannot create bad debt.** Refused when the position is already below maintenance (liquidation's job), and the worst case (limit inside the band, taker fee) must keep equity ≥ 0. Same rule in batches (the order is skipped).
- **ADL needs a provably empty book.** liquidate_fast needs every approved maker (no steering to the liquidator's own quotes), a book with makers (a spare empty book proves nothing), and, when the book takes signed prices, a current one (fresh-only ladders are not "empty" just because no price was attached). Triggers also need every maker.
- **Quote blocks are bound to their market.** `fast_join` writes `Slab.idx_market[idx] = market`; takes refuse a slab whose block belongs to another market. Slabs grow by 4,096 B (`SLAB_SIZE` 41,056). Migration: upgrade → `grow_all.py --go` → `rejoin_all.py` (a re-join of an approved slab only registers the block).
- **Signed prices are current.** A take accepts a signed price at most `FAST_PX_TAKE_SLOTS` (2) slots old, so a taker can't pick the best of the last few. `px_signer.py` signs nothing when the keeper's fair price is older than `FAIR_MAX_AGE_MS` (400 ms) and carries the slot forward at the measured slot time (200 ms ready).
- **Maker settles always have a position slot.** `slab_fund` pins the maker's position in the market (`POS_FLAG_MAKER_PIN`) while margin is locked or fills are unsettled; `gc_pos` keeps pinned slots.
- **OI cap counts the book's pending fills** (`oi_long + oi_long_delta`).
- **No self-trade by quoting key**: a taker signing with the slab's quoting key never fills its own quotes.
- **Price band without rounding to zero** for prices below 10,000 units.
- **One slab per maker account per book.**
- `fast_mm`: compute limit sized to each quote chunk (a flat 4,000 CU failed chunks with ladder settings).

Operational constraints (admin): one FastBook per side per market (the inventory cap is per book), at most 8 makers per book.
