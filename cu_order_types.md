| order-type instruction (LiteSVM, Agave 4.3 SBF runtime, perp_program.so) | CU |
|---|---|
| init_market | 1992 |
| init_rank (admin) | 175 |
| set_tier (admin: MM tier 1 = rebate) | 174 |
| discrete: place 10 orders (5 bid + 5 ask levels) | 10244 |
| discrete: replace by cid (cancel 10 + place 10) | 12645 |
| discrete: cancel 10 by cid | 4117 |
| spline: place 2 ladders (=10 levels) | 4018 |
| spline: replace by cid (cancel 2 + place 2 ladders) | 4142 |
| spline: cancel 2 ladders by cid | 1716 |
| taker IOC sweep 5 discrete levels (3 shards) | 7786 |
| taker IOC sweep 5 spline levels (1 shard) | 7590 |
| place oracle-peg ask (offset +15, min/max band) | 2426 |
| taker IOC vs peg (lazy reprice) | 3544 |
| ALO crossing (skipped, tx ok) | 2240 |
| ALO slide (re-priced to best ask - 1 tick) | 2383 |
| FOK unfilled in batch (skipped, rest of batch runs) | 4029 |
| FOK filled | 3767 |
| STP decrement (no fill, both reduced) | 2922 |
| STP cancel-newest (taker dropped) | 2781 |
| set_trigger SL (reduce-only) | 307 |
| exec SL trigger (keeper crank, IOC reduce-only) | 4914 |
| exec TWAP slice (1/3, sell 10) | 4271 |
| admin pause | 136 |
| paused: reduce-only allowed | 1586 |
| deposit under TVL cap (cap check + CPI) | 1369 |
| liquidate (cancel target resting orders + close) | 4983 |
| crank_claim (1 shard -> 1 maker) | 129 |
