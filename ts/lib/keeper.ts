// What a market on its own needs done when a position can no longer carry itself. Anyone may do it: no key of the
// market is involved, the program judges every account itself, and whoever sends the instruction decides nothing but
// that it is sent. In the order the program allows (program/src/liquidation.rs, adl.rs):
//
//   1. liquidation. An account worth less than its maintenance requirement has a quarter of its position at a time
//      sold into (or bought from) the orders resting in the book: inside 5% of the mark first, then 10%, then 20%.
//      Its own resting orders are cancelled first. The fee is 0.25% of what is closed (twice that per wider step);
//      a fifth of it goes to the trading account of whoever sent the instruction, the rest to the market's insurance.
//   2. auto-deleveraging, only for an account worth less than nothing: what is left of the position is closed at the
//      mark against the holders of the other side with the largest gain for their size, in the order of the market's
//      own queue. The loss is taken from the market's insurance first (its bond), then from those holders' gain on the
//      lots closed, and what still remains is written down as the market's bad debt.
//
// Between the two there is a gap that this file does not close: an account under maintenance but still worth something,
// with no order to fill against within 20% of the mark, waits. The program has a third way for it (IX 5: a liquidator
// with money of its own in the market takes the position over at the mark); it is left out here because it puts the
// sender's own money into the position.
//
// Nothing is decided here that the program does not decide again: the numbers below only choose which accounts are
// worth asking about. Every instruction is simulated by the caller first and sent only when the program accepts it.
import { Buffer } from "buffer";
import { Connection, PublicKey } from "@solana/web3.js";
import { decodeLayout, spec as S } from "./sdk/index";
import { ixOf, ro, u64, wr } from "./createMarket";

const ZERO = PublicKey.default.toBase58();
const HOME_OFF = S.LAYOUTS.user.find((f) => f[0] === "home_vault")![2];
const AUTH_OFF = S.LAYOUTS.user.find((f) => f[0] === "authority")![2];
const ALL = (1n << 64n) - 1n;
const abs = (x: bigint) => (x < 0n ? -x : x);

/** The addresses of a market that the instructions here name (lib/percolator.ts Found has them all). */
export type Places = {
  market: string;
  vault: string;
  insurance: string;
  rank: string;
  shards: { key: string; side: 0 | 1 }[];
};

/** What a market's account says an account in it is judged by. */
export type Mark = {
  /** the mark, price units (the oracle's price until a mark exists) */
  px: bigint;
  /** maintenance requirement in bps, with the oracle's band on top (margin.rs band_bps) */
  maintBps: bigint;
  /** cumulative funding per lot */
  funding: bigint;
  /** the book that fills a long's liquidation (makers' bids) and a short's (asks); zero address = none registered */
  books: [string, string];
  /** the market's insurance (its bond), collateral base units */
  insurance: bigint;
};
export function markOf(market: Buffer, insurance: Buffer | null): Mark {
  const m: any = decodeLayout("market", market);
  const band = m.oracle_px
    ? ((x: bigint) => (x < BigInt(S.BAND_MAX_BPS) ? x : BigInt(S.BAND_MAX_BPS)))(
        (m.oracle_conf * 10000n) / m.oracle_px,
      )
    : 0n;
  return {
    px: m.mark_px || m.oracle_px,
    maintBps: m.mmr_bps + band,
    funding: m.funding_index,
    books: [m.fast_book[0], m.fast_book[1]],
    insurance: insurance ? ((decodeLayout("insurance", insurance) as any).balance as bigint) : 0n,
  };
}

/** One trading account's position in a market, valued the way the program values it (margin.rs health, iso_health). */
export type Held = {
  account: string;
  /** lots; above zero = long */
  base: bigint;
  /** collateral base units: cash (or the position's own margin), funding owed taken out, the position at the mark */
  equity: bigint;
  /** what it must be worth to be left alone */
  maint: bigint;
  /** unrealized gain at the mark */
  gain: bigint;
  /** resting orders in this market (a liquidation cancels them first) */
  orders: boolean;
};
/** Null when the account holds nothing in that market. An account in a market on its own can hold no other market
 *  (its cash is a claim on this market's vault alone), so this one position is its whole health. */
