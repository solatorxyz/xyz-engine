// Markets on their own: opened by anyone, without a pool (lib/createMarket.ts says how and why that is safe for
// everybody else). They are found on chain, not in a file: a market account whose pool is empty and whose feed id reads
// as a name. Used by the CLI (cli/solator.ts) and by the site's list (app/api/percolator), which shows them in a
// category of their own, called Percolator after the engine whose shape they have: one market, one vault.
import { Buffer } from "buffer";
import { Connection, PublicKey } from "@solana/web3.js";
import { decodeLayout, spec as S } from "./sdk/index";
import { bondFor, fullName, nameOf, vaultPda } from "./createMarket";

const ZERO = PublicKey.default.toBase58();
const POOL_OFF = S.LAYOUTS.market.find((f) => f[0] === "pool")![2];
const SHARD_MARKET_OFF = S.LAYOUTS.shard.find((f) => f[0] === "market")![2];
const SHARD_SIDE_OFF = S.LAYOUTS.shard.find((f) => f[0] === "side")![2];

export type Found = {
  /** xyz:GOLD, or GOLD */
  name: string;
  dex: string;
  ticker: string;
  market: string;
  /** its creator, until it is handed on */
  admin: string;
  mint: string;
  vault: string;
  vaultPda: string;
  insurance: string;
  feed: string;
  rank: string;
  book: string;
  shards: { key: string; side: 0 | 1 }[];
  /** 1 price unit = pxDiv / 1e8 USD */
  pxDiv: number;
  /** price units; 0 before the first price */
  px: number;
  /** when that price was published, unix seconds */
  pxTs: number;
  maxAgeSecs: number;
  imrBps: number;
  mmrBps: number;
  /** lots; 0 = no cap */
  maxOi: number;
  paused: number;
  /** collateral base units */
  fees: number;
  bond: number;
  /** the bond that covers the cap at this price (bondFor); 0 when there is no cap to cover */
  bondNeeded: number;
};
/** A market's bond covers it when there is a cap and the insurance holds at least a twentieth of it (bondFor). */
export const covered = (f: Pick<Found, "maxOi" | "bond" | "bondNeeded">) =>
  f.maxOi > 0 && f.bond >= f.bondNeeded;

/** A market account as a named market on its own, or null when it is pooled or its feed id is not a name. */
export function readMarket(
  market: string,
  data: Buffer,
): Omit<Found, "shards" | "bond" | "bondNeeded"> | null {
  const m: any = decodeLayout("market", data);
  if (m.pool !== ZERO) return null;
  const n = nameOf(new PublicKey(m.feed_id).toBuffer());
  if (!n) return null;
  return {
    name: fullName(n),
    ...n,
    market,
    admin: m.admin,
    mint: m.mint,
    vault: m.vault,
    vaultPda: vaultPda(new PublicKey(market))[0].toBase58(),
    insurance: m.insurance,
    feed: m.oracle,
    rank: m.adl_rank,
    book: m.fast_book[0],
    pxDiv: Number(m.px_div),
    px: Number(m.oracle_px),
    pxTs: Number(m.oracle_ts),
    maxAgeSecs: Number(m.max_age_secs),
    imrBps: Number(m.imr_bps),
    mmrBps: Number(m.mmr_bps),
    maxOi: Number(m.max_oi),
    paused: Number(m.paused),
    fees: Number(m.fees),
  };
}
async function finish(
  conn: Connection,
  pid: PublicKey,
  heads: NonNullable<ReturnType<typeof readMarket>>[],
): Promise<Found[]> {
  const ins: (Buffer | null)[] = [];
  for (let i = 0; i < heads.length; i += 100) {
    const part = await conn.getMultipleAccountsInfo(
      heads.slice(i, i + 100).map((h) => new PublicKey(h.insurance)),
      "confirmed",
    );
    ins.push(...part.map((a) => (a ? Buffer.from(a.data) : null)));
  }
  return Promise.all(
    heads.map(async (h, i) => {
      // only the eight bytes that say which side each shard is on
      const sh = await conn.getProgramAccounts(pid, {
        commitment: "confirmed",
        dataSlice: { offset: SHARD_SIDE_OFF, length: 8 },
        filters: [{ dataSize: S.SHARD_SIZE }, { memcmp: { offset: SHARD_MARKET_OFF, bytes: h.market } }],
      });
      const bond = ins[i] ? Number((decodeLayout("insurance", ins[i]!) as any).balance) : 0;
      return {
        ...h,
        shards: sh
          .map((x) => ({
            key: x.pubkey.toBase58(),
            side: (Buffer.from(x.account.data).readUInt32LE(0) ? 1 : 0) as 0 | 1,
          }))
          .sort((a, b) => a.side - b.side || (a.key < b.key ? -1 : 1)),
        bond,
        bondNeeded: Number(bondFor(BigInt(h.maxOi), BigInt(h.px))),
      };
    }),
  );
}
/** Every named market without a pool that the program owns. */
export async function discover(conn: Connection, programId: string): Promise<Found[]> {
  const pid = new PublicKey(programId);
  const accts = await conn.getProgramAccounts(pid, {
    commitment: "confirmed",
    filters: [{ dataSize: S.MARKET_SIZE }, { memcmp: { offset: POOL_OFF, bytes: ZERO } }],
  });
  const heads = accts
    .map((a) => readMarket(a.pubkey.toBase58(), Buffer.from(a.account.data)))
    .filter((x): x is NonNullable<typeof x> => !!x);
  return finish(conn, pid, heads);
}
/** One market by its address; null when it is not a named market on its own. */
export async function find(conn: Connection, programId: string, market: string): Promise<Found | null> {
  const a = await conn.getAccountInfo(new PublicKey(market), "confirmed");
  if (!a || a.owner.toBase58() !== programId || a.data.length !== S.MARKET_SIZE) return null;
  const h = readMarket(market, Buffer.from(a.data));
  return h ? (await finish(conn, new PublicKey(programId), [h]))[0] : null;
}
