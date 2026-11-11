# Security model and invariants

This is the set of rules the program enforces after the November 2026 audit rounds. Anything here is a test in
`program-tests/tests/` (suites `poc_a`, `poc_b`, `poc_c`, `review`..`review5` are the attacks that were found and are now
refused). Comments in `lib.rs` may lag behind; this file and the tests are authoritative.

## Signers and authority

- A signature counts only from an account the program does not own (`sg`). Vaults, multisigs and users are program-owned,
  so a keypair kept for one of them is no authority: it cannot withdraw from vault sub-accounts or act as a multisig alone.
- Admin actions that change prices or risk go through the timelock (`TIMELOCK_SLOTS`, ~24 h): margin ratios, admin key,
  primary oracle (`ADMIN_ORACLE`), fallback oracle, the fast-price signer (`ADMIN_FAST_SIGNER`), discovery bounds / strict
  take / session key (`ADMIN_MARKET_CFG`). Immediate admin actions only reduce risk: pause, TVL cap, OI cap, position cap,
  clearing a signer, and configuration before a market has its first price.
- A session key (delegate) may place and cancel only, within its quantity cap, counted on what rests (spline levels included).

## Order book

- A shard may be passed once per instruction. The cursor into a shard is validated against the order it points at.
- Cancel by id looks in the shard the id names (overflow shards share a price bucket).
- A resting bid above the mark (ask below it) margins the loss it would take the moment it fills, so a resting order alone
  can never leave an account bankrupt when it fills.
- A market has at most 16 shards per side and 64 in all, so "every opposite shard" always fits one instruction.

## Maker fast path

- Takes, triggers and liquidations use only the market's registered books (`Market.fast_book`), with every approved
  maker's slab passed where a third party chooses the accounts.
- Signed prices are current (≤ 2 slots), within 100 bps of the oracle, newer than the stored one, and named for the market.
- A maker's market stays cross-margined. Its locked margin is mirrored on the position (`Pos.iso_margin` of a pinned cross
  position) so liquidation and ADL refuse while any is locked; a settle returns it whenever the account is below
  maintenance without it. The quoting budget refills only from margin that initial margin leaves free.
- Every fast take, reduce-only included, needs a current oracle.

## Account views and liquidation

- Before withdraw or moving isolated margin out: every shard of each market where the account has resting orders (so all
  fills are claimed), and the registered books where it still has maker margin locked (fills settled).
- Before any liquidation or ADL judges an account: no resting orders on the judged positions (`no_resting`). The liquidated
  market's own orders are cancelled inside the liquidation; orders elsewhere are cleared first with the permissionless
  `liq_cancel` (IX 77), allowed only while the account is below maintenance and only with that market's complete view.
- `liquidate_book` is for sides without a registered FastBook. A side with one is liquidated by `liquidate_fast`, which
  fills resting orders and maker quotes in price order (all opposite shards and all makers passed), widens the shard band
  5% → 10% → 20% with the fee doubling per step, and opens the ADL window only when every maker and every shard is
  provably empty within the widest band.
- Third-party takeovers (`liquidate`) are one chunk, never more than restores maintenance. The protocol vault's takeover
  (below 2/3 of maintenance) may take the whole position.
- ADL closes lots against the top-ranked counterparty, which must have been re-scored in the same call and hold the
  opposite side; an entry that cannot be evaluated keeps its rank and blocks until it is. Each step carries only the closed
  lots' share of the target's deficit, paid from insurance, then from the counterparty up to its gain on those lots, the
  rest to `bad_debt`.
- Keeper tips come from the position's own bucket and are judged there.
- The protocol vault's takeover judges its inventory cap on the live NAV (every active sub passed), and halts on drawdown.

## Oracle, mark and funding

- The primary oracle is an OracleSet: median of independent publishers with a quorum; every member must be passed, a
  missing one is a hard error (no fallback by omission). With a fallback oracle configured, both must be passed (the
  fallback keeps its own replay floor); a single source is never applied while the divergence breaker is tripped.
- A market whose primary is a single AGG account has a single-key oracle: a jump over 5% is held until a later publish
  confirms it (within 0.5%, or further the same way), but that publish comes from the same key. Production markets use an
  OracleSet, whose median is its own guard; the deploy scripts create one per market.
- Pool and vault admins can be a Multisig account (named at creation); the creating key alone then has no power.
- The external perp price needs the set's quorum and bounded publish times.
- `agg_post` (IX 41) with a publish time no newer than the stored one lands as a no-op (nothing applied): the same replay
  protection, but an overtaken post (transactions land out of order across leaders) no longer fails the other posts and
  pushes bundled in its transaction. A future publish time (> clock + 5 s) is still an error.
- `update_oracle_soft` (IX 78) is `update_oracle` with an exact revert instead of an error: the market is the only account
  the update writes, a byte snapshot is restored on any failure (including a partially advanced fallback replay floor), so
  a publisher can bundle it with its price post without an unusable oracle sinking the post. Same accounts, same checks,
  no new authority; a non-market first account is a no-op.
- Book-derived inputs (mid, premium) are taken only from the complete book: every shard and every registered FastBook with
  all slabs; otherwise the previous sample is held. Only orders that rested at least one slot count, as impact prices
  (size behind them), never a dust best price; a crossed book is no price.
