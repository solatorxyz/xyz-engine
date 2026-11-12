// The liquidator (cli/solator.ts "liquidate run", lib/keeper.ts) against the program's own binary on the local chain,
// run by a stranger to the market: every state an account under its margin can be in, in the order they happen to one
// position as the price falls.
//
//   healthy                           -> left alone
//   under maintenance, book empty     -> waits, and says so once
//   the price too old                 -> nothing is judged
//   an order within 5% of the mark    -> a quarter is sold into it; fee to the bond, a fifth of it to the sender
//   only an order far from the mark,
//   whose fill the bond cannot pay    -> refused by the program, waits
//   worth less than nothing           -> closed against the other side at the mark; the bond pays first, then the
//                                        other side's gain; no bad debt
//
//   cd program && cargo build-sbf && cd ../ui && npm i --no-save litesvm
//   npx tsx scripts/test-liquidate.ts
import assert from "node:assert/strict";
import { Keypair, PublicKey } from "@solana/web3.js";
import { PROGRAM_ID } from "../lib/ids";
import { decodeLayout } from "../lib/sdk/index";
import { find } from "../lib/percolator";
import { trader } from "./trader";
import { sleep, start } from "./harness";

const PORT = 8931;

async function main() {
  const { conn, rpc, keyfile, cli, log, stop } = await start(PORT);
  try {
    const mint: string = await rpc("localMint");
    const creator = Keypair.generate();
    const ck = keyfile(creator, "creator");
    await rpc("requestAirdrop", [creator.publicKey.toBase58(), 2_000_000_000]);
    await rpc("localMintTo", [creator.publicKey.toBase58(), 1_000_000_000]);
    // GOLD at $3,000 in steps of $0.1, lots of 0.00001: 1 GOLD = 100,000 lots, worth 3,000,000,000 base units.
    // A bond of $5: small on purpose, so that a loss of a few dollars reaches past it.
    const out = cli(
      [
        "market",
        "create",
        "GOLD",
        "--dex",
        "liq",
        "--price",
        "3000",
        "--cap",
        "100000",
        "--bond",
        "5",
        "--mint",
        mint,
      ],
      ck,
    );
    const addr = /^market (\w{32,44})$/m.exec(out)![1];
    const f = (await find(conn, PROGRAM_ID, addr))!;
    const read = async (key: string, layout: string): Promise<any> =>
      decodeLayout(layout, Buffer.from((await conn.getAccountInfo(new PublicKey(key)))!.data));
    const pos = async (user: string) =>
      ((await read(user, "user")).pos as any[]).find((p) => p.market === addr) ?? null;
    const bond = async () => (await read(f.insurance, "insurance")).balance as bigint;
    const price = async (usd: string) => {
      await sleep(1100); // a newer stamp than the last price's
      assert.match(cli(["price", "post", addr, usd], ck), new RegExp(`liq:GOLD \\$${usd}\\.0\\n`));
    };

    const long = await trader(conn, rpc, f, 400_000_000); // $400 carries 1 GOLD at 10x with $100 to spare
    const short = await trader(conn, rpc, f, 5_000_000_000);
    const bidder = await trader(conn, rpc, f, 5_000_000_000);
    const keeper = await trader(conn, rpc, f, 10_000_000); // a stranger: an account in the market, no position
    const kk = keyfile(keeper.kp, "keeper");
    const sweep = (...more: string[]) => cli(["liquidate", "run", addr, "--once", "--earn", ...more], kk);

    // ---------- a position ----------
    await short.order(false, 30_000n, 100_000n);
    await long.order(true, 30_000n, 100_000n, true);
    assert.equal((await pos(long.user)).base, 100_000n);
    // cash is the deposit less what the position cost and the taker fee (5 bps of $3,000 = $1.50)
    assert.equal((await read(long.user, "user")).cash, 400_000_000n - 3_000_000_000n - 1_500_000n);

    // ---------- healthy: left alone ----------
    // (the short's resting order was filled: this pass books it, as the creator's loop would)
    let o = sweep();
    assert.match(
      o,
      /looking every 3 s for accounts under their maintenance margin; a fifth of each liquidation fee to/,
    );
    assert.match(o, /settled 1 filled resting order account/);
    assert.doesNotMatch(o, /liquidated|closed against|waits/);
    assert.equal((await pos(short.user)).base, -100_000n);
    // --earn needs an account of that key in this market
    assert.match(
      cli(["liquidate", "run", addr, "--once", "--earn"], ck, true),
      /has no trading account in liq:GOLD yet/,
    );
    assert.match(
      cli(["liquidate", "run", addr, "--once", "--earn", short.user], kk, true),
      /is not a trading account of this key/,
    );

    // ---------- under maintenance, nothing to fill against: waits ----------
    // Each post moves the price under 5% (a larger move is held until the next post confirms it).
    // At $2,730 the position is worth 400 - 1.5 - 270 = $128.50 and must be worth 5.04% of $2,730 = $137.59
    // (5% maintenance, and the 4 or 5 bps by which a single source's price is taken to be uncertain: 1/2000 of it,
    // in whole price steps, as a whole number of bps).
    await price("2860");
    await price("2730");
    o = sweep();
    assert.match(
      o,
      /\(long \$2,730\.00, worth \$128\.50, must be \$137\.59\): no order within 20% of the mark to fill against; it waits/,
    );
    assert.equal((await pos(long.user)).base, 100_000n);

    // ---------- the price too old: nobody is judged ----------
    // (a price is good for 150 slots, a minute on this chain)
    await sleep(62_000);
    assert.match(sweep(), /the price is too old to judge anyone by/);
    await price("2730");

    // ---------- an order near the mark: a quarter is sold into it ----------
    // 0.25 GOLD bid at $2,700. The quarter (25,000 lots) fills there: 675,000,000 base units.
    // Fee 25 bps of that = 1,687,500: a fifth (337,500) to the sender's account, the rest (1,350,000) to the bond.
    await bidder.order(true, 27_000n, 25_000n);
    const [k0, b0] = [(await read(keeper.user, "user")).cash as bigint, await bond()];
    assert.equal(b0, 5_000_000n);
    o = sweep();
    assert.match(
      o,
      /liquidated into the book: .*\(long \$2,730\.00, worth \$128\.50, must be \$137\.59\) -> \$2,047\.50 left/,
    );
    assert.equal((await pos(long.user)).base, 75_000n);
    assert.equal((await read(keeper.user, "user")).cash - k0, 337_500n, "the sender's fifth");
    assert.equal((await bond()) - b0, 1_350_000n, "the rest of the fee, into the bond");
    // sold $7.50 under the mark, charged the liquidation fee ($1.6875) and, like any order that takes from the book, the
    // taker fee (5 bps, $0.3375): 128.5 - 7.5 - 1.6875 - 0.3375 = $118.975, above the $103.19 it now needs
    assert.equal((await read(long.user, "user")).cash + 75_000n * 27_300n, 118_975_000n);
    // the next pass books the bidder's fill and has nothing to liquidate
    o = sweep();
    assert.match(o, /settled 1 filled resting order account/);
    assert.doesNotMatch(o, /liquidated|closed against|waits/);
    assert.equal((await pos(bidder.user)).base, 25_000n);

    // ---------- only a far order, whose fill the bond could not pay: refused, waits ----------
    // At $2,600 it is worth 118.975 - 0.75 x 130 = $21.475 and must be worth $98.48 (5.05% of $1,950). The only bid is at $2,250:
    // selling a quarter there (0.1875 GOLD) loses $65.63, more than the position is worth and more than the bond holds.
    await price("2600");
    await bidder.order(true, 22_500n, 50_000n);
    o = sweep();
    assert.match(
      o,
      /\(long \$1,950\.00, worth \$21\.4[78], must be \$98\.4[78]\): no order in the book closes it without a loss larger than the bond; it waits/,
    );
    assert.equal((await pos(long.user)).base, 75_000n);
    assert.equal(await bond(), 6_350_000n);

    // ---------- worth less than nothing: closed against the other side ----------
    // At $2,560: 118.975 - 0.75 x 170 = -$8.525. The short, $440 a GOLD in gain, is the other side.
    // The loss is paid by the bond first ($6.35, all of it), then by the short's gain ($2.175); no bad debt.
    await price("2560");
    const s0 = (await read(short.user, "user")).cash as bigint;
    o = sweep();
    assert.match(
      o,
      /closed against the other side at the mark: .*\(long \$1,920\.00, worth -\$8\.5[23], must be \$96\.77\) -> nothing left; bond \$0\.00/,
    );
    assert.equal(await pos(long.user), null);
    assert.equal((await read(long.user, "user")).cash, 0n, "it ends at nothing, not below");
    assert.equal(await bond(), 0n);
    assert.equal((await pos(short.user)).base, -25_000n);
    // the short bought back 75,000 lots at the mark and paid what the bond could not
    assert.equal((await read(short.user, "user")).cash - s0, -(75_000n * 25_600n) - 2_175_000n);
    assert.equal((await read(addr, "market")).bad_debt, 0n);
    // and the bidder's far order was never touched
    assert.equal((await pos(bidder.user)).bid_open, 50_000n);
    assert.doesNotMatch(sweep(), /liquidated|closed against|waits/);

    // ---------- the same for a short, as the price rises ----------
    // $100 carries 0.25 GOLD sold at $2,560 ($640). Cash is the deposit, what the sale brought, less 5 bps.
    const short2 = await trader(conn, rpc, f, 100_000_000);
    await bidder.order(true, 25_600n, 25_000n);
    await short2.order(false, 25_600n, 25_000n, true);
    assert.equal((await read(short2.user, "user")).cash, 100_000_000n + 640_000_000n - 320_000n);
    await price("2680");
    await price("2800");
    await price("2830");
    // At $2,830 it is worth 739.68 - 707.5 = $32.18 and must be worth 5.04% of $707.50 = $35.66. An ask of 0.0625 GOLD
    // at $2,840 is what its quarter is bought from: 177,500,000 base units, fee 443,750 (88,750 + 355,000).
    await short.order(false, 28_400n, 6_250n);
    const k1 = (await read(keeper.user, "user")).cash as bigint;
    o = sweep();
    assert.match(o, /settled 1 filled resting order account/);
    assert.match(
      o,
      /liquidated into the book: .*\(short \$707\.50, worth \$32\.18, must be \$35\.66\) -> \$530\.6[23] left/,
    );
    assert.equal((await pos(short2.user)).base, -18_750n);
    assert.equal((await read(keeper.user, "user")).cash - k1, 88_750n);
    assert.equal(await bond(), 355_000n);
    assert.equal(
      (await read(short2.user, "user")).cash,
      739_680_000n - 177_500_000n - 443_750n - 88_750n,
      "what it bought, the liquidation fee, the taker fee",
    );
    // the book's asks are empty again. At $2,960 it is worth $6.65 of the $27.97 it must be: waits.
    await price("2960");
    o = sweep();
    assert.match(o, /settled 1 filled resting order account/);
    assert.match(
      o,
      /\(short \$555\.00, worth \$6\.65, must be \$27\.97\): no order within 20% of the mark to fill against; it waits/,
    );
    // At $3,000 it is worth -$0.8525. The longs are the other side: the bidder, long 0.5 GOLD from $2,700 and $2,560.
    // The bond pays what it has ($0.355), the bidder's gain the rest ($0.4975).
    await price("3000");
    const c0 = (await read(bidder.user, "user")).cash as bigint;
    o = sweep();
    assert.match(
      o,
      /closed against the other side at the mark: .*\(short \$562\.50, worth -\$0\.85, must be \$28\.\d\d\) -> nothing left; bond \$0\.00/,
    );
    assert.equal(await pos(short2.user), null);
    assert.equal((await read(short2.user, "user")).cash, 0n);
    assert.equal(await bond(), 0n);
    assert.equal((await pos(bidder.user)).base, 50_000n - 18_750n);
    assert.equal((await read(bidder.user, "user")).cash - c0, 18_750n * 30_000n - 497_500n);
    assert.equal((await read(addr, "market")).bad_debt, 0n);
    assert.doesNotMatch(sweep(), /liquidated|closed against|waits/);
    console.log(
      "ok  liquidate (a long as the price falls, a short as it rises): healthy, waits (no order), stale price, into the book (fee split), refused beyond the bond, closed against the other side (bond, then gain; no bad debt)",
    );
  } catch (e) {
    console.error(log().split("\n").slice(-40).join("\n"));
    throw e;
  } finally {
    stop();
  }
}
main().catch((e) => {
  console.error(e);
  process.exit(1);
});
