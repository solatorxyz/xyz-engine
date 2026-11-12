// solator: open and run a market from a terminal.
//
// There is no screen for this, on purpose. A market's creator is its price source and its operator; what they do is
// send signed instructions, on a schedule, from a machine that stays on. This is the script for it (Hyperliquid's
// builder-deployed markets work the same way: signed actions from an SDK, no form).
//
//   npm run solator -- market create GOLD --dex xyz --price 2650.5 --cap 250000
//   npm run solator -- bond add <market> 12500
//   npm run solator -- price run <market> --from url:https://example.com/gold.json --json price
//   npm run solator -- market show <market>
//   npm run solator -- liquidate run <market>        (anyone, on anyone's market)
//   npm run solator -- help
//
// Every command takes --url <rpc> (default: devnet) and --keypair <file> (default: ~/.config/solana/id.json). Money
// amounts are dollars of the market's collateral; prices are dollars. Another deployment of the program, or another
// collateral, is named in the environment: SOLATOR_PROGRAM, SOLATOR_MINT (lib/ids.ts).
import { readFileSync, writeFileSync, mkdirSync } from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";
import { execSync } from "node:child_process";
import { Buffer } from "buffer";
import {
  ComputeBudgetProgram,
  Connection,
  Keypair,
  PublicKey,
  Transaction,
  TransactionInstruction,
  VersionedTransaction,
} from "@solana/web3.js";
import { MINT, PROGRAM_ID } from "../lib/ids";
import { spec as S } from "../lib/sdk/index";
import {
  ADMIN,
  DRAFT0,
  FRESH,
  PARTS,
  SIZES,
  adminIx,
  aggregateIx,
  ataOf,
  bondFor,
  bondIx,
  crankClaimIx,
  owedBy,
  ensureAtaIx,
  feeClaimIx,
  madeOf,
  newKeys,
  parseDec,
  postPriceIxs,
  shape,
  steps,
  txOf,
  type Live,
} from "../lib/createMarket";
import { covered, discover, find, type Found } from "../lib/percolator";
import {
  ANSWER,
  accountsOf,
  again,
  counterparties,
  deleverageIx,
  liquidateIx,
  makersOf,
  watch,
  type Held,
  type Mark,
} from "../lib/keeper";

const HELP = `solator: open and run a market

  market create <TICKER> --price <usd> [--dex <abc>] [--cap <usd>] [--fresh 1m|10m|1h] [--tick finer|coarser]
                         [--mint <address>] [--bond <usd>]
      Three transactions: the price feed, the market with its own vault and insurance, the order book.
      Writes the market's addresses to ./solator-markets/<name>-<market>.json (--out <dir> for another place).
  market show <market>          its settings, price, bond and fees as the chain has them
  market list [--mine]          every named market without a pool
  market pause <market>         no new orders or deposits (closing and withdrawing keep working)
  market resume <market>
  market cap <market> <usd>     open-interest cap at the current price (0 = none)

  price post <market> <usd>     one price
  price run <market> --from url:<http url> [--json a.b.c] | --from cmd:"<shell command>"
            [--min-move <bps>] [--heartbeat <seconds>] [--every <seconds>]
      Keeps the market running: reads the source every --every seconds (default 3) and posts when the price has moved
      --min-move bps (default 1) or --heartbeat seconds have passed (default: a third of how long a price is good).
      Each turn it also books filled resting orders into their owners' accounts and liquidates what has fallen
      under its maintenance margin, as "liquidate run" does (--earn works here too).

  liquidate run <market> [--earn [<account>]] [--every <seconds>] [--once]
      For anyone, on anyone's market; no key of the market is needed. Every --every seconds (default 3): an account
      under maintenance is sold into the book a quarter at a time; one worth less than nothing is closed against
      the other side's largest gains, its loss paid from the market's bond first. The program judges each account
      itself; this only asks. With --earn a fifth of each liquidation fee is paid to your trading account in that
      market (the one your first deposit there opened; name it if you have several).

  bond add <market> <usd>       pay into the market's insurance (one way; first money taken by a loss)
  fees claim <market>           half of the market's net trading fees to you, half into its insurance

  --url <rpc>  --keypair <file>`;