- The mid EMA credits a deviation only for slots it was present at two consecutive samples (the smaller of the two), at most
  5 slots per sample. The premium sample is clamped to ±5%. Pushers sample every ~2 s.

## The vault as a maker (2026-11-11)

Depositors' money can quote the markets: a sub-account of the vault is the maker (tests: `vault_maker.rs`).

- A vault slab's quoting authority is the vault account, which is program-owned and signs nothing, so `slab_quote`,
  `slab_kill`, `slab_fund` and `fast_join` never apply to it. Only IX 58, 59, 79-83 write it.
- Roles. Vault admin (may be a Multisig): makes the slab, approves it on a book together with the market admin, locks
  margin or raises its target, names the operator and the quote rules. Operator (a hot key): writes quotes inside the
  rules, stops and resumes them, releases margin or lowers its target, moves idle cash between the vault's own
  sub-accounts. Anyone: settles fills, stops the quotes of a halted vault. None of these moves value out of the vault.
- Quote rules, checked on every ladder a write names, as the ladder stands after the write: pegged to the book's price;
  an ask at or above it and a bid at or below it; with `q_min_edge_bps`, at least that far from it, measured on the
  ladder's reference price, which then needs a tolerance no wider than `q_max_tol_bps` (required, at most 2,000 bps);
  with `q_fresh`, the ladder's fresh-price setting. A ladder offering nothing passes.
  What the rules do not cover: an operator can still quote too tight, too large or not at all. The loss from a bad quote
  is bounded by the margin the admin locked (budget = margin / IMR between settles), not by the rules.
- The vault's value counts the margin its accounts have locked in the books (the program takes it out of their cash) and
  is refused while any of their fills is unsettled or unclaimed: `vault_nav` needs each active sub-account's complete
  view (the registered books of every market it quotes). Deposits, withdrawals, takeovers and the crank all go through
  it, so a deposit can't buy the vault below its worth and a withdrawal can't leave before a loss is booked.
- Withdrawals are paid from the idle cash of `subs[0]`. Cash locked for quoting has to be released first (admin or
  operator), and other sub-accounts' cash moved back with IX 83.
- A halted vault takes no new quotes and no more margin; what is posted can be stopped by anyone.
- Limits to know: a sub-account quotes at most 16 markets (its position slots); valuing the vault takes every active
  sub-account, its markets and their books in one transaction, which caps how many markets one vault can quote before
  the value has to be computed in steps. Quotes expire after 30 slots like any maker's: the operator re-posts them.

## Next-price execution (2026-11-11)

A maker ladder flagged `LADDER_NEXT` never fills an immediate take. It fills only an order that was committed first, at the
price the book's signer signed for the slot `NEXT_DELAY_SLOTS` (2) after the commitment (tests: `vault_maker.rs`).

- Commitments: a next-price order (`TRIGGER_KIND_NEXT`, made with `set_trigger`) from its creation; a stop, take-profit or
  TWAP slice once a keeper arms it (`exec_trigger` with arm = 1, allowed only when it would fire). The slot is recorded in
  the trigger. While a commitment is live its trigger slot can't be replaced or cleared, and the account can't withdraw or
  move margin to an isolated position: there is no taking it back once its price is known.
- The fill needs the Ed25519-verified message `etnextpx | market | px | slot | ts` from the book's signer with exactly that
  slot, a price within `FAST_PX_MAX_DEV_BPS` of the oracle, inside the same transaction. Quotes are priced off that price,
  not the book's newest; the ladder's fresh-price setting does not apply. Whoever executes cannot choose the price.
- A commitment lapses `NEXT_EXPIRY_SLOTS` (10) after its pricing slot: a next-price order is dropped, a stop or TWAP slice
  goes back to being armable. Anyone may clear a lapsed one.
- Where a maker of the book quotes next-price only, a stop or TWAP slice can't be filled without being armed first (it
  would be routed around those quotes).
- Liquidations fill next-price ladders at the book's current price.
- Trust: the signer must sign one price a market a slot. It is the key the market admin set through the timelock, the same
  key whose prices pegged ladders already follow.
- Residual: an order can still be made to fail at its fill (margin spent elsewhere, the position cap reached); the account
  gains nothing by it except not trading. A vault whose bot stops executing leaves the choice of which commitments fill to
  the takers until its quotes expire (30 slots).

## Invariants (fuzz target `program/fuzz`)

Σ base = 0 per market; `oi_long` = Σ long base; insurance ≥ 0;
vault + bad_debt = Σ cash + Σ iso_margin (isolated positions only) + insurance + fees.

## Oracle band (2026-11-06)

The oracle's confidence — for an OracleSet the publishers' dispersion (median conf vs median deviation) — is added, in bps of
the price and capped at `BAND_MAX_BPS` = 50, to every initial and maintenance requirement (`health`, `iso_health`,
`worst_ok`, `liq_needed`). Margin tightens by itself while sources disagree or a venue is thin, and relaxes when they agree.
Equity is still valued at the mark, so bankruptcy, ADL ordering, deficits and vault NAV are unchanged; only the gates move:
opening risk needs IM + band, liquidation starts at MM + band, and a liquidation restores MM + band. `update_oracle` already
refuses a confidence above `max_conf_bps`, so the band is bounded twice. Tests: `band.rs`.
