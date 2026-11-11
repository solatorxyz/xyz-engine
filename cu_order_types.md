| order-type instruction (LiteSVM, Agave 4.3 SBF runtime, perp_program.so) | CU |
|---|---|
| init_market | 2006 |
| init_rank (admin) | 196 |
| set_tier (admin: MM tier 1 = rebate) | 192 |
| discrete: place 10 orders (5 bid + 5 ask levels) | 8840 |
| discrete: replace by cid (cancel 10 + place 10) | 11728 |
| discrete: cancel 10 by cid | 3664 |
| spline: place 2 ladders (=10 levels) | 2865 |
| spline: replace by cid (cancel 2 + place 2 ladders) | 3350 |
| spline: cancel 2 ladders by cid | 1216 |
| taker IOC sweep 5 discrete levels (3 shards) | 6481 |
| taker IOC sweep 5 spline levels (1 shard) | 6818 |
| place oracle-peg ask (offset +15, min/max band) | 1653 |
| taker IOC vs peg (lazy reprice) | 2872 |
| ALO crossing (skipped, tx ok) | 1641 |
| ALO slide (re-priced to best ask - 1 tick) | 1594 |
| FOK unfilled in batch (skipped, rest of batch runs) | 3381 |
| FOK filled | 3113 |
| STP decrement (no fill, both reduced) | 2277 |
| STP cancel-newest (taker dropped) | 2062 |
| set_trigger SL (reduce-only) | 384 |
| exec SL trigger (keeper crank, IOC reduce-only) | 6820 |
| exec TWAP slice (1/3, sell 10) | 4855 |
| admin pause | 184 |
| paused: reduce-only allowed | 1088 |
| deposit under TVL cap (cap check + CPI) | 1388 |
| liquidate (cancel target resting orders + close) | 5259 |
| crank_claim (1 shard -> 1 maker) | 131 |
