// Opening a market, by anyone, with nothing but a key. There is no screen for it: as on Hyperliquid (HIP-3), a market's
// creator sends signed instructions from a script (cli/solator.ts is the one this repository ships).
//
// The program has always allowed it: init_market called without a pool makes the signer the market's admin and gives the
// market a vault and an insurance account of its own (program/src/init.rs). A user account's cash is a claim on exactly one
// vault (custody.rs, home_vault), so whatever happens in such a market stays in it: nobody who has not deposited there can
// lose anything there. What needed the pool admin's signature and a day's wait is only joining the shared pool.
//
// What a market is made of, all of it paid for and signed by its creator:
//   feed       the price, posted by the creator's key (agg_init / agg_post)
//   market     the parameters and the live state; with its insurance account
//   vault      an SPL token account owned by the market's own address (PDA "vault" + market)
//   rank       who is reduced first if a loss is larger than the insurance (ADL)
//   book       the fast book takers trade through
//   2 shards   where resting orders sit, one for bids and one for asks
//
// Three transactions, each one whole: an account is created and claimed in the same transaction wherever the claim can
// be made by anyone (the feed, the market, its insurance, its vault), so nobody can step in between.
import { Buffer } from "buffer";
import {
  ComputeBudgetProgram,
  Keypair,
  PublicKey,
  SystemProgram,
  Transaction,
  TransactionInstruction,
} from "@solana/web3.js";
import { decodeLayout, spec as S } from "./sdk/index";
import { ATA_PROGRAM as ATA_ID, PROGRAM_ID, TOKEN_PROGRAM } from "./ids";

const PID = new PublicKey(PROGRAM_ID);
const TOKEN = new PublicKey(TOKEN_PROGRAM);

/** Bytes of every account a market needs. Rent is a deposit on these bytes. */
export const SIZES = {
  feed: S.AGG_SIZE,
  market: S.MARKET_SIZE,
  insurance: S.INSURANCE_SIZE,
  vault: 165, // an SPL token account
  rank: S.RANK_SIZE,
  book: S.LAYOUT_SIZE.fast_book,
  shard: S.SHARD_SIZE,
} as const;
export type Part = keyof typeof SIZES;
/** Every account of one market, in the order they are made: [part, how many]. */
export const PARTS: [Part, number][] = [
  ["feed", 1],
  ["rank", 1],
  ["book", 1],
  ["market", 1],
  ["insurance", 1],
  ["vault", 1],
  ["shard", 2],
];
/** Lamports that keep an account of `size` bytes alive: the cluster's rent-exempt minimum as it stands in November 2026
 *  (5,080 a byte; it was 6,960 and is being lowered in steps). An estimate for the screen only: before anything is
 *  signed the page asks the cluster (getMinimumBalanceForRentExemption) and creates the accounts with its answer. */
export const rentOf = (size: number) => (size + 128) * 5080;
export const totalRent = (rent: (size: number) => number = rentOf) =>
  PARTS.reduce((a, [p, n]) => a + n * rent(SIZES[p]), 0);

// ---------- what the creator chooses ----------

/** How old a price may be before the market stops taking new risk. The program checks two clocks (margin.rs): the
 *  price's own publish time against `secs`, and the slot it was applied in against `slots`. */
export const FRESH = [
  { secs: 120, slots: 150, label: "1 min" },
  { secs: 600, slots: 1500, label: "10 min" },
  { secs: 3600, slots: 9000, label: "1 hour" },
] as const;

export type Draft = {
  /** the creator's prefix, as in xyz:GOLD; empty for none */
  dex: string;
  ticker: string;
  /** starting price in USD, as typed */
  price: string;
  /** 0 = the tick this module picks; -1 / +1 = ten times finer / coarser */
  tickStep: -1 | 0 | 1;
  /** open-interest cap in USD at the starting price, as typed; empty = none */
  oiCap: string;
  /** index into FRESH */
  fresh: number;
};
export const DRAFT0: Draft = { dex: "", ticker: "", price: "", tickStep: 0, oiCap: "", fresh: 0 };

