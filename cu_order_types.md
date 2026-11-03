| order-type instruction (LiteSVM, Agave 4.3 SBF runtime, perp_program.so) | CU |
|---|---|
| init_market | 1992 |
| init_rank (admin) | 175 |
| set_tier (admin: MM tier 1 = rebate) | 174 |
| discrete: place 10 orders (5 bid + 5 ask levels) | 9196 |
| discrete: replace by cid (cancel 10 + place 10) | 11909 |
| discrete: cancel 10 by cid | 3569 |
| spline: place 2 ladders (=10 levels) | 2970 |
| spline: replace by cid (cancel 2 + place 2 ladders) | 3406 |
| spline: cancel 2 ladders by cid | 1168 |
| taker IOC sweep 5 discrete levels (3 shards) | 6738 |
| taker IOC sweep 5 spline levels (1 shard) | 6854 |
| place oracle-peg ask (offset +15, min/max band) | 1692 |
| taker IOC vs peg (lazy reprice) | 2808 |
| ALO crossing (skipped, tx ok) | 1504 |
| ALO slide (re-priced to best ask - 1 tick) | 1647 |
| FOK unfilled in batch (skipped, rest of batch runs) | 3293 |
| FOK filled | 3031 |
| STP decrement (no fill, both reduced) | 2188 |
| STP cancel-newest (taker dropped) | 2047 |
| set_trigger SL (reduce-only) | 307 |
| exec SL trigger (keeper crank, IOC reduce-only) | 4366 |
| exec TWAP slice (1/3, sell 10) | 3210 |
| admin pause | 136 |
| paused: reduce-only allowed | 1045 |
| deposit under TVL cap (cap check + CPI) | 1369 |
| liquidate (cancel target resting orders + close) | 3184 |
| crank_claim (1 shard -> 1 maker) | 129 |
