| instruction (LiteSVM, Agave SBF runtime, perp_program.so) | CU |
|---|---|
| init_market | 2009 |
| init_rank (admin) | 193 |
| update_oracle (Pyth parse+conf+staleness, funding accrue) | 1197 |
| deposit (SPL transfer CPI) | 1383 |
| deposit (SPL transfer CPI) | 1383 |
| deposit (SPL transfer CPI) | 1383 |
| deposit (SPL transfer CPI) | 1383 |
| place 1 post-only (resting, no match) | 1725 |
| batch place 10 (5 bid + 5 ask levels) | 9372 |
| batch replace (cancel 10 + place 10) | 12163 |
| batch cancel 10 | 3665 |
| taker IOC: match 5 levels across 3 shards | 6872 |
| taker IOC: match 1 level | 2816 |
| withdraw (margin check + PDA-signed SPL CPI) | 2131 |
| batch replace + claim maker fills (3 shards w/ ledgers) | 11337 |
| update_oracle + premium/mark from full shard set (5 shards) | 3795 |
| liquidate (takeover at mark: health x2, transfer, fees) | 4579 |
| deposit (SPL transfer CPI) | 1383 |
| adl_rank (permissionless re-score) | 2026 |
| ADL (bankrupt -> on-chain ranked counterparty -> insurance) | 6764 |
| settle_funding (permissionless, 1 position) | 199 |
| aggregate OI/fees from 9 shards (crank) | 733 |
| liquidate_book (permissionless, 1 chunk into the book, no liquidator capital) | 5620 |
| exec SL trigger on mark (keeper crank, IOC reduce-only) | 3525 |
