| order-type instruction (LiteSVM, Agave 4.3 SBF runtime, perp_program.so) | CU |
|---|---|
| init_market | 2009 |
| init_rank (admin) | 193 |
| set_tier (admin: MM tier 1 = rebate) | 195 |
| discrete: place 10 orders (5 bid + 5 ask levels) | 9261 |
| discrete: replace by cid (cancel 10 + place 10) | 12010 |
| discrete: cancel 10 by cid | 3617 |
| spline: place 2 ladders (=10 levels) | 2962 |
| spline: replace by cid (cancel 2 + place 2 ladders) | 3412 |
| spline: cancel 2 ladders by cid | 1193 |
| taker IOC sweep 5 discrete levels (3 shards) | 6949 |
| taker IOC sweep 5 spline levels (1 shard) | 7302 |
| place oracle-peg ask (offset +15, min/max band) | 1717 |
| taker IOC vs peg (lazy reprice) | 2861 |
| ALO crossing (skipped, tx ok) | 1484 |
| ALO slide (re-priced to best ask - 1 tick) | 1663 |
| FOK unfilled in batch (skipped, rest of batch runs) | 3364 |
| FOK filled | 3105 |
| STP decrement (no fill, both reduced) | 2234 |
| STP cancel-newest (taker dropped) | 2095 |
| set_trigger SL (reduce-only) | 321 |
| exec SL trigger (keeper crank, IOC reduce-only) | 6539 |
| exec TWAP slice (1/3, sell 10) | 4561 |
| admin pause | 152 |
| paused: reduce-only allowed | 1057 |
| deposit under TVL cap (cap check + CPI) | 1388 |
| liquidate (cancel target resting orders + close) | 5181 |
| crank_claim (1 shard -> 1 maker) | 128 |
