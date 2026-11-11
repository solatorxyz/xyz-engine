# Architecture

How the program is put together, why it is shaped this way, and how each claim in it can be checked. `SECURITY.md` lists
the rules the program enforces and the tests that pin them; `FAST_PATH.md` goes deep on matching and compute cost. This
file is the map.

## The one constraint everything follows from

On Solana a transaction names every account it touches, and two transactions that write the same account cannot run in
parallel. An order book in one account is therefore a queue of one. The design question is always the same: **who writes
what, and how do we keep people who are not trading with each other off each other's write locks?**

| Account | Written by | So that |
|---|---|---|
| `Slab` (one per maker) | that maker only | a maker re-quoting never waits for takers, and makers never wait for each other |
| `FastBook` (one or two per market) | takers | fills go to one small account; with one book per side, buys and sells do not contend either |
| `Shard` (one per price range per side) | whoever places or crosses in that range | resting orders far apart in price never contend |
| `User` | its owner, and anyone settling into it | a fill does not write the maker's `User`: it is credited later (`slab_settle`, `claim`) |
| `Market` | oracle updates and cranks | trading paths only read it; open interest and fees accumulate in the books and are folded in by a crank |

The cost of this is that state is spread out, and anything that judges an account (a withdrawal, a liquidation, the
vault's valuation) must first be sure it is looking at all of it. That rule has a name in the code, `complete_view`, and
most of the hard bugs this program has had were places that judged an account without it.

## Modules (`program/src`)

`lib.rs` holds the entry point, the instruction table and the compile-time checks that every account layout matches
`perp-core/src/spec.rs` (the layout shared with the SDKs, the bots and the UI). Everything else is one file per
subsystem; each file opens with what it is responsible for and the rule it keeps.

| Module | Lines | What it owns |
|---|---:|---|
| `state.rs` | 469 | Account layouts. Fixed size, 8-byte aligned, no padding, read in place. |
| `accounts.rs` | 101 | `load` (bytes to typed reference), who signed, multisig. |
| `math.rs` | 115 | Fixed-point arithmetic and its native fast paths. |
| `error.rs`, `events.rs` | 155 | Error codes; fill and liquidation events. |
| `init.rs`, `custody.rs` | 407 | Creating accounts; deposit and withdrawal. |
| `margin.rs` | 493 | Equity and margin: the single definition (`health_g`) and its variants. |
| `book.rs` | 1,149 | Resting orders in shards: place, cancel, match, lazy maker credit. |
| `quote.rs`, `signed_px.rs`, `fast_take.rs` | 1,299 | The fast path: maker ladders, the book's signed price, taker matching. |
| `trigger.rs` | 409 | TP / SL / TWAP and next-price orders. |
| `oracle.rs` | 809 | Price feeds, confidence band, circuit breaker, mark, funding. |
| `liquidation.rs`, `adl.rs` | 822 | Liquidation into the book, takeover, auto-deleveraging. |
| `vault.rs` | 597 | The liquidity vault: shares, valuation in parts, the vault as a maker. |
| `lease.rs` | 143 | Which host of a signing key may sign for a slot. |
| `admin.rs`, `delegate.rs` | 358 | Parameters and the timelock; delegated keys. |
| `proofs.rs` | 317 | Kani harnesses (compiled only under `cfg(kani)`). |

## Decisions worth knowing before reading the code

**Zero-copy, outside the borrow checker.** `load` returns `&'static mut T` pointing into the account's data. No
deserialisation, no allocator (the program has none), and no compiler-checked aliasing. The discipline that replaces it:
a path never holds a `&mut` to an account while it loads that account again. Trading paths hold the market shared;
the two places that once did not (`adl`, `vault_withdraw`) copy what they need first. `program-tests/diff-build.sh`
reruns the suites on builds at other optimisation levels, where an aliasing assumption would show up as a different
answer.

**Money is exact; rounding always has a named loser.** Money is `i128` while it is computed and `i64` where it is
stored; `i64c` is the only way back and fails instead of wrapping (`overflow-checks` is on in release). Fees round up
against the taker, share prices round against whoever deposits or withdraws, the liquidation fee is capped by what is
left. Where a native 64-bit path replaces the wide one for compute cost, a Kani proof or an exhaustive test says they
agree.

**Stale prices fail closed.** Everything that opens risk or pays out reads the price with `need_fresh`. A market
whose oracle is older than its limit can be exited and liquidated into the book but not entered.

**The executor never chooses.** Liquidations, trigger fills and next-price fills are permissionless, so whoever sends
them picks the accounts. The program takes that choice away wherever it matters: every opposite shard must be present
(`all_shards`), every approved maker must be present (`collect_slabs(.., all)`), and a next-price order fills only at
the price signed for its own slot.

**Next-price execution.** An order is committed at slot t and priced at slot t + 2 by a price nobody had when it was
sent. A quote can then be tight without being picked off by someone who has seen the next tick, which is what lets a
passive vault quote at all (measured on a week of 1 s Binance data: adverse move after a fill 1.2 to 4.8 bp for a
same-second fill, 0.2 to 0.4 bp at the next price). The price is one Ed25519 signature per market per slot.
`lease.rs` is how more than one machine can hold that key without ever producing two prices for a slot.

**The vault is valued in parts.** A vault that quotes 128 markets has more accounts than a transaction can carry.
Each sub-account is valued on its own and remembered; a deposit or withdrawal may use a remembered part only for 75
slots, and cash moved between sub-accounts is applied to the parts on the spot, so a part is only ever out of date by
what the market did since. The operator's key can write quotes and move idle cash between the vault's own accounts.
It has no instruction that moves value out.

**Nothing about parameters is instant unless it reduces risk.** Pause, lower a cap, clear a signer: immediate.
Everything else is proposed, waits 24 hours in the open, and is executed by anyone.

## How to check it

| Claim | Evidence | Run |
|---|---|---|
| The program does what the rules in `SECURITY.md` say | 238 tests against the built SBF program in LiteSVM | `cd program-tests && cargo test --release` |
| Invariants hold under arbitrary instruction sequences | three native fuzz targets with their own models: `native` (order book), `lanes` (withdrawals, fast path, liquidation, triggers, multisig, funding), `vault` | `cd program/fuzz && cargo +nightly fuzz run vault corpus/vault seeds_vault` |
| The fuzzers would notice | planted bugs are caught from the seeds (five for `vault`, listed in `SECURITY.md`) | see `SECURITY.md` |
| Arithmetic fast paths equal the wide formula; the lease never gives a slot twice | Kani proofs, full bit-widths where the solver finishes | `deploy/ops/kani_all.sh` |
| Compute cost per instruction | measured, written to `cu_results.md` by the test run | `cargo test --release --test cu -- --nocapture` |
| A refactor changed nothing that runs | `tools/same-code.py` compares two builds function by function after removing layout (addresses, call offsets, module names) | `tools/same-code.py old.so new.so` |
| The Python tools were reformatted, not rewritten | `tools/same-tree.py` refuses a file whose syntax tree would change | `tools/same-tree.py file.py` |
| No behaviour hides behind the optimiser | all suites again at `opt-level` 1 and z without LTO | `program-tests/diff-build.sh` |

The split of the original single file into these modules was checked with `tools/same-code.py`: 183 functions, 58,349
instruction slots, identical.

## What is not here

The program is unaudited. Known limits are stated where they live: the funding identity is exact only up to
per-account rounding across lazily settled fills (`fuzz_targets/lanes.rs`, "funding"); a signing key that ignores its own
lease can still sign what it likes (`lease.rs`); devnet state is created by `deploy/setup_devnet.py` and is not a
template for mainnet parameters.
