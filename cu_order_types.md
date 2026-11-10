| order-type instruction (LiteSVM, Agave 4.3 SBF runtime, perp_program.so) | CU |
|---|---|
| init_market | 2009 |
| init_rank (admin) | 193 |
| set_tier (admin: MM tier 1 = rebate) | 195 |
| discrete: place 10 orders (5 bid + 5 ask levels) | 8843 |
| discrete: replace by cid (cancel 10 + place 10) | 11731 |
| discrete: cancel 10 by cid | 3667 |
| spline: place 2 ladders (=10 levels) | 2868 |
| spline: replace by cid (cancel 2 + place 2 ladders) | 3353 |
| spline: cancel 2 ladders by cid | 1219 |
| taker IOC sweep 5 discrete levels (3 shards) | 6480 |
| taker IOC sweep 5 spline levels (1 shard) | 6817 |
| place oracle-peg ask (offset +15, min/max band) | 1652 |
| taker IOC vs peg (lazy reprice) | 2871 |
| ALO crossing (skipped, tx ok) | 1640 |
| ALO slide (re-priced to best ask - 1 tick) | 1593 |
| FOK unfilled in batch (skipped, rest of batch runs) | 3384 |
| FOK filled | 3112 |
| STP decrement (no fill, both reduced) | 2276 |
| STP cancel-newest (taker dropped) | 2061 |
| set_trigger SL (reduce-only) | 260 |
| exec SL trigger (keeper crank, IOC reduce-only) | 6764 |
| exec TWAP slice (1/3, sell 10) | 4797 |
| admin pause | 181 |
| paused: reduce-only allowed | 1087 |
| deposit under TVL cap (cap check + CPI) | 1388 |
| liquidate (cancel target resting orders + close) | 5258 |
| crank_claim (1 shard -> 1 maker) | 128 |
