// TypeScript SDK for the perp program: account decoders, batch codec, instruction data builders and an
// event decoder / in-memory indexer. Layouts and constants (spec.ts) are generated from the program's own sources,
// program/src/state.rs and perp-core/src/spec.rs, by scripts/gen-spec.py. Zero runtime deps (Node >= 22, Buffer).
// Published with the kernel as ts/lib/sdk, where the command line (cli/solator.ts) is its first user.
import * as S from "./spec";
export * as spec from "./spec";

const B58 = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";
export function base58(b: Uint8Array): string {
  let n = 0n;
  for (const x of b) n = n * 256n + BigInt(x);
  let s = "";
  while (n > 0n) {
    s = B58[Number(n % 58n)] + s;
    n /= 58n;
  }
  for (const x of b) {
    if (x !== 0) break;
    s = "1" + s;
  }
  return s;
}
export function fromBase58(s: string): Uint8Array {
  let n = 0n;
  for (const c of s) {
    const i = B58.indexOf(c);
    if (i < 0) throw new Error("bad base58");
    n = n * 58n + BigInt(i);
  }
  const out: number[] = [];
  while (n > 0n) {
    out.unshift(Number(n % 256n));
    n /= 256n;
  }
  for (const c of s) {
    if (c !== "1") break;
    out.unshift(0);
  }
  return Uint8Array.from(out);
}

export type Decoded = {
  [k: string]: bigint | number | string | Decoded | (bigint | number | string | Decoded)[];
};
function typeSize(t: string): number {
  const m = /^(.+)\[(\d+)\]$/.exec(t);
  if (m) return typeSize(m[1]) * Number(m[2]);
  switch (t) {
    case "u8":
      return 1;
    case "u16":
      return 2;
    case "u32":
      return 4;
    case "u64":
    case "i64":
      return 8;
    case "key":
      return 32;
  }
  const sz = S.LAYOUT_SIZE[t];
  if (sz === undefined) throw new Error("unknown type " + t);
  return sz;
}
function readType(buf: Buffer, t: string, o: number): any {
  const m = /^(.+)\[(\d+)\]$/.exec(t);
  if (m) {
    const n = Number(m[2]),
      es = typeSize(m[1]);
    return Array.from({ length: n }, (_, i) => readType(buf, m[1], o + i * es));
  }
  switch (t) {
    case "u8":
      return buf.readUInt8(o);
    case "u16":
      return buf.readUInt16LE(o);
    case "u32":
      return buf.readUInt32LE(o);
    case "u64":
      return buf.readBigUInt64LE(o);
    case "i64":
      return buf.readBigInt64LE(o);
    case "key":
      return base58(buf.subarray(o, o + 32));
  }
  return decodeLayout(t, buf, o);
}
/** Decode any spec layout (market, user, shard, order, pos, insurance, rank, pool, place_item, ...). */
export function decodeLayout(name: string, buf: Buffer, off = 0): Decoded {
  const l = S.LAYOUTS[name];
  if (!l) throw new Error("unknown layout " + name);
  if (buf.length < off + S.LAYOUT_SIZE[name]) throw new Error(`${name}: short buffer`);
  const out: Decoded = {};
  for (const [f, t, o] of l) out[f] = readType(buf, t, off + o);
  return out;
}
const DISCS: [bigint, string][] = [
  [S.DISC_MARKET, "market"],
  [S.DISC_SHARD, "shard"],
  [S.DISC_USER, "user"],
  [S.DISC_INS, "insurance"],
  [S.DISC_RANK, "rank"],
  [S.DISC_POOL, "pool"],
  [S.DISC_MULTISIG, "multisig"],
  [S.DISC_DELEGATE, "delegate"],
  [S.DISC_SLAB, "slab"],
  [S.DISC_FAST, "fast_book"],
  [S.DISC_VAULT, "vault"],
  [S.DISC_VDEP, "vault_deposit"],
  [S.DISC_VLINK, "vault_link"],
].map(([d, n]) => [BigInt(d), n as string]);
/** Decode a program account by its 8-byte discriminator. */
export function decodeAccount(data: Buffer): { type: string; data: Decoded } {
  const d = data.readBigUInt64LE(0);
  const hit = DISCS.find(([x]) => x === d);
  if (!hit) throw new Error("unknown discriminator " + d.toString(16));
  return { type: hit[1], data: decodeLayout(hit[1], data) };
}

