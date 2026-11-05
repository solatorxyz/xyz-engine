| order-type instruction (LiteSVM, Agave 4.3 SBF runtime, perp_program.so) | CU |
|---|---|
| init_market | 2009 |
| init_rank (admin) | 193 |
| set_tier (admin: MM tier 1 = rebate) | 195 |
| discrete: place 10 orders (5 bid + 5 ask levels) | 9174 |
| discrete: replace by cid (cancel 10 + place 10) | 11933 |
| discrete: cancel 10 by cid | 3627 |
| spline: place 2 ladders (=10 levels) | 2931 |
| spline: replace by cid (cancel 2 + place 2 ladders) | 3383 |
| spline: cancel 2 ladders by cid | 1195 |
| taker IOC sweep 5 discrete levels (3 shards) | 6934 |
| taker IOC sweep 5 spline levels (1 shard) | 7287 |
| place oracle-peg ask (offset +15, min/max band) | 1644 |
| taker IOC vs peg (lazy reprice) | 2846 |
| ALO crossing (skipped, tx ok) | 1467 |
| ALO slide (re-priced to best ask - 1 tick) | 1609 |
| FOK unfilled in batch (skipped, rest of batch runs) | 3352 |
| FOK filled | 3092 |
| STP decrement (no fill, both reduced) | 2219 |
| STP cancel-newest (taker dropped) | 2080 |
| set_trigger SL (reduce-only) | 321 |
| exec SL trigger (keeper crank, IOC reduce-only) | 6515 |
| exec TWAP slice (1/3, sell 10) | 4536 |
| admin pause | 152 |
| paused: reduce-only allowed | 1057 |
| deposit under TVL cap (cap check + CPI) | 1388 |
| liquidate (cancel target resting orders + close) | 5811 |
| crank_claim (1 shard -> 1 maker) | 128 |
