| instruction (LiteSVM, Agave SBF runtime, perp_program.so) | CU |
|---|---|
| init_market | 2001 |
| init_rank (admin) | 179 |
| update_oracle (Pyth parse+conf+staleness, funding accrue) | 999 |
| deposit (SPL transfer CPI) | 1373 |
| deposit (SPL transfer CPI) | 1373 |
| deposit (SPL transfer CPI) | 1373 |
| deposit (SPL transfer CPI) | 1373 |
| place 1 post-only (resting, no match) | 1689 |
| batch place 10 (5 bid + 5 ask levels) | 9234 |
| batch replace (cancel 10 + place 10) | 12214 |
| batch cancel 10 | 3855 |
| taker IOC: match 5 levels across 3 shards | 6615 |
| taker IOC: match 1 level | 2739 |
| withdraw (margin check + PDA-signed SPL CPI) | 2048 |
| batch replace + claim maker fills (3 shards w/ ledgers) | 11255 |
| update_oracle + premium/mark from full shard set (5 shards) | 3139 |
| liquidate (takeover at mark: health x2, transfer, fees) | 3085 |
| deposit (SPL transfer CPI) | 1373 |
| adl_rank (permissionless re-score) | 2026 |
| ADL (bankrupt -> on-chain ranked counterparty -> insurance) | 5921 |
| settle_funding (permissionless, 1 position) | 196 |
| aggregate OI/fees from 9 shards (crank) | 872 |
| liquidate_book (permissionless, 1 chunk into the book, no liquidator capital) | 5138 |
| exec SL trigger on mark (keeper crank, IOC reduce-only) | 2653 |
