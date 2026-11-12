// A stranger who trades, for the scripts that exercise a market on the local chain: a new key with SOL and the chain's
// test dollars, an account of its own in the market's vault, and limit orders. The instructions' data comes from the
// SDK's encoders (lib/sdk); the accounts are named here in the order program/src/lib.rs reads them.
import assert from "node:assert/strict";
import {
  ComputeBudgetProgram,
  Connection,
  Keypair,
  PublicKey,
  SystemProgram,
  Transaction,
  TransactionInstruction,
} from "@solana/web3.js";
import { PROGRAM_ID, TOKEN_PROGRAM } from "../lib/ids";
import { encodePlace, ix, spec as S, type PlaceArgs } from "../lib/sdk/index";

export type Rpc = (method: string, params?: any[]) => Promise<any>;
/** What a trader's instructions need to know of a market (lib/percolator.ts Found has it, and so has the site's list). */
export type Where = { market: string; vault: string; shards: { key: string }[] };
const PID = new PublicKey(PROGRAM_ID);
const sg = (k: PublicKey) => ({ pubkey: k, isSigner: true, isWritable: true });
const w = (k: string) => ({ pubkey: new PublicKey(k), isSigner: false, isWritable: true });
const r = (k: string) => ({ pubkey: new PublicKey(k), isSigner: false, isWritable: false });
/** deposit. accs: owner(s), account(w), market, vault(w), the owner's token account(w), token program. */
const depositIx = (owner: PublicKey, user: string, m: Where, amount: bigint, src: string) =>
  new TransactionInstruction({
    programId: PID,
    keys: [sg(owner), w(user), r(m.market), w(m.vault), w(src), r(TOKEN_PROGRAM)],
    data: ix.deposit(amount),
  });
/** place. accs: owner(s), account(w), market, the market's shards(w). */
function placeIx(owner: PublicKey, user: string, m: Where, p: PlaceArgs) {
  const d = Buffer.alloc(1 + S.PLACE_ITEM_SIZE);
  d[0] = S.IX_PLACE;
  encodePlace(d, 1, p);
  return new TransactionInstruction({
    programId: PID,
    keys: [sg(owner), w(user), r(m.market), ...m.shards.map((s) => w(s.key))],
    data: d,
  });
}
export async function sendIxs(
  conn: Connection,
  payer: Keypair,
  ixs: TransactionInstruction[],
  extra: Keypair[] = [],
) {
  const t = new Transaction().add(ComputeBudgetProgram.setComputeUnitLimit({ units: 200_000 }), ...ixs);
  t.feePayer = payer.publicKey;
  t.recentBlockhash = (await conn.getLatestBlockhash()).blockhash;
  t.sign(payer, ...extra);
  const sig = await conn.sendRawTransaction(t.serialize(), { skipPreflight: true });
  const st = (await conn.getSignatureStatuses([sig])).value[0];
  assert.ok(st && !st.err, "refused: " + JSON.stringify(st?.err));
  return sig;
}
/** A funded key with `usdc` (base units) deposited into the market's vault. */
export async function trader(conn: Connection, rpc: Rpc, m: Where, usdc: number) {
  const kp = Keypair.generate();
  await rpc("requestAirdrop", [kp.publicKey.toBase58(), 1_000_000_000]);
  const tok: string = await rpc("localMintTo", [kp.publicKey.toBase58(), usdc]);
  const user = Keypair.generate();
  await sendIxs(
    conn,
    kp,
    [
      SystemProgram.createAccount({
        fromPubkey: kp.publicKey,
        newAccountPubkey: user.publicKey,
        lamports: await conn.getMinimumBalanceForRentExemption(S.USER_SIZE),
        space: S.USER_SIZE,
        programId: PID,
      }),
      new TransactionInstruction({
        programId: PID,
        keys: [sg(kp.publicKey), w(user.publicKey.toBase58())],
        data: Buffer.from([S.IX_INIT_USER]),
      }),
      depositIx(kp.publicKey, user.publicKey.toBase58(), m, BigInt(usdc), tok),
    ],
    [user],
  );
  const order = (bid: boolean, price: bigint, qty: bigint, ioc = false) =>
    sendIxs(conn, kp, [placeIx(kp.publicKey, user.publicKey.toBase58(), m, { bid, price, qty, ioc })]);
  return { kp, user: user.publicKey.toBase58(), order };
}
