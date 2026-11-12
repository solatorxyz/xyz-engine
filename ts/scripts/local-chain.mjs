// A chain in one process, for exercising pages that sign: the program's own .so in a local VM, behind the few JSON-RPC
// methods and the one websocket subscription the app uses. Nothing here is a mock of the program: every transaction the
// page sends is executed by the same binary that is on chain. Time is the machine's clock (0.4 s a slot).
//
//   cd program && cargo build-sbf && cd ../ui && npm i --no-save litesvm
//   node scripts/local-chain.mjs            # http://127.0.0.1:8899, ws://127.0.0.1:8900
//   NEXT_PUBLIC_RPC=http://127.0.0.1:8899 NEXT_PUBLIC_RPC_WS=ws://127.0.0.1:8900 NEXT_PUBLIC_RELAY= \
//     NEXT_PUBLIC_BURNER=1 npx next dev -p 3100
//
// Extra methods, for the scripts that drive it: requestAirdrop (any amount), localMint (the 6-decimal mint this chain
// was started with), localMintTo [owner, amount] (sets the owner's usual token account of that mint to the amount),
// localFailNext (the next send is refused, as by a node that is down). simulateTransaction answers as the cluster does.
import http from "node:http";
import { readFileSync } from "node:fs";
import { WebSocketServer } from "ws";
import bs58 from "bs58";
import { address, getTransactionDecoder, lamports } from "@solana/kit";
import { Keypair, PublicKey, Transaction } from "@solana/web3.js";
import { LiteSVM, Clock, FailedTransactionMetadata } from "litesvm";

const PROGRAM = process.env.PROGRAM_ID ?? "7AY437YN1dupEhyahmKAQ6KTRm4nVx52BdyVzTcJbuRZ";
const TOKEN = "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA";
const HTTP_PORT = Number(process.env.PORT ?? 8899);
// A new blockhash every slot, as on a cluster, so that the same instruction sent twice is two transactions. How old
// a transaction's blockhash may be is not checked: on a cluster one lasts about a minute, the app keeps one for up to
// 30 s, and this VM knows only the newest. The same signed bytes twice are still refused (AlreadyProcessed).
const svm = new LiteSVM().withBlockhashCheck(false);
svm.addProgram(
  address(PROGRAM),
  readFileSync(new URL("../../program/target/deploy/perp_program.so", import.meta.url)),
);
const mint = Keypair.generate().publicKey.toBase58();
{
  const d = new Uint8Array(82);
  d[44] = 6;
  d[45] = 1;
  svm.setAccount({
    address: address(mint),
    data: d,
    executable: false,
    lamports: lamports(1_000_000_000n),
    programAddress: address(TOKEN),
    space: 82n,
  });
}
const T0 = Date.now();
const SLOT0 = 1000n;
const slotNow = () => SLOT0 + BigInt(Math.floor((Date.now() - T0) / 400));
const clock = () => svm.setClock(new Clock(slotNow(), 0n, 0n, 0n, BigInt(Math.floor(Date.now() / 1000))));
const dec = getTransactionDecoder();
/** signature -> { slot, err } */
const seen = new Map();
const subs = new Set();
/** every address a transaction has named: what getProgramAccounts looks through (the VM cannot list its accounts) */
const known = new Set();
const ATA = new PublicKey("ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL");
const ataOf = (owner) =>
  PublicKey.findProgramAddressSync(
    [new PublicKey(owner).toBuffer(), new PublicKey(TOKEN).toBuffer(), new PublicKey(mint).toBuffer()],
    ATA,
  )[0].toBase58();
