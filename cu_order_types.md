| order-type instruction (LiteSVM, Agave 4.3 SBF runtime, perp_program.so) | CU |
|---|---|
| init_market | 2009 |
| init_rank (admin) | 193 |
| set_tier (admin: MM tier 1 = rebate) | 195 |
| discrete: place 10 orders (5 bid + 5 ask levels) | 9300 |
| discrete: replace by cid (cancel 10 + place 10) | 12059 |
| discrete: cancel 10 by cid | 3630 |
| spline: place 2 ladders (=10 levels) | 2975 |
| spline: replace by cid (cancel 2 + place 2 ladders) | 3427 |
| spline: cancel 2 ladders by cid | 1198 |
| taker IOC sweep 5 discrete levels (3 shards) | 6955 |
| taker IOC sweep 5 spline levels (1 shard) | 7308 |
| place oracle-peg ask (offset +15, min/max band) | 1721 |
| taker IOC vs peg (lazy reprice) | 2867 |
| ALO crossing (skipped, tx ok) | 1488 |
| ALO slide (re-priced to best ask - 1 tick) | 1666 |
| FOK unfilled in batch (skipped, rest of batch runs) | 3373 |
| FOK filled | 3113 |
| STP decrement (no fill, both reduced) | 2240 |
| STP cancel-newest (taker dropped) | 2101 |
| set_trigger SL (reduce-only) | 321 |
| exec SL trigger (keeper crank, IOC reduce-only) | 6554 |
| exec TWAP slice (1/3, sell 10) | 4567 |
| admin pause | 152 |
| paused: reduce-only allowed | 1062 |
| deposit under TVL cap (cap check + CPI) | 1388 |
| liquidate (cancel target resting orders + close) | 5211 |
| crank_claim (1 shard -> 1 maker) | 128 |
