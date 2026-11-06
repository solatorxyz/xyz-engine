| order-type instruction (LiteSVM, Agave 4.3 SBF runtime, perp_program.so) | CU |
|---|---|
| init_market | 2009 |
| init_rank (admin) | 193 |
| set_tier (admin: MM tier 1 = rebate) | 195 |
| discrete: place 10 orders (5 bid + 5 ask levels) | 9526 |
| discrete: replace by cid (cancel 10 + place 10) | 12285 |
| discrete: cancel 10 by cid | 3630 |
| spline: place 2 ladders (=10 levels) | 3193 |
| spline: replace by cid (cancel 2 + place 2 ladders) | 3645 |
| spline: cancel 2 ladders by cid | 1198 |
| taker IOC sweep 5 discrete levels (3 shards) | 7172 |
| taker IOC sweep 5 spline levels (1 shard) | 7535 |
| place oracle-peg ask (offset +15, min/max band) | 1938 |
| taker IOC vs peg (lazy reprice) | 3081 |
| ALO crossing (skipped, tx ok) | 1696 |
| ALO slide (re-priced to best ask - 1 tick) | 1874 |
| FOK unfilled in batch (skipped, rest of batch runs) | 3588 |
| FOK filled | 3327 |
| STP decrement (no fill, both reduced) | 2452 |
| STP cancel-newest (taker dropped) | 2315 |
| set_trigger SL (reduce-only) | 337 |
| exec SL trigger (keeper crank, IOC reduce-only) | 6989 |
| exec TWAP slice (1/3, sell 10) | 4785 |
| admin pause | 152 |
| paused: reduce-only allowed | 1063 |
| deposit under TVL cap (cap check + CPI) | 1388 |
| liquidate (cancel target resting orders + close) | 5823 |
| crank_claim (1 shard -> 1 maker) | 128 |
