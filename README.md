# Solator kernel

The on-chain kernel of a perpetual-futures exchange for Solana: one program that holds the order book, matching,
margin, liquidation, funding and the liquidity vault. Every fill is a transaction anyone can replay.

This repository is the kernel, the evidence for it, and the one tool a stranger needs to use it without asking
anyone: a command line that opens a market, runs it and liquidates on anyone's (`ts/`). It does not contain the
trading terminal, the price publishers, the market maker, the exchange's own keepers, the upgrade-authority program
or any deployment tooling. Those are built on the kernel; none of them can make it break a rule in
`docs/SECURITY.md`.

**Research software. Unaudited. Do not use it with real funds.** It runs on devnet as
`7AY437YN1dupEhyahmKAQ6KTRm4nVx52BdyVzTcJbuRZ` with about 150 markets.

## What is here

| Path | What it is |
|---|---|
| `program/src` | The program: Rust on [pinocchio](https://github.com/anza-xyz/pinocchio), zero-copy, no allocator, 8,300 lines, one file per subsystem. |
| `perp-core/src/spec.rs` | Account layouts and constants. The program checks its own structs against this file at compile time; every client reads the same file. |
| `program-tests/` | 244 tests that run the built SBF program in LiteSVM: each rule, each regression, and the compute cost of each instruction. |
| `program/fuzz/` | Native fuzz targets with their own reference models: `native` (order book), `lanes` (withdrawals, fast path, liquidation, triggers, multisig, funding), `vault`. |
| `program/src/proofs.rs` | Kani proofs: the arithmetic fast paths equal the wide formula; a lease never gives one slot to two holders. |
| `docs/` | `ARCHITECTURE.md` (the design in one page, and how to check each claim), `SECURITY.md` (every rule and the test that pins it), `FAST_PATH.md`, `ORDER_RULES.md`. |
| `tools/same-code.py` | Says whether two builds contain the same machine code, function by function. |
| `ts/` | `solator`, a command line: open a market with a key and nothing else, keep its price, collect its fees, liquidate on any market. Its tests run the built program in a local VM. See `ts/README.md`. |

## Build and test

```sh
# toolchain: https://release.anza.xyz/stable/install
(cd program && cargo build-sbf) && (cd audit/noop && cargo build-sbf)
cd program-tests && cargo test --release
```

```sh
cd ts && npm install && npm test          # the command line, end to end, against program/target/deploy
```

```sh
cd program/fuzz && cargo +nightly fuzz run vault corpus/vault seeds_vault      # or: lanes, native
deploy/ops/kani_all.sh                                                          # every Kani harness, one at a time
```

## The ideas, briefly

- **Who writes what.** Two transactions that write one account cannot run in parallel, so an order book in one
  account is a queue of one. Makers write their own quote accounts, takers write the market's fill account, resting
  orders are sharded by price. People who are not trading with each other do not share a write lock.
- **Next-price execution.** An order is committed now and priced two slots later by a price nobody had when it was
  sent, so a passive quote can be tight without being picked off.
- **A vault that its operator cannot drain.** The operator's key writes quotes inside on-chain rules and moves idle
  cash between the vault's own accounts. No instruction lets it move value out.
- **Parameters wait in the open.** A change that does not strictly reduce risk is proposed, waits 24 hours, and is
  then executed by anyone.
- **A market needs nobody's permission.** A market opened without a pool has a vault and an insurance account of
  its own; a trading account's cash is a claim on one vault only, so what happens there stays there. Its creator
  posts its price, may post a bond, and collects half its fees.

Start with `docs/ARCHITECTURE.md`.

## Licence

Apache-2.0 (`LICENSE`). Copyright 2026 Solator.
