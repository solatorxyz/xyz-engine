| order-type instruction (LiteSVM, Agave 4.3 SBF runtime, perp_program.so) | CU |
|---|---|
| init_market | 2005 |
| init_rank (admin) | 180 |
| set_tier (admin: MM tier 1 = rebate) | 182 |
| discrete: place 10 orders (5 bid + 5 ask levels) | 9162 |
| discrete: replace by cid (cancel 10 + place 10) | 11875 |
| discrete: cancel 10 by cid | 3585 |
| spline: place 2 ladders (=10 levels) | 2930 |
| spline: replace by cid (cancel 2 + place 2 ladders) | 3366 |
| spline: cancel 2 ladders by cid | 1178 |
| taker IOC sweep 5 discrete levels (3 shards) | 6700 |
| taker IOC sweep 5 spline levels (1 shard) | 6812 |
| place oracle-peg ask (offset +15, min/max band) | 1639 |
| taker IOC vs peg (lazy reprice) | 2766 |
| ALO crossing (skipped, tx ok) | 1464 |
| ALO slide (re-priced to best ask - 1 tick) | 1605 |
| FOK unfilled in batch (skipped, rest of batch runs) | 3251 |
| FOK filled | 2989 |
| STP decrement (no fill, both reduced) | 2135 |
| STP cancel-newest (taker dropped) | 1996 |
| set_trigger SL (reduce-only) | 315 |
| exec SL trigger (keeper crank, IOC reduce-only) | 4778 |
| exec TWAP slice (1/3, sell 10) | 3216 |
| admin pause | 139 |
| paused: reduce-only allowed | 1057 |
| deposit under TVL cap (cap check + CPI) | 1384 |
| liquidate (cancel target resting orders + close) | 3102 |
| crank_claim (1 shard -> 1 maker) | 128 |