// ---------- batch codec (byte-identical to perp-core::codec) ----------
export interface PlaceArgs {
  bid: boolean;
  price: bigint;
  qty: bigint;
  postOnly?: boolean;
  reduceOnly?: boolean;
  ioc?: boolean;
  fok?: boolean;
  aloSlide?: boolean;
  stp?: number;
  kind?: number;
  expirySlot?: bigint;
  oracleBandBps?: number;
  cid?: bigint;
  a1?: bigint;
  a2?: bigint;
  a3?: bigint;
}
export interface CancelItem {
  orderId: bigint;
  side: number;
  price: bigint;
}
export function placeFlags(p: PlaceArgs): number {
  return (
    (p.postOnly ? S.FLAG_POST_ONLY : 0) |
    (p.reduceOnly ? S.FLAG_REDUCE_ONLY : 0) |
    (p.ioc ? S.FLAG_IOC : 0) |
    (p.fok ? S.FLAG_FOK : 0) |
    (p.aloSlide ? S.FLAG_ALO_SLIDE : 0) |
    (((p.stp ?? 0) << S.FLAG_STP_SHIFT) & S.FLAG_STP_MASK)
  );
}
export function encodePlace(b: Buffer, o: number, p: PlaceArgs) {
  b.writeUInt8(p.bid ? S.SIDE_BID : S.SIDE_ASK, o + S.PLACE_ITEM_SIDE_OFF);
  b.writeUInt8(placeFlags(p), o + S.PLACE_ITEM_FLAGS_OFF);
  b.writeUInt8(p.kind ?? 0, o + S.PLACE_ITEM_KIND_OFF);
  b.writeBigUInt64LE(p.price, o + S.PLACE_ITEM_PRICE_OFF);
  b.writeBigUInt64LE(p.qty, o + S.PLACE_ITEM_QTY_OFF);
  b.writeBigUInt64LE(p.expirySlot ?? 0n, o + S.PLACE_ITEM_EXPIRY_SLOT_OFF);
  b.writeUInt16LE(p.oracleBandBps ?? 0, o + S.PLACE_ITEM_ORACLE_BAND_BPS_OFF);
  b.writeBigUInt64LE(p.cid ?? 0n, o + S.PLACE_ITEM_CID_OFF);
  b.writeBigInt64LE(p.a1 ?? 0n, o + S.PLACE_ITEM_A1_OFF);
  b.writeBigUInt64LE(p.a2 ?? 0n, o + S.PLACE_ITEM_A2_OFF);
  b.writeBigUInt64LE(p.a3 ?? 0n, o + S.PLACE_ITEM_A3_OFF);
}
export function encodeBatch(market: number, cancels: CancelItem[], places: PlaceArgs[]): Buffer {
  const b = Buffer.alloc(
    S.BATCH_HEADER_SIZE + cancels.length * S.CANCEL_ITEM_SIZE + places.length * S.PLACE_ITEM_SIZE,
  );
  b.writeUInt8(S.IX_BATCH, S.BATCH_HEADER_TAG_OFF);
  b.writeUInt16LE(market, S.BATCH_HEADER_MARKET_OFF);
  b.writeUInt8(cancels.length, S.BATCH_HEADER_N_CANCEL_OFF);
  b.writeUInt8(places.length, S.BATCH_HEADER_N_PLACE_OFF);
  let o = S.BATCH_HEADER_SIZE;
  for (const c of cancels) {
    b.writeBigUInt64LE(c.orderId, o + S.CANCEL_ITEM_ORDER_ID_OFF);
    b.writeUInt8(c.side, o + S.CANCEL_ITEM_SIDE_OFF);
    b.writeBigUInt64LE(c.price, o + S.CANCEL_ITEM_PRICE_OFF);
    o += S.CANCEL_ITEM_SIZE;
  }
  for (const p of places) {
    encodePlace(b, o, p);
    o += S.PLACE_ITEM_SIZE;
  }
  return b;
}
export function decodeBatch(
  b: Buffer,
): { market: number; cancels: CancelItem[]; places: PlaceArgs[] } | null {
  if (b.length < S.BATCH_HEADER_SIZE || b[0] !== S.IX_BATCH) return null;
  const nc = b[S.BATCH_HEADER_N_CANCEL_OFF],
    np = b[S.BATCH_HEADER_N_PLACE_OFF];
  if (b.length !== S.BATCH_HEADER_SIZE + nc * S.CANCEL_ITEM_SIZE + np * S.PLACE_ITEM_SIZE) return null;
  let o = S.BATCH_HEADER_SIZE;
  const cancels: CancelItem[] = [],
    places: PlaceArgs[] = [];
  for (let i = 0; i < nc; i++, o += S.CANCEL_ITEM_SIZE)
    cancels.push({
      orderId: b.readBigUInt64LE(o + S.CANCEL_ITEM_ORDER_ID_OFF),
      side: b[o + S.CANCEL_ITEM_SIDE_OFF],
      price: b.readBigUInt64LE(o + S.CANCEL_ITEM_PRICE_OFF),
    });
  for (let i = 0; i < np; i++, o += S.PLACE_ITEM_SIZE) {
    const f = b[o + S.PLACE_ITEM_FLAGS_OFF];
    places.push({
      bid: b[o] === S.SIDE_BID,
      price: b.readBigUInt64LE(o + S.PLACE_ITEM_PRICE_OFF),
      qty: b.readBigUInt64LE(o + S.PLACE_ITEM_QTY_OFF),
      postOnly: !!(f & S.FLAG_POST_ONLY),
      reduceOnly: !!(f & S.FLAG_REDUCE_ONLY),
      ioc: !!(f & S.FLAG_IOC),
      fok: !!(f & S.FLAG_FOK),
      aloSlide: !!(f & S.FLAG_ALO_SLIDE),
      stp: (f & S.FLAG_STP_MASK) >> S.FLAG_STP_SHIFT,
      kind: b[o + S.PLACE_ITEM_KIND_OFF],
      expirySlot: b.readBigUInt64LE(o + S.PLACE_ITEM_EXPIRY_SLOT_OFF),
      oracleBandBps: b.readUInt16LE(o + S.PLACE_ITEM_ORACLE_BAND_BPS_OFF),
      cid: b.readBigUInt64LE(o + S.PLACE_ITEM_CID_OFF),
      a1: b.readBigInt64LE(o + S.PLACE_ITEM_A1_OFF),
      a2: b.readBigUInt64LE(o + S.PLACE_ITEM_A2_OFF),
      a3: b.readBigUInt64LE(o + S.PLACE_ITEM_A3_OFF),
    });
  }
  return { market: b.readUInt16LE(S.BATCH_HEADER_MARKET_OFF), cancels, places };
}

