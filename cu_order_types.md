| order-type instruction (LiteSVM, Agave 4.3 SBF runtime, perp_program.so) | CU |
|---|---|
| init_market | 2009 |
| init_rank (admin) | 193 |
| set_tier (admin: MM tier 1 = rebate) | 195 |
| discrete: place 10 orders (5 bid + 5 ask levels) | 9310 |
| discrete: replace by cid (cancel 10 + place 10) | 12069 |
| discrete: cancel 10 by cid | 3630 |
| spline: place 2 ladders (=10 levels) | 2977 |
| spline: replace by cid (cancel 2 + place 2 ladders) | 3429 |
| spline: cancel 2 ladders by cid | 1198 |
| taker IOC sweep 5 discrete levels (3 shards) | 6956 |
| taker IOC sweep 5 spline levels (1 shard) | 7319 |
| place oracle-peg ask (offset +15, min/max band) | 1722 |
| taker IOC vs peg (lazy reprice) | 2874 |
| ALO crossing (skipped, tx ok) | 1489 |
| ALO slide (re-priced to best ask - 1 tick) | 1667 |
| FOK unfilled in batch (skipped, rest of batch runs) | 3381 |
| FOK filled | 3120 |
| STP decrement (no fill, both reduced) | 2245 |
| STP cancel-newest (taker dropped) | 2108 |
| set_trigger SL (reduce-only) | 321 |
| exec SL trigger (keeper crank, IOC reduce-only) | 6555 |
| exec TWAP slice (1/3, sell 10) | 4569 |
| admin pause | 152 |
| paused: reduce-only allowed | 1063 |
| deposit under TVL cap (cap check + CPI) | 1388 |
| liquidate (cancel target resting orders + close) | 5202 |
| crank_claim (1 shard -> 1 maker) | 128 |