// ---------- arguments ----------
const argv = process.argv.slice(2);
const flags: Record<string, string> = {};
const words: string[] = [];
for (let i = 0; i < argv.length; i++) {
  const a = argv[i];
  if (a.startsWith("--")) {
    const next = argv[i + 1];
    if (next === undefined || next.startsWith("--")) flags[a.slice(2)] = "1";
    else flags[a.slice(2)] = argv[++i];
  } else words.push(a);
}
const die = (m: string): never => {
  console.error(m);
  process.exit(1);
};
const need = (v: string | undefined, what: string): string => v ?? die(`missing: ${what}\n\n${HELP}`);
const PID = PROGRAM_ID;
const conn = new Connection(flags.url ?? "https://api.devnet.solana.com", "confirmed");
let _me: Keypair | null = null;
const me = (): Keypair => {
  if (_me) return _me;
  const f = flags.keypair ?? join(homedir(), ".config", "solana", "id.json");
  try {
    return (_me = Keypair.fromSecretKey(Uint8Array.from(JSON.parse(readFileSync(f, "utf8")))));
  } catch {
    return die(`no keypair at ${f} (--keypair <file>; make one with: solana-keygen new)`);
  }
};

// ---------- sending ----------
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
/** The cluster's clock: a price stamped ahead of it is refused, and a machine's own clock can be off. */
async function chainTime(): Promise<number> {
  try {
    const t = await conn.getBlockTime(await conn.getSlot("confirmed"));
    if (t) return t;
  } catch {}
  return Math.floor(Date.now() / 1000) - 2;
}
/** Sign once, then send those same bytes until the cluster has them or their blockhash is too old. A transaction is
 *  never built twice for one intent: sending the same signed bytes again cannot pay twice. */
async function land(tx: Transaction, lastValid: number, what: string): Promise<string> {
  const raw = tx.serialize();
  const sig = await conn.sendRawTransaction(raw, { skipPreflight: true, maxRetries: 0 });
  for (let i = 0; ; i++) {
    await sleep(500);
    const s = (await conn.getSignatureStatuses([sig]).catch(() => null))?.value?.[0];
    if (s?.err) {
      const c = (s.err as any)?.InstructionError?.[1]?.Custom;
      throw new Error(
        `${what}: refused${typeof c === "number" ? ` (program code ${c})` : ""} ${JSON.stringify(s.err)}\n  ${sig}`,
      );
    }
    if (s && (s.confirmationStatus === "confirmed" || s.confirmationStatus === "finalized")) return sig;
    if (i % 4 === 3) {
      if ((await conn.getBlockHeight("confirmed").catch(() => 0)) > lastValid)
        throw new Error(
          `${what}: not confirmed before its blockhash expired. Look it up before sending again:\n  ${sig}`,
        );
      await conn.sendRawTransaction(raw, { skipPreflight: true, maxRetries: 0 }).catch(() => {});
    }
  }
}
async function signed(ixs: TransactionInstruction[], units: number) {
  const { blockhash, lastValidBlockHeight } = await conn.getLatestBlockhash("confirmed");
  const t = new Transaction().add(
    ComputeBudgetProgram.setComputeUnitLimit({ units }),
    ComputeBudgetProgram.setComputeUnitPrice({ microLamports: 1000 }),
    ...ixs,
  );
  t.feePayer = me().publicKey;
  t.recentBlockhash = blockhash;
  t.sign(me());
  return { t, lastValidBlockHeight };
}
async function send(ixs: TransactionInstruction[], what: string, units = 60_000): Promise<string> {
  const { t, lastValidBlockHeight } = await signed(ixs, units);
  return land(t, lastValidBlockHeight, what);
}
/** Ask first, send only what the program accepts: the transaction is signed once, the cluster is asked what the
 *  program would answer to exactly those bytes, and they are sent when the answer is yes. A no costs nothing and comes
 *  back as the program's code (-1 when the refusal is not one of the program's own). */
async function ask(
  ixs: TransactionInstruction[],
  what: string,
  units = 400_000,
): Promise<{ sig: string } | { code: number; why: string }> {
  const { t, lastValidBlockHeight } = await signed(ixs, units);
  const r = await conn.simulateTransaction(VersionedTransaction.deserialize(t.serialize()), {
    sigVerify: true,
    commitment: "confirmed",
  });
  if (r.value.err) {
    const c = (r.value.err as any)?.InstructionError?.[1]?.Custom;
    return { code: typeof c === "number" ? c : -1, why: JSON.stringify(r.value.err) };
  }
  return { sig: await land(t, lastValidBlockHeight, what) };
}

