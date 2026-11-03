| instruction (LiteSVM, Agave SBF runtime, perp_program.so) | CU |
|---|---|
| init_market | 1992 |
| init_rank (admin) | 174 |
| update_oracle (Pyth parse+conf+staleness, funding accrue) | 835 |
| deposit (SPL transfer CPI) | 1361 |
| deposit (SPL transfer CPI) | 1361 |
| deposit (SPL transfer CPI) | 1361 |
| deposit (SPL transfer CPI) | 1361 |
| place 1 post-only (resting, no match) | 1857 |
| batch place 10 (5 bid + 5 ask levels) | 9073 |
| batch replace (cancel 10 + place 10) | 12047 |
| batch cancel 10 | 3889 |
| taker IOC: match 5 levels across 3 shards | 6653 |
| taker IOC: match 1 level | 2839 |
| withdraw (margin check + PDA-signed SPL CPI) | 2166 |
| batch replace + claim maker fills (3 shards w/ ledgers) | 11072 |
| update_oracle + premium/mark from full shard set (5 shards) | 2894 |
| liquidate (takeover at mark: health x2, transfer, fees) | 3452 |
| deposit (SPL transfer CPI) | 1361 |
| adl_rank (permissionless re-score) | 2165 |
| ADL (bankrupt -> on-chain ranked counterparty -> insurance) | 6441 |
| settle_funding (permissionless, 1 position) | 226 |
| aggregate OI/fees from 9 shards (crank) | 852 |
| liquidate_book (permissionless, 1 chunk into the book, no liquidator capital) | 4245 |
| exec SL trigger on mark (keeper crank, IOC reduce-only) | 2616 |
