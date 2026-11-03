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
| 63 | slab_quote | about 95 levels per tx, 1,849 CU. No margin read. Quotes expire after 30 slots and only fill within ±5% of mark |
| 64 | slab_fund | lock or release quoting margin; optional `target` > 0 turns on auto mode |
| 65 | slab_settle | permissionless; moves fills into the maker's User, refills budget. Auto mode tops up or hands margin back. Below maintenance returns all margin |
| 66 | take_fast | IOC; best price across slabs and, if shards are passed, resting orders. 2,710 CU for 1 fill |
| 67 | slab_kill | maker's stop switch |
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
