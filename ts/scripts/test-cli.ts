// The CLI (cli/solator.ts) from end to end, as a creator would use it, against the program's own binary behind the local
// chain (scripts/local-chain.mjs): open a market with a bond, keep its price, let two strangers trade on it, collect
// the fees, change its cap, pause it. Every command is run as a command and judged by what it prints and by the chain.
//
//   cd program && cargo build-sbf && cd ../ui && npm i --no-save litesvm
//   npx tsx scripts/test-cli.ts
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { readFileSync, readdirSync } from "node:fs";
import { join } from "node:path";
import { Keypair, PublicKey } from "@solana/web3.js";
import { PROGRAM_ID } from "../lib/ids";
import { trader } from "./trader";
import { find } from "../lib/percolator";
import { start } from "./harness";

const PORT = 8911;

async function main() {
  const { conn, rpc, dir, keyfile, cli, stop } = await start(PORT);
  try {
    const mint: string = await rpc("localMint");
    const creator = Keypair.generate();
    const ck = keyfile(creator, "creator");
    await rpc("requestAirdrop", [creator.publicKey.toBase58(), 2_000_000_000]);
    const ata: string = await rpc("localMintTo", [creator.publicKey.toBase58(), 50_000_000_000]); // $50,000

    // ---------- a market, with the bond that covers its cap ----------
    // refusals first: each says what is wrong
    assert.match(cli(["market", "create", "GOLD", "--mint", mint], ck, true), /missing: --price/);
    assert.match(cli(["market", "create", "g", "--price", "1", "--mint", mint], ck, true), /ticker: 2 to 10/);
    assert.match(
      cli(["market", "create", "GOLD", "--price", "1", "--dex", "X", "--mint", mint], ck, true),
      /dex: /,
    );
    const out = cli(
      [
        "market",
        "create",
        "GOLD",
        "--dex",
        "xyz",
        "--price",
        "2650.5",
        "--cap",
        "250000",
        "--bond",
        "12500",
        "--mint",
        mint,
      ],
      ck,
    );
    assert.match(out, /xyz:GOLD-USDC at \$2650\.5/);
    assert.match(out, /the bond that covers it: \$12,500\.00/, "a twentieth of the cap");
    assert.match(out, /price step \$0\.1, smallest order 0\.00001 GOLD, a price is good for 1 min/);
    assert.match(out, /1\/3 Price feed/);
    assert.match(out, /3\/3 Order book/);
    assert.match(out, /covers the cap/);
    const addr = /^market (\w{32,44})$/m.exec(out)![1];
    // the addresses are written down
    const file = readdirSync(dir).find((f) => f.includes(addr))!;
    assert.equal(file, `xyz-GOLD-${addr}.json`);
    assert.equal(JSON.parse(readFileSync(join(dir, file), "utf8")).market, addr);
    let f = (await find(conn, PROGRAM_ID, addr))!;
    assert.equal(f.name, "xyz:GOLD");
    assert.equal(f.admin, creator.publicKey.toBase58());
    assert.equal(f.bond, 12_500_000_000);
    assert.equal(f.shards.length, 2);
    assert.deepEqual(
      f.shards.map((s) => s.side),
      [0, 1],
    );
    const show = cli(["market", "show", addr], ck);
    for (const re of [
      /xyz:GOLD-USDC/,
      /\$2650\.5/,
      /\(live\)/,
      /10% to open, 5% to keep +\(10x\)/,
      /bond +\$12,500\.00 +covers the cap/,
    ])
      assert.match(show, re);
    assert.match(
      cli(["market", "list"], ck),
      /xyz:GOLD +\$2650\.5 live +bond \$12,500 +cap \$\d{3},\d{3} covered/,
    );
    assert.match(cli(["market", "list", "--mine"], ck), /xyz:GOLD/);

    // ---------- its price ----------
    await new Promise((r) => setTimeout(r, 1100)); // a newer stamp than the first price's
    assert.match(cli(["price", "post", addr, "2660"], ck), /xyz:GOLD \$2660\.0\n/);
    await new Promise((r) => setTimeout(r, 1100));
    assert.match(cli(["price", "post", addr, "3000"], ck), /\$2660\.0 +\(held: a move over 5%/);
    assert.match(cli(["market", "list"], ck), /xyz:GOLD +\$2660\.0 holding /, "a held move is not a pause");
    await new Promise((r) => setTimeout(r, 1100));
    assert.match(cli(["price", "post", addr, "3000"], ck), /xyz:GOLD \$3000\.0\n/);
    // the loop, one turn, from a command and from a URL
    await new Promise((r) => setTimeout(r, 1100));
    assert.match(cli(["price", "run", addr, "--from", "cmd:echo 3010.5", "--once"], ck), /\$3010\.5/);
    // (the source is another process: this one stands still while a command runs)
    const src = spawn("node", [
      "-e",
      `require("http").createServer((q, r) => r.end(JSON.stringify({ data: { price: "3012.25" } }))).listen(${PORT + 5}, "127.0.0.1")`,
    ]);
    await new Promise((r) => setTimeout(r, 1100));
    try {
      assert.match(
        cli(
          [
            "price",
            "run",
            addr,
            "--from",
            `url:http://127.0.0.1:${PORT + 5}/gold`,
            "--json",
            "data.price",
            "--once",
          ],
          ck,
        ),
        /\$3012\.25/,
      );
    } finally {
      src.kill();
    }
    f = (await find(conn, PROGRAM_ID, addr))!;
    assert.equal(f.px, 30_122, "$3,012.25 in steps of $0.1");
    // a heartbeat that could not keep the price fresh is refused before it starts
    assert.match(
      cli(["price", "run", addr, "--from", "cmd:echo 1", "--heartbeat", "500", "--once"], ck, true),
      /--heartbeat must be under 120 s/,
    );
    // nobody else's key
    const stranger = Keypair.generate();
    const sk = keyfile(stranger, "stranger");
    await rpc("requestAirdrop", [stranger.publicKey.toBase58(), 1_000_000_000]);
    assert.match(cli(["price", "post", addr, "1"], sk, true), /belongs to/);
    assert.match(cli(["fees", "claim", addr], sk, true), /belongs to/);
    assert.match(cli(["market", "pause", addr], sk, true), /belongs to/);

    // ---------- two strangers trade on it ----------
    const a = await trader(conn, rpc, f, 1_000_000_000);
    const b = await trader(conn, rpc, f, 1_000_000_000);
    // 1 GOLD (100,000 lots of 0.00001) at $3,012.20: a resting bid, then a sell into it. Notional 3,012,200,000.
    await a.order(true, 30_122n, 100_000n);
    await b.order(false, 30_122n, 100_000n, true);

    // ---------- the fees ----------
    // taker fee 5 bps of 3,012,200,000 = 1,506,100: half to the creator, half into the bond
    const fees = cli(["fees", "claim", addr], ck);
    assert.match(fees, /xyz:GOLD: \$0\.75 to /);
    assert.equal(
      (await conn.getTokenAccountBalance(new PublicKey(ata))).value.amount,
      String(50_000_000_000 - 12_500_000_000 + 753_050),
    );
    f = (await find(conn, PROGRAM_ID, addr))!;
    assert.equal(f.bond, 12_500_000_000 + 753_050);
    assert.equal(f.fees, 0);
    // (to a named account this time: the same claim in its other form)
    assert.match(cli(["fees", "claim", addr, "--to", ata], ck, true), /refused/, "nothing left to claim");

    // ---------- more bond, a larger cap, the brake ----------
    // The cap is a number of lots, so what it is worth moved with the price: 94,321,826 lots were $250,000 at $2,650.50
    // and are $284,116 at $3,012.20. The bond that covered it no longer does, and the CLI says by how much.
    assert.match(
      cli(["bond", "add", addr, "100"], ck),
      /bond \$12,600\.75 \(\+\$100\.00\) +the cap needs \$14,205\.81/,
    );
    assert.match(
      cli(["bond", "add", addr, "1605.06"], ck),
      /bond \$14,205\.81 \(\+\$1,605\.06\) +covers the cap/,
    );
    assert.match(cli(["bond", "add", addr, "0"], ck, true), /above zero/);
    const cap = cli(["market", "cap", addr, "1000000"], ck);
    assert.match(cap, /open interest +up to \$(999,\d{3}|1,000,000)/);
    assert.match(
      cap,
      /the cap needs \$(49,\d{3}|50,000)\.\d\d/,
      "a larger cap is no longer covered: a twentieth of it",
    );
    assert.match(cli(["market", "list"], ck), /not covered/);
    assert.match(cli(["market", "cap", addr, "0"], ck), /open interest +no cap/);
    assert.match(cli(["market", "pause", addr], ck), /\(paused\)/);
    assert.match(cli(["market", "resume", addr], ck), /\((live|waiting for a price)\)/);
    assert.match(
      cli(["market", "show", "11111111111111111111111111111111"], ck, true),
      /is not a named market/,
    );
    assert.match(cli(["help"], ck), /solator: open and run a market/);
    console.log(
      "ok  cli: create with bond, show, list, price post/held/run (cmd, url), trade, fees claim, bond add, cap, pause",
    );
  } finally {
    stop();
  }
}
main().catch((e) => {
  console.error(e);
  process.exit(1);
});
