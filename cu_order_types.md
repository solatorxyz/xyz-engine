| order-type instruction (LiteSVM, Agave 4.3 SBF runtime, perp_program.so) | CU |
|---|---|
| init_market | 1992 |
| init_rank (admin) | 175 |
| set_tier (admin: MM tier 1 = rebate) | 174 |
| discrete: place 10 orders (5 bid + 5 ask levels) | 9347 |
| discrete: replace by cid (cancel 10 + place 10) | 12014 |
| discrete: cancel 10 by cid | 3629 |
| spline: place 2 ladders (=10 levels) | 3121 |
| spline: replace by cid (cancel 2 + place 2 ladders) | 3511 |
| spline: cancel 2 ladders by cid | 1228 |
| taker IOC sweep 5 discrete levels (3 shards) | 6886 |
| taker IOC sweep 5 spline levels (1 shard) | 6956 |
| place oracle-peg ask (offset +15, min/max band) | 1794 |
| taker IOC vs peg (lazy reprice) | 2910 |
| ALO crossing (skipped, tx ok) | 1605 |
| ALO slide (re-priced to best ask - 1 tick) | 1748 |
| FOK unfilled in batch (skipped, rest of batch runs) | 3395 |
| FOK filled | 3132 |
| STP decrement (no fill, both reduced) | 2282 |
| STP cancel-newest (taker dropped) | 2149 |
| set_trigger SL (reduce-only) | 307 |
| exec SL trigger (keeper crank, IOC reduce-only) | 4426 |
| exec TWAP slice (1/3, sell 10) | 3372 |
| admin pause | 136 |
| paused: reduce-only allowed | 1098 |
| deposit under TVL cap (cap check + CPI) | 1369 |
| liquidate (cancel target resting orders + close) | 3462 |
| crank_claim (1 shard -> 1 maker) | 129 |
