| order-type instruction (LiteSVM, Agave 4.3 SBF runtime, perp_program.so) | CU |
|---|---|
| init_market | 2011 |
| init_rank (admin) | 194 |
| set_tier (admin: MM tier 1 = rebate) | 194 |
| discrete: place 10 orders (5 bid + 5 ask levels) | 8841 |
| discrete: replace by cid (cancel 10 + place 10) | 11729 |
| discrete: cancel 10 by cid | 3665 |
| spline: place 2 ladders (=10 levels) | 2866 |
| spline: replace by cid (cancel 2 + place 2 ladders) | 3351 |
| spline: cancel 2 ladders by cid | 1217 |
| taker IOC sweep 5 discrete levels (3 shards) | 6482 |
| taker IOC sweep 5 spline levels (1 shard) | 6819 |
| place oracle-peg ask (offset +15, min/max band) | 1654 |
| taker IOC vs peg (lazy reprice) | 2873 |
| ALO crossing (skipped, tx ok) | 1642 |
| ALO slide (re-priced to best ask - 1 tick) | 1595 |
| FOK unfilled in batch (skipped, rest of batch runs) | 3382 |
| FOK filled | 3114 |
| STP decrement (no fill, both reduced) | 2278 |
| STP cancel-newest (taker dropped) | 2063 |
| set_trigger SL (reduce-only) | 382 |
| exec SL trigger (keeper crank, IOC reduce-only) | 6822 |
| exec TWAP slice (1/3, sell 10) | 4857 |
| admin pause | 182 |
| paused: reduce-only allowed | 1089 |
| deposit under TVL cap (cap check + CPI) | 1391 |
| liquidate (cancel target resting orders + close) | 5260 |
| crank_claim (1 shard -> 1 maker) | 129 |
