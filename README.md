# Solator kernel

The on-chain kernel of a perpetual-futures exchange for Solana: one program that holds the order book, matching,
margin, liquidation, funding and the liquidity vault. Every fill is a transaction anyone can replay.

This repository is the kernel and the evidence for it, and nothing else. It does not contain the trading terminal,
the price publishers, the market maker, the keepers, client SDKs, the upgrade-authority program or any deployment
tooling. Those are built on the kernel; none of them can make it break a rule in `docs/SECURITY.md`.

**Research software. Unaudited. Do not use it with real funds.** It runs on devnet as
`7AY437YN1dupEhyahmKAQ6KTRm4nVx52BdyVzTcJbuRZ` with 152 markets.

## What is here

| Path | What it is |
|---|---|
| `program/src` | The program: Rust on [pinocchio](https://github.com/anza-xyz/pinocchio), zero-copy, no allocator, 8,100 lines, one file per subsystem. |
| `perp-core/src/spec.rs` | Account layouts and constants. The program checks its own structs against this file at compile time; every client reads the same file. |
| `program-tests/` | 230 tests that run the built SBF program in LiteSVM: each rule, each regression, and the compute cost of each instruction. |
| `program/fuzz/` | Native fuzz targets with their own reference models: `native` (order book), `lanes` (withdrawals, fast path, liquidation, triggers, multisig, funding), `vault`. |
| `program/src/proofs.rs` | Kani proofs: the arithmetic fast paths equal the wide formula; a lease never gives one slot to two holders. |
| `docs/` | `ARCHITECTURE.md` (the design in one page, and how to check each claim), `SECURITY.md` (every rule and the test that pins it), `FAST_PATH.md`, `ORDER_RULES.md`. |
| `tools/same-code.py` | Says whether two builds contain the same machine code, function by function. |

## Build and test

```sh
# toolchain: https://release.anza.xyz/stable/install
(cd program && cargo build-sbf) && (cd audit/noop && cargo build-sbf)
cd program-tests && cargo test --release
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

Start with `docs/ARCHITECTURE.md`.