// ---------- reading ----------
const usd = (base: number, dp = 2) =>
  (base < 0 ? "-$" : "$") +
  (Math.abs(base) / 1e6).toLocaleString("en-US", { minimumFractionDigits: dp, maximumFractionDigits: dp });
const priceOf = (f: Found) => (f.px * f.pxDiv) / 1e8;
const dpOf = (f: Found) => Math.max(0, 8 - (String(f.pxDiv).length - 1));
/** the open-interest cap at the current price, in collateral base units (a lot at one price unit is one base unit) */
const capBase = (f: Found) => f.maxOi * f.px;
async function market(addr: string): Promise<Found> {
  let k: PublicKey;
  try {
    k = new PublicKey(addr);
  } catch {
    return die(`not an address: ${addr}`);
  }
  return (
    (await find(conn, PID, k.toBase58())) ??
    die(`${addr} is not a named market without a pool of program ${PID}`)
  );
}
const live = (f: Found): Live => ({
  market: new PublicKey(f.market),
  insurance: new PublicKey(f.insurance),
  vault: new PublicKey(f.vault),
  feed: new PublicKey(f.feed),
  mint: new PublicKey(f.mint),
});
const mine = (f: Found) =>
  f.admin === me().publicKey.toBase58() ||
  die(`${f.name} belongs to ${f.admin}, not to this key (${me().publicKey.toBase58()})`);
function show(f: Found, now: number) {
  const age = f.pxTs ? now - f.pxTs : null;
  const state =
    f.paused === S.PAUSED_ADMIN
      ? "paused"
      : f.paused
        ? "holding a large move"
        : age == null
          ? "no price yet"
          : age > f.maxAgeSecs
            ? "waiting for a price"
            : "live";
  console.log(`${f.name}-USDC   ${f.market}`);
  console.log(`  creator        ${f.admin}`);
  console.log(
    `  price          ${f.px ? "$" + priceOf(f).toFixed(dpOf(f)) : "--"}${age != null ? `   posted ${age} s ago` : ""}   (${state})`,
  );
  console.log(`  good for       ${f.maxAgeSecs} s`);
  console.log(
    `  margin         ${f.imrBps / 100}% to open, ${f.mmrBps / 100}% to keep   (${(10000 / f.imrBps).toFixed(0)}x)`,
  );
  console.log(`  open interest  ${f.maxOi ? "up to " + usd(capBase(f), 0) : "no cap"}`);
  console.log(
    `  bond           ${usd(f.bond)}   ${f.maxOi ? (covered(f) ? "covers the cap" : `the cap needs ${usd(f.bondNeeded)}`) : "nothing covers an open cap"}`,
  );
  console.log(`  fees waiting   ${usd(Math.max(0, f.fees))}`);
  console.log(`  feed           ${f.feed}`);
  console.log(`  vault          ${f.vault}`);
  console.log(
    `  book           ${f.book}   shards ${f.shards.map((s) => (s.side ? "ask " : "bid ") + s.key).join("  ")}`,
  );
}

// ---------- a market's chores: what anyone may do for it, and somebody has to ----------
const clock = () => new Date().toISOString().slice(11, 19);
const short = (a: string) => a.slice(0, 6) + "…";
const abs = (x: bigint) => (x < 0n ? -x : x);
/** A resting order that was filled is booked into its owner's account when that account is next touched. Here it is
 *  touched for them, so a maker sees the position without doing anything, and a liquidation sees every position. */
async function settle(f: Found) {
  const mk = new PublicKey(f.market);
  const keys = f.shards.map((s) => new PublicKey(s.key));
  const bufs = await conn.getMultipleAccountsInfo(keys, "confirmed");
  for (const [i, b] of bufs.entries()) {
    const owed = b ? owedBy(Buffer.from(b.data)) : [];
    for (let j = 0; j < owed.length; j += 8) {
      const users = owed.slice(j, j + 8).map((k) => new PublicKey(k));
      await send(
        [crankClaimIx(keys[i], users), aggregateIx(mk, [new PublicKey(f.book), ...keys])],
        "settle",
        120_000,
      );
      console.log(`${clock()}  settled ${users.length} filled resting order account(s)`);
    }
  }
}
/** What was last said about an account (or about the price), so that a state that lasts is said once. */
const said = new Map<string, string>();
const say = (about: string, line: string) => {
  if (said.get(about) === line) return;
  said.set(about, line);
  console.log(`${clock()}  ${line}`);
};
/** One pass over the market's positions (lib/keeper.ts says what is done and in which order). The program is asked
 *  about every account that looks under its maintenance margin and decides; what it accepts is sent. */