let failNext = false;
/** the slot whose blockhash is the newest one handed out */
let hashSlot = 0n;
/** A refused transaction's error in the shape the cluster reports it: { InstructionError: [index, { Custom: code }] }. Null = it ran. */
const errOf = (r) => {
  if (!(r instanceof FailedTransactionMetadata)) return null;
  const m = /InstructionError\D+(\d+)\D+Custom\D+(\d+)/s.exec(r.toString());
  return m ? { InstructionError: [Number(m[1]), { Custom: Number(m[2]) }] } : { Failed: String(r.err()) };
};
const missing = new Set();
const ctx = () => ({ slot: Number(slotNow()) });
const acct = (k) => {
  const a = svm.getAccount(address(k));
  if (!a?.exists) return null;
  return {
    data: [Buffer.from(a.data).toString("base64"), "base64"],
    executable: a.executable,
    lamports: Number(a.lamports),
    owner: a.programAddress,
    rentEpoch: 0,
    space: a.data.length,
  };
};
const M = {
  getHealth: () => "ok",
  getVersion: () => ({ "solana-core": "local-vm", "feature-set": 0 }),
  getSlot: () => Number(slotNow()),
  getBlockTime: () => Math.floor(Date.now() / 1000),
  getBlockHeight: () => Number(slotNow()),
  getLatestBlockhash: () => {
    if (hashSlot !== slotNow()) {
      svm.expireBlockhash();
      hashSlot = slotNow();
    }
    return {
      context: ctx(),
      value: { blockhash: svm.latestBlockhash(), lastValidBlockHeight: Number(slotNow()) + 150 },
    };
  },
  getMinimumBalanceForRentExemption: ([n]) => Number(svm.minimumBalanceForRentExemption(BigInt(n))),
  getBalance: ([k]) => ({ context: ctx(), value: Number(svm.getBalance(address(k)) ?? 0n) }),
  getAccountInfo: ([k]) => ({ context: ctx(), value: acct(k) }),
  getMultipleAccounts: ([ks]) => ({ context: ctx(), value: ks.map(acct) }),
  getSignatureStatuses: ([sigs]) => ({
    context: ctx(),
    value: sigs.map((s) => {
      const r = seen.get(s);
      return r
        ? {
            slot: r.slot,
            confirmations: 1,
            err: r.err,
            confirmationStatus: "confirmed",
            status: r.err ? { Err: r.err } : { Ok: null },
          }
        : null;
    }),
  }),
  getProgramAccounts: ([pid, cfg = {}]) => {
    const out = [];
    for (const k of known) {
      const a = svm.getAccount(address(k));
      if (!a?.exists || a.programAddress !== pid) continue;
      const d = Buffer.from(a.data);
      const ok = (cfg.filters ?? []).every((f) =>
        f.dataSize != null
          ? d.length === f.dataSize
          : f.memcmp
            ? d
                .subarray(f.memcmp.offset, f.memcmp.offset + 64)
                .indexOf(Buffer.from(bs58.decode(f.memcmp.bytes))) === 0
            : true,
      );
      if (!ok) continue;
      const cut = cfg.dataSlice
        ? d.subarray(cfg.dataSlice.offset, cfg.dataSlice.offset + cfg.dataSlice.length)
        : d;
      out.push({
        pubkey: k,
        account: {
          data: [cut.toString("base64"), "base64"],
          executable: false,
          lamports: Number(a.lamports),
          owner: a.programAddress,
          rentEpoch: 0,
          space: d.length,
        },
      });
    }
    return cfg.withContext ? { context: ctx(), value: out } : out;
  },
  getTokenAccountBalance: ([k]) => {
    const a = svm.getAccount(address(k));
    if (!a?.exists) throw new Error("could not find account");
    const amount = Buffer.from(a.data).readBigUInt64LE(64).toString();
    return {
      context: ctx(),
      value: {
        amount,
        decimals: 6,
        uiAmount: Number(amount) / 1e6,
        uiAmountString: String(Number(amount) / 1e6),
      },
    };
  },
  sendTransaction: ([b64]) => {
    if (failNext) {
      failNext = false;
      throw new Error("local chain: this send was told to fail");
    }
    const raw = Buffer.from(b64, "base64");
    const sig = bs58.encode(raw.subarray(1, 65));
    if (seen.has(sig)) return sig; // the same signed bytes again: the cluster has them already
    const named = Transaction.from(raw)
      .compileMessage()
      .accountKeys.map((k) => k.toBase58());
    for (const k of named) known.add(k);
    clock();
    const r = svm.sendTransaction(dec.decode(raw));
    const err = errOf(r);
    if (err) {
      console.log("tx failed", sig.slice(0, 8), r.toString().split("\n").slice(0, 3).join(" | "));
      console.log(r.meta().logs().slice(-6).join("\n"));
    } else console.log("tx ok", sig.slice(0, 8), Number(r.computeUnitsConsumed()), "CU");
    seen.set(sig, {
      slot: Number(slotNow()),
      err,
      raw,
      time: Math.floor(Date.now() / 1000),
      logs: err ? r.meta().logs() : r.logs(),
    });
    for (const s of subs) if (s.sig === sig) s.fire(err);
    if (!err) setTimeout(() => tell(named), 0);
    return sig;
  },
  // what the program would answer, with nothing written: the keeper asks before it sends (lib/keeper.ts)
  simulateTransaction: ([b64]) => {
    clock();
    const r = svm.simulateTransaction(dec.decode(Buffer.from(b64, "base64")));
    const err = errOf(r);
    const meta = r.meta();
    return {
      context: ctx(),
      value: { err, logs: meta.logs(), accounts: null, unitsConsumed: Number(meta.computeUnitsConsumed()) },
    };
  },
  // a confirmed transaction as the cluster reports it ("json" encoding): what the app reads fills and failures from
  getTransaction: ([sig]) => {
    const t = seen.get(sig);
    if (!t) return null;
    const m = Transaction.from(t.raw).compileMessage();
    return {
      slot: t.slot,
      blockTime: t.time,
      version: "legacy",
      transaction: {
        signatures: [sig],
        message: {
          header: m.header,
          accountKeys: m.accountKeys.map((k) => k.toBase58()),
          recentBlockhash: m.recentBlockhash,
          instructions: m.instructions.map((i) => ({
            programIdIndex: i.programIdIndex,
            accounts: i.accounts,
            data: i.data,
          })),
        },
      },
      meta: {
        err: t.err,
        status: t.err ? { Err: t.err } : { Ok: null },
        fee: 5000,
        preBalances: [],
        postBalances: [],
        innerInstructions: [],
        logMessages: t.logs,
        preTokenBalances: [],
        postTokenBalances: [],
        rewards: [],
      },
    };
  },
  // no history by address here: the app's trade tape for a market stays empty on this chain
  getSignaturesForAddress: () => [],
  requestAirdrop: ([k, n]) => {
    svm.airdrop(address(k), lamports(BigInt(n)));
    return "1".repeat(64);
  },
  localMint: () => mint,
  localMintTo: ([owner, amount]) => {
    const k = ataOf(owner);
    const d = new Uint8Array(165);
    d.set(new PublicKey(mint).toBytes(), 0);
    d.set(new PublicKey(owner).toBytes(), 32);
    new DataView(d.buffer).setBigUint64(64, BigInt(amount), true);
    d[108] = 1;
    svm.setAccount({
      address: address(k),
      data: d,
      executable: false,
      lamports: lamports(1_000_000_000n),
      programAddress: address(TOKEN),
      space: 165n,
    });
    known.add(k);
    return k;
  },
  /** the next sendTransaction is refused before it reaches the VM (a node that is down) */
  localFailNext: () => (failNext = true),
};
const answer = (q) => {
  const f = M[q.method];
  if (!f) {
    if (!missing.has(q.method)) console.log("not served:", q.method);
    missing.add(q.method);
    return { jsonrpc: "2.0", id: q.id, error: { code: -32601, message: "local chain: no " + q.method } };
  }
  try {
    clock();
    return { jsonrpc: "2.0", id: q.id, result: f(q.params ?? []) };
  } catch (e) {
    return { jsonrpc: "2.0", id: q.id, error: { code: -32000, message: String(e?.message ?? e) } };
  }
};
http
  .createServer((req, res) => {
    const h = {
      "access-control-allow-origin": "*",
      "access-control-allow-headers": "*",
      "access-control-allow-methods": "POST, OPTIONS",
      "content-type": "application/json",
      // one request, one connection: a caller that keeps sockets open would otherwise meet one this server has closed
      connection: "close",
    };
    if (req.method !== "POST") return res.writeHead(204, h).end();
    let b = "";
    req.on("data", (c) => (b += c));
    req.on("end", () => {
      let out;
      try {
        const q = JSON.parse(b);
        out = Array.isArray(q) ? q.map(answer) : answer(q);
      } catch {
        out = { jsonrpc: "2.0", id: null, error: { code: -32700, message: "parse error" } };
      }
      res.writeHead(200, h).end(JSON.stringify(out));
    });
  })
  .listen(HTTP_PORT, "127.0.0.1");