// ---------- instruction data builders (account lists: see ARCHITECTURE.md) ----------
const u64le = (v: bigint) => {
  const b = Buffer.alloc(8);
  b.writeBigUInt64LE(v);
  return b;
};
export const ix = {
  deposit: (amount: bigint) => Buffer.concat([Buffer.from([S.IX_DEPOSIT]), u64le(amount)]),
  withdraw: (amount: bigint) => Buffer.concat([Buffer.from([S.IX_WITHDRAW]), u64le(amount)]),
  updateOracle: () => Buffer.from([S.IX_SET_ORACLE]),
  liquidate: (maxQty: bigint) => Buffer.concat([Buffer.from([S.IX_LIQUIDATE]), u64le(maxQty)]),
  adl: () => Buffer.from([S.IX_BACKSTOP]),
  adlRank: () => Buffer.from([S.IX_ADL_RANK]),
  admin: (op: number, v: bigint) => Buffer.concat([Buffer.from([S.IX_ADMIN, op]), u64le(v)]),
  adminPropose: (op: number, v: bigint, key?: Uint8Array) =>
    Buffer.concat([Buffer.from([S.IX_ADMIN_PROPOSE, op]), u64le(v), Buffer.from(key ?? [])]),
  adminExecute: () => Buffer.from([S.IX_ADMIN_EXECUTE]),
  batch: encodeBatch,
  /** v6: permissionless book liquidation. accs: target user(w), market, insurance(w), ALL market shards(w), [other markets] */
  liquidateBook: (maxQty: bigint) => Buffer.concat([Buffer.from([S.IX_LIQUIDATE_BOOK]), u64le(maxQty)]),
  /** TP/SL/TWAP. `mark`: fire on the mark price; `confirmOracle` (with mark): mark AND oracle through the trigger. */
  setTrigger: (t: TriggerArgs) => {
    if (t.confirmOracle && !t.mark) throw new Error("confirmOracle requires mark");
    const k = t.kind | (t.mark ? 0x40 : 0) | (t.confirmOracle ? 0x80 : 0);
    return Buffer.concat([
      Buffer.from([S.IX_SET_TRIGGER, t.slot, k, t.side]),
      ...[t.triggerPx, t.limitPx, t.qty, t.tip ?? 0n, t.interval ?? 1n, t.slices ?? 1n].map(u64le),
    ]);
  },
  execTrigger: (slot: number) => Buffer.from([S.IX_EXEC_TRIGGER, slot]),
  setIsolated: (on: boolean) => Buffer.from([S.IX_SET_ISOLATED, on ? 1 : 0]),
  isoTransfer: (delta: bigint) => {
    const b = Buffer.alloc(9);
    b[0] = S.IX_ISO_TRANSFER;
    b.writeBigInt64LE(delta, 1);
    return b;
  },
};
export type TriggerArgs = {
  slot: number;
  kind: number;
  side: number;
  triggerPx: bigint;
  limitPx: bigint;
  qty: bigint;
  tip?: bigint;
  interval?: bigint;
  slices?: bigint;
  mark?: boolean;
  confirmOracle?: boolean;
};
/** Position flags (v7): isolated, ADL-eligible (liquidate_book found an empty book; slot in bits 8..63). */
export function posFlags(flags: bigint): { isolated: boolean; adlEligible: boolean; adlSlot: bigint } {
  return {
    isolated: (flags & BigInt(S.POS_FLAG_ISOLATED)) !== 0n,
    adlEligible: (flags & BigInt(S.POS_FLAG_ADL_ELIGIBLE)) !== 0n,
    adlSlot: flags >> 8n,
  };
}