export function heldBy(account: string, data: Buffer, market: string, k: Mark): Held | null {
  const u: any = decodeLayout("user", data);
  const p = (u.pos as any[]).find((x) => x.market === market);
  if (!p || p.base === 0n) return null;
  const own = (p.flags & BigInt(S.POS_FLAG_ISOLATED)) !== 0n;
  const value = p.base * k.px;
  return {
    account,
    base: p.base,
    equity: (own ? p.iso_margin : u.cash) - p.base * (k.funding - p.funding_snap) + value,
    maint: (abs(value) * k.maintBps + 9999n) / 10000n,
    gain: value - p.cost,
    orders: p.n_orders !== 0n || p.bid_open !== 0n || p.ask_open !== 0n,
  };
}

/** The market's state and every account holding a position in it, as of now. */
export async function watch(
  conn: Connection,
  programId: string,
  f: Places,
): Promise<{ k: Mark; held: Held[]; rank: Buffer | null }> {
  const pid = new PublicKey(programId);
  const [heads, users] = await Promise.all([
    conn.getMultipleAccountsInfo(
      [f.market, f.insurance, f.rank].map((x) => new PublicKey(x)),
      "confirmed",
    ),
    conn.getProgramAccounts(pid, {
      commitment: "confirmed",
      filters: [{ dataSize: S.USER_SIZE }, { memcmp: { offset: HOME_OFF, bytes: f.vault } }],
    }),
  ]);
  if (!heads[0]) throw new Error("the market's account is gone: " + f.market);
  const k = markOf(Buffer.from(heads[0].data), heads[1] ? Buffer.from(heads[1].data) : null);
  return {
    k,
    held: users
      .map((x) => heldBy(x.pubkey.toBase58(), Buffer.from(x.account.data), f.market, k))
      .filter((x): x is Held => !!x),
    rank: heads[2] ? Buffer.from(heads[2].data) : null,
  };
}
/** One account again, after something was done to it. */
export async function again(
  conn: Connection,
  f: Places,
  account: string,
): Promise<{ k: Mark; h: Held | null }> {
  const [m, i, u] = await conn.getMultipleAccountsInfo(
    [f.market, f.insurance, account].map((x) => new PublicKey(x)),
    "confirmed",
  );
  if (!m) throw new Error("the market's account is gone: " + f.market);
  const k = markOf(Buffer.from(m.data), i ? Buffer.from(i.data) : null);
  return { k, h: u ? heldBy(account, Buffer.from(u.data), f.market, k) : null };
}

/** The trading accounts a key has in this market's vault (the one a fifth of a liquidation's fee is paid to must be
 *  one of them: the program refuses an account that holds its cash anywhere else). */
export async function accountsOf(
  conn: Connection,
  programId: string,
  f: Places,
  owner: PublicKey,
): Promise<string[]> {
  const r = await conn.getProgramAccounts(new PublicKey(programId), {
    commitment: "confirmed",
    dataSlice: { offset: 0, length: 0 },
    filters: [
      { dataSize: S.USER_SIZE },
      { memcmp: { offset: AUTH_OFF, bytes: owner.toBase58() } },
      { memcmp: { offset: HOME_OFF, bytes: f.vault } },
    ],
  });
  return r.map((x) => x.pubkey.toBase58()).sort();
}

/** The makers' quote blocks of a fast book, in its own order: a liquidation through that book must name every one
 *  (fast_take.rs collect_slabs), so its sender cannot choose whose quotes fill. A market made by this repository's
 *  command line has a book with no makers, and its liquidity is the orders resting in its shards. */
export function makersOf(book: Buffer): string[] {
  const b: any = decodeLayout("fast_book", book);
  return (b.makers as any[]).slice(0, Math.min(Number(b.n), S.FAST_MAKERS)).map((m) => m.slab as string);
}