export type Shape = {
  dex: string;
  ticker: string;
  feedId: Buffer;
  /** the starting price in 1e-8 USD (what the feed carries, expo -8) */
  pxE8: bigint;
  /** 1 price unit = pxDiv / 1e8 USD */
  pxDiv: bigint;
  /** the starting price in price units */
  units: bigint;
  tickUsd: number;
  /** asset units per lot (the smallest size) */
  lot: number;
  /** price units one shard covers; both shards start at 0 */
  bucket: bigint;
  /** lots; 0 = no cap */
  maxOi: bigint;
  maxAgeSecs: number;
  maxStaleSlots: number;
};
export type Problems = Partial<Record<"dex" | "ticker" | "price" | "oiCap", string>>;

/** A decimal as typed -> an integer of `dp` decimals. null when it is not a plain number or has more decimals than that. */
export function parseDec(s: string, dp: number): bigint | null {
  const m = /^(\d*)(?:\.(\d*))?$/.exec(s.trim());
  if (!m || (!m[1] && !m[2])) return null;
  if ((m[2] ?? "").length > dp) return null;
  return BigInt(m[1] || "0") * 10n ** BigInt(dp) + BigInt((m[2] ?? "").padEnd(dp, "0") || "0");
}
const digits = (v: bigint) => v.toString().length;
const pow10 = (n: number) => 10n ** BigInt(n);

/** The price unit that gives the starting price five significant digits (10,000 to 99,999 units): $0.01 for a $150
 *  asset, $0.0001 for a $1 one, $1 for a $65,000 one. Never finer than the feed itself (1e-8 USD). */
export const autoDiv = (pxE8: bigint) => pow10(Math.max(0, digits(pxE8) - 5));
/** The three ticks on offer for a price: [finer, picked, coarser] as px_div values; a slot is null where it cannot exist. */
export function tickChoices(pxE8: bigint): [bigint | null, bigint, bigint | null] {
  const a = autoDiv(pxE8);
  const coarser = a * 10n;
  return [a > 1n ? a / 10n : null, a, pxE8 / coarser >= 100n ? coarser : null];
}
/** One shard per side from price 0 up to this many units: the smallest power of ten that is at least eight times the
 *  starting price. The price can fall to nothing or rise eightfold (up to eightyfold) without leaving the book. */
export const bucketFor = (units: bigint) => pow10(digits(units * 8n - 1n));

/** The feed id is the market's name on chain: `dex:TICKER/USD` in ASCII, zero-filled to 32 bytes (the `dex:` part is
 *  optional). It is what ties a market to its feed (init_market stores it, every price read compares it), and anyone can
 *  read it back. The dex is the creator's own prefix, as in `xyz:GOLD`: two to six lower-case letters. Nothing on chain
 *  makes a name unique, so a list must show who made a market next to what it is called. */
export function feedIdFor(ticker: string, dex = ""): Buffer {
  const b = Buffer.alloc(32);
  b.write((dex ? dex + ":" : "") + ticker + "/USD", "ascii");
  return b;
}
/** The name a feed id of that form carries, or null for any other feed id. */
export function nameOf(feedId: Uint8Array): { dex: string; ticker: string } | null {
  const s = Buffer.from(feedId).toString("latin1").replace(/\0+$/, "");
  const m = /^(?:([a-z]{2,6}):)?([A-Z0-9]{2,10})\/USD$/.exec(s);
  return m ? { dex: m[1] ?? "", ticker: m[2] } : null;
}
/** `xyz:GOLD`, or `GOLD` without a dex. */
export const fullName = (n: { dex: string; ticker: string }) => (n.dex ? n.dex + ":" : "") + n.ticker;