// signatureSubscribe: answered when the transaction is (or already was) executed. accountSubscribe: told after every
// transaction that names the account. Everything else is acknowledged and silent.
let nextSub = 1;
/** address -> the subscriptions watching it: { id, ws } */
const watchers = new Map();
const tell = (keys) => {
  for (const k of keys)
    for (const w of watchers.get(k) ?? []) {
      const value = acct(k);
      if (value)
        w.ws.send(
          JSON.stringify({
            jsonrpc: "2.0",
            method: "accountNotification",
            params: { subscription: w.id, result: { context: ctx(), value } },
          }),
        );
    }
};
new WebSocketServer({ port: HTTP_PORT + 1, host: "127.0.0.1" }).on("connection", (ws) => {
  ws.on("message", (raw) => {
    let q;
    try {
      q = JSON.parse(String(raw));
    } catch {
      return;
    }
    const id = nextSub++;
    ws.send(JSON.stringify({ jsonrpc: "2.0", id: q.id, result: /Unsubscribe$/.test(q.method) ? true : id }));
    if (q.method === "accountSubscribe") {
      const k = q.params[0];
      const w = { id, ws };
      if (!watchers.has(k)) watchers.set(k, new Set());
      watchers.get(k).add(w);
      ws.on("close", () => watchers.get(k)?.delete(w));
      return;
    }
    if (q.method !== "signatureSubscribe") return;
    const s = {
      sig: q.params[0],
      fire: (err) => {
        subs.delete(s);
        ws.send(
          JSON.stringify({
            jsonrpc: "2.0",
            method: "signatureNotification",
            params: { subscription: id, result: { context: ctx(), value: { err } } },
          }),
        );
      },
    };
    const r = seen.get(s.sig);
    if (r) setTimeout(() => s.fire(r.err), 0);
    else subs.add(s);
    ws.on("close", () => subs.delete(s));
  });
});
console.log(`local chain: http://127.0.0.1:${HTTP_PORT}  ws://127.0.0.1:${HTTP_PORT + 1}  mint ${mint}`);
