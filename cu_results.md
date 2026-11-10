| instruction (LiteSVM, Agave SBF runtime, perp_program.so) | CU |
|---|---|
| init_market | 2009 |
| init_rank (admin) | 193 |
| update_oracle (Pyth parse+conf+staleness, funding accrue) | 1167 |
| deposit (SPL transfer CPI) | 1383 |
| deposit (SPL transfer CPI) | 1383 |
| deposit (SPL transfer CPI) | 1383 |
| deposit (SPL transfer CPI) | 1383 |
| place 1 post-only (resting, no match) | 1655 |
| batch place 10 (5 bid + 5 ask levels) | 8915 |
| batch replace (cancel 10 + place 10) | 11826 |
| batch cancel 10 | 3693 |
| taker IOC: match 5 levels across 3 shards | 6398 |
| taker IOC: match 1 level | 2718 |
| withdraw (margin check + PDA-signed SPL CPI) | 2083 |
| batch replace + claim maker fills (3 shards w/ ledgers) | 11146 |
| update_oracle + premium/mark from full shard set (5 shards) | 3309 |
| liquidate (takeover at mark: health x2, transfer, fees) | 4628 |
| deposit (SPL transfer CPI) | 1383 |
| adl_rank (permissionless re-score) | 2054 |
| ADL (bankrupt -> on-chain ranked counterparty -> insurance) | 7465 |
| settle_funding (permissionless, 1 position) | 199 |
| aggregate OI/fees from 9 shards (crank) | 733 |
| liquidate_book (permissionless, 1 chunk into the book, no liquidator capital) | 5932 |
| exec SL trigger on mark (keeper crank, IOC reduce-only) | 3747 |