export function shape(d: Draft): { ok: Shape } | { bad: Problems } {
  const bad: Problems = {};
  const ticker = d.ticker.trim().toUpperCase();
  if (!/^[A-Z0-9]{2,10}$/.test(ticker)) bad.ticker = "2 to 10 letters or digits.";
  const dex = d.dex.trim();
  if (dex && !/^[a-z]{2,6}$/.test(dex)) bad.dex = "2 to 6 lower-case letters, or none.";
  const pxE8 = parseDec(d.price, 8);
  if (pxE8 == null) bad.price = "A number with up to 8 decimals.";
  else if (pxE8 < 100n) bad.price = "Too small: at least $0.000001.";
  else if (pxE8 >= 1n << 62n) bad.price = "Too large.";
  let cap: bigint | null = 0n;
  if (d.oiCap.trim() !== "") {
    cap = parseDec(d.oiCap, 2);
    if (cap == null || cap === 0n) bad.oiCap = "A dollar amount, or leave it empty for no cap.";
  }
  if (bad.dex || bad.ticker || bad.price || bad.oiCap || pxE8 == null || cap == null) return { bad };
  const [finer, auto, coarser] = tickChoices(pxE8);
  const pxDiv = (d.tickStep < 0 ? finer : d.tickStep > 0 ? coarser : auto) ?? auto;
  const units = pxE8 / pxDiv;
  // Notional of one lot at one price unit is one base unit of the collateral (1e-6 USDC), so lots = dollars x 1e6 / units.
  const maxOi = (cap * 10_000n) / units;
  if (cap !== 0n && maxOi === 0n) return { bad: { oiCap: "Smaller than one lot at this price." } };
  const f = FRESH[d.fresh] ?? FRESH[0];
  const tickUsd = Number(pxDiv) / 1e8;
  return {
    ok: {
      dex,
      ticker,
      feedId: feedIdFor(ticker, dex),
      pxE8,
      pxDiv,
      units,
      tickUsd,
      lot: 1e-6 / tickUsd,
      bucket: bucketFor(units),
      maxOi,
      maxAgeSecs: f.secs,
      maxStaleSlots: f.slots,
    },
  };
}

// ---------- instructions ----------

// (exported for lib/keeper.ts, which builds the instructions that keep a market sound the same way)
export const u64 = (v: bigint | number) => {
  const b = Buffer.alloc(8);
  b.writeBigUInt64LE(BigInt.asUintN(64, BigInt(v)));
  return b;
};
export const sgn = (k: PublicKey) => ({ pubkey: k, isSigner: true, isWritable: true });
export const wr = (k: PublicKey) => ({ pubkey: k, isSigner: false, isWritable: true });
export const ro = (k: PublicKey) => ({ pubkey: k, isSigner: false, isWritable: false });
export const ixOf = (keys: ReturnType<typeof wr>[], ...data: (Buffer | number[])[]) =>
  new TransactionInstruction({ programId: PID, keys, data: Buffer.concat(data.map((d) => Buffer.from(d))) });

const IX_AGG_INIT = 40;
const IX_AGG_POST = 41;
const IX_UPDATE_ORACLE = 4;
/** The feed carries 1e-8 USD. */
const EXPO = -8;
/** A price whose confidence interval is wider than this share of it is not used (max_conf_bps). */
export const MAX_CONF_BPS = 100;

/** The vault's owner: the market's own address, which only the program can sign for. */
export const vaultPda = (market: PublicKey) =>
  PublicKey.findProgramAddressSync([Buffer.from("vault"), market.toBuffer()], PID);

/** agg_init. accs: authority(s), feed(w). data: tag, feed id, expo i32, max jump per post in bps (0 = no limit here: the
 *  market's own guard holds any move over 5% until a later post confirms it, oracle.rs). */
export function feedInitIx(authority: PublicKey, feed: PublicKey, feedId: Buffer) {
  const expo = Buffer.alloc(4);
  expo.writeInt32LE(EXPO);
  return ixOf([sgn(authority), wr(feed)], [IX_AGG_INIT], feedId, expo, u64(0));
}
/** agg_post. accs: authority(s), feed(w). data: tag, price i64, conf u64, publish time i64, number of sources u64.
 *  The publish time must be newer than the last one and not ahead of the cluster's clock by more than 5 s. */
export function feedPostIx(authority: PublicKey, feed: PublicKey, pxE8: bigint, publishTime: number) {
  const conf = pxE8 / 2000n || 1n; // 5 bps: one source has no spread of its own to report
  return ixOf([sgn(authority), wr(feed)], [IX_AGG_POST], u64(pxE8), u64(conf), u64(publishTime), u64(1));
}
/** update_oracle: the market takes the feed's price. Anyone may send it. accs: market(w), feed. */
export const applyPriceIx = (market: PublicKey, feed: PublicKey) =>
  ixOf([wr(market), ro(feed)], [IX_UPDATE_ORACLE]);

