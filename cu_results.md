| instruction (LiteSVM, Agave SBF runtime, perp_program.so) | CU |
|---|---|
| init_market | 2009 |
| init_rank (admin) | 193 |
| update_oracle (Pyth parse+conf+staleness, funding accrue) | 1257 |
| deposit (SPL transfer CPI) | 1383 |
| deposit (SPL transfer CPI) | 1383 |
| deposit (SPL transfer CPI) | 1383 |
| deposit (SPL transfer CPI) | 1383 |
| place 1 post-only (resting, no match) | 1942 |
| batch place 10 (5 bid + 5 ask levels) | 9598 |
| batch replace (cancel 10 + place 10) | 12389 |
| batch cancel 10 | 3665 |
| taker IOC: match 5 levels across 3 shards | 7089 |
| taker IOC: match 1 level | 3039 |
| withdraw (margin check + PDA-signed SPL CPI) | 2282 |
| batch replace + claim maker fills (3 shards w/ ledgers) | 11563 |
| update_oracle + premium/mark from full shard set (5 shards) | 4342 |
| liquidate (takeover at mark: health x2, transfer, fees) | 5200 |
| deposit (SPL transfer CPI) | 1383 |
| adl_rank (permissionless re-score) | 2233 |
| ADL (bankrupt -> on-chain ranked counterparty -> insurance) | 8165 |
| settle_funding (permissionless, 1 position) | 199 |
| aggregate OI/fees from 9 shards (crank) | 733 |
| liquidate_book (permissionless, 1 chunk into the book, no liquidator capital) | 6232 |
| exec SL trigger on mark (keeper crank, IOC reduce-only) | 3944 |