async function sweep(f: Found, earn: string | undefined) {
  const { k, held, rank } = await watch(conn, PID, f);
  const low = held
    .filter((h) => h.equity < h.maint)
    .sort((a, b) => (a.equity - a.maint < b.equity - b.maint ? -1 : 1));
  for (const a of [...said.keys()]) if (a !== "price" && !low.some((h) => h.account === a)) said.delete(a);
  const makers = new Map<string, string[]>();
  for (const first of low) {
    let h: Held = first;
    let kk: Mark = k;
    // a quarter of a position at a time: eight asks are more than one whole
    for (let i = 0; i < 8; i++) {
      const book = kk.books[h.base > 0n ? 0 : 1];
      if (book !== PublicKey.default.toBase58() && !makers.has(book)) {
        const b = await conn.getAccountInfo(new PublicKey(book), "confirmed");
        makers.set(book, b ? makersOf(Buffer.from(b.data)) : []);
      }
      const who = `${short(h.account)} (${h.base > 0n ? "long" : "short"} ${usd(Number(abs(h.base) * kk.px))}, worth ${usd(Number(h.equity))}, must be ${usd(Number(h.maint))})`;
      const r = await ask(
        [liquidateIx(f, kk, h, makers.get(book) ?? [], earn === h.account ? undefined : earn)],
        "liquidate",
      );
      if ("code" in r && r.code === ANSWER.stale)
        return say(
          "price",
          "the price is too old to judge anyone by: nothing is liquidated until it is fresh",
        );
      said.delete("price");
      let did: { what: string; sig: string } | null = null;
      if ("sig" in r) did = { what: "liquidated into the book", sig: r.sig };
      else if (r.code === ANSWER.healthy)
        break; // by the program's own count it carries itself (a fill was booked, or the price moved since the reading)
      else if (r.code === ANSWER.deleverage || (r.code === ANSWER.nothingToFill && h.equity < 0n)) {
        const against = counterparties(rank, held, kk, h);
        const d = against.length
          ? await ask([deleverageIx(f, h.account, against)], "deleverage", 600_000)
          : { code: ANSWER.nobodyAgainst, why: "" };
        if ("sig" in d) did = { what: "closed against the other side at the mark", sig: d.sig };
        else {
          say(
            h.account,
            d.code === ANSWER.notBankrupt
              ? `${who}: no order in the book closes it without a loss larger than the bond; it waits`
              : d.code === ANSWER.nobodyAgainst
                ? `${who}: worth less than nothing, and nobody on the other side has a gain to close it against; it waits`
                : `${who}: closing it against the other side was refused ${d.why}`,
          );
          break;
        }
      } else {
        say(
          h.account,
          r.code === ANSWER.nothingToFill
            ? `${who}: no order within 20% of the mark to fill against; it waits`
            : `${who}: its liquidation was refused ${r.why}`,
        );
        break;
      }
      said.delete(h.account);
      const now = await again(conn, f, h.account);
      console.log(
        `${clock()}  ${did.what}: ${who} -> ${now.h ? `${usd(Number(abs(now.h.base) * now.k.px))} left, worth ${usd(Number(now.h.equity))}` : "nothing left"}; bond ${usd(Number(now.k.insurance))}  ${did.sig.slice(0, 12)}…`,
      );
      if (!now.h || now.h.equity >= now.h.maint) break;
      h = now.h;
      kk = now.k;
    }
  }
}
/** Which of this key's trading accounts in the market's vault is paid the sender's fifth of a liquidation fee. */
async function earner(f: Found): Promise<string | undefined> {
  if (!flags.earn) return undefined;
  const have = await accountsOf(conn, PID, f, me().publicKey);
  if (flags.earn !== "1")
    return have.includes(flags.earn)
      ? flags.earn
      : die(`${flags.earn} is not a trading account of this key in ${f.name}`);
  if (!have.length)
    return die(
      `this key has no trading account in ${f.name} yet. Deposit there once (the site's Percolator tab opens one), then use --earn.`,
    );
  if (have.length > 1)
    return die(
      `this key has ${have.length} trading accounts in ${f.name}; name one: --earn <account>\n  ${have.join("\n  ")}`,
    );
  return have[0];
}
async function chores(f: Found, earn: string | undefined) {
  for (const [what, chore] of [
    ["settle", () => settle(f)],
    ["liquidate", () => sweep(f, earn)],
  ] as const)
    try {
      await chore();
    } catch (e: any) {
      // said and tried again at the next turn: one failed reading must not stop the loop that keeps the market sound
      console.error(`${clock()}  ${what}: ${String(e?.message ?? e).split("\n")[0]}`);
    }
}
async function liquidate(addr: string) {
  const f = await market(addr);
  const earn = await earner(f);
  const every = Math.max(1, Number(flags.every ?? 3));
  console.log(
    `${f.name}: looking every ${every} s for accounts under their maintenance margin${earn ? `; a fifth of each liquidation fee to ${earn}` : ""}. Ctrl-C stops it.`,
  );
  for (;;) {
    const t0 = Date.now();
    await chores(f, earn);
    if (flags.once) return;
    await sleep(Math.max(0, every * 1000 - (Date.now() - t0)));
  }
}