/** init_market without a pool. accs: admin(s), market(w), insurance(w), vault token account, oracle.
 *  data: tag, vault bump, feed id, then u64s: bucket width, max staleness (slots), max conf (bps), max age (s), expo,
 *  px_mul, px_div, max open interest (lots). Margin starts at the program's defaults (10x: 10% initial, 5% maintenance). */
export function initMarketIx(
  admin: PublicKey,
  market: PublicKey,
  insurance: PublicKey,
  vault: PublicKey,
  feed: PublicKey,
  bump: number,
  sh: Shape,
) {
  return ixOf(
    [sgn(admin), wr(market), wr(insurance), ro(vault), ro(feed)],
    [S.IX_INIT_MARKET, bump],
    sh.feedId,
    u64(sh.bucket),
    u64(sh.maxStaleSlots),
    u64(MAX_CONF_BPS),
    u64(sh.maxAgeSecs),
    u64(EXPO),
    u64(1),
    u64(sh.pxDiv),
    u64(sh.maxOi),
  );
}
/** init_rank. accs: admin(s), market(w), rank(w). */
export const initRankIx = (admin: PublicKey, market: PublicKey, rank: PublicKey) =>
  ixOf([sgn(admin), wr(market), wr(rank)], [S.IX_INIT_RANK]);
/** fast_init: the market's one book for both sides. accs: admin(s), market(w), book(w). data: tag, the block of a maker's
 *  slab this market's quotes use (0: the market has its own makers, nothing is shared). */
export const initBookIx = (admin: PublicKey, market: PublicKey, book: PublicKey) =>
  ixOf([sgn(admin), wr(market), wr(book)], [S.IX_FAST_INIT], u64(0));
/** init_shard. accs: admin(s), market(w), shard(w). data: tag, side (0 bids, 1 asks), lowest price it holds. */
export const initShardIx = (admin: PublicKey, market: PublicKey, shard: PublicKey, side: 0 | 1, lo: bigint) =>
  ixOf([sgn(admin), wr(market), wr(shard)], [S.IX_INIT_SHARD, side], u64(lo));
/** SPL Token InitializeAccount3: the account's owner is in the data, no rent sysvar. accs: account(w), mint. */
const initTokenIx = (account: PublicKey, mint: PublicKey, owner: PublicKey) =>
  new TransactionInstruction({
    programId: TOKEN,
    keys: [wr(account), ro(mint)],
    data: Buffer.concat([Buffer.from([18]), owner.toBuffer()]),
  });

export type Keys = Record<
  "feed" | "rank" | "book" | "market" | "insurance" | "vault" | "bid" | "ask",
  Keypair
>;
export const newKeys = (): Keys => ({
  feed: Keypair.generate(),
  rank: Keypair.generate(),
  book: Keypair.generate(),
  market: Keypair.generate(),
  insurance: Keypair.generate(),
  vault: Keypair.generate(),
  bid: Keypair.generate(),
  ask: Keypair.generate(),
});

export type Step = {
  key: "feed" | "market" | "book";
  title: string;
  /** one line for the person signing */
  what: string;
  ixs: TransactionInstruction[];
  /** the new accounts' own keys; the wallet signs last */
  signers: Keypair[];
  /** compute units to ask for */
  units: number;
};

