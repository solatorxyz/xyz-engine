| instruction (LiteSVM, Agave SBF runtime, perp_program.so) | CU |
|---|---|
| init_market | 2011 |
| init_rank (admin) | 194 |
| update_oracle (Pyth parse+conf+staleness, funding accrue) | 1169 |
| deposit (SPL transfer CPI) | 1386 |
| deposit (SPL transfer CPI) | 1386 |
| deposit (SPL transfer CPI) | 1386 |
| deposit (SPL transfer CPI) | 1386 |
| place 1 post-only (resting, no match) | 1657 |
| batch place 10 (5 bid + 5 ask levels) | 8913 |
| batch replace (cancel 10 + place 10) | 11824 |
| batch cancel 10 | 3691 |
| taker IOC: match 5 levels across 3 shards | 6400 |
| taker IOC: match 1 level | 2720 |
| withdraw (margin check + PDA-signed SPL CPI) | 2141 |
| batch replace + claim maker fills (3 shards w/ ledgers) | 11144 |
| update_oracle + premium/mark from full shard set (5 shards) | 3318 |
| liquidate (takeover at mark: health x2, transfer, fees) | 4630 |
| deposit (SPL transfer CPI) | 1386 |
| adl_rank (permissionless re-score) | 2056 |
| ADL (bankrupt -> on-chain ranked counterparty -> insurance) | 7499 |
| settle_funding (permissionless, 1 position) | 202 |
| aggregate OI/fees from 9 shards (crank) | 740 |
| liquidate_book (permissionless, 1 chunk into the book, no liquidator capital) | 5940 |
| exec SL trigger on mark (keeper crank, IOC reduce-only) | 3790 |