// ---------- events ----------
/** Book fill: maker key. Fast-path fill (EVENTS_FAST_FILL): maker = FastBook.makers[makerIndex] of the book in that tx; the key is
 *  resolved by the caller (`fast` + `makerIndex`), `maker` is "" until then. */
export type Fill = {
  maker: string;
  px: bigint;
  qty: bigint;
  orderId: bigint;
  makerIndex?: number;
  gen?: number;
};
export type Event =
  | { kind: "fill"; takerBid: boolean; taker: string; fills: Fill[]; fast?: boolean }
  | { kind: "fast_batch"; book: string; fills: (Fill & { orderSlot: number; takerBid: boolean })[] }
  | { kind: "trigger"; slot: number; triggerKind: number; filled: bigint; owner: string }
  | { kind: "liquidation"; target: string; qty: bigint; px: bigint; liquidator: string; book: boolean }
  | { kind: "liq_nofill"; target: string; slot: bigint }
  | { kind: "adl"; target: string; counterparty: string; qty: bigint }
  | { kind: "unknown"; raw: Buffer };
const EV_ENTRY = 56;
export function decodeEvent(b: Buffer): Event {
  const k = b[0];
  if (k === S.EVENTS_FILL) {
    const n = b[34];
    const fills: Fill[] = [];
    for (let i = 0; i < n; i++) {
      const o = 35 + i * EV_ENTRY;
      fills.push({
        maker: base58(b.subarray(o, o + 32)),
        px: b.readBigUInt64LE(o + 32),
        qty: b.readBigUInt64LE(o + 40),
        orderId: b.readBigUInt64LE(o + 48),
      });
    }
    return { kind: "fill", takerBid: b[1] === S.SIDE_BID, taker: base58(b.subarray(2, 34)), fills };
  }
  if (k === S.EVENTS_FAST_FILL) {
    // [6, side, taker 32, n, n x (px u32, qty u32, id u64)], id = maker index << 32 | quote gen
    const n = b[34];
    const fills: Fill[] = [];
    for (let i = 0; i < n; i++) {
      const o = 35 + i * 16;
      const id = b.readBigUInt64LE(o + 8);
      fills.push({
        maker: "",
        px: BigInt(b.readUInt32LE(o)),
        qty: BigInt(b.readUInt32LE(o + 4)),
        orderId: id,
        makerIndex: Number((id >> 32n) & 0xffn),
        gen: Number(id & 0xffffffffn),
      });
    }
    return {
      kind: "fill",
      takerBid: b[1] === S.SIDE_BID,
      taker: base58(b.subarray(2, 34)),
      fills,
      fast: true,
    };
  }
  if (k === S.EVENTS_FAST_BATCH) {
    // [7, 0, fast book 32, n, entries], id = order slot << 40 | side << 39 | maker index << 32 | gen
    const n = b[34];
    const fills = [];
    for (let i = 0; i < n; i++) {
      const o = 35 + i * 16;
      const id = b.readBigUInt64LE(o + 8);
      fills.push({
        maker: "",
        px: BigInt(b.readUInt32LE(o)),
        qty: BigInt(b.readUInt32LE(o + 4)),
        orderId: id,
        makerIndex: Number((id >> 32n) & 0x7fn),
        gen: Number(id & 0xffffffffn),
        orderSlot: Number(id >> 40n),
        takerBid: ((id >> 39n) & 1n) === 0n,
      });
    }
    return { kind: "fast_batch", book: base58(b.subarray(2, 34)), fills };
  }
  if (k === S.EVENTS_TRIGGER)
    return {
      kind: "trigger",
      slot: b[1],
      triggerKind: b[2],
      owner: base58(b.subarray(3, 35)),
      filled: b.readBigUInt64LE(35),
    };
  if (k === S.EVENTS_LIQUIDATION) {
    const lq = b.subarray(49, 81);
    return {
      kind: "liquidation",
      target: base58(b.subarray(1, 33)),
      qty: b.readBigUInt64LE(33),
      px: b.readBigUInt64LE(41),
      liquidator: base58(lq),
      book: lq.every((x) => x === 0),
    };
  }
  if (k === S.EVENTS_LIQ_NOFILL)
    return { kind: "liq_nofill", target: base58(b.subarray(1, 33)), slot: b.readBigUInt64LE(33) };
  if (k === S.EVENTS_ADL)
    return {
      kind: "adl",
      target: base58(b.subarray(1, 33)),
      counterparty: base58(b.subarray(33, 65)),
      qty: b.readBigUInt64LE(65),
    };
  return { kind: "unknown", raw: b };
}
/** Program logs ("Program data: <b64> ...") -> events. Each sol_log_data field is a separate b64 chunk; they are concatenated. */
export function parseLogs(logs: string[]): Event[] {
  const out: Event[] = [];
  for (const l of logs) {
    const m = /^Program data: (.*)$/.exec(l);
    if (!m) continue;
    out.push(
      decodeEvent(
        Buffer.concat(
          m[1]
            .trim()
            .split(/\s+/)
            .map((x) => Buffer.from(x, "base64")),
        ),
      ),
    );
  }
  return out;
}
/** Minimal indexer: trades tape, per-account volume and net position, last trade price. */
export class Indexer {
  trades: { taker: string; maker: string; takerBid: boolean; px: bigint; qty: bigint; orderId: bigint }[] =
    [];
  volume = new Map<string, bigint>();
  position = new Map<string, bigint>();
  lastPx = 0n;
  liquidations: Event[] = [];
  adls: Event[] = [];
  ingest(logs: string[]) {
    for (const e of parseLogs(logs)) {
      if (e.kind === "fill")
        for (const f of e.fills) {
          this.trades.push({
            taker: e.taker,
            maker: f.maker,
            takerBid: e.takerBid,
            px: f.px,
            qty: f.qty,
            orderId: f.orderId,
          });
          this.lastPx = f.px;
          const n = f.px * f.qty;
          for (const a of [e.taker, f.maker]) this.volume.set(a, (this.volume.get(a) ?? 0n) + n);
          const d = e.takerBid ? f.qty : -f.qty;
          this.position.set(e.taker, (this.position.get(e.taker) ?? 0n) + d);
          this.position.set(f.maker, (this.position.get(f.maker) ?? 0n) - d);
        }
      else if (e.kind === "liquidation" || e.kind === "liq_nofill") this.liquidations.push(e);
      else if (e.kind === "adl") this.adls.push(e);
    }
  }
}