/** The liquidation of `h`, through whatever the market has registered for the side that fills it.
 *  With a fast book (liquidate_fast): accs account(w), market, insurance(w), book(w), makers' quote blocks, then every
 *  shard(w), then the sender's own account(w) if it wants its share. data: tag, number of quote blocks, most lots.
 *  Without one (liquidate_book): accs account(w), market, insurance(w), every shard(w), [own account(w)]. data: tag, most lots.
 *  `makers` = makersOf(that book). Both shards of each side are named: the account's own orders are cancelled wherever they rest. */
export function liquidateIx(f: Places, k: Mark, h: Held, makers: string[], earn?: string) {
  const book = k.books[h.base > 0n ? 0 : 1];
  const tail = [...f.shards.map((s) => wr(new PublicKey(s.key))), ...(earn ? [wr(new PublicKey(earn))] : [])];
  const head = [wr(new PublicKey(h.account)), ro(new PublicKey(f.market)), wr(new PublicKey(f.insurance))];
  if (book === ZERO) return ixOf([...head, ...tail], [S.IX_LIQUIDATE_BOOK], u64(ALL));
  return ixOf(
    [...head, wr(new PublicKey(book)), ...makers.map((m) => ro(new PublicKey(m))), ...tail],
    [S.IX_LIQ_FAST, makers.length],
    u64(ALL),
  );
}

/** adl_score (adl.rs): gain over equity times size over equity, both in 1e-4. Null = cannot be closed against. */
export function score(h: Held, k: Mark): bigint | null {
  if (h.base === 0n || h.equity <= 0n || h.gain <= 0n) return null;
  return (((h.gain * 10000n) / h.equity) * ((abs(h.base * k.px) * 10000n) / h.equity)) / 10000n;
}
/** Who a bankrupt position may be closed against, to be named in the instruction. The program does the choosing: it
 *  scores every account named here again and takes the head of its queue. So the queue's present entries are all named
 *  (it refuses to pass over one it could not score), then the best of the other side by the same score; eight in all,
 *  the queue's length. */
export function counterparties(rank: Buffer | null, held: Held[], k: Mark, target: Held): string[] {
  const long = target.base < 0n; // the other side of a short is the longs
  let queued: string[] = [];
  if (rank) {
    const r: any = decodeLayout("rank", rank);
    const n = Math.min(Number(long ? r.n_long : r.n_short), S.RANK_N);
    const o = long ? 0 : S.RANK_N;
    queued = (r.entries as any[]).slice(o, o + n).map((e) => e.user as string);
  }
  const rest = held
    .filter((h) => h.account !== target.account && h.base > 0n === long)
    .map((h) => [h.account, score(h, k)] as const)
    .filter((x): x is readonly [string, bigint] => x[1] !== null)
    .sort((a, b) => (a[1] < b[1] ? 1 : a[1] > b[1] ? -1 : 0))
    .map((x) => x[0]);
  return [...new Set([...queued, ...rest])].filter((a) => a !== target.account).slice(0, S.RANK_N);
}
/** backstop (auto-deleveraging). accs: account(w), market(w), insurance(w), rank(w), the accounts it may be closed
 *  against(w), then every shard(w) (the bankrupt account's own orders are cancelled there). data: tag. */
export const deleverageIx = (f: Places, account: string, against: string[]) =>
  ixOf(
    [
      wr(new PublicKey(account)),
      wr(new PublicKey(f.market)),
      wr(new PublicKey(f.insurance)),
      wr(new PublicKey(f.rank)),
      ...against.map((a) => wr(new PublicKey(a))),
      ...f.shards.map((s) => wr(new PublicKey(s.key))),
    ],
    [S.IX_BACKSTOP],
  );

/** The program's answers that are not failures of the sender (error.rs), by what they mean for the account asked
 *  about. Whether the price is fresh enough to judge anyone by is one of them: it is the program's clock that counts. */
export const ANSWER = {
  stale: S.ERR_STALE,
  healthy: S.ERR_NOT_LIQUIDATABLE,
  nothingToFill: S.ERR_NO_FILL,
  deleverage: S.ERR_USE_ADL,
  notBankrupt: S.ERR_NOT_BANKRUPT,
  nobodyAgainst: S.ERR_RANK,
} as const;