/** The three transactions that make a market. `publishTime` is the cluster's clock in seconds for the first price. */
export function steps(
  sh: Shape,
  payer: PublicKey,
  k: Keys,
  mint: PublicKey,
  publishTime: number,
  rent: (size: number) => number = rentOf,
): Step[] {
  const make = (kp: Keypair, part: Part, owner = PID) =>
    SystemProgram.createAccount({
      fromPubkey: payer,
      newAccountPubkey: kp.publicKey,
      lamports: rent(SIZES[part]),
      space: SIZES[part],
      programId: owner,
    });
  const [pda, bump] = vaultPda(k.market.publicKey);
  const m = k.market.publicKey;
  const f = k.feed.publicKey;
  return [
    {
      key: "feed",
      title: "Price feed",
      what: `A feed named ${fullName(sh)}/USD that only this key can post to.`,
      // The rank and the book are only allocated here and claimed in the next step: with them the next transaction
      // would not fit. An allocated account can be claimed by any market's admin for that market, so a stranger could
      // take one before the next step lands; that step then fails whole, and what is lost is this step's rent.
      ixs: [
        make(k.feed, "feed"),
        feedInitIx(payer, f, sh.feedId),
        make(k.rank, "rank"),
        make(k.book, "book"),
      ],
      signers: [k.feed, k.rank, k.book],
      units: 20_000,
    },
    {
      key: "market",
      title: "Market",
      what: `The market, its own vault and insurance, and the first price.`,
      ixs: [
        make(k.market, "market"),
        make(k.insurance, "insurance"),
        make(k.vault, "vault", TOKEN),
        initTokenIx(k.vault.publicKey, mint, pda),
        feedPostIx(payer, f, sh.pxE8, publishTime),
        initMarketIx(payer, m, k.insurance.publicKey, k.vault.publicKey, f, bump, sh),
        applyPriceIx(m, f),
        initRankIx(payer, m, k.rank.publicKey),
        initBookIx(payer, m, k.book.publicKey),
      ],
      signers: [k.market, k.insurance, k.vault],
      units: 80_000,
    },
    {
      key: "book",
      title: "Order book",
      what: `Room for resting orders: one account for bids, one for asks.`,
      ixs: [
        make(k.bid, "shard"),
        initShardIx(payer, m, k.bid.publicKey, 0, 0n),
        make(k.ask, "shard"),
        initShardIx(payer, m, k.ask.publicKey, 1, 0n),
      ],
      signers: [k.bid, k.ask],
      units: 20_000,
    },
  ];
}

/** One step as the transaction that is signed: the step's own compute limit, the app's usual priority fee, the new accounts'
 *  signatures already on it. The wallet signs last (it is the fee payer). The largest of the three is 1,079 of the 1,232
 *  bytes a transaction may be (scripts/test-create.ts measures it). */
export function txOf(payer: PublicKey, st: Step, blockhash: string, microLamports = 1000): Transaction {
  const t = new Transaction().add(
    ComputeBudgetProgram.setComputeUnitLimit({ units: st.units }),
    ComputeBudgetProgram.setComputeUnitPrice({ microLamports }),
    ...st.ixs,
  );
  t.feePayer = payer;
  t.recentBlockhash = blockhash;
  if (st.signers.length) t.partialSign(...st.signers);
  return t;
}

/** A new price for a market that exists: post it and have the market take it. The feed's key signs. */
export const postPriceIxs = (
  authority: PublicKey,
  market: PublicKey,
  feed: PublicKey,
  pxE8: bigint,
  publishTime: number,
) => [feedPostIx(authority, feed, pxE8, publishTime), applyPriceIx(market, feed)];

// ---------- a market's addresses ----------

export type Made = {
  dex: string;
  ticker: string;
  creator: string;
  market: string;
  feed: string;
  vault: string;
  vaultPda: string;
  insurance: string;
  rank: string;
  book: string;
  bid: string;
  ask: string;
  mint: string;
  pxDiv: number;
  at: number;
};
export const madeOf = (sh: Shape, payer: PublicKey, k: Keys, mint: PublicKey, at: number): Made => ({
  dex: sh.dex,
  ticker: sh.ticker,
  creator: payer.toBase58(),
  market: k.market.publicKey.toBase58(),
  feed: k.feed.publicKey.toBase58(),
  vault: k.vault.publicKey.toBase58(),
  vaultPda: vaultPda(k.market.publicKey)[0].toBase58(),
  insurance: k.insurance.publicKey.toBase58(),
  rank: k.rank.publicKey.toBase58(),
  book: k.book.publicKey.toBase58(),
  bid: k.bid.publicKey.toBase58(),
  ask: k.ask.publicKey.toBase58(),
  mint: mint.toBase58(),
  pxDiv: Number(sh.pxDiv),
  at,
});

// ---------- running a market ----------