// ---------- commands ----------
async function create() {
  const ticker = need(words[2], "the ticker");
  const fresh = { "1m": 0, "10m": 1, "1h": 2 }[flags.fresh ?? "1m"] ?? die("--fresh is 1m, 10m or 1h");
  const tickStep =
    flags.tick === "finer"
      ? -1
      : flags.tick === "coarser"
        ? 1
        : flags.tick
          ? die("--tick is finer or coarser")
          : 0;
  const r = shape({
    ...DRAFT0,
    dex: flags.dex ?? "",
    ticker,
    price: need(flags.price, "--price"),
    oiCap: flags.cap ?? "",
    fresh,
    tickStep,
  });
  if ("bad" in r)
    return die(
      Object.entries(r.bad)
        .map(([k, v]) => `${k}: ${v}`)
        .join("\n"),
    );
  const sh = r.ok;
  const bond = flags.bond ? (parseDec(flags.bond, 6) ?? die("--bond is a dollar amount")) : 0n;
  const payer = me().publicKey;
  const mint = new PublicKey(flags.mint ?? MINT);
  const sizes = [...new Set(Object.values(SIZES))];
  const rents = Object.fromEntries(
    await Promise.all(sizes.map(async (s) => [s, await conn.getMinimumBalanceForRentExemption(s)] as const)),
  );
  const rent = (size: number) => rents[size];
  const total = PARTS.reduce((a, [p, n]) => a + n * rent(SIZES[p]), 0);
  const bal = await conn.getBalance(payer);
  const name = (sh.dex ? sh.dex + ":" : "") + sh.ticker;
  console.log(
    `${name}-USDC at $${(Number(sh.units) * sh.tickUsd).toFixed(Math.max(0, Math.round(-Math.log10(sh.tickUsd))))}`,
  );
  console.log(
    `  price step $${sh.tickUsd}, smallest order ${sh.lot} ${sh.ticker}, a price is good for ${FRESH[fresh].label}`,
  );
  console.log(
    `  open interest ${sh.maxOi ? `up to ${flags.cap} dollars; the bond that covers it: ${usd(Number(bondFor(sh.maxOi, sh.units)))}` : "no cap (nothing can cover it)"}`,
  );
  console.log(
    `  rent ${(total / 1e9).toFixed(4)} SOL on 8 accounts (not returned); this key has ${(bal / 1e9).toFixed(4)} SOL`,
  );
  if (bal < total + 100_000) return die("not enough SOL");
  const k = newKeys();
  for (let i = 0; i < 3; i++) {
    const st = steps(sh, payer, k, mint, await chainTime(), rent)[i];
    const { blockhash, lastValidBlockHeight } = await conn.getLatestBlockhash("confirmed");
    const t = txOf(payer, st, blockhash);
    t.partialSign(me());
    console.log(`  ${i + 1}/3 ${st.title}  ${await land(t, lastValidBlockHeight, st.title)}`);
  }
  const made = madeOf(sh, payer, k, mint, Date.now());
  const out = flags.out ?? "solator-markets";
  mkdirSync(out, { recursive: true });
  const file = join(out, `${name.replace(":", "-")}-${made.market}.json`);
  writeFileSync(file, JSON.stringify(made, null, 2) + "\n");
  console.log(`market ${made.market}\n  written to ${file}`);
  if (bond > 0n) await bondAdd(made.market, bond);
  console.log(
    `next: keep its price fresh (price run ${made.market} --from ...). It stops taking orders ${FRESH[fresh].label} after the last price.`,
  );
}
async function bondAdd(addr: string, amount: bigint) {
  const f = await market(addr);
  if (amount <= 0n) return die("the amount is a dollar amount above zero");
  const src = flags.from ? new PublicKey(flags.from) : ataOf(me().publicKey, new PublicKey(f.mint));
  const sig = await send([bondIx(me().publicKey, live(f), src, amount)], "bond");
  const g = await market(addr);
  console.log(
    `bond ${usd(g.bond)} (+${usd(Number(amount))})  ${g.maxOi ? (covered(g) ? "covers the cap" : `the cap needs ${usd(g.bondNeeded)}`) : ""}\n  ${sig}`,
  );
}
async function post(addr: string, price: string) {
  const f = await market(addr);
  mine(f);
  const px = parseDec(price, 8) ?? die("the price is a dollar amount with up to 8 decimals");
  if (px <= 0n) return die("the price is above zero");
  const sig = await send(
    postPriceIxs(me().publicKey, new PublicKey(f.market), new PublicKey(f.feed), px, await chainTime()),
    "price",
  );
  const g = await market(addr);
  const took = BigInt(g.px) === px / BigInt(g.pxDiv);
  console.log(
    `${g.name} $${priceOf(g).toFixed(dpOf(g))}${took ? "" : "  (held: a move over 5% is taken when the next post confirms it; post it again)"}\n  ${sig}`,
  );
}
/** One reading of the creator's source: a number of dollars. */
function reader(): () => Promise<number> {
  const from = need(flags.from, '--from url:<http url> or --from cmd:"<command>"');
  if (from.startsWith("cmd:"))
    return async () => Number(execSync(from.slice(4), { encoding: "utf8", timeout: 10_000 }).trim());
  if (from.startsWith("url:")) {
    const path = (flags.json ?? "").split(".").filter(Boolean);
    return async () => {
      const r = await fetch(from.slice(4), { signal: AbortSignal.timeout(10_000) });
      if (!r.ok) throw new Error("source answered " + r.status);
      let v: any = path.length ? await r.json() : (await r.text()).trim();
      for (const p of path) v = v?.[p];
      return Number(v);
    };
  }
  return die('--from is url:<http url> or cmd:"<command>"');
}
async function run(addr: string) {
  const f = await market(addr);
  mine(f);
  const read = reader();
  const every = Math.max(1, Number(flags.every ?? 3));
  const minMove = Number(flags["min-move"] ?? 1);
  const beat = Number(flags.heartbeat ?? Math.floor(f.maxAgeSecs / 3));
  if (!(beat > 0) || beat >= f.maxAgeSecs)
    return die(`--heartbeat must be under ${f.maxAgeSecs} s, how long this market's price is good`);
  const feed = new PublicKey(f.feed);
  const mk = new PublicKey(f.market);
  const earn = await earner(f);
  console.log(
    `${f.name}: reading every ${every} s, posting on a move of ${minMove} bp or every ${beat} s. Ctrl-C stops it.`,
  );
  let last = 0;
  let lastAt = 0;
  let lastStamp = 0;
  let skew = (await chainTime()) - Math.floor(Date.now() / 1000);
  for (let n = 0; ; n++) {
    const t0 = Date.now();
    try {
      const p = await read();
      if (!(p > 0) || !Number.isFinite(p)) throw new Error("the source did not give a price: " + p);
      const moved = last ? (Math.abs(p - last) / last) * 10000 : Infinity;
      if (moved >= minMove || t0 / 1000 - lastAt >= beat) {
        if (n % 20 === 0) skew = (await chainTime()) - Math.floor(Date.now() / 1000);
        // a newer stamp than the last one, never ahead of the cluster
        const stamp = Math.max(Math.floor(Date.now() / 1000) + skew, lastStamp + 1);
        const px = parseDec(p.toFixed(8), 8)!;
        const sig = await send(postPriceIxs(me().publicKey, mk, feed, px, stamp), "price");
        lastStamp = stamp;
        last = p;
        lastAt = Date.now() / 1000;
        console.log(`${clock()}  $${p}  ${sig.slice(0, 12)}…`);
      }
    } catch (e: any) {
      // a missed reading or a refused post is said and tried again at the next turn: the loop is what keeps the market open
      console.error(`${clock()}  ${String(e?.message ?? e).split("\n")[0]}`);
    }
    await chores(f, earn);
    if (flags.once) return;
    await sleep(Math.max(0, every * 1000 - (Date.now() - t0)));
  }
}
async function claim(addr: string) {
  const f = await market(addr);
  mine(f);
  const who = me().publicKey;
  const mint = new PublicKey(f.mint);
  // the books hold the fees until they are folded into the market; do that first, in the same transaction
  const books = [f.book, ...f.shards.map((s) => s.key)]
    .filter((k) => k !== PublicKey.default.toBase58())
    .map((k) => new PublicKey(k));
  const dst = flags.to ? new PublicKey(flags.to) : ataOf(who, mint);
  const before = (await conn.getTokenAccountBalance(dst).catch(() => null))?.value?.amount ?? "0";
  const sig = await send(
    [
      aggregateIx(new PublicKey(f.market), books),
      ...(flags.to ? [] : [ensureAtaIx(who, who, mint)]),
      feeClaimIx(who, live(f), dst),
    ],
    "fees",
    120_000,
  );
  const after = (await conn.getTokenAccountBalance(dst)).value.amount;
  const g = await market(addr);
  console.log(
    `${g.name}: ${usd(Number(BigInt(after) - BigInt(before)))} to ${dst.toBase58()}, the same into the bond (now ${usd(g.bond)})\n  ${sig}`,
  );
}
async function admin(addr: string, op: number, value: bigint, what: string) {
  const f = await market(addr);
  mine(f);
  const sig = await send([adminIx(me().publicKey, new PublicKey(f.market), op, value)], what);
  show(await market(addr), await chainTime());
  console.log(`  ${sig}`);
}

