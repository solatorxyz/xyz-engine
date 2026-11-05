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
- `update_oracle_soft` (IX 78) is `update_oracle` with an exact revert instead of an error: the market is the only account
  the update writes, a byte snapshot is restored on any failure (including a partially advanced fallback replay floor), so
  a publisher can bundle it with its price post without an unusable oracle sinking the post. Same accounts, same checks,
  no new authority; a non-market first account is a no-op.
- Book-derived inputs (mid, premium) are taken only from the complete book: every shard and every registered FastBook with
  all slabs; otherwise the previous sample is held. Only orders that rested at least one slot count, as impact prices
  (size behind them), never a dust best price; a crossed book is no price.
- The mid EMA credits a deviation only for slots it was present at two consecutive samples (the smaller of the two), at most
  5 slots per sample. The premium sample is clamped to ±5%. Pushers sample every ~2 s.

## Invariants (fuzz target `program/fuzz`)

Σ base = 0 per market; `oi_long` = Σ long base; insurance ≥ 0;
vault + bad_debt = Σ cash + Σ iso_margin (isolated positions only) + insurance + fees.