const ATA_PROGRAM = new PublicKey(ATA_ID);
/** The owner's usual token account for a mint (the associated token account). */
export const ataOf = (owner: PublicKey, mint: PublicKey) =>
  PublicKey.findProgramAddressSync([owner.toBuffer(), TOKEN.toBuffer(), mint.toBuffer()], ATA_PROGRAM)[0];
/** Create that account if it is not there yet (the idempotent form: no error when it is). */
export const ensureAtaIx = (payer: PublicKey, owner: PublicKey, mint: PublicKey) =>
  new TransactionInstruction({
    programId: ATA_PROGRAM,
    keys: [sgn(payer), wr(ataOf(owner, mint)), ro(owner), ro(mint), ro(SystemProgram.programId), ro(TOKEN)],
    data: Buffer.from([1]),
  });

/** What an instruction on an existing market needs to know of it. */
export type Live = {
  market: PublicKey;
  insurance: PublicKey;
  vault: PublicKey;
  feed: PublicKey;
  mint: PublicKey;
};

/** ins_fund: pay collateral into the market's insurance. One way: it is the market's bond, taken first when a loss is
 *  larger than a position's margin. accs: payer(s), market, insurance(w), vault(w), src token(w), token program. */
export const bondIx = (payer: PublicKey, m: Live, src: PublicKey, amount: bigint) =>
  ixOf(
    [sgn(payer), ro(m.market), wr(m.insurance), wr(m.vault), wr(src), ro(TOKEN)],
    [S.IX_INS_FUND],
    u64(amount),
  );
/** fee_claim: half of the market's net fees to `dst`, half into its insurance. Only its admin, only without a pool.
 *  accs: admin(s), market(w), insurance(w), vault(w), dst token(w), token program, vault pda. */
export const feeClaimIx = (admin: PublicKey, m: Live, dst: PublicKey) =>
  ixOf(
    [sgn(admin), wr(m.market), wr(m.insurance), wr(m.vault), wr(dst), ro(TOKEN), ro(vaultPda(m.market)[0])],
    [S.IX_FEE_CLAIM],
  );
/** aggregate: fold the books' fee and open-interest counters into the market. Anyone may send it.
 *  accs: market(w), then books and shards(w). */
export const aggregateIx = (market: PublicKey, books: PublicKey[]) =>
  ixOf([wr(market), ...books.map(wr)], [S.IX_AGGREGATE]);
/** crank_claim: book the fills of resting orders that wait in one shard into their owners' accounts. Anyone may send
 *  it; nothing is moved but what those accounts are already owed. accs: shard(w), users(w)... */
export const crankClaimIx = (shard: PublicKey, users: PublicKey[]) =>
  ixOf([wr(shard), ...users.map(wr)], [S.IX_CRANK_CLAIM]);
/** The accounts a shard owes a fill to (the owners in its ledger), each once. */
export function owedBy(shard: Buffer): string[] {
  const s: any = decodeLayout("shard", shard);
  return [...new Set((s.ledger as any[]).slice(0, Number(s.nled)).map((l) => l.owner as string))];
}
/** The admin's immediate settings (admin.rs): pause 1, deposit cap 2, open-interest cap 3, position cap 6. */
export const ADMIN = {
  pause: S.ADMIN_PAUSE,
  depositCap: S.ADMIN_TVL_CAP,
  oiCap: S.ADMIN_MAX_OI,
  positionCap: S.ADMIN_MAX_POSITION,
} as const;
export const adminIx = (admin: PublicKey, market: PublicKey, op: number, value: bigint) =>
  ixOf([sgn(admin), wr(market)], [S.IX_ADMIN, op], u64(value));

/** The bond that covers an open-interest cap: enough for the whole of one side to lose one more full step of the
 *  price after its margin is gone. One post of a single feed may move the price ORACLE_SINGLE_SOURCE_MAX_MOVE_BPS (5%)
 *  without being held, and maintenance margin is that same 5%, so the step beyond it is 5% of the capped notional:
 *  a bond of 1/20 of the cap. `capLots` and `units` as the market has them; the result is in collateral base units. */
export const bondFor = (capLots: bigint, units: bigint) =>
  (capLots * units * BigInt(S.ORACLE_SINGLE_SOURCE_MAX_MOVE_BPS) + 9999n) / 10000n;
