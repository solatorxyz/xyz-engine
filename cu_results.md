| instruction (LiteSVM, Agave SBF runtime, perp_program.so) | CU |
|---|---|
| init_market | 1992 |
| init_rank (admin) | 175 |
| update_oracle (Pyth parse+conf+staleness, funding accrue) | 917 |
| deposit (SPL transfer CPI) | 1361 |
| deposit (SPL transfer CPI) | 1361 |
| deposit (SPL transfer CPI) | 1361 |
| deposit (SPL transfer CPI) | 1361 |
| place 1 post-only (resting, no match) | 1729 |
| batch place 10 (5 bid + 5 ask levels) | 9268 |
| batch replace (cancel 10 + place 10) | 12248 |
| batch cancel 10 | 3839 |
| taker IOC: match 5 levels across 3 shards | 6653 |
| taker IOC: match 1 level | 2781 |
| withdraw (margin check + PDA-signed SPL CPI) | 2081 |
| batch replace + claim maker fills (3 shards w/ ledgers) | 11300 |
| update_oracle + premium/mark from full shard set (5 shards) | 3077 |
| liquidate (takeover at mark: health x2, transfer, fees) | 3170 |
| deposit (SPL transfer CPI) | 1361 |
| adl_rank (permissionless re-score) | 2080 |
| ADL (bankrupt -> on-chain ranked counterparty -> insurance) | 6123 |
| settle_funding (permissionless, 1 position) | 196 |
| aggregate OI/fees from 9 shards (crank) | 851 |
| liquidate_book (permissionless, 1 chunk into the book, no liquidator capital) | 4137 |
| exec SL trigger on mark (keeper crank, IOC reduce-only) | 2612 |
