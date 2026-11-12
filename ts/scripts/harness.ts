// What the scripts that run the command line against the local chain share: the chain itself (scripts/local-chain.mjs,
// the program's own binary), a directory for key files, and the CLI run as a command.
import assert from "node:assert/strict";
import { spawn, spawnSync } from "node:child_process";
import { mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { Connection, Keypair } from "@solana/web3.js";

export const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
export async function start(port: number) {
  const url = `http://127.0.0.1:${port}`;
  const dir = mkdtempSync(join(tmpdir(), "solator-cli-"));
  const rpc = (method: string, params: any[] = []) =>
    fetch(url, { method: "POST", body: JSON.stringify({ jsonrpc: "2.0", id: 1, method, params }) })
      .then((r) => r.json())
      .then((j: any) => {
        if (j.error) throw new Error(method + ": " + j.error.message);
        return j.result;
      });
  const chain = spawn("node", ["scripts/local-chain.mjs"], {
    env: { ...process.env, PORT: String(port) },
    stdio: "pipe",
  });
  let log = "";
  chain.stdout.on("data", (d) => (log += d));
  chain.stderr.on("data", (d) => (log += d));
  for (let i = 0; i < 50; i++) {
    if (await rpc("getHealth").catch(() => null)) break;
    await sleep(200);
  }
  const keyfile = (kp: Keypair, name: string) => {
    const f = join(dir, name + ".json");
    writeFileSync(f, JSON.stringify([...kp.secretKey]));
    return f;
  };
  /** Run the CLI; returns what it printed. `fails` = it must exit non-zero. */
  const cli = (args: string[], key: string, fails = false): string => {
    const r = spawnSync(
      "npx",
      ["tsx", "cli/solator.ts", ...args, "--url", url, "--keypair", key, "--out", dir],
      { encoding: "utf8" },
    );
    const out = (r.stdout ?? "") + (r.stderr ?? "");
    assert.equal(r.status !== 0, fails, `solator ${args.join(" ")}\n${out}`);
    return out;
  };
  return {
    url,
    dir,
    rpc,
    conn: new Connection(url, "confirmed"),
    keyfile,
    cli,
    /** what the chain printed (every transaction, with the program's log when it was refused) */
    log: () => log,
    stop: () => chain.kill(),
  };
}
