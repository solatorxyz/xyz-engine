| order-type instruction (LiteSVM, Agave 4.3 SBF runtime, perp_program.so) | CU |
|---|---|
| init_market | 1992 |
| init_rank (admin) | 174 |
| set_tier (admin: MM tier 1 = rebate) | 173 |
| discrete: place 10 orders (5 bid + 5 ask levels) | 9046 |
| discrete: replace by cid (cancel 10 + place 10) | 11707 |
| discrete: cancel 10 by cid | 3624 |
| spline: place 2 ladders (=10 levels) | 3066 |
| spline: replace by cid (cancel 2 + place 2 ladders) | 3456 |
| spline: cancel 2 ladders by cid | 1223 |
| taker IOC sweep 5 discrete levels (3 shards) | 6739 |
| taker IOC sweep 5 spline levels (1 shard) | 6862 |
| place oracle-peg ask (offset +15, min/max band) | 1771 |
| taker IOC vs peg (lazy reprice) | 2864 |
| ALO crossing (skipped, tx ok) | 1604 |
| ALO slide (re-priced to best ask - 1 tick) | 1727 |
| FOK unfilled in batch (skipped, rest of batch runs) | 3345 |
| FOK filled | 3089 |
| STP decrement (no fill, both reduced) | 2237 |
| STP cancel-newest (taker dropped) | 2102 |
| set_trigger SL (reduce-only) | 310 |
| exec SL trigger (keeper crank, IOC reduce-only) | 4343 |
| exec TWAP slice (1/3, sell 10) | 3311 |
| admin pause | 139 |
| paused: reduce-only allowed | 1097 |
| deposit under TVL cap (cap check + CPI) | 1369 |
| liquidate (cancel target resting orders + close) | 3466 |
| crank_claim (1 shard -> 1 maker) | 126 |