async function main() {
  const [a, b] = words;
  const cmd = `${a ?? ""} ${b ?? ""}`.trim();
  if (!a || a === "help" || flags.help) return console.log(HELP);
  if (cmd === "market create") return create();
  if (cmd === "market show")
    return show(await market(need(words[2], "the market's address")), await chainTime());
  if (cmd === "market list") {
    const all = (await discover(conn, PID)).filter(
      (f) => !flags.mine || f.admin === me().publicKey.toBase58(),
    );
    const now = await chainTime();
    if (!all.length) return console.log("no named market without a pool yet");
    for (const f of all.sort((x, y) => y.bond - x.bond)) {
      const age = f.pxTs ? now - f.pxTs : Infinity;
      console.log(
        [
          f.name.padEnd(18),
          (f.px ? "$" + priceOf(f).toFixed(dpOf(f)) : "--").padStart(14),
          (age > f.maxAgeSecs ? "waiting" : f.paused ? "paused" : "live").padEnd(8),
          ("bond " + usd(f.bond, 0)).padEnd(16),
          (f.maxOi
            ? "cap " + usd(capBase(f), 0) + (covered(f) ? " covered" : " not covered")
            : "no cap"
          ).padEnd(30),
          f.market,
        ].join(" "),
      );
    }
    return;
  }
  if (cmd === "market pause") return admin(need(words[2], "the market's address"), ADMIN.pause, 1n, "pause");
  if (cmd === "market resume")
    return admin(need(words[2], "the market's address"), ADMIN.pause, 0n, "resume");
  if (cmd === "market cap") {
    const f = await market(need(words[2], "the market's address"));
    const cap = parseDec(need(words[3], "the cap in dollars"), 2) ?? die("the cap is a dollar amount");
    if (!f.px) return die("the market has no price yet");
    return admin(f.market, ADMIN.oiCap, (cap * 10_000n) / BigInt(f.px), "cap");
  }
  if (cmd === "price post") return post(need(words[2], "the market's address"), need(words[3], "the price"));
  if (cmd === "price run") return run(need(words[2], "the market's address"));
  if (cmd === "bond add")
    return bondAdd(
      need(words[2], "the market's address"),
      parseDec(need(words[3], "the amount"), 6) ?? die("the amount is a dollar amount"),
    );
  if (cmd === "fees claim") return claim(need(words[2], "the market's address"));
  if (cmd === "liquidate run") return liquidate(need(words[2], "the market's address"));
  die(`unknown command: ${cmd}\n\n${HELP}`);
}
main().catch((e) => die(String(e?.message ?? e)));
